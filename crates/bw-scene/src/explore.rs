//! Guided exploration: three levels, moved between by choosing, never by
//! free flight.
//!
//! 1. **Machine**: towers at human scale. Choosing one (click, arrows, Tab,
//!    N for the next tower with an issue) flies the camera low to its foot.
//! 2. **Inside a process**: Enter dives through black into a world of its
//!    own (see `interior.rs`).
//! 3. **An element**: a floor, stratum, conduit or satellite; the camera
//!    flies to it. N visits the next anomaly. Enter on a satellite dives
//!    into that child. Esc climbs back out one level at a time.

use crate::camera::{InputBlock, OrbitCam};
use crate::city::DotMeshBuilder;
use crate::interior::{self, Anomaly, ELEMENT_ID_BASE, ElementKind, Interior};
use crate::*;
use bw_model::{Health, ProcessDetail, Realm};
use crossbeam_channel::Sender;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Explore>()
        .init_resource::<SourceFocus>()
        .add_systems(
            Update,
            (
                explore_input,
                run_fade,
                rebuild_interior,
                place_camera,
                layer_visibility,
            )
                .chain()
                .after(SceneSet::Layout)
                .before(SceneSet::Visuals),
        );
}

/// Where the explorer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Machine,
    Inside(ProcKey),
}

/// Tells the source which process to collect internals for.
#[derive(Resource, Default)]
pub struct SourceFocus(pub Option<Sender<Option<ProcKey>>>);

#[derive(Resource)]
pub struct Explore {
    pub level: Level,
    /// The level being faded toward; switched at full black.
    pending: Option<Level>,
    /// 0 = clear, 1 = black.
    pub fade: f32,
    pub interior: Interior,
    pub anomalies: Vec<Anomaly>,
    pub detail: Option<ProcessDetail>,
    history: Vec<ProcessDetail>,
    /// Chosen and hovered interior elements (indices into `interior.elements`).
    pub element: Option<usize>,
    pub hovered: Option<usize>,
    /// When the current interior was entered, for its grow-in.
    entered_at: f32,
    /// Bumped whenever the camera should fly to a new station.
    station: u64,
    pub interior_mesh: Option<Handle<Mesh>>,
}

impl Default for Explore {
    fn default() -> Self {
        Self {
            level: Level::Machine,
            pending: None,
            fade: 0.0,
            interior: Interior::default(),
            anomalies: vec![],
            detail: None,
            history: vec![],
            element: None,
            hovered: None,
            entered_at: 0.0,
            station: 0,
            interior_mesh: None,
        }
    }
}

impl Explore {
    pub fn inside(&self) -> Option<ProcKey> {
        match self.level {
            Level::Inside(k) => Some(k),
            Level::Machine => None,
        }
    }

    pub fn dive(&mut self, k: ProcKey) {
        info!("explore: dive requested into {k}");
        if self.pending.is_none() {
            self.pending = Some(Level::Inside(k));
        }
    }

    pub fn surface(&mut self) {
        if self.pending.is_none() && self.inside().is_some() {
            self.pending = Some(Level::Machine);
        }
    }

    pub fn choose(&mut self, element: Option<usize>) {
        self.element = element;
        self.station += 1;
    }

    pub fn element_id(&self, i: Option<usize>) -> f32 {
        i.map_or(-1.0, |i| (ELEMENT_ID_BASE + i) as f32)
    }
}

/// Marks entities that belong to the machine level (hidden inside a process).
#[derive(Component)]
pub struct MachineLayer;

/// The interior's dot mesh.
#[derive(Component)]
pub struct InteriorLayer;

fn order_by<F: Fn(&ElementKind) -> bool>(it: &Interior, f: F, by_height: bool) -> Vec<usize> {
    let mut v: Vec<usize> = it
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| f(&e.kind))
        .map(|(i, _)| i)
        .collect();
    if by_height {
        v.sort_by(|a, b| {
            it.elements[*a]
                .anchor
                .y
                .total_cmp(&it.elements[*b].anchor.y)
        });
    }
    v
}

fn step(list: &[usize], cur: Option<usize>, dir: i32) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    match cur.and_then(|c| list.iter().position(|x| *x == c)) {
        Some(i) => Some(list[(i as i32 + dir).rem_euclid(list.len() as i32) as usize]),
        None => Some(if dir >= 0 {
            list[0]
        } else {
            list[list.len() - 1]
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn explore_input(
    keys: Res<ButtonInput<KeyCode>>,
    block: Res<InputBlock>,
    m: Res<Machine>,
    mut ex: ResMut<Explore>,
    mut sel: ResMut<Selection>,
    mut cam: ResMut<OrbitCam>,
    sl: Res<SceneLayout>,
) {
    if block.keyboard || ex.pending.is_some() {
        return;
    }
    match ex.level {
        Level::Machine => {
            if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
                && let Some(k) = sel.key
            {
                ex.dive(k);
            }
            // N: the next tower with an issue.
            if keys.just_pressed(KeyCode::KeyN) {
                let mut flagged: Vec<&bw_model::Process> = m
                    .snapshot
                    .processes
                    .values()
                    .filter(|p| p.health() != Health::Healthy && p.realm == Realm::User)
                    .collect();
                flagged.sort_by(|a, b| b.health().cmp(&a.health()).then(a.key.cmp(&b.key)));
                let keys: Vec<ProcKey> = flagged.iter().map(|p| p.key).collect();
                let next = match sel.key.and_then(|k| keys.iter().position(|x| *x == k)) {
                    Some(i) => keys.get((i + 1) % keys.len()).copied(),
                    None => keys.first().copied(),
                };
                if next.is_some() {
                    sel.key = next;
                    sel.follow = true;
                }
            }
            if keys.just_pressed(KeyCode::Home) {
                sel.key = None;
                cam.frame_all(sl.layout.extent);
            }
        }
        Level::Inside(_) => {
            let it = &ex.interior;
            let vertical = order_by(
                it,
                |k| matches!(k, ElementKind::Floor { .. } | ElementKind::Stratum { .. }),
                true,
            );
            let around = order_by(
                it,
                |k| {
                    matches!(
                        k,
                        ElementKind::Conduit { .. } | ElementKind::Satellite { .. }
                    )
                },
                false,
            );
            let cur = ex.element;
            let mut next = None;
            if keys.just_pressed(KeyCode::ArrowUp) {
                next = step(&vertical, cur, 1);
            } else if keys.just_pressed(KeyCode::ArrowDown) {
                next = step(&vertical, cur, -1);
            } else if keys.just_pressed(KeyCode::ArrowRight) {
                next = step(&around, cur, 1);
            } else if keys.just_pressed(KeyCode::ArrowLeft) {
                next = step(&around, cur, -1);
            } else if keys.just_pressed(KeyCode::KeyN) || keys.just_pressed(KeyCode::Tab) {
                let flagged: Vec<usize> = ex
                    .anomalies
                    .iter()
                    .filter_map(|a| it.elements.iter().position(|e| e.kind == a.element))
                    .collect();
                next = step(&flagged, cur, 1);
            }
            if next.is_some() {
                ex.choose(next);
            }
            if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
                && let Some(ElementKind::Satellite { child }) =
                    ex.element.map(|i| ex.interior.elements[i].kind.clone())
            {
                sel.key = Some(child);
                ex.dive(child);
            }
            if keys.just_pressed(KeyCode::Escape) {
                if ex.element.is_some() {
                    ex.choose(None);
                } else {
                    ex.surface();
                }
            }
            if keys.just_pressed(KeyCode::Home) {
                ex.surface();
            }
        }
    }
}

/// Fade to black, switch levels at full black, fade back in.
fn run_fade(
    time: Res<Time>,
    mut ex: ResMut<Explore>,
    focus: Res<SourceFocus>,
    mut sel: ResMut<Selection>,
    mut cam: ResMut<OrbitCam>,
    sl: Res<SceneLayout>,
    settings: Res<SceneSettings>,
) {
    let dt = time.delta_secs();
    let speed = if settings.reduced_motion { 8.0 } else { 3.0 };
    if let Some(target) = ex.pending {
        ex.fade = (ex.fade + dt * speed).min(1.0);
        if ex.fade >= 1.0 {
            let was = ex.inside();
            info!("explore: {:?} → {:?}", ex.level, target);
            ex.level = target;
            ex.pending = None;
            ex.element = None;
            ex.hovered = None;
            ex.station += 1;
            if let Some(tx) = &focus.0 {
                let _ = tx.send(ex.inside());
            }
            match target {
                Level::Inside(_) => {
                    ex.detail = None;
                    ex.history.clear();
                    ex.anomalies.clear();
                    ex.interior = Interior::default();
                    ex.entered_at = time.elapsed_secs();
                    // Arrive high above the core, looking down into it.
                    cam.focus = Vec3::new(0.0, 12.0, 0.0);
                    cam.dist = 70.0;
                    cam.pitch = 0.9;
                }
                Level::Machine => {
                    // Surface at the foot of the tower we were inside.
                    sel.key = was;
                    sel.follow = true;
                    if let Some(c) = was.and_then(|k| sl.layout.column(&k)) {
                        cam.focus = c.top();
                        cam.dist = 2.0;
                    }
                }
            }
        }
    } else if ex.fade > 0.0 {
        ex.fade = (ex.fade - dt * speed * 0.7).max(0.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn rebuild_interior(
    mut commands: Commands,
    m: Res<Machine>,
    time: Res<Time>,
    mut ex: ResMut<Explore>,
    mut meshes: ResMut<Assets<Mesh>>,
    mats: Res<crate::city::CityHandles>,
    mut last_gen: Local<u64>,
) {
    if ex.interior_mesh.is_none() {
        let h = meshes.add(DotMeshBuilder::default().into_mesh());
        commands.spawn((
            InteriorLayer,
            Mesh3d(h.clone()),
            MeshMaterial3d(mats.mat.clone()),
            Transform::default(),
            bevy::camera::visibility::NoFrustumCulling,
            Visibility::Hidden,
        ));
        ex.interior_mesh = Some(h);
    }
    let Some(key) = ex.inside() else { return };
    if m.detail_gen == *last_gen {
        return;
    }
    *last_gen = m.detail_gen;
    let Some(d) = m.detail.as_ref().filter(|d| d.key == Some(key)) else {
        return;
    };
    let Some(p) = m.snapshot.processes.get(&key) else {
        // The process exited while we were inside it.
        ex.surface();
        return;
    };
    let anomalies = interior::find_anomalies(d, &ex.history);
    let built = interior::build(p, d, &m.snapshot, &anomalies);
    let first = ex.interior.elements.is_empty();
    ex.history.push(d.clone());
    if ex.history.len() > 6 {
        ex.history.remove(0);
    }
    ex.detail = Some(d.clone());
    ex.anomalies = anomalies;
    ex.interior = built;
    if first {
        ex.station += 1;
    }
    let born = ex.entered_at + 0.2;
    let mesh = crate::city::interior_mesh(&ex.interior, d, &m.snapshot, born, time.elapsed_secs());
    if let Some(h) = &ex.interior_mesh
        && let Some(mut target) = meshes.get_mut(h)
    {
        *target = mesh;
    }
}

/// Fly the camera to the station for whatever is chosen.
fn place_camera(
    ex: Res<Explore>,
    sel: Res<Selection>,
    sl: Res<SceneLayout>,
    jack: Res<crate::camera::JackIn>,
    mut cam: ResMut<OrbitCam>,
    mut last: Local<(u64, Option<ProcKey>, bool, bool)>,
) {
    // The jack-in owns the camera until it is done.
    let key = (ex.station, sel.key, ex.inside().is_some(), jack.done);
    if *last == key || ex.pending.is_some() || jack.running() {
        return;
    }
    *last = key;
    match ex.level {
        Level::Machine => match sel.key.and_then(|k| sl.layout.column(&k)) {
            // Stand low at the tower's foot and look up at it.
            Some(c) => {
                let h = c.height();
                cam.t_focus = c.base + Vec3::Y * (h * 0.6 + 0.4);
                cam.t_dist = h * 0.55 + 3.2;
                cam.t_pitch = -0.12;
            }
            None if cam.framed => cam.frame_all(sl.layout.extent),
            None => {}
        },
        Level::Inside(_) => match ex.element.and_then(|i| ex.interior.elements.get(i)) {
            Some(e) => {
                cam.t_focus = e.anchor;
                cam.t_dist = e.reach;
                cam.t_pitch = 0.12;
            }
            None => {
                let h = ex.interior.height.max(12.0);
                cam.t_focus = Vec3::new(0.0, h * 0.45, 0.0);
                cam.t_dist = h * 1.15 + 14.0;
                cam.t_pitch = 0.22;
            }
        },
    }
}

fn layer_visibility(
    ex: Res<Explore>,
    mut machine: Query<&mut Visibility, (With<MachineLayer>, Without<InteriorLayer>)>,
    mut inner: Query<&mut Visibility, (With<InteriorLayer>, Without<MachineLayer>)>,
) {
    let inside = ex.inside().is_some();
    for mut v in &mut machine {
        let want = if inside {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *v != want {
            *v = want;
        }
    }
    for mut v in &mut inner {
        let want = if inside {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
}
