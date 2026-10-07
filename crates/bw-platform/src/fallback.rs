//! Other platforms (BSDs etc.): `sysinfo` baseline only.

use bw_model::Capabilities;
use sysinfo::Process;

#[derive(Default)]
pub struct State;

impl State {
    pub fn kernel_pressure(&mut self) -> Option<f32> {
        None
    }
}

pub fn capabilities() -> Capabilities {
    Capabilities {
        load_average: true,
        ..Default::default()
    }
}

pub fn is_userland_thread(_p: &Process) -> bool {
    false
}

pub fn is_kernel_side(pid: u32, _name: &str, _p: &Process) -> bool {
    pid == 0
}

pub fn is_system_account(uid: &str, _name: Option<&str>) -> bool {
    uid == "0"
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
