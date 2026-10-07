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

/// Length of the jack-in sequence (design system: `jack-in`).
pub const JACK_IN_SECS: f32 = 3.2;

/// The entry sequence: falling through the Wall's rain into the chamber.
/// 0–1.2s the camera drops through racing rain close to the Wall; then it
/// pulls back to the overview while the city resolves out of black.
#[derive(Resource, Debug, Clone)]
pub struct JackIn {
    /// Off with `--no-intro` or reduced motion.
    pub enabled: bool,
    /// Seconds since it started; `None` until the first data arrives.
    pub elapsed: Option<f32>,
    pub done: bool,
    start: (Vec3, f32, f32, f32),
    end: (Vec3, f32, f32, f32),
}

impl Default for JackIn {
    fn default() -> Self {
        Self {
            enabled: true,
            elapsed: None,
            done: false,
            start: (Vec3::ZERO, 0.0, 0.0, 1.0),
            end: (Vec3::ZERO, 0.0, 0.0, 1.0),
        }
    }
}

impl JackIn {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub fn running(&self) -> bool {
        self.elapsed.is_some() && !self.done
    }

    /// How much faster the rain falls: you are falling through it.
    pub fn rain_boost(&self) -> f32 {
        match self.elapsed {
            Some(t) if !self.done => 1.0 + 7.0 * (1.0 - smooth(0.0, 1.8, t)),
            _ => 1.0,
        }
    }

    /// Delay before the city starts rising, read when the first data arrives.
    pub fn city_delay(&self) -> f32 {
        if self.enabled && !self.done { 1.2 } else { 0.0 }
    }
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
        .init_resource::<JackIn>()
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
    mut cam: ResMut<OrbitCam>,
) {
    if block.keyboard {
        return;
    }
    let dt = time.delta_secs();
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let active = keys.get_just_pressed().next().is_some();

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
    if active {
        cam.idle = 0.0;
    }
}

#[allow(clippy::too_many_arguments)]
fn follow_and_apply(
    time: Res<Time>,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    mut cam: ResMut<OrbitCam>,
    mut jack: ResMut<JackIn>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    mut tf: Query<&mut Transform, With<MainCamera>>,
) {
    let dt = time.delta_secs();
    if m.received && !cam.framed && sl.layout.extent > 0.0 {
        cam.framed = true;
        cam.frame_all(sl.layout.extent);
        if jack.enabled && !settings.reduced_motion {
            // Start inside the rain, a few units in front of the Wall.
            jack.start = (Vec3::new(0.0, 18.0, sl.layout.wall_z()), 0.0, 0.05, 13.0);
            jack.end = (cam.t_focus, cam.t_yaw, cam.t_pitch, cam.t_dist);
            jack.elapsed = Some(0.0);
        } else {
            jack.done = true;
            let (f, d) = (cam.t_focus, cam.t_dist);
            cam.focus = f;
            cam.dist = d;
            cam.yaw = cam.t_yaw;
            cam.pitch = cam.t_pitch;
        }
    }
    if jack.running() {
        let skip = keys.get_just_pressed().next().is_some()
            || buttons.get_just_pressed().next().is_some()
            || touches.iter_just_pressed().next().is_some();
        let e = jack.elapsed.unwrap_or(0.0) + dt;
        jack.elapsed = Some(e);
        let (sf, sy, sp, sd) = jack.start;
        let (ef, ey, ep, ed) = jack.end;
        if skip || e >= JACK_IN_SECS {
            jack.done = true;
            (cam.t_focus, cam.t_yaw, cam.t_pitch, cam.t_dist) = (ef, ey, ep, ed);
        } else {
            // Fall: drop through the rain, then pull back as the city forms.
            let fall = smooth(0.0, 1.4, e);
            let pull = smooth(1.0, 3.0, e);
            let falling = sf - Vec3::Y * 7.0 * fall;
            cam.focus = falling.lerp(ef, pull);
            cam.yaw = sy + (ey - sy) * pull;
            cam.pitch = sp + (ep - sp) * pull;
            cam.dist = sd + (ed - sd) * pull * pull;
            (cam.t_focus, cam.t_yaw, cam.t_pitch, cam.t_dist) =
                (cam.focus, cam.yaw, cam.pitch, cam.dist);
            if let Ok(mut t) = tf.single_mut() {
                *t = Transform::from_translation(cam.eye()).looking_at(cam.focus, Vec3::Y);
            }
            return;
        }
    }
    cam.idle += dt;
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
        // Never below the floor: looking up at a tower stands you on it.
        let mut eye = cam.eye();
        eye.y = eye.y.max(0.25);
        *t = Transform::from_translation(eye).looking_at(cam.focus, Vec3::Y);
    }
}
