//! Semantic zoom: one continuous map, from the whole machine down into a
//! single process.
//!
//! * Drag (left or middle button, one finger) pans the ground.
//! * Scroll, pinch, +/− zoom toward the cursor. The view tilts with the
//!   zoom: a steep map from far out, a low street-level look close in.
//! * Right-drag or Q/E turns; right-drag up/down adjusts the tilt.
//! * WASD or the arrow keys pan.
//!
//! Choosing and opening towers lives in `explore.rs`; this module only moves
//! the eye. Keyboard movement uses *physical* keys (layout-independent);
//! character shortcuts live in the UI and use logical keys (PLAN §3.4).

use crate::*;
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input::touch::Touches;
use bevy::window::PrimaryWindow;

#[derive(Component)]
pub struct MainCamera;

/// Map camera state. Fields prefixed `t_` are targets; the rest are smoothed.
#[derive(Resource, Debug, Clone)]
pub struct OrbitCam {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub t_focus: Vec3,
    pub t_yaw: f32,
    /// The user's tilt on top of the automatic, zoom-driven one.
    pub tilt: f32,
    pub t_dist: f32,
    /// Closest the eye may come (smaller inside a process).
    pub min_dist: f32,
    /// Seconds since the last user input; drives the idle drift.
    pub idle: f32,
    /// Set once the camera has been framed on the first layout.
    pub framed: bool,
    /// Set by any user movement this frame (cancels follow).
    pub moved: bool,
    /// Extra tilt that lifts the view over towers blocking it (smoothed).
    pub lift: f32,
}

impl Default for OrbitCam {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: -0.6,
            pitch: 1.0,
            dist: 80.0,
            t_focus: Vec3::ZERO,
            t_yaw: -0.6,
            tilt: 0.0,
            t_dist: 80.0,
            min_dist: 1.2,
            idle: 0.0,
            framed: false,
            moved: false,
            lift: 0.0,
        }
    }
}

/// The automatic tilt: a steep map far out, a low look close in.
fn auto_pitch(dist: f32) -> f32 {
    let x = ((dist.max(0.01).ln() - 1.0) / 3.2).clamp(0.0, 1.0);
    0.16 + x * x * (3.0 - 2.0 * x) * 0.62
}

impl OrbitCam {
    /// The whole machine, as a map.
    pub fn frame_all(&mut self, extent: f32) {
        self.t_focus = Vec3::ZERO;
        self.t_dist = extent * 2.7 + 6.0;
        self.tilt = 0.0;
    }

    /// Frame a box: focus on its center, far enough to see all of it.
    pub fn frame(&mut self, center: Vec3, size: f32) {
        self.t_focus = center;
        self.t_dist = (size * 1.9 + 1.5).max(self.min_dist * 1.5);
    }

    pub fn eye(&self) -> Vec3 {
        self.focus
            + self.dist
                * Vec3::new(
                    self.pitch.cos() * self.yaw.sin(),
                    self.pitch.sin(),
                    self.pitch.cos() * self.yaw.cos(),
                )
    }

    fn pan(&mut self, d: Vec2) {
        // Screen drag moves the ground under the cursor: right and forward
        // on the floor plane, scaled by the distance.
        let right = Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin());
        let fwd = Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos());
        let k = self.dist * 0.0018;
        self.t_focus += (-right * d.x + fwd * d.y) * k;
        self.moved = true;
    }
}

/// Fog range from the eye: close in, the crowd sinks into black just past
/// what you are looking at; far out, the whole map stays visible.
pub fn fog(cam: &OrbitCam) -> (f32, f32) {
    (cam.dist * 0.9 + 2.0, cam.dist * 2.2 + 12.0)
}

/// Length of the arrival sequence.
pub const JACK_IN_SECS: f32 = 2.6;

/// Arrival: dropping from high above onto the map while the city rises.
#[derive(Resource, Debug, Clone)]
pub struct JackIn {
    /// Off with `--no-intro` or reduced motion.
    pub enabled: bool,
    /// Seconds since it started; `None` until the first data arrives.
    pub elapsed: Option<f32>,
    pub done: bool,
}

impl Default for JackIn {
    fn default() -> Self {
        Self {
            enabled: true,
            elapsed: None,
            done: false,
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

    /// How much faster the Wall's rain falls during arrival.
    pub fn rain_boost(&self) -> f32 {
        match self.elapsed {
            Some(t) if !self.done => 1.0 + 5.0 * (1.0 - smooth(0.0, 1.8, t)),
            _ => 1.0,
        }
    }

    /// Delay before the city starts rising, read when the first data arrives.
    pub fn city_delay(&self) -> f32 {
        if self.enabled && !self.done { 0.6 } else { 0.0 }
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
            (pointer_input, keyboard_input, apply)
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
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            fov: 0.75,
            near: 0.005,
            far: 2000.0,
            ..default()
        }),
        Tonemapping::TonyMcMapface,
        DebandDither::Enabled,
        Transform::from_xyz(0.0, 60.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Whether the segment from `o` along unit `d` for `len` crosses the box.
fn segment_hits(o: Vec3, d: Vec3, len: f32, min: Vec3, max: Vec3) -> bool {
    let inv = d.recip();
    let t1 = (min - o) * inv;
    let t2 = (max - o) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    tmax >= tmin.max(0.0) && tmin < len
}

/// Where the ray under the cursor meets the horizontal plane at `y`.
fn ground_hit(cam: &Camera, tf: &GlobalTransform, screen: Vec2, y: f32) -> Option<Vec3> {
    let ray = cam.viewport_to_world(tf, screen).ok()?;
    let d = *ray.direction;
    if d.y.abs() < 1e-4 {
        return None;
    }
    let t = (y - ray.origin.y) / d.y;
    (t > 0.0).then(|| ray.origin + d * t)
}

#[allow(clippy::too_many_arguments)]
fn pointer_input(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    touches: Res<Touches>,
    block: Res<InputBlock>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut cam: ResMut<OrbitCam>,
    mut pinch_prev: Local<Option<f32>>,
) {
    cam.moved = false;
    if block.pointer {
        *pinch_prev = None;
        return;
    }
    let d = motion.delta;
    if (buttons.pressed(MouseButton::Left) || buttons.pressed(MouseButton::Middle))
        && d.length() > 0.0
    {
        cam.pan(d);
    }
    if buttons.pressed(MouseButton::Right) && d != Vec2::ZERO {
        cam.t_yaw -= d.x * 0.005;
        cam.tilt = (cam.tilt + d.y * 0.003).clamp(-0.5, 0.6);
        cam.moved = true;
    }
    if scroll.delta.y != 0.0 {
        let step = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y * 0.14,
            MouseScrollUnit::Pixel => scroll.delta.y * 0.0025,
        }
        .clamp(-0.6, 0.6);
        let f = 1.0 - step;
        let cursor = window.single().ok().and_then(|w| w.cursor_position());
        let hit = match (cursor, camera.single()) {
            (Some(c), Ok((camera, tf))) => ground_hit(camera, tf, c, cam.t_focus.y),
            _ => None,
        };
        zoom(&mut cam, f, hit);
    }

    // Touch: one finger pans, two fingers pinch to zoom.
    let ts: Vec<_> = touches.iter().collect();
    match ts.as_slice() {
        [t] => {
            if t.delta() != Vec2::ZERO {
                cam.pan(t.delta());
            }
            *pinch_prev = None;
        }
        [a, b, ..] => {
            let dist = a.position().distance(b.position());
            if let Some(prev) = *pinch_prev
                && prev > 1.0
            {
                zoom(&mut cam, prev / dist.max(1.0), None);
            }
            *pinch_prev = Some(dist);
        }
        [] => *pinch_prev = None,
    }
}

/// Zoom by factor `f` (< 1 zooms in), toward `toward` if given.
fn zoom(cam: &mut OrbitCam, f: f32, toward: Option<Vec3>) {
    let before = cam.t_dist;
    cam.t_dist = (cam.t_dist * f).clamp(cam.min_dist, 600.0);
    let real = cam.t_dist / before;
    if let Some(p) = toward
        && real < 1.0
    {
        cam.t_focus += (p - cam.t_focus) * (1.0 - real);
    }
    cam.moved = true;
}

fn keyboard_input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    block: Res<InputBlock>,
    ex: Res<crate::explore::Explore>,
    mut cam: ResMut<OrbitCam>,
) {
    if block.keyboard {
        return;
    }
    let dt = time.delta_secs();
    let held = |k: &[KeyCode]| keys.any_pressed(k.iter().copied()) as i32 as f32;
    // Inside a tower the arrows walk its elements; WASD still pans.
    let arrows = ex.inside().is_none();
    let (r, l, u, d) = if arrows {
        (
            KeyCode::ArrowRight,
            KeyCode::ArrowLeft,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
        )
    } else {
        (KeyCode::KeyD, KeyCode::KeyA, KeyCode::KeyW, KeyCode::KeyS)
    };
    let x = held(&[KeyCode::KeyD, r]) - held(&[KeyCode::KeyA, l]);
    let y = held(&[KeyCode::KeyW, u]) - held(&[KeyCode::KeyS, d]);
    if x != 0.0 || y != 0.0 {
        cam.pan(Vec2::new(x, -y) * dt * 500.0);
    }
    let turn = held(&[KeyCode::KeyE]) - held(&[KeyCode::KeyQ]);
    if turn != 0.0 {
        cam.t_yaw += turn * dt * 1.4;
        cam.moved = true;
    }
    let z = held(&[KeyCode::Equal, KeyCode::NumpadAdd, KeyCode::PageUp])
        - held(&[KeyCode::Minus, KeyCode::NumpadSubtract, KeyCode::PageDown]);
    if z != 0.0 {
        zoom(&mut cam, 1.0 - z * dt * 1.6, None);
    }
}

#[allow(clippy::too_many_arguments)]
fn apply(
    time: Res<Time>,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
    mut cam: ResMut<OrbitCam>,
    mut jack: ResMut<JackIn>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    ex: Res<crate::explore::Explore>,
    sel: Res<Selection>,
    mut tf: Query<&mut Transform, With<MainCamera>>,
) {
    let dt = time.delta_secs();
    if m.received && !cam.framed && sl.layout.extent > 0.0 {
        cam.framed = true;
        cam.frame_all(sl.layout.extent);
        if jack.enabled && !settings.reduced_motion {
            // Arrive from high above, turned a little away.
            cam.focus = Vec3::ZERO;
            cam.dist = cam.t_dist * 3.0;
            cam.yaw = cam.t_yaw - 0.9;
            jack.elapsed = Some(0.0);
        } else {
            jack.done = true;
            cam.focus = cam.t_focus;
            cam.dist = cam.t_dist;
            cam.yaw = cam.t_yaw;
        }
    }
    if jack.running() {
        let e = jack.elapsed.unwrap_or(0.0) + dt;
        jack.elapsed = Some(e);
        if e >= JACK_IN_SECS
            || keys.get_just_pressed().next().is_some()
            || buttons.get_just_pressed().next().is_some()
        {
            jack.done = true;
        }
    }
    if cam.moved {
        cam.idle = 0.0;
    } else {
        cam.idle += dt;
    }
    // Idle drift: a slow turn when nobody has touched anything for a while.
    if cam.idle > 45.0 && !settings.reduced_motion {
        cam.t_yaw += dt * 0.02;
    }
    // Keep the eye over the map.
    let lim = sl.layout.extent.max(4.0) + 6.0;
    cam.t_focus.x = cam.t_focus.x.clamp(-lim, lim);
    cam.t_focus.z = cam.t_focus.z.clamp(-lim, lim + 4.0);
    cam.t_dist = cam.t_dist.max(cam.min_dist);

    let speed = if jack.running() { 1.6 } else { 6.0 };
    let k = 1.0 - (-dt * speed).exp();
    cam.focus = cam.focus.lerp(cam.t_focus, k);
    cam.yaw += (cam.t_yaw - cam.yaw) * k;
    // Zoom eases in log space, so diving 100× feels as smooth as 2×.
    let kd = 1.0 - (-dt * speed * 0.7).exp();
    cam.dist = (cam.dist.ln() + (cam.t_dist.ln() - cam.dist.ln()) * kd).exp();
    // Keep the view clear: if a tower stands between the eye and the focus,
    // tilt up until it doesn't (in steps), then ease toward that tilt.
    let base = (auto_pitch(cam.dist) + cam.tilt).clamp(0.05, 1.45);
    let open = ex.inside();
    let chosen = sel.key;
    let blocked = |pitch: f32| {
        let mut probe = cam.clone();
        probe.pitch = pitch;
        let eye = probe.eye();
        let dir = cam.focus - eye;
        let len = dir.length();
        sl.layout.columns.iter().any(|c| {
            Some(c.key) != open
                && Some(c.key) != chosen
                && segment_hits(
                    eye,
                    dir / len.max(1e-6),
                    len * 0.97,
                    Vec3::new(c.min.x, 0.0, c.min.y),
                    Vec3::new(c.max.x, c.height(), c.max.y),
                )
        })
    };
    // With something chosen the cutaway shows it instead (towers.wgsl).
    let mut need = 0.0;
    while chosen.is_none() && need < 1.2 && blocked(base + need) {
        need += 0.1;
    }
    cam.lift += (need - cam.lift) * (1.0 - (-dt * 3.0).exp());
    cam.pitch = (base + cam.lift).clamp(0.05, 1.5);
    if let Ok(mut t) = tf.single_mut() {
        let mut eye = cam.eye();
        eye.y = eye.y.max(cam.min_dist * 0.1);
        *t = Transform::from_translation(eye).looking_at(cam.focus, Vec3::Y);
    }
}
