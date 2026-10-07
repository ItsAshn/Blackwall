//! Linux refinements: kernel threads, system uids, and PSI pressure.

use bw_model::{
    Capabilities, FdInfo, FdKind, MemRegion, ProcState, ProcessDetail, RegionKind, ThreadInfo,
};
use sysinfo::{Process, ThreadKind};

#[derive(Default)]
pub struct State {
    net: crate::linux_net::NetCache,
}

pub fn net(
    state: &mut State,
    keys: &std::collections::HashMap<u32, bw_model::ProcKey>,
) -> bw_model::NetState {
    crate::linux_net::net(&mut state.net, keys)
}

pub fn services() -> Vec<bw_model::Service> {
    crate::linux_services::services()
}

pub fn firewall(elevate: bool) -> Option<bw_model::Firewall> {
    crate::linux_net::firewall(elevate)
}

impl State {
    /// Pressure Stall Information: the share of time tasks were stalled on
    /// CPU, memory or IO over the last 10 s (`/proc/pressure/*`, Linux ≥ 4.20).
    pub fn kernel_pressure(&mut self) -> Option<f32> {
        let read = |res: &str| -> Option<f32> {
            let text = std::fs::read_to_string(format!("/proc/pressure/{res}")).ok()?;
            parse_psi_some_avg10(&text)
        };
        let (cpu, mem, io) = (
            read("cpu")?,
            read("memory").unwrap_or(0.0),
            read("io").unwrap_or(0.0),
        );
        // Weighted, then mapped so ~40% stall already reads as "critical".
        Some(((cpu * 0.4 + mem * 0.35 + io * 0.25) / 40.0).clamp(0.0, 1.0))
    }
}

pub fn parse_psi_some_avg10(text: &str) -> Option<f32> {
    let line = text.lines().find(|l| l.starts_with("some"))?;
    let field = line
        .split_whitespace()
        .find_map(|f| f.strip_prefix("avg10="))?;
    field.parse().ok()
}

pub fn capabilities() -> Capabilities {
    Capabilities {
        kernel_threads: true,
        process_io: true,
        per_proc_net: true,
        load_average: true,
        pressure_stall: std::path::Path::new("/proc/pressure/cpu").exists(),
        process_detail: true,
        smart: false,
        temperatures: false,
    }
}

pub fn is_userland_thread(p: &Process) -> bool {
    matches!(p.thread_kind(), Some(ThreadKind::Userland))
}

pub fn is_kernel_side(pid: u32, _name: &str, p: &Process) -> bool {
    pid == 2 || matches!(p.thread_kind(), Some(ThreadKind::Kernel))
}

pub fn is_system_account(uid: &str, _name: Option<&str>) -> bool {
    // Debian/Fedora/Arch convention: system accounts are below 1000; 65534 is nobody.
    uid.parse::<u32>()
        .map(|u| u < 1000 || u == 65534)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn psi() {
        let t = "some avg10=12.50 avg60=3.00 avg300=1.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert_eq!(super::parse_psi_some_avg10(t), Some(12.5));
    }
}

/// Per-process internals from /proc: threads, memory map and descriptors.
/// `ticks` keeps each thread's last CPU tick count for the next delta.
pub fn detail(
    pid: u32,
    exe: Option<&str>,
    ticks: &mut std::collections::HashMap<u32, u64>,
    elapsed_secs: f32,
) -> Option<ProcessDetail> {
    use std::fs;
    let base = format!("/proc/{pid}");
    if !std::path::Path::new(&base).exists() {
        return None;
    }
    let mut d = ProcessDetail::default();

    // Threads. CPU % from utime+stime deltas; Linux's USER_HZ is 100.
    let mut seen = std::collections::HashMap::new();
    if let Ok(entries) = fs::read_dir(format!("{base}/task")) {
        for e in entries.flatten() {
            let Ok(tid) = e.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(e.path().join("stat")) else {
                continue;
            };
            let Some((name, state, total)) = parse_task_stat(&stat) else {
                continue;
            };
            let cpu = match ticks.get(&tid) {
                Some(prev) if elapsed_secs > 0.0 => {
                    (total.saturating_sub(*prev) as f32 / 100.0) / elapsed_secs * 100.0
                }
                _ => 0.0,
            };
            seen.insert(tid, total);
            d.threads.push(ThreadInfo {
                tid,
                name,
                state,
                cpu_pct: cpu,
            });
        }
    }
    *ticks = seen;
    d.threads.sort_by_key(|t| t.tid);

    // Memory map, grouped by what each mapping holds.
    if let Ok(maps) = fs::read_to_string(format!("{base}/maps")) {
        d.regions = group_maps(&maps, exe);
    } else {
        d.restricted = true;
    }

    // Descriptors.
    match fs::read_dir(format!("{base}/fd")) {
        Ok(entries) => {
            for e in entries.flatten() {
                d.fd_count += 1;
                if d.fds.len() >= 512 {
                    continue;
                }
                let Ok(fd) = e.file_name().to_string_lossy().parse::<i32>() else {
                    continue;
                };
                let target = fs::read_link(e.path())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                d.fds.push(FdInfo {
                    fd,
                    kind: classify_fd(&target),
                    target,
                });
            }
            d.fds.sort_by_key(|f| f.fd);
        }
        Err(_) => d.restricted = true,
    }
    Some(d)
}

/// (comm, state, utime + stime) from a `/proc/<pid>/task/<tid>/stat` line.
pub fn parse_task_stat(stat: &str) -> Option<(String, ProcState, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat[open + 1..close].to_string();
    let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let state = match rest.first()?.chars().next()? {
        'R' => ProcState::Running,
        'S' => ProcState::Sleeping,
        'D' => ProcState::DiskWait,
        'T' | 't' => ProcState::Stopped,
        'Z' | 'X' => ProcState::Zombie,
        'I' => ProcState::Idle,
        _ => ProcState::Unknown,
    };
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    Some((name, state, utime + stime))
}

pub fn classify_fd(target: &str) -> FdKind {
    if target.starts_with("socket:") {
        FdKind::Socket
    } else if target.starts_with("pipe:") {
        FdKind::Pipe
    } else if target.starts_with("anon_inode:") {
        FdKind::Event
    } else if target.starts_with("/dev/") {
        FdKind::Device
    } else if target.starts_with('/') {
        FdKind::File
    } else {
        FdKind::Other
    }
}

/// Group `/proc/<pid>/maps` lines into regions: one per library or file,
/// one each for heap, stacks, anonymous memory and kernel pages. Libraries
/// beyond the 24 largest are folded into one "other libraries" region.
pub fn group_maps(maps: &str, exe: Option<&str>) -> Vec<MemRegion> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<(RegionKind, String), u64> = BTreeMap::new();
    for line in maps.lines() {
        let mut parts = line.split_whitespace();
        let Some(range) = parts.next() else { continue };
        let Some((a, b)) = range.split_once('-') else {
            continue;
        };
        let (Ok(a), Ok(b)) = (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16)) else {
            continue;
        };
        let path = parts.nth(4).unwrap_or("").trim();
        let (kind, label) = match path {
            "" => (RegionKind::Anonymous, "anonymous".to_string()),
            "[heap]" => (RegionKind::Heap, "[heap]".to_string()),
            p if p.starts_with("[stack") => (RegionKind::Stack, "[stack]".to_string()),
            "[vdso]" | "[vvar]" | "[vsyscall]" | "[vvar_vclock]" => {
                (RegionKind::Kernel, path.to_string())
            }
            p if exe == Some(p) => (RegionKind::Code, file_name(p)),
            p if p.contains(".so") => (RegionKind::Library, file_name(p)),
            p if p.starts_with('[') => (RegionKind::Anonymous, p.to_string()),
            p => (RegionKind::File, file_name(p)),
        };
        *groups.entry((kind, label)).or_default() += b.saturating_sub(a);
    }
    let mut regions: Vec<MemRegion> = groups
        .into_iter()
        .map(|((kind, label), size_bytes)| MemRegion {
            kind,
            label,
            size_bytes,
        })
        .collect();
    let mut libs: Vec<MemRegion> = Vec::new();
    regions.retain(|r| {
        if r.kind == RegionKind::Library {
            libs.push(r.clone());
            false
        } else {
            true
        }
    });
    libs.sort_by_key(|r| std::cmp::Reverse(r.size_bytes));
    if libs.len() > 24 {
        let rest: u64 = libs.drain(24..).map(|r| r.size_bytes).sum();
        libs.push(MemRegion {
            kind: RegionKind::Library,
            label: "other libraries".into(),
            size_bytes: rest,
        });
    }
    regions.extend(libs);
    regions.sort_by(|a, b| a.kind.cmp(&b.kind).then(b.size_bytes.cmp(&a.size_bytes)));
    regions
}

fn file_name(p: &str) -> String {
    p.rsplit('/')
        .next()
        .unwrap_or(p)
        .trim_end_matches(" (deleted)")
        .to_string()
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    #[test]
    fn task_stat_with_spaces_in_name() {
        let s = "1234 (Web Content (x)) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 1 0 100";
        let (name, state, ticks) = parse_task_stat(s).unwrap();
        assert_eq!(name, "Web Content (x)");
        assert_eq!(state, ProcState::Sleeping);
        assert_eq!(ticks, 300);
    }

    #[test]
    fn maps_are_grouped() {
        let maps = "\
55d0a0000000-55d0a0100000 r-xp 00000000 08:01 1 /usr/bin/demo
55d0a1000000-55d0a1400000 rw-p 00000000 00:00 0 [heap]
7f0000000000-7f0000200000 r-xp 00000000 08:01 2 /usr/lib/libc.so.6
7f0000200000-7f0000210000 r--p 00200000 08:01 2 /usr/lib/libc.so.6
7f1000000000-7f1000800000 rw-p 00000000 00:00 0
7ffc00000000-7ffc00021000 rw-p 00000000 00:00 0 [stack]
";
        let r = group_maps(maps, Some("/usr/bin/demo"));
        let find = |k: RegionKind| r.iter().find(|x| x.kind == k).unwrap();
        assert_eq!(find(RegionKind::Heap).size_bytes, 0x400000);
        assert_eq!(find(RegionKind::Library).label, "libc.so.6");
        assert_eq!(find(RegionKind::Library).size_bytes, 0x210000);
        assert_eq!(find(RegionKind::Code).label, "demo");
        assert_eq!(find(RegionKind::Anonymous).size_bytes, 0x800000);
    }

    #[test]
    fn fds_classified() {
        assert_eq!(classify_fd("socket:[123]"), FdKind::Socket);
        assert_eq!(classify_fd("pipe:[9]"), FdKind::Pipe);
        assert_eq!(classify_fd("anon_inode:[eventfd]"), FdKind::Event);
        assert_eq!(classify_fd("/dev/null"), FdKind::Device);
        assert_eq!(classify_fd("/home/a.txt"), FdKind::File);
    }
}
