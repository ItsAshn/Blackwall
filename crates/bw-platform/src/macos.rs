//! macOS refinements: kernel_task and system (below-500 / underscore) accounts.

use bw_model::Capabilities;
use sysinfo::Process;

#[derive(Default)]
pub struct State;

impl State {
    /// `host_statistics64` VM-pressure counters come in Milestone 6.
    pub fn kernel_pressure(&mut self) -> Option<f32> {
        None
    }
}

pub fn capabilities() -> Capabilities {
    Capabilities {
        kernel_threads: true, // kernel_task only
        process_io: true,
        load_average: true,
        ..Default::default()
    }
}

pub fn is_userland_thread(_p: &Process) -> bool {
    false
}

pub fn is_kernel_side(pid: u32, name: &str, _p: &Process) -> bool {
    pid == 0 || name == "kernel_task"
}

pub fn is_system_account(uid: &str, name: Option<&str>) -> bool {
    uid.parse::<u32>().map(|u| u < 500).unwrap_or(false) || name.is_some_and(|n| n.starts_with('_'))
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

/// Ports and connections per process are Linux-only for now (Milestone 6:
/// GetExtendedTcpTable on Windows, proc_pidfdinfo on macOS).
pub fn net(
    _state: &mut State,
    _keys: &std::collections::HashMap<u32, bw_model::ProcKey>,
) -> bw_model::NetState {
    bw_model::NetState::default()
}

/// OS service units (launchd, Windows services) come later; containers and
/// project folders are found on every OS (see `services.rs`).
pub fn services() -> Vec<bw_model::Service> {
    vec![]
}

pub fn firewall(_elevate: bool) -> Option<bw_model::Firewall> {
    None
}
