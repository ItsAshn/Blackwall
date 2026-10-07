//! The inside of a process: a world of its own you dive into.
//!
//! * **Core:** the memory map as stacked strata of dot rings, one band per
//!   region (code, libraries, heap, anonymous, files, stacks). Band height
//!   follows size; dot pattern follows kind, so kinds read without color.
//! * **Floors:** one per thread, a square plate of dots ringing the core.
//!   Brightness and the pulse running around the plate are the thread's CPU.
//! * **Conduits:** open descriptors as lines of flowing dots leaving the
//!   floors: files drop toward storage below, sockets climb out into the
//!   dark, pipes and events stay close.
//! * **Satellites:** child processes as smaller towers on bridges.
//!
//! Color is health only; anomalies blink red or glow violet. Pure functions
//! build the model; one Bevy system turns it into a dot mesh.

use crate::layout::LEVEL_H;
use crate::palette::{self, linear};
use bevy::math::Vec3;
use bw_model::{FdKind, Health, ProcKey, ProcState, Process, ProcessDetail, RegionKind, Snapshot};
use std::f32::consts::TAU;

/// Ids for interior elements in the dot shader's selection uniform.
pub const ELEMENT_ID_BASE: usize = 3_000_000;
pub const PLATE: f32 = 6.0;
pub const CORE_R: f32 = 2.4;
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
pub fn build(p: &Process, d: &ProcessDetail, snap: &Snapshot, anomalies: &[Anomaly]) -> Interior {
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
    let span = core_top.max(n as f32 * 0.7);
    for (i, t) in d.threads.iter().enumerate() {
        let fy = 0.6 + i as f32 * span / n as f32;
        let kind = ElementKind::Floor { thread: i };
        let a = flagged(&kind);
        elements.push(Element {
            anchor: Vec3::new(PLATE * 0.6, fy, PLATE * 0.6),
            reach: 13.0,
            min: Vec3::new(-PLATE - 0.3, fy - 0.25, -PLATE - 0.3),
            max: Vec3::new(PLATE + 0.3, fy + 0.25, PLATE + 0.3),
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

    // Satellites: children on a ring, each joined by a bridge at the base.
    let kids: Vec<&Process> = snap
        .processes
        .values()
        .filter(|c| c.parent == Some(p.key))
        .collect();
    for (i, c) in kids.iter().enumerate() {
        let ang = i as f32 / kids.len().max(1) as f32 * TAU + 0.4;
        let pos = Vec3::new(ang.cos() * 26.0, 0.0, ang.sin() * 26.0);
        let h = crate::layout::levels(c) as f32 * LEVEL_H;
        let kind = ElementKind::Satellite { child: c.key };
        elements.push(Element {
            anchor: pos + Vec3::Y * h * 0.5,
            reach: 8.0 + h * 0.5,
            min: pos - Vec3::new(1.0, 0.0, 1.0),
            max: pos + Vec3::new(1.0, h, 1.0),
            label: format!("{} · {}", c.name, c.key.pid),
            health: c.health(),
            anomaly: c
                .health_reason()
                .filter(|_| c.health() != Health::Healthy)
                .map(String::from),
            path: vec![pos, pos.normalize_or_zero() * (PLATE + 0.5)],
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

/// The path of the k-th of n conduits: out from its floor's plate, then
/// down to storage (files), up into the dark (sockets) or round (pipes).
pub fn conduit_path(k: usize, n: usize, kind: FdKind, span: f32) -> Vec<Vec3> {
    let ang = (k as f32 + 0.5) / n.max(1) as f32 * TAU * 3.0;
    let dir = Vec3::new(ang.cos(), 0.0, ang.sin());
    let y = 0.6 + (k as f32 / n.max(1) as f32) * span;
    // Leave from the plate's edge (a square of half-size PLATE).
    let edge = dir * (PLATE / dir.x.abs().max(dir.z.abs()).max(1e-3));
    let start = edge + Vec3::Y * y;
    let out = start + dir * 6.0;
    let end = match kind {
        FdKind::File => out + dir * 4.0 - Vec3::Y * (y + 8.0),
        FdKind::Socket => out + dir * 22.0 + Vec3::Y * 14.0,
        FdKind::Device => out + dir * 2.0,
        FdKind::Pipe | FdKind::Event | FdKind::Other => out + Vec3::new(-dir.z, 0.0, dir.x) * 6.0,
    };
    vec![start, out, end]
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

/// Dot pattern per region kind, so strata read without color: which of a
/// ring's 48 dots are drawn.
pub fn stratum_keep(kind: RegionKind, i: usize, level: usize) -> bool {
    match kind {
        RegionKind::Code | RegionKind::Heap => true,
        RegionKind::Anonymous => !i.is_multiple_of(3),
        RegionKind::Library => (i + level).is_multiple_of(2),
        RegionKind::File => i.is_multiple_of(3),
        RegionKind::Stack => i.is_multiple_of(4) && level.is_multiple_of(2),
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
