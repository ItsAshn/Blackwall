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
    /// Threads, memory map and descriptors of a single process.
    pub process_detail: bool,
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
    /// Listening ports, connections and the firewall: the Blackwall.
    pub net: NetState,
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
    pub net: NetState,
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
            net: new.net.clone(),
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
        // The firewall's rules arrive separately (they may need elevation):
        // keep what we know unless the delta brings something new.
        let fw = std::mem::take(&mut self.net.firewall);
        self.net = delta.net.clone();
        if self.net.firewall.status == FirewallStatus::Unknown {
            self.net.firewall = fw;
        }
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

/// One thread of a process: a floor of its tower.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub tid: u32,
    pub name: String,
    pub state: ProcState,
    /// Percent of one core since the previous detail sample.
    pub cpu_pct: f32,
}

/// What a memory region holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RegionKind {
    /// The executable's own code and data.
    Code,
    /// Shared libraries.
    Library,
    Heap,
    /// Anonymous mappings (allocator arenas, JIT, GPU buffers…).
    Anonymous,
    /// Memory-mapped files.
    File,
    Stack,
    /// vdso, vvar, vsyscall and the like.
    Kernel,
}

/// A group of mappings in a process's address space: a stratum of its core.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemRegion {
    pub kind: RegionKind,
    /// The library or file name, or a kind label ("[heap]", "anonymous").
    pub label: String,
    /// Virtual size in bytes (what the OS reports for the mappings).
    pub size_bytes: u64,
}

/// What an open file descriptor points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FdKind {
    File,
    Socket,
    Pipe,
    Device,
    /// anon_inode: eventfd, epoll, timerfd, signalfd…
    Event,
    Other,
}

/// One open descriptor: a conduit leaving the tower.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FdInfo {
    pub fd: i32,
    pub kind: FdKind,
    /// Path, `socket:[inode]`, `pipe:[inode]` or the anon_inode name.
    pub target: String,
}

/// The inside of one process, collected only for the process being explored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessDetail {
    pub key: Option<ProcKey>,
    pub time_ms: u64,
    pub threads: Vec<ThreadInfo>,
    pub regions: Vec<MemRegion>,
    /// At most a few hundred descriptors are listed; `fd_count` is the total.
    pub fds: Vec<FdInfo>,
    pub fd_count: u32,
    /// The OS refused some of it (another user's process without elevation).
    pub restricted: bool,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Proto {
    #[default]
    Tcp,
    Udp,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
}

/// A port something is listening on: a gate in the Wall.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listener {
    pub proto: Proto,
    pub port: u16,
    /// The address it is bound to ("0.0.0.0", "::", "127.0.0.1"…).
    pub addr: String,
    /// Reachable from other machines (bound to all or a non-loopback address).
    pub exposed: bool,
    /// The owning process, when the OS lets us see it.
    pub process: Option<ProcKey>,
}

/// An established connection: a stream crossing the Wall.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub proto: Proto,
    pub local_port: u16,
    pub remote_addr: String,
    pub remote_port: u16,
    /// We opened it (the local port is not one we listen on).
    pub outbound: bool,
    pub process: Option<ProcKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirewallAction {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirewallStatus {
    /// Rules not read (they need admin rights, or no firewall tool was found).
    #[default]
    Unknown,
    /// A firewall exists but is not filtering.
    Inactive,
    Active,
}

/// One inbound rule, reduced to what the Wall can show.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FirewallRule {
    /// `None`: any port.
    pub port: Option<u16>,
    pub proto: Option<Proto>,
    pub action: FirewallAction,
    /// The rule as the tool printed it.
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Firewall {
    pub status: FirewallStatus,
    /// "ufw", "nftables", "firewalld", "Windows Defender Firewall"…
    pub backend: String,
    /// What happens to inbound traffic no rule matches.
    pub inbound_default: Option<FirewallAction>,
    pub rules: Vec<FirewallRule>,
}

impl Firewall {
    /// What the firewall does with inbound traffic to `port`, if known.
    pub fn verdict(&self, port: u16, proto: Proto) -> Option<FirewallAction> {
        match self.status {
            FirewallStatus::Unknown => None,
            FirewallStatus::Inactive => Some(FirewallAction::Allow),
            FirewallStatus::Active => self
                .rules
                .iter()
                .find(|r| r.port.is_none_or(|p| p == port) && r.proto.is_none_or(|p| p == proto))
                .map(|r| r.action)
                .or(self.inbound_default),
        }
    }
}

/// The network as the Wall shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetState {
    pub listening: Vec<Listener>,
    pub connections: Vec<Connection>,
    pub firewall: Firewall,
}

/// Where a service is defined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ServiceKind {
    /// A system service unit (systemd, launchd daemon, Windows service).
    SystemUnit,
    /// A per-user service unit.
    UserUnit,
    /// A Docker or Podman container.
    Container,
    /// A project folder that defines services (compose file, Procfile…).
    Project,
    /// Started at login.
    Autostart,
    /// A scheduled job (cron, timer).
    Scheduled,
}

impl ServiceKind {
    pub fn label(self) -> &'static str {
        match self {
            ServiceKind::SystemUnit => "system service",
            ServiceKind::UserUnit => "user service",
            ServiceKind::Container => "container",
            ServiceKind::Project => "project",
            ServiceKind::Autostart => "autostart",
            ServiceKind::Scheduled => "scheduled job",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceState {
    Running,
    /// Defined (and perhaps enabled) but not running.
    Idle,
    Failed,
}

/// A service defined on this machine, running or not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Service {
    pub kind: ServiceKind,
    pub name: String,
    /// Unit file, container id, project folder, desktop file…
    pub location: String,
    pub state: ServiceState,
    /// Starts by itself (enabled unit, autostart entry, restart policy).
    pub enabled: bool,
    /// When it last ran (seconds since the Unix epoch), if anything recorded it.
    pub last_active: Option<u64>,
    /// When its definition last changed (seconds since the Unix epoch).
    pub changed: Option<u64>,
}

impl Service {
    /// Seconds since it last ran (or since it was last touched), as of `now`.
    pub fn idle_secs(&self, now: u64) -> Option<u64> {
        self.last_active
            .or(self.changed)
            .map(|t| now.saturating_sub(t))
    }
}

/// Message stream from a source to its consumers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Update {
    Snapshot(Box<Snapshot>),
    Delta(Box<Delta>),
    /// Internals of the process currently being explored.
    Detail(Box<ProcessDetail>),
    /// Services defined on the machine (collected every minute or so).
    Services(Vec<Service>),
    /// Firewall rules, read on request (may need elevation).
    Firewall(Box<Firewall>),
}

/// Requests from the viewer to its source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// Read the firewall's rules, asking for admin rights if needed.
    FirewallRules,
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
