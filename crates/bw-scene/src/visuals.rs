//! Scene entities other than the dots: the Wall, world labels, logical
//! process entities (for labels, camera follow and navigation) and the
//! family lines of the selection (PLAN §6, v3 look).

use crate::city::health_color;
use crate::layout::Subsystem;
use crate::quality::Quality;
use crate::wall::WallMaterial;
use crate::*;
use bevy::color::LinearRgba;
use bw_model::{Health, ProcState, Realm};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Startup, setup).add_systems(
        Update,
        (
            sync_entities,
            track_columns,
            update_wall,
            update_labels,
            draw_lines,
        )
            .chain()
            .in_set(SceneSet::Visuals),
    );
}

#[derive(Component)]
struct WallSurface;

#[derive(Component)]
struct SubsystemLabel(Subsystem);

#[derive(Component)]
struct VolumeLabel(usize);

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut wall_mats: ResMut<Assets<WallMaterial>>,
) {
    // The Wall; resized once the first layout is known.
    commands.spawn((
        WallSurface,
        crate::explore::MachineLayer,
        Mesh3d(meshes.add(Rectangle::new(1.0, 1.0))),
        MeshMaterial3d(wall_mats.add(WallMaterial::default())),
        Transform::from_xyz(0.0, 17.0, -40.0),
    ));
    for s in Subsystem::ALL {
        commands.spawn((
            SubsystemLabel(s),
            crate::explore::MachineLayer,
            Transform::default(),
            Visibility::default(),
            WorldLabel {
                text: s.label().into(),
                kind: LabelKind::Subsystem,
            },
        ));
    }
}

/// Logical entities for processes: no mesh (the dots are in the city mesh),
/// but labels, navigation and the camera follow them.
fn sync_entities(
    mut commands: Commands,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    mut ents: ResMut<ProcEntities>,
) {
    if !m.is_changed() {
        return;
    }
    for p in m.snapshot.processes.values() {
        if ents.0.contains_key(&p.key) {
            continue;
        }
        let pos = sl.targets.get(&p.key).copied().unwrap_or_default();
        let e = commands
            .spawn((
                ProcNode { key: p.key },
                Shown {
                    pos,
                    radius: 0.3,
                    presence: 1.0,
                },
                Transform::from_translation(pos),
            ))
            .id();
        ents.0.insert(p.key, e);
    }
    let dead: Vec<ProcKey> = ents
        .0
        .keys()
        .filter(|k| !m.snapshot.processes.contains_key(k))
        .copied()
        .collect();
    for k in dead {
        if let Some(e) = ents.0.remove(&k) {
            commands.entity(e).despawn();
        }
    }
}

fn track_columns(
    sl: Res<SceneLayout>,
    time: Res<Time>,
    mut q: Query<(&ProcNode, &mut Shown, &mut Transform)>,
) {
    let k = 1.0 - (-time.delta_secs() * 6.0).exp();
    for (node, mut shown, mut tf) in &mut q {
        let Some(c) = sl.layout.column(&node.key) else {
            continue;
        };
        shown.pos = shown.pos.lerp(c.top(), k);
        shown.radius = 0.15 + c.footprint as f32 * 0.12;
        tf.translation = shown.pos;
    }
}

#[allow(clippy::too_many_arguments)]
fn update_wall(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    quality: Res<Quality>,
    time: Res<Time>,
    jack: Res<crate::camera::JackIn>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut wall_mats: ResMut<Assets<WallMaterial>>,
    mut q: Query<(&mut Transform, &Mesh3d, &MeshMaterial3d<WallMaterial>), With<WallSurface>>,
    mut pressure: Local<f32>,
    mut clock: Local<f32>,
    mut last_size: Local<(f32, f32)>,
) {
    let Ok((mut tf, mesh, mat)) = q.single_mut() else {
        return;
    };
    let size = sl.layout.wall_size();
    if (size.0 - last_size.0).abs() > 1.0 || (size.1 - last_size.1).abs() > 1.0 {
        *last_size = size;
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            *mesh = Rectangle::new(size.0, size.1).into();
        }
    }
    tf.translation = Vec3::new(0.0, size.1 / 2.0, sl.layout.wall_z());
    let dt = time.delta_secs();
    *pressure += (m.snapshot.system.kernel_pressure - *pressure) * (1.0 - (-dt * 1.5).exp());
    // The rain runs on its own clock so the jack-in can speed it up smoothly;
    // under reduced motion it stands still.
    if !settings.reduced_motion {
        *clock += dt * jack.rain_boost();
    }
    if let Some(mut w) = wall_mats.get_mut(&mat.0) {
        let glitch = if settings.reduced_motion { 0.0 } else { 1.0 };
        w.params.state = Vec4::new(*pressure, *clock, glitch, quality.tier.wall_intensity());
        w.params.extra = Vec4::new(1.0, size.1, 0.0, 0.0);
    }
}

#[allow(clippy::type_complexity)]
fn update_labels(
    mut commands: Commands,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    ex: Res<crate::explore::Explore>,
    sel: Res<Selection>,
    mut subs: Query<(&SubsystemLabel, &mut Transform, &mut Visibility), Without<VolumeLabel>>,
    mut vols: Query<
        (Entity, &VolumeLabel, &mut Transform, &mut WorldLabel),
        Without<SubsystemLabel>,
    >,
) {
    for (s, mut tf, mut vis) in &mut subs {
        tf.translation = sl.layout.district_pos(s.0) + Vec3::Y * 1.2;
        // Named only from the overview; at street level they'd be clutter.
        *vis = if settings.show_kernel && ex.inside().is_none() && sel.key.is_none() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    let volumes = &m.snapshot.volumes;
    if vols.iter().count() != volumes.len() {
        for (e, ..) in &vols {
            commands.entity(e).despawn();
        }
        for i in 0..volumes.len() {
            commands.spawn((
                VolumeLabel(i),
                crate::explore::MachineLayer,
                Transform::default(),
                Visibility::default(),
                WorldLabel {
                    text: String::new(),
                    kind: LabelKind::Volume,
                },
            ));
        }
        return;
    }
    for (_, v, mut tf, mut label) in &mut vols {
        let Some(vol) = volumes.get(v.0) else {
            continue;
        };
        let levels = (8.0
            + (vol.total_bytes as f32 / (1u64 << 30) as f32)
                .max(1.0)
                .log2()
                * 2.0)
            .round();
        tf.translation = sl.layout.volume_base(v.0) + Vec3::Y * (levels * crate::layout::LEVEL_H);
        label.text = format!("{} {:.0}%", vol.mount, vol.used_pct());
    }
}

fn line_color(h: Health, realm: Realm, k: f32) -> Color {
    let [r, g, b] = health_color(h, realm);
    Color::LinearRgba(LinearRgba::rgb(r * k, g * k, b * k))
}

/// Family lines along the floor for the selection (and for everything when
/// "tree links" is on), plus a tether from stuck-in-IO processes through the
/// Wall to the storage district.
fn draw_lines(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    sel: Res<Selection>,
    mut gizmos: Gizmos,
) {
    let lay = &sl.layout;
    let floor = Vec3::Y * 0.02;
    let storage = lay.district_pos(Subsystem::Storage);
    for c in &lay.columns {
        let Some(p) = m.snapshot.processes.get(&c.key) else {
            continue;
        };
        if p.state == ProcState::DiskWait {
            gizmos.line(
                c.base + floor,
                storage + floor,
                line_color(Health::Critical, Realm::User, 1.5),
            );
        }
        let Some(parent) = p.parent.and_then(|k| lay.column(&k)) else {
            continue;
        };
        let family = sel.key == Some(c.key) || sel.key == Some(parent.key);
        if family {
            // Right-angled, like streets: along x, then along z.
            let corner = Vec3::new(parent.base.x, 0.0, c.base.z) + floor;
            let col = line_color(p.health(), c.realm, 3.0);
            gizmos.line(c.base + floor, corner, col);
            gizmos.line(corner, parent.base + floor, col);
        } else if settings.show_links && c.realm == Realm::User {
            gizmos.line(
                c.base + floor,
                parent.base + floor,
                line_color(p.health(), c.realm, 0.35),
            );
        }
    }
    // A beam above the selected column.
    if let Some(c) = sel.key.and_then(|k| lay.column(&k)) {
        gizmos.line(
            c.top() + Vec3::Y * 0.2,
            c.top() + Vec3::Y * 2.5,
            Color::LinearRgba(LinearRgba::rgb(2.0, 2.4, 3.0)),
        );
    }
}
