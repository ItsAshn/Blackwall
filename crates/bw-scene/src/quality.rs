//! Adaptive quality tiers (PLAN §13).
//!
//! The initial tier comes from the GPU adapter. A governor then steps down if
//! frames stay over budget and back up if they stay well under it. A tier
//! forced from the command line or settings disables the governor.

use crate::camera::MainCamera;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::view::Msaa;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Low,
    Medium,
    High,
    Ultra,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Low, Tier::Medium, Tier::High, Tier::Ultra];

    pub fn parse(s: &str) -> Option<Tier> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Tier::Low),
            "medium" | "med" => Some(Tier::Medium),
            "high" => Some(Tier::High),
            "ultra" => Some(Tier::Ultra),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tier::Low => "LOW",
            Tier::Medium => "MEDIUM",
            Tier::High => "HIGH",
            Tier::Ultra => "ULTRA",
        }
    }

    pub fn particle_budget(self) -> usize {
        [200, 500, 900, 1500][self as usize]
    }

    pub fn wall_intensity(self) -> f32 {
        if self == Tier::Low { 0.8 } else { 1.0 }
    }

    fn bloom(self) -> Option<f32> {
        [None, Some(0.12), Some(0.16), Some(0.2)][self as usize]
    }

    fn msaa(self) -> Msaa {
        if self == Tier::Low {
            Msaa::Off
        } else {
            Msaa::Sample4
        }
    }

    fn down(self) -> Tier {
        Tier::ALL[(self as usize).saturating_sub(1)]
    }

    fn up(self) -> Tier {
        Tier::ALL[(self as usize + 1).min(3)]
    }
}

#[derive(Resource, Debug)]
pub struct Quality {
    pub tier: Tier,
    /// Governor enabled (no forced tier).
    pub auto: bool,
    /// The best tier this adapter should reach automatically.
    pub ceiling: Tier,
    pub adapter: String,
    pub backend: String,
    pub software: bool,
    pub frame_ms: f32,
    over: f32,
    under: f32,
    detected: bool,
}

impl Quality {
    /// `forced`: a tier chosen by the user; `None` = automatic.
    pub fn new(forced: Option<Tier>) -> Self {
        Self {
            tier: forced.unwrap_or(Tier::Medium),
            auto: forced.is_none(),
            ceiling: Tier::High,
            adapter: String::new(),
            backend: String::new(),
            software: false,
            frame_ms: 16.7,
            over: 0.0,
            under: 0.0,
            detected: false,
        }
    }

    pub fn set_manual(&mut self, tier: Option<Tier>) {
        match tier {
            Some(t) => {
                self.tier = t;
                self.auto = false;
            }
            None => self.auto = true,
        }
    }
}

impl Default for Quality {
    fn default() -> Self {
        Self::new(None)
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Quality>()
        .add_systems(Update, (detect, govern, apply).chain());
}

/// Classify the adapter. `DeviceType`/`Backend` aren't re-exported by Bevy,
/// so this matches their stable Debug names instead of pinning wgpu-types.
fn detect(info: Option<Res<RenderAdapterInfo>>, mut q: ResMut<Quality>) {
    if q.detected {
        return;
    }
    let Some(info) = info else { return };
    q.detected = true;
    let device = format!("{:?}", info.device_type);
    q.backend = format!("{:?}", info.backend);
    q.adapter = info.name.clone();
    q.software = device == "Cpu"
        || [
            "llvmpipe",
            "lavapipe",
            "swiftshader",
            "microsoft basic render",
        ]
        .iter()
        .any(|s| info.name.to_ascii_lowercase().contains(s));
    let mut ceiling = match device.as_str() {
        _ if q.software => Tier::Low,
        "DiscreteGpu" => Tier::Ultra,
        "IntegratedGpu" | "VirtualGpu" => Tier::High,
        _ => Tier::Medium,
    };
    // The GL fallback backend gets at most Medium (PLAN §3.3).
    if q.backend == "Gl" {
        ceiling = ceiling.min(Tier::Medium);
    }
    q.ceiling = ceiling;
    if q.auto {
        q.tier = match ceiling {
            Tier::Ultra => Tier::High,
            Tier::High => Tier::Medium,
            t => t,
        };
    }
    info!(
        "GPU: {} ({device}, {}) → quality {:?} (ceiling {:?}, auto: {})",
        q.adapter, q.backend, q.tier, q.ceiling, q.auto
    );
}

fn govern(time: Res<Time>, mut q: ResMut<Quality>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    q.frame_ms += (dt * 1000.0 - q.frame_ms) * 0.05;
    if !q.auto || time.elapsed_secs() < 5.0 {
        return;
    }
    if q.frame_ms > 22.0 {
        q.over += dt;
        q.under = 0.0;
    } else if q.frame_ms < 9.0 {
        q.under += dt;
        q.over = 0.0;
    } else {
        q.over = 0.0;
        q.under = 0.0;
    }
    if q.over > 3.0 && q.tier > Tier::Low {
        q.tier = q.tier.down();
        q.over = 0.0;
        info!(
            "quality governor: down to {:?} ({:.1} ms/frame)",
            q.tier, q.frame_ms
        );
    } else if q.under > 30.0 && q.tier < q.ceiling {
        q.tier = q.tier.up();
        q.under = 0.0;
        info!("quality governor: up to {:?}", q.tier);
    }
}

fn apply(
    mut commands: Commands,
    q: Res<Quality>,
    cams: Query<(Entity, Option<&Bloom>), With<MainCamera>>,
    mut last: Local<Option<Tier>>,
) {
    let Ok((cam, bloom)) = cams.single() else {
        return;
    };
    if *last == Some(q.tier) && (bloom.is_some() == q.tier.bloom().is_some()) {
        return;
    }
    *last = Some(q.tier);
    match q.tier.bloom() {
        Some(intensity) => {
            commands.entity(cam).insert(Bloom {
                intensity,
                ..Bloom::NATURAL
            });
        }
        None => {
            commands.entity(cam).remove::<Bloom>();
        }
    }
    commands.entity(cam).insert(q.tier.msaa());
}
