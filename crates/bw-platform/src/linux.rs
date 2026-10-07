//! Linux refinements: kernel threads, system uids, and PSI pressure.

use bw_model::Capabilities;
use sysinfo::{Process, ThreadKind};

#[derive(Default)]
pub struct State;

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
        per_proc_net: false,
        load_average: true,
        pressure_stall: std::path::Path::new("/proc/pressure/cpu").exists(),
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
