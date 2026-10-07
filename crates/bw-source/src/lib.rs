//! Data sources. The scene and UI can't tell them apart (PLAN §4.1):
//! every source sends one [`Update::Snapshot`] followed by [`Update::Delta`]s.

mod demo;

pub use demo::DemoWorld;

use bw_model::{Delta, Snapshot, Update};
use bw_platform::{Collector, SysCollector};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::time::Duration;

/// Which source to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// This machine, via `bw-platform`.
    Live,
    /// A deterministic synthetic workstation, for demos, screenshots and CI.
    Demo,
}

/// Start a source on a background thread. Updates arrive on the returned
/// channel; the thread stops when the receiver is dropped.
pub fn spawn(kind: SourceKind, interval: Duration) -> Receiver<Update> {
    let (tx, rx) = bounded(8);
    std::thread::Builder::new()
        .name(format!("bw-source-{kind:?}").to_lowercase())
        .spawn(move || match kind {
            SourceKind::Live => run(SysCollector::new(), tx, interval),
            SourceKind::Demo => run(DemoWorld::new(0xB1AC_3A11), tx, interval),
        })
        .expect("spawn source thread");
    rx
}

fn run(mut c: impl Collector, tx: Sender<Update>, interval: Duration) {
    // Let CPU counters accumulate one interval before the first real sample.
    std::thread::sleep(interval.min(Duration::from_millis(500)));
    let mut last: Snapshot = c.sample();
    if tx.send(Update::Snapshot(Box::new(last.clone()))).is_err() {
        return;
    }
    loop {
        std::thread::sleep(interval);
        let next = c.sample();
        let delta = Delta::between(&last, &next);
        last = next;
        if tx.send(Update::Delta(Box::new(delta))).is_err() {
            return;
        }
    }
}
