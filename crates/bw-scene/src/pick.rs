//! Picking columns with the mouse or a tap: a ray against each column's box.
//! Cheaper than triangle picking on the dot mesh, and it works identically
//! on every backend.

use crate::camera::{InputBlock, MainCamera};
use crate::*;
use bevy::input::touch::Touches;
use bevy::window::PrimaryWindow;

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        pick.after(SceneSet::Layout).before(SceneSet::Visuals),
    );
}

/// Slab test: distance along the ray to the box, if hit.
fn ray_box(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = dir.recip();
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    (tmax >= tmin.max(0.0)).then_some(tmin.max(0.0))
}

#[allow(clippy::too_many_arguments)]
fn pick(
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    buttons: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    block: Res<InputBlock>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    mut sel: ResMut<Selection>,
    mut press: Local<Option<Vec2>>,
) {
    let (Ok(window), Ok((cam, cam_tf))) = (window.single(), camera.single()) else {
        return;
    };
    let hit_at = |screen: Vec2| -> Option<ProcKey> {
        let ray = cam.viewport_to_world(cam_tf, screen).ok()?;
        let dir: Vec3 = *ray.direction;
        sl.layout
            .columns
            .iter()
            .filter(|c| settings.show_kernel || c.realm == bw_model::Realm::User)
            .filter_map(|c| {
                let r = 0.18 + c.footprint as f32 * 0.12;
                ray_box(
                    ray.origin,
                    dir,
                    c.base - Vec3::new(r, 0.0, r),
                    c.top() + Vec3::new(r, 0.1, r),
                )
                .map(|t| (t, c.key))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, k)| k)
    };

    // Taps select (a short touch that didn't drag).
    for t in touches.iter_just_released() {
        if !block.pointer
            && t.start_position().distance(t.position()) < 12.0
            && let Some(k) = hit_at(t.position())
        {
            sel.key = Some(k);
            sel.follow = true;
        }
    }

    let Some(cursor) = window.cursor_position() else {
        sel.hovered = None;
        return;
    };
    if block.pointer {
        sel.hovered = None;
        *press = None;
        return;
    }
    let hovered = hit_at(cursor);
    if sel.hovered != hovered {
        sel.hovered = hovered;
    }
    if buttons.just_pressed(MouseButton::Left) {
        *press = Some(cursor);
    }
    // A click is a press and release without dragging (dragging orbits).
    if buttons.just_released(MouseButton::Left) {
        if press.is_some_and(|p| p.distance(cursor) < 5.0)
            && let Some(k) = hovered
        {
            sel.key = Some(k);
            sel.follow = true;
        }
        *press = None;
    }
}
