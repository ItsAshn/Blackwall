//! Picking with the mouse or a tap: a ray against each tower's box (and,
//! inside an open tower, against its interior elements first). Cheaper than
//! triangle picking, and identical on every backend.
//!
//! Click a tower to choose it; click the chosen tower again to open it.
//! Inside, click an element to go to it. A press that drags pans instead.

use crate::camera::{InputBlock, MainCamera};
use crate::explore::Explore;
use crate::interior::ElementKind;
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

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Tower(ProcKey),
    Element(usize),
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
    mut ex: ResMut<Explore>,
    mut press: Local<Option<Vec2>>,
) {
    let (Ok(window), Ok((cam, cam_tf))) = (window.single(), camera.single()) else {
        return;
    };
    let hit_at = |screen: Vec2, ex: &Explore| -> Option<Hit> {
        let ray = cam.viewport_to_world(cam_tf, screen).ok()?;
        let dir: Vec3 = *ray.direction;
        if ex.inside().is_some() {
            let local = ex.to_local(ray.origin);
            if let Some(i) = crate::interior::pick(&ex.interior, local, dir) {
                return Some(Hit::Element(i));
            }
        }
        sl.layout
            .columns
            .iter()
            .filter(|c| settings.show_kernel || c.realm == bw_model::Realm::User)
            // The open tower is an outline: look through it.
            .filter(|c| ex.inside() != Some(c.key))
            .filter_map(|c| {
                ray_box(
                    ray.origin,
                    dir,
                    Vec3::new(c.min.x, 0.0, c.min.y),
                    Vec3::new(c.max.x, c.height(), c.max.y),
                )
                .map(|t| (t, c.key))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, k)| Hit::Tower(k))
    };
    let activate = |h: Hit, ex: &mut Explore, sel: &mut Selection| match h {
        Hit::Tower(k) if sel.key == Some(k) && ex.inside().is_none() => ex.dive(k),
        Hit::Tower(k) => {
            sel.key = Some(k);
            sel.follow = true;
        }
        Hit::Element(i) => {
            // A child's link inside: go to that child's tower.
            if let ElementKind::Satellite { child } = ex.interior.elements[i].kind {
                sel.key = Some(child);
                sel.follow = true;
            } else {
                ex.choose(Some(i));
            }
        }
    };

    // Taps choose (a short touch that didn't drag).
    for t in touches.iter_just_released() {
        if !block.pointer
            && t.start_position().distance(t.position()) < 12.0
            && let Some(h) = hit_at(t.position(), &ex)
        {
            activate(h, &mut ex, &mut sel);
        }
    }

    let Some(cursor) = window.cursor_position().filter(|_| !block.pointer) else {
        sel.hovered = None;
        ex.hovered = None;
        *press = None;
        return;
    };
    let hovered = hit_at(cursor, &ex);
    let (ht, he) = match hovered {
        Some(Hit::Tower(k)) => (Some(k), None),
        Some(Hit::Element(i)) => (None, Some(i)),
        None => (None, None),
    };
    if sel.hovered != ht {
        sel.hovered = ht;
    }
    if ex.hovered != he {
        ex.hovered = he;
    }
    if buttons.just_pressed(MouseButton::Left) {
        *press = Some(cursor);
    }
    // A click is a press and release without dragging (dragging pans).
    if buttons.just_released(MouseButton::Left) {
        if press.is_some_and(|p| p.distance(cursor) < 5.0)
            && let Some(h) = hovered
        {
            activate(h, &mut ex, &mut sel);
        }
        *press = None;
    }
}
