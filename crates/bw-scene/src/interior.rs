//! The inside of a process: a world of its own you dive into.
//!
//! * **Core:** a square brutalist monolith; the memory map is its stacked
//!   slabs, one per region (code, libraries, heap, anonymous, files,
//!   stacks), each capped by a ledge. Slab height follows size; the window
//!   pattern follows kind, so kinds read without color.
//! * **Floors:** one per thread, a slab cantilevered out of the core, faces
//!   taking turns. How far it projects is the thread's CPU, and so is the
//!   share of its windows that are lit.
//! * **Conduits:** open descriptors as pipes of flowing dots: straight out of
//!   the core, then files drop to storage below, sockets climb into the
//!   dark, pipes and events run along the face.
//! * Children are not inside: they stand as towers of their own on the map,
//!   cabled to this one.
//!
//! Color is health only; anomalies blink red or glow violet and raise a
//! beacon. Pure functions build the model; one Bevy system turns it into a
//! dot mesh.

use crate::layout::LEVEL_H;
use crate::palette::{self, linear};
use bevy::math::Vec3;
use bw_model::{FdKind, Health, ProcKey, ProcState, Process, ProcessDetail, RegionKind, Snapshot};

/// Ids for interior elements in the dot shader's selection uniform.
pub const ELEMENT_ID_BASE: usize = 3_000_000;
/// Half the width of the core monolith.
pub const CORE_R: f32 = 2.4;
/// Window pitch on the core's faces.
pub const CORE_WINDOW: f32 = 0.3;
/// Longest a thread's slab projects (at 100% CPU), plus the stub every
/// thread has.
pub const SLAB_MAX: f32 = 6.0;
const SLAB_MIN: f32 = 0.9;
/// Where conduits turn, clear of the longest slab.
const CONDUIT_OUT: f32 = CORE_R + SLAB_MIN + SLAB_MAX + 1.4;
const MAX_CONDUITS: usize = 96;

#[derive(Clone, Debug, PartialEq)]
pub enum ElementKind {
    Floor { thread: usize },
    Stratum { region: usize },
    Conduit { fd: usize },
    Satellite { child: ProcKey },
}

#[derive(Clone, Debug)]
pub struct Element {
    pub kind: ElementKind,
    /// Where the camera looks when this element is chosen.
    pub anchor: Vec3,
    /// How far the camera stands from it.
    pub reach: f32,
    pub min: Vec3,
    pub max: Vec3,
    pub label: String,
    pub health: Health,
    /// Why it is flagged, in words.
    pub anomaly: Option<String>,
    /// Conduits and bridges: the dotted path the element is drawn along.
    pub path: Vec<Vec3>,
}

/// Something worth finding inside a process.
#[derive(Clone, Debug, PartialEq)]
pub struct Anomaly {
    pub health: Health,
    pub text: String,
    pub element: ElementKind,
}

#[derive(Clone, Debug, Default)]
pub struct Interior {
    pub key: Option<ProcKey>,
    pub elements: Vec<Element>,
    /// Height of the tallest part, for framing.
    pub height: f32,
    /// Descriptors not drawn (beyond the conduit budget).
    pub hidden_fds: u32,
}

/// Detect anomalies from the current detail and the recent history of the
/// same process (oldest first).
pub fn find_anomalies(d: &ProcessDetail, history: &[ProcessDetail]) -> Vec<Anomaly> {
    let mut out = Vec::new();
    for (i, t) in d.threads.iter().enumerate() {
        let issue = match t.state {
            ProcState::DiskWait => Some("stuck in uninterruptible IO wait"),
            ProcState::Zombie => Some("zombie: exited, never reaped"),
            ProcState::Stopped => Some("stopped"),
            _ => None,
        };
        if let Some(why) = issue {
            out.push(Anomaly {
                health: Health::Critical,
                text: format!("thread {} {why}", t.name),
                element: ElementKind::Floor { thread: i },
            });
        } else if t.cpu_pct >= 90.0 {
            out.push(Anomaly {
                health: Health::Warning,
                text: format!("thread {} spinning at {:.0}%", t.name, t.cpu_pct),
                element: ElementKind::Floor { thread: i },
            });
        }
    }
    // A region that grew in each of the last three samples by 2% or more overall.
    if history.len() >= 3 {
        let recent = &history[history.len() - 3..];
        for (i, r) in d.regions.iter().enumerate() {
            let sizes: Vec<u64> = recent
                .iter()
                .filter_map(|h| {
                    h.regions
                        .iter()
                        .find(|x| x.kind == r.kind && x.label == r.label)
                        .map(|x| x.size_bytes)
                })
                .chain(std::iter::once(r.size_bytes))
                .collect();
            if sizes.len() == 4
                && sizes.windows(2).all(|w| w[1] > w[0])
                && sizes[3] as f64 > sizes[0] as f64 * 1.02
            {
                let grew = (sizes[3] - sizes[0]) as f32 / 1_048_576.0;
                out.push(Anomaly {
                    health: Health::Warning,
                    text: format!("{} growing: +{grew:.0} MB in {} samples", r.label, 3),
                    element: ElementKind::Stratum { region: i },
                });
            }
        }
        let counts: Vec<u32> = recent
            .iter()
            .map(|h| h.fd_count)
            .chain(std::iter::once(d.fd_count))
            .collect();
        if counts.windows(2).all(|w| w[1] > w[0]) && counts[3] >= counts[0] + 6 {
            // Point at the newest descriptor.
            if let Some(last) = d.fds.len().checked_sub(1) {
                out.push(Anomaly {
                    health: Health::Warning,
                    text: format!("descriptors climbing: {} → {}", counts[0], counts[3]),
                    element: ElementKind::Conduit { fd: last },
                });
            }
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.health));
    out
}

fn thread_health(state: ProcState, cpu: f32) -> Health {
    match state {
        ProcState::DiskWait | ProcState::Zombie | ProcState::Stopped => Health::Critical,
        _ if cpu >= 90.0 => Health::Warning,
        _ => Health::Healthy,
    }
}

/// Lay out the interior world for one process.
pub fn build(p: &Process, d: &ProcessDetail, _snap: &Snapshot, anomalies: &[Anomaly]) -> Interior {
    let mut elements = Vec::new();
    let flagged = |k: &ElementKind| anomalies.iter().find(|a| &a.element == k);

    // Core strata: band height ∝ sqrt(size), the whole core about 24 units.
    let weights: Vec<f32> = d
        .regions
        .iter()
        .map(|r| ((r.size_bytes as f32 / 1_048_576.0).max(0.05)).sqrt())
        .collect();
    let total: f32 = weights.iter().sum::<f32>().max(1e-3);
    let core_h = 24.0;
    let mut y = 0.0;
    for (i, r) in d.regions.iter().enumerate() {
        let h = (weights[i] / total * core_h).max(LEVEL_H * 2.0);
        let kind = ElementKind::Stratum { region: i };
        let a = flagged(&kind);
        elements.push(Element {
            anchor: Vec3::new(0.0, y + h / 2.0, 0.0),
            reach: 10.0 + h * 0.8,
            min: Vec3::new(-CORE_R, y, -CORE_R),
            max: Vec3::new(CORE_R, y + h, CORE_R),
            label: format!("{} · {}", r.label, crate::fmt_mb(r.size_bytes)),
            health: a.map_or(Health::Healthy, |a| a.health),
            anomaly: a.map(|a| a.text.clone()),
            path: vec![],
            kind,
        });
        y += h;
    }
    let core_top = y;

    // Floors: evenly up the core's height (taller if there are many threads).
    let n = d.threads.len().max(1);
    let span = core_top.max(n as f32 * 0.5);
    for (i, t) in d.threads.iter().enumerate() {
        let fy = 0.6 + i as f32 * span / n as f32;
        let (out, side) = face(i);
        let reach_out = slab_len(t.cpu_pct);
        let a0 = out * (CORE_R + 0.15) - side * (CORE_R * 0.8) + Vec3::Y * (fy - LEVEL_H);
        let a1 = out * (CORE_R + 0.15 + reach_out) + side * (CORE_R * 0.8) + Vec3::Y * fy;
        let kind = ElementKind::Floor { thread: i };
        let a = flagged(&kind);
        elements.push(Element {
            anchor: (a0 + a1) * 0.5,
            reach: 9.0 + reach_out * 0.6,
            min: a0.min(a1) - Vec3::splat(0.12),
            max: a0.max(a1) + Vec3::splat(0.12),
            label: format!("{} · tid {} · {:.0}%", t.name, t.tid, t.cpu_pct),
            health: a.map_or(thread_health(t.state, t.cpu_pct), |a| a.health),
            anomaly: a.map(|a| a.text.clone()),
            path: vec![],
            kind,
        });
    }

    // Conduits: one per descriptor, up to the budget; the newest last.
    let shown = d.fds.len().min(MAX_CONDUITS);
    let skip = d.fds.len() - shown;
    for (j, f) in d.fds.iter().enumerate().skip(skip) {
        let k = j - skip;
        let kind = ElementKind::Conduit { fd: j };
        let a = flagged(&kind);
        let pts = conduit_path(k, shown, f.kind, span);
        let (mut min, mut max) = (pts[0], pts[0]);
        let mid = pts[1];
        for p in &pts {
            min = min.min(*p);
            max = max.max(*p);
        }
        elements.push(Element {
            anchor: mid,
            reach: 11.0,
            min: min - Vec3::splat(0.15),
            max: max + Vec3::splat(0.15),
            label: format!("fd {} · {}", f.fd, f.target),
            health: a.map_or(Health::Healthy, |a| a.health),
            anomaly: a.map(|a| a.text.clone()),
            path: pts,
            kind,
        });
    }

    Interior {
        key: Some(p.key),
        elements,
        height: span.max(core_top),
        hidden_fds: d.fd_count.saturating_sub(shown as u32),
    }
}

/// Outward normal and sideways direction of the core face `i` (mod 4).
pub fn face(i: usize) -> (Vec3, Vec3) {
    match i % 4 {
        0 => (Vec3::X, Vec3::Z),
        1 => (Vec3::Z, -Vec3::X),
        2 => (-Vec3::X, -Vec3::Z),
        _ => (-Vec3::Z, Vec3::X),
    }
}

/// How far a thread's slab projects from the core.
pub fn slab_len(cpu_pct: f32) -> f32 {
    SLAB_MIN + (cpu_pct / 100.0).clamp(0.0, 1.0) * SLAB_MAX
}

/// The path of the k-th of n conduits, like pipes on a building: straight
/// out of a core face, then files drop to storage below, sockets climb into
/// the dark, devices go to the ground, pipes and events run along the face.
pub fn conduit_path(k: usize, n: usize, kind: FdKind, span: f32) -> Vec<Vec3> {
    let (out, side) = face(k);
    let slot = (k / 4) % 13;
    let off = (slot as f32 / 12.0 * 2.0 - 1.0) * CORE_R * 0.9;
    let y = 0.6 + (k as f32 / n.max(1) as f32) * span;
    let start = out * (CORE_R + 0.05) + side * off + Vec3::Y * y;
    let turn = out * (CONDUIT_OUT + ((k / 4) % 3) as f32 * 0.5) + side * off + Vec3::Y * y;
    let end = match kind {
        FdKind::File => turn - Vec3::Y * (y + 6.0),
        FdKind::Socket => turn + Vec3::Y * (span + 16.0 - y),
        FdKind::Device => turn - Vec3::Y * y,
        FdKind::Pipe | FdKind::Event | FdKind::Other => turn + side * 3.0,
    };
    vec![start, turn, end]
}

/// Square dots along a polyline at `pitch`; calls `f(point, t)` with t in 0..1.
pub fn along(pts: &[Vec3], pitch: f32, mut f: impl FnMut(Vec3, f32)) {
    let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut walked = 0.0;
    for w in pts.windows(2) {
        let len = w[0].distance(w[1]);
        let steps = (len / pitch).max(1.0) as usize;
        for s in 0..steps {
            let u = s as f32 / steps as f32;
            f(w[0].lerp(w[1], u), (walked + u * len) / total.max(1e-3));
        }
        walked += len;
    }
}

/// Window pattern per region kind, so slabs read without color: whether
/// the `i`-th window round the core (of 4 × 16) is drawn at `level`.
pub fn stratum_keep(kind: RegionKind, i: usize, level: usize) -> bool {
    match kind {
        RegionKind::Code | RegionKind::Heap => true,
        RegionKind::Anonymous => !(i + level).is_multiple_of(3),
        // Vertical ribs.
        RegionKind::Library => i.is_multiple_of(2),
        RegionKind::File => i.is_multiple_of(3) && level.is_multiple_of(2),
        // Horizontal bands.
        RegionKind::Stack => level.is_multiple_of(2),
        RegionKind::Kernel => i.is_multiple_of(8),
    }
}

pub fn health_rgb(h: Health) -> [f32; 3] {
    linear(match h {
        Health::Critical => palette::HEALTH_ISSUE,
        Health::Warning => palette::HEALTH_WATCH,
        Health::Healthy => palette::HEALTH_OK,
    })
}

/// Ray against the elements' boxes; the nearest hit.
pub fn pick(interior: &Interior, origin: Vec3, dir: Vec3) -> Option<usize> {
    let inv = dir.recip();
    interior
        .elements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let t1 = (e.min - origin) * inv;
            let t2 = (e.max - origin) * inv;
            let tmin = t1.min(t2).max_element();
            let tmax = t1.max(t2).min_element();
            (tmax >= tmin.max(0.0)).then_some((tmin.max(0.0), i))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bw_platform::Collector;
    use bw_source::DemoWorld;

    #[test]
    fn interior_has_every_part_and_finds_the_leak() {
        let mut w = DemoWorld::new(3);
        let mut history = Vec::new();
        let mut snap = w.sample();
        let leaky = snap
            .processes
            .values()
            .filter(|p| p.name == "Web Content")
            .nth(2)
            .unwrap()
            .key;
        for _ in 0..4 {
            history.push(w.detail(leaky).unwrap());
            snap = w.sample();
        }
        let d = w.detail(leaky).unwrap();
        let anomalies = find_anomalies(&d, &history);
        let texts: Vec<_> = anomalies.iter().map(|a| a.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| t.contains("[heap] growing")),
            "{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("descriptors climbing")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t.contains("spinning")), "{texts:?}");

        let p = &snap.processes[&leaky];
        let i = build(p, &d, &snap, &anomalies);
        let count = |f: fn(&ElementKind) -> bool| i.elements.iter().filter(|e| f(&e.kind)).count();
        assert_eq!(
            count(|k| matches!(k, ElementKind::Floor { .. })),
            d.threads.len()
        );
        assert_eq!(
            count(|k| matches!(k, ElementKind::Stratum { .. })),
            d.regions.len()
        );
        assert!(count(|k| matches!(k, ElementKind::Conduit { .. })) > 0);
        assert!(
            i.elements
                .iter()
                .all(|e| e.min.is_finite() && e.max.is_finite())
        );
        assert!(i.elements.iter().filter(|e| e.anomaly.is_some()).count() >= 3);
    }

    #[test]
    fn rsync_is_stuck() {
        let mut w = DemoWorld::new(3);
        let snap = w.sample();
        let k = snap
            .processes
            .values()
            .find(|p| p.name == "rsync")
            .unwrap()
            .key;
        let d = w.detail(k).unwrap();
        let a = find_anomalies(&d, &[]);
        assert_eq!(a[0].health, Health::Critical);
        assert!(a[0].text.contains("uninterruptible"));
    }
}
