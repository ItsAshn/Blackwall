//! Windows refinements: kernel pseudo-processes and well-known service SIDs.

use bw_model::Capabilities;
use sysinfo::Process;

#[derive(Default)]
pub struct State;

impl State {
    /// No PSI on Windows. PDH processor-queue / DPC counters come in
    /// Milestone 6; until then the generic CPU+memory fallback is used.
    pub fn kernel_pressure(&mut self) -> Option<f32> {
        None
    }
}

pub fn capabilities() -> Capabilities {
    Capabilities {
        kernel_threads: true, // pseudo-processes only, see is_kernel_side
        process_io: true,
        load_average: false,
        ..Default::default()
    }
}

pub fn is_userland_thread(_p: &Process) -> bool {
    false
}

/// System Idle (0), System (4), and the kernel-managed pseudo-processes.
pub fn is_kernel_side(pid: u32, name: &str, _p: &Process) -> bool {
    pid == 0
        || pid == 4
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "registry" | "memory compression" | "secure system" | "system interrupts"
        )
}

pub fn is_system_account(sid: &str, _name: Option<&str>) -> bool {
    // LocalSystem, LocalService, NetworkService, and service SIDs.
    matches!(sid, "S-1-5-18" | "S-1-5-19" | "S-1-5-20") || sid.starts_with("S-1-5-80-")
}

/// Process internals are Linux-only for now (Milestone 6 adds Toolhelp
/// thread snapshots and VirtualQueryEx on Windows, proc_pidinfo on macOS).
pub fn detail(
    _pid: u32,
    _exe: Option<&str>,
    _ticks: &mut std::collections::HashMap<u32, u64>,
    _elapsed: f32,
) -> Option<bw_model::ProcessDetail> {
    None
}
