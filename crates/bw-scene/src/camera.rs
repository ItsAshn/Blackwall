//! Orbit camera with mouse, touch and keyboard control (PLAN §10).
//!
//! Keyboard movement uses *physical* keys (layout-independent); character
//! shortcuts live in the UI and use logical keys (PLAN §3.4).

use crate::*;
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input::touch::Touches;

#[derive(Component)]
pub struct MainCamera;

/// Orbit state. Fields prefixed `t_` are targets; the rest are smoothed.
#[derive(Resource, Debug, Clone)]
pub struct OrbitCam {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub t_focus: Vec3,
    pub t_yaw: f32,
    pub t_pitch: f32,
    pub t_dist: f32,
    /// Seconds since the last user input; drives the idle drift.
    pub idle: f32,
    /// Set once the camera has been framed on the first layout.
    pub framed: bool,
}

impl Default for OrbitCam {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.42,
            dist: 80.0,
            t_focus: Vec3::ZERO,
            t_yaw: 0.0,
            t_pitch: 0.42,
            t_dist: 80.0,
            idle: 0.0,
            framed: false,
        }
    }
}

impl OrbitCam {
    /// Frame the whole scene: a low, diagonal look across the city at the
    /// Wall, like standing on the floor of the Blackwall's chamber.
    pub fn frame_all(&mut self, extent: f32) {
        self.t_focus = Vec3::new(0.0, 1.5, -extent * 0.35);
        self.t_dist = extent * 1.9 + 10.0;
        self.t_pitch = 0.3;
        self.t_yaw = -0.55;
    }

    fn eye(&self) -> Vec3 {
        self.focus
            + self.dist
                * Vec3::new(
                    self.pitch.cos() * self.yaw.sin(),
                    self.pitch.sin(),
                    self.pitch.cos() * self.yaw.cos(),
                )
    }
}

/// Written by the UI each frame: input the UI has claimed.
#[derive(Resource, Default, Debug)]
pub struct InputBlock {
    pub pointer: bool,
    pub keyboard: bool,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<OrbitCam>()
        .init_resource::<InputBlock>()
        .add_systems(Startup, spawn_camera)
        .add_systems(
            Update,
            (mouse_touch_input, keyboard_input, follow_and_apply)
                .chain()
                .after(SceneSet::Layout)
                .before(SceneSet::Visuals),
        );
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        MainCamera,
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.004, 0.003, 0.012)),
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            fov: 0.75,
            far: 2000.0,
            ..default()
        }),
        Tonemapping::TonyMcMapface,
        DebandDither::Enabled,
        Transform::from_xyz(0.0, 30.0, 80.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn mouse_touch_input(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    touches: Res<Touches>,
    block: Res<InputBlock>,
    mut cam: ResMut<OrbitCam>,
    mut sel: ResMut<Selection>,
    mut pinch_prev: Local<Option<f32>>,
) {
    if block.pointer {
        *pinch_prev = None;
        return;
    }
    let d = motion.delta;
    let mut active = false;
    if buttons.pressed(MouseButton::Left) && d != Vec2::ZERO {
        cam.t_yaw -= d.x * 0.005;
        cam.t_pitch = (cam.t_pitch + d.y * 0.004).clamp(-1.2, 1.45);
        active = true;
    }
    if (buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle))
        && d != Vec2::ZERO
    {
        pan(&mut cam, d);
        sel.follow = false;
        active = true;
    }
    if scroll.delta.y != 0.0 {
        let step = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y * 0.12,
            MouseScrollUnit::Pixel => scroll.delta.y * 0.002,
        };
        cam.t_dist = (cam.t_dist * (1.0 - step)).clamp(2.0, 2000.0);
        active = true;
    }

    // Touch: one finger orbits, two fingers pinch-zoom and pan (PLAN §10).
    let ts: Vec<_> = touches.iter().collect();
    match ts.as_slice() {
        [t] => {
            let d = t.delta();
            cam.t_yaw -= d.x * 0.006;
            cam.t_pitch = (cam.t_pitch + d.y * 0.005).clamp(-1.2, 1.45);
            *pinch_prev = None;
            active |= d != Vec2::ZERO;
        }
        [a, b, ..] => {
            let dist = a.position().distance(b.position());
            if let Some(prev) = *pinch_prev
                && prev > 1.0
            {
                cam.t_dist = (cam.t_dist * prev / dist.max(1.0)).clamp(2.0, 2000.0);
            }
            *pinch_prev = Some(dist);
            let avg = (a.delta() + b.delta()) * 0.5;
            if avg.length() > 0.5 {
                pan(&mut cam, avg);
                sel.follow = false;
            }
            active = true;
        }
        [] => *pinch_prev = None,
    }
    if active {
        cam.idle = 0.0;
    }
}

fn pan(cam: &mut OrbitCam, d: Vec2) {
    let right = Vec3::new(cam.yaw.cos(), 0.0, -cam.yaw.sin());
    let up = Vec3::Y;
    let k = cam.dist * 0.0016;
    cam.t_focus += (-right * d.x + up * d.y) * k;
}

fn keyboard_input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    block: Res<InputBlock>,
    sl: Res<SceneLayout>,
    mut cam: ResMut<OrbitCam>,
    mut sel: ResMut<Selection>,
) {
    if block.keyboard {
        return;
    }
    let dt = time.delta_secs();
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mut active = keys.get_just_pressed().next().is_some();

    // WASD pans across the plane, relative to the view direction.
    let fwd = Vec3::new(-cam.yaw.sin(), 0.0, -cam.yaw.cos());
    let right = Vec3::new(cam.yaw.cos(), 0.0, -cam.yaw.sin());
    let mut mv = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        mv += fwd;
    }
    if keys.pressed(KeyCode::KeyS) {
        mv -= fwd;
    }
    if keys.pressed(KeyCode::KeyD) {
        mv += right;
    }
    if keys.pressed(KeyCode::KeyA) {
        mv -= right;
    }
    if keys.pressed(KeyCode::KeyR) {
        mv += Vec3::Y;
    }
    if keys.pressed(KeyCode::KeyF) && shift {
        mv -= Vec3::Y;
    }
    if mv != Vec3::ZERO {
        let step = mv.normalize() * cam.dist * 0.8 * dt;
        cam.t_focus += step;
        sel.follow = false;
        active = true;
    }
    // Shift + arrows orbit (plain arrows walk the process tree, see nav.rs).
    if shift {
        let mut o = Vec2::ZERO;
        if keys.pressed(KeyCode::ArrowLeft) {
            o.x += 1.0;
        }
        if keys.pressed(KeyCode::ArrowRight) {
            o.x -= 1.0;
        }
        if keys.pressed(KeyCode::ArrowUp) {
            o.y += 1.0;
        }
        if keys.pressed(KeyCode::ArrowDown) {
            o.y -= 1.0;
        }
        cam.t_yaw += o.x * dt * 1.2;
        cam.t_pitch = (cam.t_pitch + o.y * dt * 0.9).clamp(-1.2, 1.45);
    }
    let zoom = keys.pressed(KeyCode::PageUp) as i32 - keys.pressed(KeyCode::PageDown) as i32
        + keys.pressed(KeyCode::Equal) as i32
        - keys.pressed(KeyCode::Minus) as i32
        + keys.pressed(KeyCode::NumpadAdd) as i32
        - keys.pressed(KeyCode::NumpadSubtract) as i32;
    if zoom != 0 {
        cam.t_dist = (cam.t_dist * (1.0 - zoom as f32 * dt * 1.5)).clamp(2.0, 2000.0);
    }
    if keys.just_pressed(KeyCode::Home) {
        sel.follow = false;
        cam.frame_all(sl.layout.extent);
    }
    if keys.just_pressed(KeyCode::KeyF) && !shift {
        sel.follow = !sel.follow;
    }
    if active {
        cam.idle = 0.0;
    }
}

#[allow(clippy::too_many_arguments)]
fn follow_and_apply(
    time: Res<Time>,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    sel: Res<Selection>,
    settings: Res<SceneSettings>,
    ents: Res<ProcEntities>,
    shown: Query<&Shown>,
    mut cam: ResMut<OrbitCam>,
    mut tf: Query<&mut Transform, With<MainCamera>>,
    mut last_sel: Local<Option<ProcKey>>,
) {
    let dt = time.delta_secs();
    if m.received && !cam.framed && sl.layout.extent > 0.0 {
        cam.framed = true;
        cam.frame_all(sl.layout.extent);
        let (f, d) = (cam.t_focus, cam.t_dist);
        cam.focus = f;
        cam.dist = d * 1.6; // fly in on start
    }
    cam.idle += dt;
    if let Some(s) = sel
        .key
        .and_then(|k| ents.0.get(&k))
        .and_then(|e| shown.get(*e).ok())
    {
        if sel.follow {
            cam.t_focus = s.pos;
        }
        if *last_sel != sel.key {
            cam.t_dist = (s.radius * 12.0 + 7.0).clamp(6.0, 40.0);
        }
    }
    *last_sel = sel.key;
    // Idle drift: a slow orbit when nobody has touched anything for a while.
    if cam.idle > 45.0 && !settings.reduced_motion {
        cam.t_yaw += dt * 0.03;
    }
    let k = 1.0 - (-dt * 5.0).exp();
    cam.focus = cam.focus.lerp(cam.t_focus, k);
    cam.yaw += (cam.t_yaw - cam.yaw) * k;
    cam.pitch += (cam.t_pitch - cam.pitch) * k;
    cam.dist += (cam.t_dist - cam.dist) * (1.0 - (-dt * 3.0).exp());
    if let Ok(mut t) = tf.single_mut() {
        *t = Transform::from_translation(cam.eye()).looking_at(cam.focus, Vec3::Y);
    }
}
