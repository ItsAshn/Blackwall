//! Exploration as semantic zoom: choosing, opening and walking towers.
//!
//! * **Choose** a tower (click, Tab for the busiest, N for the next with an
//!   issue): the camera frames it and the source starts collecting its
//!   internals.
//! * **Open** it by zooming in close (scroll), Enter, or clicking it again:
//!   its walls fall away to an outline and the process's interior stands in
//!   their place, scaled to the plot (see `interior.rs`). There is no fade
//!   and no separate world; zoom back out and it closes.
//! * **Inside**, click an element or walk them (↑↓ floors and slabs, ←→
//!   pipes); N visits the next anomaly. Esc steps back out one level at a
//!   time: element → tower → map.

use crate::camera::{InputBlock, OrbitCam};
use crate::city::DotMeshBuilder;
use crate::interior::{self, Anomaly, ELEMENT_ID_BASE, ElementKind, Interior};
use crate::*;
use bw_model::{Health, ProcessDetail};
use crossbeam_channel::Sender;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Explore>()
        .init_resource::<SourceFocus>()
        .add_systems(
            Update,
            (
                explore_input,
                follow_selection,
                open_close,
                rebuild_interior,
                place_camera,
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

/// Half the interior's width at scale 1 (the farthest pipes), used to fit
/// it to a plot.
const INTERIOR_HALF: f32 = 13.5;

#[derive(Resource)]
pub struct Explore {
    pub level: Level,
    /// An open or close requested by input, applied once per frame.
    pending: Option<Level>,
    /// Kept for the HUD's fade overlay; zooming never fades.
    pub fade: f32,
    pub interior: Interior,
    pub anomalies: Vec<Anomaly>,
    pub detail: Option<ProcessDetail>,
    history: Vec<ProcessDetail>,
    /// Chosen and hovered interior elements (indices into `interior.elements`).
    pub element: Option<usize>,
    pub hovered: Option<usize>,
    /// When the current interior was opened, for its grow-in.
    entered_at: f32,
    /// Bumped whenever the camera should move to a new station.
    station: u64,
    pub interior_mesh: Option<Handle<Mesh>>,
    /// Where the interior stands: its origin on the plot and its scale.
    pub origin: Vec3,
    pub scale: f32,
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
            origin: Vec3::ZERO,
            scale: 1.0,
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

    /// Open a tower (zoom into it).
    pub fn dive(&mut self, k: ProcKey) {
        self.pending = Some(Level::Inside(k));
    }

    /// Close the open tower (zoom back out to it).
    pub fn surface(&mut self) {
        if self.inside().is_some() {
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

    /// An interior point in world space.
    pub fn to_world(&self, p: Vec3) -> Vec3 {
        self.origin + p * self.scale
    }

    /// A world point in interior space.
    pub fn to_local(&self, p: Vec3) -> Vec3 {
        (p - self.origin) / self.scale
    }
}

/// Marks entities that belong to the machine level.
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

/// The interior's frame for a tower: centered on its plot, scaled so the
/// widest part fits the plot.
fn frame_for(c: &crate::layout::Column) -> (Vec3, f32) {
    let half = c.half().min_element();
    (c.base(), (half / INTERIOR_HALF).max(0.004))
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
    sus: Res<Suspicions>,
    requests: Res<SourceRequests>,
) {
    if block.keyboard {
        return;
    }
    // F: read the firewall's rules (the OS may ask for admin rights).
    if keys.just_pressed(KeyCode::KeyF)
        && let Some(tx) = &requests.0
    {
        info!("requesting firewall rules");
        let _ = tx.send(bw_model::Request::FirewallRules);
    }
    let enter = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);
    match ex.level {
        Level::Machine => {
            if enter && let Some(k) = sel.key {
                ex.dive(k);
            }
            // N: the next thing that needs attention: suspicious processes
            // first (most suspicious first), then issues, worst first.
            if keys.just_pressed(KeyCode::KeyN) {
                let mut keys: Vec<ProcKey> = crate::suspect::by_target(&sus.list)
                    .into_iter()
                    .filter_map(|(t, _)| match t {
                        crate::suspect::Target::Process(k) => Some(k),
                        _ => None,
                    })
                    .collect();
                let mut flagged: Vec<&bw_model::Process> = m
                    .snapshot
                    .processes
                    .values()
                    .filter(|p| p.health() != Health::Healthy && !keys.contains(&p.key))
                    .collect();
                flagged.sort_by(|a, b| b.health().cmp(&a.health()).then(a.key.cmp(&b.key)));
                keys.extend(flagged.iter().map(|p| p.key));
                let next = match sel.key.and_then(|k| keys.iter().position(|x| *x == k)) {
                    Some(i) => keys.get((i + 1) % keys.len()).copied(),
                    None => keys.first().copied(),
                };
                if next.is_some() {
                    sel.key = next;
                    sel.follow = true;
                }
            }
            // Tab: the busiest towers in turn.
            if keys.just_pressed(KeyCode::Tab) {
                let mut busy: Vec<&bw_model::Process> = m.snapshot.processes.values().collect();
                busy.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct).then(a.key.cmp(&b.key)));
                let keys: Vec<ProcKey> = busy.iter().take(12).map(|p| p.key).collect();
                let next = match sel.key.and_then(|k| keys.iter().position(|x| *x == k)) {
                    Some(i) => keys.get((i + 1) % keys.len()).copied(),
                    None => keys.first().copied(),
                };
                if next.is_some() {
                    sel.key = next;
                    sel.follow = true;
                }
            }
            if keys.just_pressed(KeyCode::Escape) && sel.key.is_some() {
                sel.key = None;
                cam.frame_all(sl.layout.extent);
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
            let around = order_by(it, |k| matches!(k, ElementKind::Conduit { .. }), false);
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
            if keys.just_pressed(KeyCode::Escape) {
                if ex.element.is_some() {
                    ex.choose(None);
                } else {
                    ex.surface();
                }
            }
            if keys.just_pressed(KeyCode::Home) {
                ex.surface();
                sel.key = None;
                cam.frame_all(sl.layout.extent);
            }
        }
    }
}

/// A newly chosen tower: frame it and ask the source for its internals.
fn follow_selection(
    sel: Res<Selection>,
    sl: Res<SceneLayout>,
    focus: Res<SourceFocus>,
    mut ex: ResMut<Explore>,
    mut cam: ResMut<OrbitCam>,
    mut last: Local<Option<ProcKey>>,
) {
    if *last == sel.key {
        return;
    }
    *last = sel.key;
    if let Some(tx) = &focus.0 {
        let _ = tx.send(sel.key);
    }
    // Choosing another tower closes the open one.
    if ex.inside().is_some() && ex.inside() != sel.key {
        ex.pending = Some(Level::Machine);
    }
    if let Some(c) = sel.key.and_then(|k| sl.layout.column(&k))
        && sel.follow
    {
        let size = c.half().max_element().max(c.height() * 0.6);
        cam.frame(c.center(), size);
    }
}

/// Apply requested opens and closes, and open or close by zoom alone.
#[allow(clippy::too_many_arguments)]
fn open_close(
    time: Res<Time>,
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    mut ex: ResMut<Explore>,
    mut sel: ResMut<Selection>,
    mut cam: ResMut<OrbitCam>,
) {
    // Zoom alone: close in on the chosen tower and it opens; pull back out
    // past it and it closes.
    if ex.pending.is_none() {
        match ex.level {
            Level::Machine => {
                if let Some(c) = sel.key.and_then(|k| sl.layout.column(&k)) {
                    let reach = c.half().max_element() * 1.6 + 0.4;
                    let over = Vec2::new(cam.t_focus.x, cam.t_focus.z);
                    let on_plot = over.cmpge(c.min - 0.3).all() && over.cmple(c.max + 0.3).all();
                    if cam.moved && cam.t_dist < reach && on_plot {
                        ex.pending = Some(Level::Inside(c.key));
                    }
                }
            }
            Level::Inside(k) => {
                let fit = sl
                    .layout
                    .column(&k)
                    .map_or(4.0, |c| c.half().max_element() * 4.5 + 1.5);
                if cam.moved && cam.t_dist > fit {
                    ex.pending = Some(Level::Machine);
                }
            }
        }
    }
    let Some(target) = ex.pending.take() else {
        return;
    };
    if target == ex.level {
        return;
    }
    let was = ex.inside();
    ex.level = target;
    ex.element = None;
    ex.hovered = None;
    ex.station += 1;
    match target {
        Level::Inside(k) => {
            let Some(c) = sl.layout.column(&k) else {
                ex.level = Level::Machine;
                return;
            };
            info!("explore: open {k}");
            sel.key = Some(k);
            let (origin, scale) = frame_for(c);
            ex.origin = origin;
            ex.scale = scale;
            if m.detail.as_ref().and_then(|d| d.key) != Some(k) {
                ex.detail = None;
            }
            ex.history.clear();
            ex.anomalies.clear();
            ex.interior = Interior::default();
            ex.entered_at = time.elapsed_secs();
            cam.min_dist = scale * 4.0;
        }
        Level::Machine => {
            info!("explore: close {was:?}");
            cam.min_dist = 1.2;
            if let Some(c) = was.and_then(|k| sl.layout.column(&k)) {
                let size = c.half().max_element().max(c.height() * 0.6);
                cam.frame(c.center(), size);
            }
        }
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
    mut q: Query<&mut Visibility, With<InteriorLayer>>,
    mut last: Local<(u64, Option<ProcKey>)>,
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
    let want = if ex.inside().is_some() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut v in &mut q {
        if *v != want {
            *v = want;
        }
    }
    let Some(key) = ex.inside() else {
        *last = (0, None);
        return;
    };
    if *last == (m.detail_gen, Some(key)) {
        return;
    }
    let Some(d) = m.detail.as_ref().filter(|d| d.key == Some(key)) else {
        return;
    };
    *last = (m.detail_gen, Some(key));
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
    let born = ex.entered_at + 0.1;
    let mesh = crate::city::interior_mesh(
        &ex.interior,
        d,
        born,
        time.elapsed_secs(),
        ex.origin,
        ex.scale,
    );
    if let Some(h) = &ex.interior_mesh
        && let Some(mut target) = meshes.get_mut(h)
    {
        *target = mesh;
    }
}

/// Move the camera to whatever was just chosen inside.
fn place_camera(ex: Res<Explore>, mut cam: ResMut<OrbitCam>, mut last: Local<u64>) {
    if *last == ex.station {
        return;
    }
    *last = ex.station;
    if ex.inside().is_none() {
        return;
    }
    match ex.element.and_then(|i| ex.interior.elements.get(i)) {
        Some(e) => {
            cam.t_focus = ex.to_world(e.anchor);
            cam.t_dist = (e.reach * ex.scale).max(cam.min_dist);
        }
        None => {
            let h = ex.interior.height.max(12.0);
            cam.t_focus = ex.to_world(Vec3::new(0.0, h * 0.4, 0.0));
            cam.t_dist = ((h * 0.9 + 16.0) * ex.scale).max(cam.min_dist);
        }
    }
}
