//! The Blackwall is the firewall: a wall of falling rain round the whole
//! map. Every port something listens on is a gate in it (the rain parts):
//!
//! * **exposed and allowed** by the firewall: an open gate, health blue;
//! * **exposed and blocked**: a barred gate, red;
//! * **exposed, rules unknown** (they need admin rights; press F to read
//!   them): a gate in the Wall's own magenta;
//! * **local only** (bound to loopback): a small dim door at the Wall's foot.
//!
//! Connections cross the Wall as arcs from their tower out to a point in the
//! dark beyond it. Outside the RAM square, on its west side, stands the
//! dormant district: a ghost for every service that is defined but not
//! running, taller the longer it has been idle.
//!
//! This module holds the geometry (shared with `city.rs` and `towers.rs`),
//! the Wall's four faces, and the hoverable landmarks and labels.

use crate::camera::JackIn;
use crate::layout::Layout;
use crate::quality::Quality;
use crate::wall::WallMaterial;
use crate::*;
use bw_model::{FirewallAction, Listener, Service, ServiceState};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Startup, setup).add_systems(
        Update,
        (update_wall, update_landmarks)
            .chain()
            .in_set(SceneSet::Visuals),
    );
}

/// Side of a ghost's plot, and the pitch between ghosts.
const GHOST: f32 = 2.2;
const GHOST_PITCH: f32 = 3.2;

/// How many ghosts fit in one column of the dormant district.
fn ghosts_per_column(lay: &Layout) -> usize {
    ((lay.max.y - lay.min.y) / GHOST_PITCH).floor().max(1.0) as usize
}

/// The plot of the i-th ghost: columns stepping west from the RAM square.
pub fn ghost_plot(lay: &Layout, i: usize) -> (Vec2, Vec2) {
    let per = ghosts_per_column(lay);
    let (col, row) = (i / per, i % per);
    let x1 = lay.min.x - 2.0 - col as f32 * GHOST_PITCH;
    let z0 = lay.min.y + 0.25 + row as f32 * GHOST_PITCH;
    (Vec2::new(x1 - GHOST, z0), Vec2::new(x1, z0 + GHOST))
}

/// A ghost's height: the longer idle, the taller (log of days).
pub fn ghost_height(idle_secs: Option<u64>) -> f32 {
    let days = idle_secs.unwrap_or(0) as f32 / 86_400.0;
    0.8 + (1.0 + days).log2() * 1.3
}

/// Half the Wall's square: round the RAM square, the volumes and the
/// dormant district, with room to walk.
pub fn wall_half(lay: &Layout, ghosts: usize) -> f32 {
    let cols = ghosts.div_ceil(ghosts_per_column(lay)) as f32;
    lay.extent + 5.5 + cols * GHOST_PITCH
}

pub fn wall_height(lay: &Layout) -> f32 {
    (lay.extent * 0.45).clamp(6.0, 22.0)
}

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A point on the Wall for a stable key (a port, an address), and the
/// outward direction there. The east side stays clear for the volumes' row.
pub fn wall_point(w: f32, key: u64) -> (Vec3, Vec3) {
    let t = (mix(key) >> 11) as f32 / (1u64 << 53) as f32;
    let side = (t * 4.0).floor() as u32;
    let along = ((t * 4.0).fract() * 2.0 - 1.0) * 0.82 * w;
    match side {
        0 => (Vec3::new(along, 0.0, -w), -Vec3::Z),
        1 => (Vec3::new(w, 0.0, along), Vec3::X),
        2 => (Vec3::new(along, 0.0, w), Vec3::Z),
        _ => (Vec3::new(-w, 0.0, along), -Vec3::X),
    }
}

/// Where a listener's gate stands, and which way is out.
pub fn gate(w: f32, l: &Listener) -> (Vec3, Vec3) {
    wall_point(w, l.port as u64 * 2 + l.proto as u64)
}

/// The remote end of a connection: out in the dark beyond the Wall, in the
/// direction its address hashes to.
pub fn remote_point(w: f32, addr: &str) -> Vec3 {
    let key = addr
        .bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
    let (p, out) = wall_point(w, key);
    let far = 4.0 + (mix(key ^ 0xA5) % 600) as f32 / 100.0;
    p + out * far + Vec3::Y * (1.5 + (mix(key ^ 0x5A) % 300) as f32 / 100.0)
}

/// How a gate looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateLook {
    Open,
    Barred,
    Unknown,
    Local,
}

pub fn gate_look(m: &Machine, l: &Listener) -> GateLook {
    if !l.exposed {
        return GateLook::Local;
    }
    match m.snapshot.net.firewall.verdict(l.port, l.proto) {
        Some(FirewallAction::Allow) => GateLook::Open,
        Some(FirewallAction::Deny) => GateLook::Barred,
        None => GateLook::Unknown,
    }
}

/// Gate size: (half width, height).
pub fn gate_size(look: GateLook) -> (f32, f32) {
    match look {
        GateLook::Local => (0.35, 1.0),
        _ => (0.7, 3.2),
    }
}

/// Ghost services: the dormant ones that are not running.
pub fn ghosts(m: &Machine) -> Vec<&Service> {
    m.dormant()
        .into_iter()
        .filter(|s| s.state != ServiceState::Running)
        .collect()
}

#[derive(Component)]
struct WallFace(u8);

#[derive(Component)]
struct GateLabel(usize);

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<WallMaterial>>,
) {
    let mat = mats.add(WallMaterial::default());
    for i in 0..4 {
        commands.spawn((
            WallFace(i),
            crate::explore::MachineLayer,
            Mesh3d(meshes.add(Rectangle::new(1.0, 1.0))),
            MeshMaterial3d(mat.clone()),
            Transform::default(),
            bevy::camera::visibility::NoFrustumCulling,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn update_wall(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    quality: Res<Quality>,
    time: Res<Time>,
    jack: Res<JackIn>,
    sus: Res<Suspicions>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<WallMaterial>>,
    mut q: Query<(
        &WallFace,
        &mut Transform,
        &Mesh3d,
        &MeshMaterial3d<WallMaterial>,
    )>,
    mut clock: Local<f32>,
    mut heat: Local<f32>,
    mut last: Local<(f32, f32)>,
) {
    let lay = &sl.layout;
    if lay.extent <= 0.0 {
        return;
    }
    let w = wall_half(lay, ghosts(&m).len());
    let h = wall_height(lay);
    let dt = time.delta_secs();
    // The rain runs hot with suspicious network activity, and busier with
    // the number of connections crossing it.
    let suspect_net = sus
        .list
        .iter()
        .filter(|s| s.family == crate::suspect::Family::Network)
        .count() as f32;
    let conns = m.snapshot.net.connections.len() as f32;
    let target = (suspect_net * 0.3 + conns / 120.0).clamp(0.0, 1.0);
    *heat += (target - *heat) * (1.0 - (-dt * 1.5).exp());
    if !settings.reduced_motion {
        *clock += dt * jack.rain_boost();
    }
    let resized = (w - last.0).abs() > 0.01 || (h - last.1).abs() > 0.01;
    *last = (w, h);
    let mut gates = [Vec4::ZERO; 24];
    let mut n = 0;
    for l in &m.snapshot.net.listening {
        if n == 24 {
            break;
        }
        let (p, _) = gate(w, l);
        let (hw, gh) = gate_size(gate_look(&m, l));
        gates[n] = Vec4::new(p.x, p.z, hw, gh);
        n += 1;
    }
    for (face, mut tf, mesh, mat) in &mut q {
        if resized && let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            *mesh = Rectangle::new(w * 2.0, h).into();
        }
        let (pos, rot) = match face.0 {
            0 => (Vec3::new(0.0, h / 2.0, -w), Quat::IDENTITY),
            1 => (
                Vec3::new(w, h / 2.0, 0.0),
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ),
            2 => (Vec3::new(0.0, h / 2.0, w), Quat::IDENTITY),
            _ => (
                Vec3::new(-w, h / 2.0, 0.0),
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ),
        };
        *tf = Transform::from_translation(pos).with_rotation(rot);
        if let Some(mut wm) = mats.get_mut(&mat.0) {
            let glitch = if settings.reduced_motion { 0.0 } else { 1.0 };
            wm.params.state =
                Vec4::new(*heat, *clock, glitch, quality.tier.wall_intensity() * 0.75);
            wm.params.extra = Vec4::new(1.0, h, n as f32, 0.0);
            wm.params.gates = gates;
        }
    }
}

/// Hoverable landmarks (gates, ghosts, remote addresses) and their labels.
#[allow(clippy::too_many_arguments)]
fn update_landmarks(
    mut commands: Commands,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    sus: Res<Suspicions>,
    settings: Res<SceneSettings>,
    mut marks: ResMut<Landmarks>,
    mut labels: Query<(
        Entity,
        &GateLabel,
        &mut Transform,
        &mut WorldLabel,
        &mut Visibility,
    )>,
    mut last: Local<u64>,
) {
    if m.generation == *last {
        return;
    }
    *last = m.generation;
    let lay = &sl.layout;
    let now = m.now();
    let ghosts = ghosts(&m);
    let w = wall_half(lay, ghosts.len());
    let name_of = |k: Option<ProcKey>| {
        k.and_then(|k| m.snapshot.processes.get(&k))
            .map_or("hidden (needs admin)".to_string(), |p| p.name.clone())
    };
    let mut list = Vec::new();
    let mut want: Vec<(Vec3, String, LabelKind)> = Vec::new();
    for l in &m.snapshot.net.listening {
        let (p, out) = gate(w, l);
        let look = gate_look(&m, l);
        let (hw, gh) = gate_size(look);
        let side = Vec3::new(out.z, 0.0, out.x).abs() * hw + out.abs() * 0.3;
        let who = name_of(l.process);
        let suspect = l.process.is_some_and(|k| sus.is_suspect(k));
        let fw = match look {
            GateLook::Open => "firewall: allowed",
            GateLook::Barred => "firewall: blocked",
            GateLook::Unknown => "firewall rules unknown (F to read them)",
            GateLook::Local => "local only",
        };
        list.push(Landmark {
            min: p - side,
            max: p + side + Vec3::Y * gh,
            shader_id: (crate::towers::BLOCK_ID_BASE + 1000 + list.len()) as f32,
            title: format!(":{} {} · {}", l.port, l.proto.label(), who),
            detail: format!(
                "{} · bound to {} · {fw}",
                if l.exposed {
                    "open to the network"
                } else {
                    "this machine only"
                },
                l.addr
            ),
            suspect,
        });
        if look != GateLook::Local {
            want.push((
                p + Vec3::Y * (gh + 0.5),
                format!(":{} {}", l.port, who),
                LabelKind::Volume,
            ));
        }
    }
    for c in &m.snapshot.net.connections {
        let r = remote_point(w, &c.remote_addr);
        let suspect = c.process.is_some_and(|k| sus.is_suspect(k));
        list.push(Landmark {
            min: r - Vec3::splat(0.4),
            max: r + Vec3::splat(0.4),
            shader_id: -1.0,
            title: format!("{}:{}", c.remote_addr, c.remote_port),
            detail: format!(
                "{} {}",
                if c.outbound {
                    "reached by"
                } else {
                    "connected to us, served by"
                },
                name_of(c.process)
            ),
            suspect,
        });
        if suspect {
            want.push((
                r + Vec3::Y * 0.6,
                format!("{}:{}", c.remote_addr, c.remote_port),
                LabelKind::Alert,
            ));
        }
    }
    for (i, s) in ghosts.iter().enumerate() {
        let (a, b) = ghost_plot(lay, i);
        let idle = s.idle_secs(now);
        let gh = ghost_height(idle);
        let flagged: Vec<&str> = sus
            .of_service(&s.location)
            .map(|x| x.text.as_str())
            .collect();
        let when = match (s.last_active, idle) {
            (Some(_), Some(t)) => format!("last ran {} ago", fmt_age(t)),
            (None, Some(t)) => format!("untouched for {}", fmt_age(t)),
            _ => "never seen running".into(),
        };
        list.push(Landmark {
            min: Vec3::new(a.x, 0.0, a.y),
            max: Vec3::new(b.x, gh, b.y),
            shader_id: (crate::towers::GHOST_ID_BASE + i) as f32,
            title: format!("{} · {}", s.name, s.kind.label()),
            detail: format!(
                "{when}{}{} · {}{}",
                if s.state == ServiceState::Failed {
                    " · failed"
                } else {
                    ""
                },
                if s.enabled { " · still enabled" } else { "" },
                s.location,
                if flagged.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", flagged.join(", "))
                }
            ),
            suspect: !flagged.is_empty(),
        });
        // Name the longest-idle ghosts and anything suspicious.
        if i < 6 || !flagged.is_empty() {
            let c = (a + b) * 0.5;
            want.push((
                Vec3::new(c.x, gh + 0.4, c.y),
                format!("{} · {}", s.name, idle.map_or("?".into(), fmt_age)),
                if flagged.is_empty() {
                    LabelKind::Volume
                } else {
                    LabelKind::Alert
                },
            ));
        }
    }
    marks.list = list;
    marks.hovered = None;

    if labels.iter().count() != want.len() {
        for (e, ..) in &labels {
            commands.entity(e).despawn();
        }
        for i in 0..want.len() {
            commands.spawn((
                GateLabel(i),
                Transform::default(),
                Visibility::default(),
                WorldLabel {
                    text: String::new(),
                    kind: LabelKind::Volume,
                },
            ));
        }
        *last = 0;
        return;
    }
    for (_, l, mut tf, mut label, mut vis) in &mut labels {
        if let Some((p, text, kind)) = want.get(l.0) {
            tf.translation = *p;
            label.text.clone_from(text);
            label.kind = *kind;
        }
        *vis = if settings.show_labels {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
