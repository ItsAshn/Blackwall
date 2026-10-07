//! Data sources. The scene and UI can't tell them apart (PLAN §4.1):
//! every source sends one [`Update::Snapshot`] followed by [`Update::Delta`]s.

mod demo;

pub use demo::{DemoWorld, demo_firewall, demo_services};

use bw_model::{Delta, ProcKey, Request, Snapshot, Update};
use bw_platform::{Collector, SysCollector};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::time::Duration;

/// Which source to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// This machine, via `bw-platform`.
    Live,
    /// A deterministic synthetic workstation, for demos, screenshots and CI.
    Demo,
}

/// A running source: updates come in on `updates`; tell it which process
/// is being explored on `focus` and it adds that process's internals.
pub struct SourceHandle {
    pub updates: Receiver<Update>,
    pub focus: Sender<Option<ProcKey>>,
    /// Requests that may take a while (firewall rules through an admin prompt).
    pub requests: Sender<Request>,
}

/// How often the service sweep runs.
const SWEEP: Duration = Duration::from_secs(60);

/// Start a source on a background thread. The thread stops when the update
/// receiver is dropped.
pub fn spawn(kind: SourceKind, interval: Duration) -> SourceHandle {
    let (tx, rx) = bounded(8);
    let (focus_tx, focus_rx) = unbounded();
    let (req_tx, req_rx) = unbounded();
    let side = tx.clone();
    std::thread::Builder::new()
        .name(format!("bw-source-{kind:?}").to_lowercase())
        .spawn(move || match kind {
            SourceKind::Live => run(SysCollector::new(), tx, focus_rx, interval),
            SourceKind::Demo => run(DemoWorld::new(0xB1AC_3A11), tx, focus_rx, interval),
        })
        .expect("spawn source thread");
    std::thread::Builder::new()
        .name("bw-source-services".into())
        .spawn(move || slow(kind, side, req_rx))
        .expect("spawn services thread");
    SourceHandle {
        updates: rx,
        focus: focus_tx,
        requests: req_tx,
    }
}

/// The slow side: services every minute, the firewall at start (what an
/// ordinary user may read) and on request (with an admin prompt).
fn slow(kind: SourceKind, tx: Sender<Update>, requests: Receiver<Request>) {
    // The demo's clock starts where its snapshots do.
    let demo_now = 1_760_000_000 + 90_000;
    if kind == SourceKind::Live
        && let Some(fw) = bw_platform::read_firewall(false)
        && tx.send(Update::Firewall(Box::new(fw))).is_err()
    {
        return;
    }
    loop {
        let services = match kind {
            SourceKind::Live => bw_platform::sweep_services(),
            SourceKind::Demo => demo_services(demo_now),
        };
        if tx.send(Update::Services(services)).is_err() {
            return;
        }
        let deadline = std::time::Instant::now() + SWEEP;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match requests.recv_timeout(left) {
                Ok(Request::FirewallRules) => {
                    let fw = match kind {
                        SourceKind::Live => bw_platform::read_firewall(true),
                        SourceKind::Demo => Some(demo_firewall()),
                    };
                    if let Some(fw) = fw
                        && tx.send(Update::Firewall(Box::new(fw))).is_err()
                    {
                        return;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn run(
    mut c: impl Collector,
    tx: Sender<Update>,
    focus_rx: Receiver<Option<ProcKey>>,
    interval: Duration,
) {
    // Let CPU counters accumulate one interval before the first real sample.
    std::thread::sleep(interval.min(Duration::from_millis(500)));
    let mut last: Snapshot = c.sample();
    if tx.send(Update::Snapshot(Box::new(last.clone()))).is_err() {
        return;
    }
    let mut focus: Option<ProcKey> = None;
    loop {
        // Wait out the interval, but answer a new focus at once.
        let deadline = std::time::Instant::now() + interval;
        let mut fresh_focus = false;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match focus_rx.recv_timeout(left) {
                Ok(f) => {
                    fresh_focus = f != focus;
                    focus = f;
                    if fresh_focus {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !fresh_focus {
            let next = c.sample();
            let delta = Delta::between(&last, &next);
            last = next;
            if tx.send(Update::Delta(Box::new(delta))).is_err() {
                return;
            }
        }
        if let Some(d) = focus.and_then(|k| c.detail(k))
            && tx.send(Update::Detail(Box::new(d))).is_err()
        {
            return;
        }
    }
}
