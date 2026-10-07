//! Per-OS system probes behind one [`Collector`] trait.
//!
//! This is the **only** crate in the workspace allowed to use
//! `#[cfg(target_os = ...)]` (PLAN §3.2). The cross-platform baseline is
//! [`sysinfo`]; the `os` module adds what differs per platform: which
//! processes count as kernel-side, which accounts are system accounts, and the
//! best available kernel-pressure signal.

use bw_model::{
    Capabilities, HostInfo, Interface, Owner, ProcKey, ProcState, Process, ProcessDetail, Realm,
    Snapshot, SystemStats, Volume,
};
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{
    Disks, Networks, Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind,
    Users,
};

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod os;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod os;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod os;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[path = "fallback.rs"]
mod os;

#[cfg(target_os = "linux")]
mod linux_net;
#[cfg(target_os = "linux")]
mod linux_services;
pub mod services;

/// Every service defined on the machine, running or not: the OS's own
/// units plus containers and project folders. Slow (it runs commands and
/// walks the home folder); call it every minute or so, off the UI thread.
pub fn sweep_services() -> Vec<bw_model::Service> {
    let mut v = os::services();
    v.extend(services::common());
    v
}

/// The firewall's rules. Without `elevate`, only what an ordinary user may
/// read; with it, the OS's own admin prompt may appear.
pub fn read_firewall(elevate: bool) -> Option<bw_model::Firewall> {
    os::firewall(elevate)
}

/// Something that can produce full snapshots of the local machine.
pub trait Collector: Send {
    fn capabilities(&self) -> Capabilities;
    fn sample(&mut self) -> Snapshot;
    /// Internals of one process (threads, memory map, descriptors), for the
    /// process being explored. Collected on demand: it is far more expensive
    /// than a sample.
    fn detail(&mut self, _key: ProcKey) -> Option<ProcessDetail> {
        None
    }
}

/// The cross-platform collector: `sysinfo` plus per-OS refinements.
pub struct SysCollector {
    sys: System,
    disks: Disks,
    networks: Networks,
    users: Users,
    host: HostInfo,
    my_uid: Option<String>,
    os: os::State,
    /// Per-thread CPU ticks of the process being explored, and when they were read.
    detail_ticks: (
        Option<ProcKey>,
        HashMap<u32, u64>,
        Option<std::time::Instant>,
    ),
}

impl Default for SysCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl SysCollector {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
        let my_uid = sysinfo::get_current_pid()
            .ok()
            .and_then(|pid| sys.process(pid))
            .and_then(|p| p.user_id())
            .map(|u| (**u).to_string());
        let host = HostInfo {
            hostname: System::host_name().unwrap_or_else(|| "localhost".into()),
            os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()),
            kernel: System::kernel_version().unwrap_or_default(),
            arch: std::env::consts::ARCH.into(),
            cpu_count: sys.cpus().len(),
            source: "live".into(),
        };
        Self {
            sys,
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            users: Users::new_with_refreshed_list(),
            host,
            my_uid,
            os: os::State::default(),
            detail_ticks: (None, HashMap::new(), None),
        }
    }

    fn owner_of(&self, uid: Option<&sysinfo::Uid>) -> (Owner, Option<String>) {
        let Some(uid) = uid else {
            return (Owner::Unknown, None);
        };
        let id = (**uid).to_string();
        let name = self.users.get_user_by_id(uid).map(|u| u.name().to_string());
        let owner = if os::is_system_account(&id, name.as_deref()) {
            Owner::System
        } else if self.my_uid.as_deref() == Some(id.as_str()) {
            Owner::CurrentUser
        } else {
            Owner::OtherUser
        };
        (owner, name.or(Some(id)))
    }
}

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_disk_usage()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_tasks()
}

fn map_state(s: ProcessStatus) -> ProcState {
    match s {
        ProcessStatus::Run => ProcState::Running,
        ProcessStatus::Sleep => ProcState::Sleeping,
        ProcessStatus::UninterruptibleDiskSleep => ProcState::DiskWait,
        ProcessStatus::Stop | ProcessStatus::Tracing => ProcState::Stopped,
        ProcessStatus::Zombie | ProcessStatus::Dead => ProcState::Zombie,
        ProcessStatus::Idle | ProcessStatus::Parked => ProcState::Idle,
        _ => ProcState::Unknown,
    }
}

/// Build validated parent links.
///
/// The OS-reported parent PID can be stale: on Windows a parent may exit and
/// its PID be reused by an unrelated, *younger* process. A parent is only
/// accepted if it started no later than the child (PLAN §5.1).
pub fn link_parents(raw: &HashMap<u32, (ProcKey, Option<u32>)>) -> HashMap<u32, Option<ProcKey>> {
    raw.iter()
        .map(|(&pid, &(key, ppid))| {
            let parent = ppid
                .filter(|&pp| pp != pid)
                .and_then(|pp| raw.get(&pp))
                .map(|&(pkey, _)| pkey)
                .filter(|pkey| pkey.start_time <= key.start_time);
            (pid, parent)
        })
        .collect()
}

impl Collector for SysCollector {
    fn capabilities(&self) -> Capabilities {
        os::capabilities()
    }

    fn detail(&mut self, key: ProcKey) -> Option<ProcessDetail> {
        let (last_key, ticks, at) = &mut self.detail_ticks;
        if *last_key != Some(key) {
            *last_key = Some(key);
            ticks.clear();
            *at = None;
        }
        let elapsed = at.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        *at = Some(std::time::Instant::now());
        let exe = self
            .sys
            .process(Pid::from_u32(key.pid))
            .and_then(|p| p.exe())
            .map(|e| e.to_string_lossy().into_owned());
        let mut d = os::detail(key.pid, exe.as_deref(), ticks, elapsed)?;
        d.key = Some(key);
        d.time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        Some(d)
    }

    fn sample(&mut self) -> Snapshot {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
        self.disks.refresh(true);
        self.networks.refresh(true);

        // First pass: identity + raw parent pid, skipping userland threads.
        let mut raw: HashMap<u32, (ProcKey, Option<u32>)> = HashMap::new();
        for (pid, p) in self.sys.processes() {
            if os::is_userland_thread(p) {
                continue;
            }
            let key = ProcKey {
                pid: pid.as_u32(),
                start_time: p.start_time(),
            };
            raw.insert(pid.as_u32(), (key, p.parent().map(Pid::as_u32)));
        }
        let parents = link_parents(&raw);

        let mut processes = BTreeMap::new();
        for (&pid, &(key, _)) in &raw {
            let Some(p) = self.sys.process(Pid::from_u32(pid)) else {
                continue;
            };
            let (owner, user) = self.owner_of(p.user_id());
            let name = p.name().to_string_lossy().into_owned();
            let realm = if os::is_kernel_side(pid, &name, p) {
                Realm::Kernel
            } else {
                Realm::User
            };
            let exe = p.exe().map(|e| e.to_string_lossy().into_owned());
            let io = p.disk_usage();
            let proc = Process {
                key,
                parent: parents.get(&pid).copied().flatten(),
                restricted: exe.is_none() && realm == Realm::User && owner != Owner::CurrentUser,
                name,
                exe,
                cmd: p
                    .cmd()
                    .iter()
                    .map(|c| c.to_string_lossy().into_owned())
                    .collect(),
                realm,
                owner: if realm == Realm::Kernel {
                    Owner::System
                } else {
                    owner
                },
                user,
                state: map_state(p.status()),
                cpu_pct: p.cpu_usage(),
                mem_bytes: p.memory(),
                virt_bytes: p.virtual_memory(),
                io_read_bytes: io.read_bytes,
                io_write_bytes: io.written_bytes,
                threads: p.tasks().map(|t| t.len() as u32),
            };
            processes.insert(key, proc);
        }

        let cpu_pct = self.sys.global_cpu_usage();
        let load = System::load_average();
        let caps = os::capabilities();
        let system = SystemStats {
            cpu_pct,
            per_cpu_pct: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            mem_total: self.sys.total_memory(),
            mem_used: self.sys.used_memory(),
            swap_total: self.sys.total_swap(),
            swap_used: self.sys.used_swap(),
            load_avg: caps
                .load_average
                .then_some([load.one, load.five, load.fifteen]),
            uptime_secs: System::uptime(),
            kernel_pressure: self.os.kernel_pressure().unwrap_or_else(|| {
                // Fallback: CPU saturation blended with memory pressure.
                let mem = self.sys.used_memory() as f32 / self.sys.total_memory().max(1) as f32;
                (cpu_pct / 100.0 * 0.7 + (mem - 0.6).max(0.0) * 0.75).clamp(0.0, 1.0)
            }),
        };

        let volumes = self
            .disks
            .list()
            .iter()
            .map(|d| {
                let u = d.usage();
                Volume {
                    mount: d.mount_point().to_string_lossy().into_owned(),
                    name: d.name().to_string_lossy().into_owned(),
                    fs: d.file_system().to_string_lossy().into_owned(),
                    total_bytes: d.total_space(),
                    available_bytes: d.available_space(),
                    removable: d.is_removable(),
                    read_bytes: u.read_bytes,
                    written_bytes: u.written_bytes,
                }
            })
            .collect();
        let mut interfaces: Vec<Interface> = self
            .networks
            .list()
            .iter()
            .map(|(name, n)| Interface {
                name: name.clone(),
                rx_bytes: n.received(),
                tx_bytes: n.transmitted(),
            })
            .collect();
        interfaces.sort_by(|a, b| a.name.cmp(&b.name));

        let keys: HashMap<u32, ProcKey> = processes.keys().map(|k| (k.pid, *k)).collect();
        let net = os::net(&mut self.os, &keys);
        Snapshot {
            time_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or_default(),
            host: self.host.clone(),
            caps,
            system,
            processes,
            volumes,
            interfaces,
            net,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(pid: u32, t: u64) -> ProcKey {
        ProcKey { pid, start_time: t }
    }

    #[test]
    fn parent_younger_than_child_is_rejected() {
        let mut raw = HashMap::new();
        raw.insert(1, (k(1, 10), None));
        raw.insert(2, (k(2, 20), Some(1)));
        // PID 3 claims parent 4, but 4 started *after* 3: a reused PID.
        raw.insert(3, (k(3, 30), Some(4)));
        raw.insert(4, (k(4, 40), Some(1)));
        raw.insert(5, (k(5, 50), Some(5))); // self-parent (Windows System Idle)
        let p = link_parents(&raw);
        assert_eq!(p[&2], Some(k(1, 10)));
        assert_eq!(p[&3], None);
        assert_eq!(p[&4], Some(k(1, 10)));
        assert_eq!(p[&5], None);
    }
}
