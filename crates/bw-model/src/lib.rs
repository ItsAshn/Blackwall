//! Platform- and engine-independent data model for Blackwall.
//!
//! Everything a [`Source`](https://docs.rs/bw-source) produces and the scene, UI,
//! detection and storage consume lives here. This crate must never depend on an
//! OS-specific crate or on Bevy (see `docs/PLAN.md` §4.1).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stable identity for a process across samples.
///
/// PIDs get reused, so identity is the PID plus the process start time
/// (seconds since the Unix epoch, as reported by the OS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcKey {
    pub pid: u32,
    pub start_time: u64,
}

impl std::fmt::Display for ProcKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.pid, self.start_time)
    }
}

/// Which side of the Blackwall an entity lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Realm {
    /// Kernel threads and kernel pseudo-processes: behind the Wall.
    Kernel,
    /// Everything else: Deep Space.
    User,
}

/// Who a process belongs to, from the viewer's perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Owner {
    /// root / SYSTEM / LocalService and other system accounts.
    System,
    /// The user running Blackwall.
    CurrentUser,
    /// Any other human user.
    OtherUser,
    /// The OS would not tell us.
    Unknown,
}

/// Normalized scheduler state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcState {
    Running,
    Sleeping,
    /// Uninterruptible sleep, usually waiting on IO ("D" state on Linux).
    DiskWait,
    Stopped,
    Zombie,
    Idle,
    Unknown,
}

/// One process as seen in a single sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Process {
    pub key: ProcKey,
    /// Validated parent (see `bw-platform`): `None` for roots and orphans.
    pub parent: Option<ProcKey>,
    /// Display name. Lossily converted from the OS representation.
    pub name: String,
    pub exe: Option<String>,
    pub cmd: Vec<String>,
    pub realm: Realm,
    pub owner: Owner,
    pub user: Option<String>,
    pub state: ProcState,
    /// CPU usage in percent of *one* core (can exceed 100 on multi-core).
    pub cpu_pct: f32,
    /// Resident set size in bytes.
    pub mem_bytes: u64,
    pub virt_bytes: u64,
    /// Bytes read/written since the previous sample.
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub threads: Option<u32>,
    /// `true` when the OS hid details from us (needs elevation).
    pub restricted: bool,
}

/// How worried to be about a process. Color encodes only this (blue →
/// violet → red), so "red" always means "look here".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Health {
    Healthy,
    /// Not broken, but worth watching (e.g. saturating a core).
    Warning,
    /// An issue: zombie, stuck in uninterruptible IO, stopped.
    Critical,
}

impl Process {
    /// Built-in health heuristics. The rules engine (Milestone 5) will
    /// replace these with configurable rules and learned baselines.
    pub fn health(&self) -> Health {
        match self.state {
            ProcState::Zombie | ProcState::DiskWait | ProcState::Stopped => Health::Critical,
            _ if self.cpu_pct >= 80.0 => Health::Warning,
            _ => Health::Healthy,
        }
    }

    /// One-line reason for a non-healthy state.
    pub fn health_reason(&self) -> Option<&'static str> {
        match self.state {
            ProcState::Zombie => Some("zombie: exited but never reaped by its parent"),
            ProcState::DiskWait => Some("stuck in uninterruptible IO wait"),
            ProcState::Stopped => Some("stopped (suspended or traced)"),
            _ if self.cpu_pct >= 80.0 => Some("saturating a CPU core"),
            _ => None,
        }
    }
}

impl Volume {
    pub fn health(&self) -> Health {
        match self.used_pct() {
            p if p >= 90.0 => Health::Critical,
            p if p >= 75.0 => Health::Warning,
            _ => Health::Healthy,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Volume {
    pub mount: String,
    pub name: String,
    pub fs: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub removable: bool,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

impl Volume {
    pub fn used_pct(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        let used = self.total_bytes.saturating_sub(self.available_bytes);
        used as f32 / self.total_bytes as f32 * 100.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interface {
    pub name: String,
    /// Bytes since the previous sample.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Whole-machine figures for one sample.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemStats {
    pub cpu_pct: f32,
    pub per_cpu_pct: Vec<f32>,
    pub mem_total: u64,
    pub mem_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub load_avg: Option<[f64; 3]>,
    pub uptime_secs: u64,
    /// 0.0 (idle) to 1.0 (saturated): drives the Wall's turbulence.
    /// Derived from the best kernel-pressure signal the platform has.
    pub kernel_pressure: f32,
}

/// What the current platform backend can see. The UI shows
/// "not available on this platform" instead of guessing (PLAN §3.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub kernel_threads: bool,
    pub process_io: bool,
    pub per_proc_net: bool,
    pub load_average: bool,
    pub pressure_stall: bool,
    pub smart: bool,
    pub temperatures: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub cpu_count: usize,
    /// e.g. "live", "demo", "replay".
    pub source: String,
}

/// A complete picture of the machine at one instant.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Milliseconds since the Unix epoch.
    pub time_ms: u64,
    pub host: HostInfo,
    pub caps: Capabilities,
    pub system: SystemStats,
    pub processes: BTreeMap<ProcKey, Process>,
    pub volumes: Vec<Volume>,
    pub interfaces: Vec<Interface>,
}

/// The change between two snapshots. Sources send one full [`Snapshot`]
/// first and [`Delta`]s afterwards (PLAN §4.1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub time_ms: u64,
    pub system: SystemStats,
    /// New or changed processes.
    pub upserted: Vec<Process>,
    pub removed: Vec<ProcKey>,
    pub volumes: Vec<Volume>,
    pub interfaces: Vec<Interface>,
}

impl Delta {
    /// Compute the delta that turns `old` into `new`.
    pub fn between(old: &Snapshot, new: &Snapshot) -> Self {
        let upserted = new
            .processes
            .values()
            .filter(|p| old.processes.get(&p.key) != Some(p))
            .cloned()
            .collect();
        let removed = old
            .processes
            .keys()
            .filter(|k| !new.processes.contains_key(k))
            .copied()
            .collect();
        Delta {
            time_ms: new.time_ms,
            system: new.system.clone(),
            upserted,
            removed,
            volumes: new.volumes.clone(),
            interfaces: new.interfaces.clone(),
        }
    }
}

impl Snapshot {
    /// Apply a delta in place.
    pub fn apply(&mut self, delta: &Delta) {
        self.time_ms = delta.time_ms;
        self.system = delta.system.clone();
        for k in &delta.removed {
            self.processes.remove(k);
        }
        for p in &delta.upserted {
            self.processes.insert(p.key, p.clone());
        }
        self.volumes = delta.volumes.clone();
        self.interfaces = delta.interfaces.clone();
    }

    /// Children of each process, sorted by key for a stable layout.
    pub fn children(&self) -> BTreeMap<ProcKey, Vec<ProcKey>> {
        let mut out: BTreeMap<ProcKey, Vec<ProcKey>> = BTreeMap::new();
        for p in self.processes.values() {
            if let Some(parent) = p.parent
                && self.processes.contains_key(&parent)
            {
                out.entry(parent).or_default().push(p.key);
            }
        }
        out
    }

    /// Processes with no (known) parent.
    pub fn roots(&self) -> Vec<ProcKey> {
        self.processes
            .values()
            .filter(|p| p.parent.is_none_or(|pp| !self.processes.contains_key(&pp)))
            .map(|p| p.key)
            .collect()
    }
}

/// Message stream from a source to its consumers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Update {
    Snapshot(Box<Snapshot>),
    Delta(Box<Delta>),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: Option<u32>, cpu: f32) -> Process {
        Process {
            key: ProcKey {
                pid,
                start_time: 100,
            },
            parent: parent.map(|pid| ProcKey {
                pid,
                start_time: 100,
            }),
            name: format!("p{pid}"),
            exe: None,
            cmd: vec![],
            realm: Realm::User,
            owner: Owner::CurrentUser,
            user: None,
            state: ProcState::Sleeping,
            cpu_pct: cpu,
            mem_bytes: 1,
            virt_bytes: 1,
            io_read_bytes: 0,
            io_write_bytes: 0,
            threads: None,
            restricted: false,
        }
    }

    fn snap(ps: Vec<Process>) -> Snapshot {
        Snapshot {
            processes: ps.into_iter().map(|p| (p.key, p)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn delta_roundtrip() {
        let a = snap(vec![
            proc(1, None, 0.0),
            proc(2, Some(1), 1.0),
            proc(3, Some(1), 0.0),
        ]);
        let b = snap(vec![
            proc(1, None, 0.0),
            proc(2, Some(1), 5.0),
            proc(4, Some(2), 0.0),
        ]);
        let d = Delta::between(&a, &b);
        assert_eq!(d.upserted.len(), 2); // 2 changed, 4 new
        assert_eq!(
            d.removed,
            vec![ProcKey {
                pid: 3,
                start_time: 100
            }]
        );
        let mut c = a.clone();
        c.apply(&d);
        assert_eq!(c.processes, b.processes);
    }

    #[test]
    fn tree() {
        let s = snap(vec![
            proc(1, None, 0.0),
            proc(2, Some(1), 0.0),
            proc(5, Some(99), 0.0),
        ]);
        let mut roots = s.roots();
        roots.sort();
        assert_eq!(roots.iter().map(|k| k.pid).collect::<Vec<_>>(), vec![1, 5]);
        assert_eq!(
            s.children()[&ProcKey {
                pid: 1,
                start_time: 100
            }]
                .len(),
            1
        );
    }
}
