//! Blackwall's 3D scene: the machine as a map of RAM, its processes as
//! towers on it, and the Wall round it.
//!
//! Engine-specific code lives only here and in `bw-ui` (PLAN §4.1). The scene
//! consumes [`bw_model::Update`]s from any source through [`SourceRx`].

mod blackwall;
pub mod camera;
mod city;
pub mod explore;
pub mod interior;
pub mod layout;
pub mod palette;
mod pick;
pub mod quality;
pub mod suspect;
mod towers;
mod visuals;
mod wall;

use bevy::prelude::*;
use bw_model::{ProcKey, Snapshot, Update};
use crossbeam_channel::Receiver;
use std::collections::{HashMap, VecDeque};

pub use city::{FloorInfo, health_color};
pub use layout::{Layout, Subsystem};
pub use quality::{Quality, Tier};

/// How many samples of history the inspector sparklines keep.
pub const HISTORY_LEN: usize = 120;

/// The incoming update stream from a `bw-source`.
#[derive(Resource)]
pub struct SourceRx(pub Receiver<Update>);

/// The machine as currently known, plus short histories for sparklines.
#[derive(Resource, Default)]
pub struct Machine {
    pub snapshot: Snapshot,
    pub received: bool,
    /// Incremented on every applied update.
    pub generation: u64,
    pub cpu_history: HashMap<ProcKey, VecDeque<f32>>,
    pub mem_history: HashMap<ProcKey, VecDeque<f32>>,
    /// (cpu %, mem %, kernel pressure) per sample.
    pub sys_history: VecDeque<[f32; 3]>,
    /// Internals of the process being explored, and a counter bumped on each.
    pub detail: Option<bw_model::ProcessDetail>,
    pub detail_gen: u64,
    /// Keys born / died in the last update, for materialize/dissolve effects.
    pub born: Vec<ProcKey>,
    pub died: Vec<ProcKey>,
    /// Services defined on the machine (running or not), and a counter
    /// bumped when they arrive.
    pub services: Vec<bw_model::Service>,
    pub services_gen: u64,
}

impl Machine {
    /// Seconds since the Unix epoch, by the source's clock.
    pub fn now(&self) -> u64 {
        self.snapshot.time_ms / 1000
    }

    /// Services that are not running, longest idle first: the dormant ones.
    pub fn dormant(&self) -> Vec<&bw_model::Service> {
        let now = self.now();
        let mut v: Vec<&bw_model::Service> = self
            .services
            .iter()
            .filter(|s| s.state != bw_model::ServiceState::Running)
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.idle_secs(now).unwrap_or(0)));
        v
    }
}

/// Suspicious behaviour found by the rules in [`suspect`].
#[derive(Resource, Default)]
pub struct Suspicions {
    pub list: Vec<suspect::Suspicion>,
    baseline: suspect::Baseline,
}

impl Suspicions {
    pub fn of_process(&self, k: ProcKey) -> impl Iterator<Item = &suspect::Suspicion> {
        self.list
            .iter()
            .filter(move |s| s.target == suspect::Target::Process(k))
    }

    pub fn is_suspect(&self, k: ProcKey) -> bool {
        self.of_process(k).next().is_some()
    }

    pub fn of_service<'a>(
        &'a self,
        location: &'a str,
    ) -> impl Iterator<Item = &'a suspect::Suspicion> {
        self.list
            .iter()
            .filter(move |s| matches!(&s.target, suspect::Target::Service(l) if l == location))
    }
}

/// Something on the map that is not a process tower but can be hovered:
/// a dormant service's ghost, a port's gate, a remote address.
#[derive(Clone, Debug)]
pub struct Landmark {
    pub min: Vec3,
    pub max: Vec3,
    pub title: String,
    pub detail: String,
    /// Shown in the suspicion color.
    pub suspect: bool,
    /// Its id in the scene's shaders (lights up on hover), or -1.
    pub shader_id: f32,
}

#[derive(Resource, Default)]
pub struct Landmarks {
    pub list: Vec<Landmark>,
    pub hovered: Option<usize>,
}

/// Requests from the scene to the source (e.g. read the firewall's rules).
#[derive(Resource, Default)]
pub struct SourceRequests(pub Option<crossbeam_channel::Sender<bw_model::Request>>);

/// What the user is looking at.
#[derive(Resource, Debug)]
pub struct Selection {
    pub key: Option<ProcKey>,
    pub hovered: Option<ProcKey>,
    /// Camera follows the selected entity.
    pub follow: bool,
    /// Remember which child we came up from, so Down returns to it.
    pub last_child: HashMap<ProcKey, ProcKey>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            key: None,
            hovered: None,
            follow: true,
            last_child: HashMap::new(),
        }
    }
}

/// User-facing toggles. Persisted settings arrive in a later milestone.
#[derive(Resource, Debug, Clone)]
pub struct SceneSettings {
    pub reduced_motion: bool,
    pub show_links: bool,
    pub show_kernel: bool,
    pub show_streams: bool,
    pub show_labels: bool,
    /// Only draw processes with issues (zombie, disk-wait) at full brightness.
    pub issues_only: bool,
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            reduced_motion: false,
            show_links: false,
            show_kernel: true,
            show_streams: true,
            show_labels: true,
            issues_only: false,
        }
    }
}

/// The current layout, rebuilt when the snapshot changes.
#[derive(Resource, Default)]
pub struct SceneLayout {
    pub layout: Layout,
    /// Column tops, keyed by process.
    pub targets: HashMap<ProcKey, Vec3>,
    /// Animation clock (stops under reduced motion).
    pub t: f32,
}

/// A process entity in the scene.
#[derive(Component, Debug)]
pub struct ProcNode {
    pub key: ProcKey,
}

/// The smoothed, on-screen state of a process entity.
#[derive(Component, Debug, Default)]
pub struct Shown {
    pub pos: Vec3,
    pub radius: f32,
    /// 0 → 1 while materializing, then stays 1; falls to 0 while dissolving.
    pub presence: f32,
}

/// A world-space label the UI projects onto the screen.
#[derive(Component, Debug, Clone)]
pub struct WorldLabel {
    pub text: String,
    pub kind: LabelKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    Subsystem,
    Volume,
    /// Something suspicious (the Wall's magenta).
    Alert,
}

/// Maps process keys to their entities.
#[derive(Resource, Default)]
pub struct ProcEntities(pub HashMap<ProcKey, Entity>);

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SceneSet {
    Ingest,
    Layout,
    Visuals,
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Machine>()
            .init_resource::<Selection>()
            .init_resource::<SceneSettings>()
            .init_resource::<SceneLayout>()
            .init_resource::<ProcEntities>()
            .init_resource::<Suspicions>()
            .init_resource::<Landmarks>()
            .init_resource::<SourceRequests>()
            .insert_resource(ClearColor(Color::BLACK))
            .configure_sets(
                Update,
                (SceneSet::Ingest, SceneSet::Layout, SceneSet::Visuals).chain(),
            )
            .add_systems(Update, ingest.in_set(SceneSet::Ingest))
            .add_systems(Update, (update_layout, assess).in_set(SceneSet::Layout));
        explore::plugin(app);
        wall::plugin(app);
        city::plugin(app);
        towers::plugin(app);
        visuals::plugin(app);
        pick::plugin(app);
        camera::plugin(app);
        quality::plugin(app);
        blackwall::plugin(app);
    }
}

fn push(h: &mut VecDeque<f32>, v: f32) {
    if h.len() == HISTORY_LEN {
        h.pop_front();
    }
    h.push_back(v);
}

/// Drain the source channel into [`Machine`].
fn ingest(rx: Option<Res<SourceRx>>, mut m: ResMut<Machine>) {
    let Some(rx) = rx else { return };
    let mut changed = false;
    m.born.clear();
    m.died.clear();
    while let Ok(update) = rx.0.try_recv() {
        match update {
            Update::Snapshot(s) => {
                m.born.extend(s.processes.keys().copied());
                // Firewall rules may have arrived first (they come separately).
                let fw = std::mem::take(&mut m.snapshot.net.firewall);
                m.snapshot = *s;
                if m.snapshot.net.firewall.status == bw_model::FirewallStatus::Unknown {
                    m.snapshot.net.firewall = fw;
                }
                m.received = true;
            }
            Update::Detail(d) => {
                m.detail = Some(*d);
                m.detail_gen += 1;
                continue;
            }
            Update::Services(v) => {
                m.services = v;
                m.services_gen += 1;
                m.generation += 1;
                continue;
            }
            Update::Firewall(fw) => {
                m.snapshot.net.firewall = *fw;
                m.generation += 1;
                continue;
            }
            Update::Delta(d) => {
                for p in &d.upserted {
                    if !m.snapshot.processes.contains_key(&p.key) {
                        m.born.push(p.key);
                    }
                }
                m.died.extend(d.removed.iter().copied());
                m.snapshot.apply(&d);
            }
        }
        changed = true;
        let Machine {
            snapshot,
            cpu_history,
            mem_history,
            sys_history,
            ..
        } = &mut *m;
        for p in snapshot.processes.values() {
            push(cpu_history.entry(p.key).or_default(), p.cpu_pct);
            push(mem_history.entry(p.key).or_default(), p.mem_bytes as f32);
        }
        let s = &snapshot.system;
        if sys_history.len() == HISTORY_LEN {
            sys_history.pop_front();
        }
        sys_history.push_back([
            s.cpu_pct,
            s.mem_used as f32 / s.mem_total.max(1) as f32 * 100.0,
            s.kernel_pressure,
        ]);
    }
    if changed {
        let Machine {
            snapshot,
            cpu_history,
            mem_history,
            ..
        } = &mut *m;
        cpu_history.retain(|k, _| snapshot.processes.contains_key(k));
        mem_history.retain(|k, _| snapshot.processes.contains_key(k));
        m.generation += 1;
    }
}

fn update_layout(
    m: Res<Machine>,
    mut sl: ResMut<SceneLayout>,
    settings: Res<SceneSettings>,
    time: Res<Time>,
    mut last_gen: Local<u64>,
) {
    if m.generation != *last_gen {
        *last_gen = m.generation;
        sl.layout = Layout::build(&m.snapshot, &m.cpu_history);
    }
    if !settings.reduced_motion {
        sl.t += time.delta_secs();
    }
    let SceneLayout {
        layout, targets, ..
    } = &mut *sl;
    layout.positions(targets);
}

/// Re-run the suspicion rules whenever the machine changes.
fn assess(m: Res<Machine>, mut sus: ResMut<Suspicions>, mut last: Local<u64>) {
    if m.generation == *last || !m.received {
        return;
    }
    *last = m.generation;
    let Suspicions { list, baseline } = &mut *sus;
    baseline.observe(&m.snapshot);
    *list = suspect::assess(
        &m.snapshot,
        &m.services,
        &m.cpu_history,
        &m.mem_history,
        baseline,
    );
}

/// A duration in words: "3 days", "14 months".
pub fn fmt_age(secs: u64) -> String {
    let d = secs / 86_400;
    match d {
        0 => match secs / 3600 {
            0 => "minutes".into(),
            1 => "an hour".into(),
            h => format!("{h} hours"),
        },
        1 => "a day".into(),
        2..=59 => format!("{d} days"),
        60..=729 => format!("{} months", d / 30),
        _ => format!("{} years", d / 365),
    }
}

/// Bytes as megabytes or gigabytes, for in-world labels.
pub fn fmt_mb(b: u64) -> String {
    let mb = b as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}
