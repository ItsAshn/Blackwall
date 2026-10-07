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
