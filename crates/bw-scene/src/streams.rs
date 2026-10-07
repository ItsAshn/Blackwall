//! Streams of dots crossing the Blackwall (PLAN §6.1). Disk IO flows from a
//! column's foot along the floor, through the Wall, to the storage district;
//! CPU-heavy processes stream to the scheduler. Many particles share each
//! path, so busy processes draw dense dotted lines like the reference.
//! Blue streams come from healthy processes, red ones from processes with
//! issues. Rates come from the aggregate counters every platform exposes;
//! per-syscall detail (eBPF/ETW) comes later.

use crate::city::health_color;
use crate::layout::Subsystem;
use crate::quality::Quality;
use crate::*;
use bevy::color::LinearRgba;
use bw_model::{Health, Realm};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Startup, setup).add_systems(
        Update,
        (pick_sources, advance).chain().in_set(SceneSet::Visuals),
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Io,
    Cpu,
}

#[derive(Component)]
struct Particle {
    from: Option<ProcKey>,
    kind: Kind,
    t: f32,
    speed: f32,
    lane: f32,
}

#[derive(Resource, Default)]
struct Emitters {
    /// (key, kind, critical, cumulative weight)
    list: Vec<(ProcKey, Kind, bool, f32)>,
    total: f32,
    rng: u64,
}

impl Emitters {
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    fn pick(&mut self) -> Option<(ProcKey, Kind, bool)> {
        if self.total <= 0.0 {
            return None;
        }
        let x = self.rand() * self.total;
        self.list.iter().find(|e| e.3 >= x).map(|e| (e.0, e.1, e.2))
    }
}

#[derive(Resource)]
struct ParticleMats {
    ok: Handle<StandardMaterial>,
    bad: Handle<StandardMaterial>,
}

fn emissive(
    mats: &mut Assets<StandardMaterial>,
    rgb: [f32; 3],
    k: f32,
) -> Handle<StandardMaterial> {
    mats.add(StandardMaterial {
        emissive: LinearRgba::rgb(rgb[0], rgb[1], rgb[2]) * k,
        base_color: Color::BLACK,
        unlit: false,
        ..default()
    })
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    quality: Res<Quality>,
) {
    let mesh = meshes.add(Cuboid::from_length(1.0));
    let ok = emissive(&mut mats, health_color(Health::Healthy, Realm::User), 6.0);
    let bad = emissive(&mut mats, health_color(Health::Critical, Realm::User), 8.0);
    for i in 0..quality.tier.particle_budget() {
        commands.spawn((
            Particle {
                from: None,
                kind: Kind::Io,
                t: (i as f32 * 0.618).fract(),
                speed: 0.3,
                lane: 0.0,
            },
            Mesh3d(mesh.clone()),
            MeshMaterial3d(ok.clone()),
            Transform::from_scale(Vec3::splat(0.06)),
            Visibility::Hidden,
            crate::explore::MachineLayer,
        ));
    }
    commands.insert_resource(ParticleMats { ok, bad });
    commands.insert_resource(Emitters {
        rng: 0xDEAD_BEEF_1234_5678,
        ..default()
    });
}

fn pick_sources(m: Res<Machine>, mut em: ResMut<Emitters>) {
    if !m.is_changed() {
        return;
    }
    let mut cands = Vec::new();
    for p in m
        .snapshot
        .processes
        .values()
        .filter(|p| p.realm == Realm::User)
    {
        let bad = p.health() == Health::Critical;
        let io = (p.io_read_bytes + p.io_write_bytes) as f32;
        if io > 4096.0 || p.state == bw_model::ProcState::DiskWait {
            cands.push((p.key, Kind::Io, bad, (io / 65536.0).sqrt().max(0.5)));
        }
        if p.cpu_pct > 3.0 {
            cands.push((p.key, Kind::Cpu, bad, p.cpu_pct.sqrt() * 0.8));
        }
    }
    cands.sort_by(|a, b| b.3.total_cmp(&a.3));
    cands.truncate(40);
    let mut acc = 0.0;
    em.list = cands
        .into_iter()
        .map(|(k, kind, bad, w)| {
            acc += w;
            (k, kind, bad, acc)
        })
        .collect();
    em.total = acc;
}

/// Point along a polyline at parameter `t` in 0..1 (by length).
fn along(pts: &[Vec3], t: f32) -> Vec3 {
    let lens: Vec<f32> = pts.windows(2).map(|w| w[0].distance(w[1])).collect();
    let total: f32 = lens.iter().sum();
    let mut d = t * total;
    for (i, l) in lens.iter().enumerate() {
        if d <= *l {
            return pts[i].lerp(pts[i + 1], d / l.max(1e-6));
        }
        d -= l;
    }
    *pts.last().unwrap_or(&Vec3::ZERO)
}

#[allow(clippy::too_many_arguments)]
fn advance(
    time: Res<Time>,
    settings: Res<SceneSettings>,
    sl: Res<SceneLayout>,
    pm: Res<ParticleMats>,
    mut em: ResMut<Emitters>,
    mut q: Query<(
        &mut Particle,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
    ex: Res<crate::explore::Explore>,
) {
    if ex.inside().is_some() {
        for (_, _, mut vis, _) in &mut q {
            *vis = Visibility::Hidden;
        }
        return;
    }
    let dt = if settings.reduced_motion {
        time.delta_secs() * 0.25
    } else {
        time.delta_secs()
    };
    let lay = &sl.layout;
    let wall_z = lay.wall_z();
    for (mut part, mut tf, mut vis, mut mat) in &mut q {
        part.t += dt * part.speed;
        let col = part.from.and_then(|k| lay.column(&k));
        if part.t >= 1.0 || col.is_none() {
            part.t = part.t.fract();
            part.from = None;
            if let Some((k, kind, bad)) = em.pick() {
                part.from = Some(k);
                part.kind = kind;
                // Same speed for every dot of an emitter, so they form a line.
                part.speed = 0.08 + (k.pid % 7) as f32 * 0.012;
                part.lane = (em.rand() - 0.5) * 0.25;
                mat.0 = if bad { pm.bad.clone() } else { pm.ok.clone() };
            }
        }
        let (Some(c), true) = (col, settings.show_streams) else {
            *vis = Visibility::Hidden;
            continue;
        };
        let target = lay.district_pos(if part.kind == Kind::Io {
            Subsystem::Storage
        } else {
            Subsystem::Scheduler
        });
        let y = 0.06;
        // Down the column, along the street toward the Wall, through it, into the district.
        let x = c.base.x + part.lane;
        let pts = [
            c.top(),
            Vec3::new(x, y, c.base.z + 0.4),
            Vec3::new(x, y, wall_z + 0.6),
            Vec3::new(x * 0.6 + target.x * 0.4, y, wall_z - 0.6),
            target + Vec3::new(part.lane * 4.0, y, 0.0),
        ];
        tf.translation = along(&pts, part.t);
        let near_wall = 1.0 - ((tf.translation.z - wall_z).abs() / 1.5).min(1.0);
        tf.scale = Vec3::splat(0.05 + near_wall * 0.08);
        *vis = Visibility::Inherited;
    }
}
