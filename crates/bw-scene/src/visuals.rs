//! Scene entities other than the meshes: world labels for the map's blocks
//! and volumes, logical process entities (for labels, camera follow and the
//! HUD) and the beam over the chosen tower.

use crate::layout::BlockKind;
use crate::*;
use bevy::color::LinearRgba;

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (sync_entities, track_columns, update_labels, draw_lines)
            .chain()
            .in_set(SceneSet::Visuals),
    );
}

#[derive(Component)]
struct MapLabel(usize);

/// Logical entities for processes: no mesh (the towers are one mesh),
/// but labels, the HUD and the camera follow them.
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
        shown.radius = c.half().max_element() + 0.1;
        tf.translation = shown.pos;
    }
}

/// Labels on the map: the kernel's own memory and free memory (on their
/// blocks), and each volume.
fn update_labels(
    mut commands: Commands,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    mut q: Query<(
        Entity,
        &MapLabel,
        &mut Transform,
        &mut WorldLabel,
        &mut Visibility,
    )>,
) {
    let lay = &sl.layout;
    let mut want: Vec<(Vec3, String, LabelKind)> = Vec::new();
    for b in &lay.blocks {
        let c = (b.min + b.max) * 0.5;
        let (text, kind) = match b.kind {
            BlockKind::KernelMemory => {
                (format!("KERNEL {}", fmt_mb(b.bytes)), LabelKind::Subsystem)
            }
            BlockKind::Free => (format!("FREE {}", fmt_mb(b.bytes)), LabelKind::Volume),
        };
        want.push((Vec3::new(c.x, 1.2, c.y), text, kind));
    }
    for (i, v) in m.snapshot.volumes.iter().enumerate() {
        let levels =
            (8.0 + (v.total_bytes as f32 / (1u64 << 30) as f32).max(1.0).log2() * 2.0).round();
        want.push((
            lay.volume_base(i) + Vec3::Y * (levels * crate::layout::LEVEL_H),
            format!("{} {:.0}%", v.mount, v.used_pct()),
            LabelKind::Volume,
        ));
    }
    if q.iter().count() != want.len() {
        for (e, ..) in &q {
            commands.entity(e).despawn();
        }
        for i in 0..want.len() {
            commands.spawn((
                MapLabel(i),
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
    for (_, l, mut tf, mut label, mut vis) in &mut q {
        let Some((pos, text, kind)) = want.get(l.0) else {
            continue;
        };
        tf.translation = *pos;
        label.text.clone_from(text);
        label.kind = *kind;
        *vis = if settings.show_labels {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

/// A short white beam above the chosen tower. Family and IO are drawn as
/// cables and conduits in the dot mesh (`city.rs`).
fn draw_lines(
    sl: Res<SceneLayout>,
    sel: Res<Selection>,
    ex: Res<crate::explore::Explore>,
    mut gizmos: Gizmos,
) {
    if ex.inside().is_some() {
        return;
    }
    if let Some(c) = sel.key.and_then(|k| sl.layout.column(&k)) {
        gizmos.line(
            c.top() + Vec3::Y * 0.2,
            c.top() + Vec3::Y * 2.5,
            Color::LinearRgba(LinearRgba::rgb(2.0, 2.4, 3.0)),
        );
    }
}
