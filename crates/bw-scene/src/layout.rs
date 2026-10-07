//! The map: one grid, and the grid is RAM.
//!
//! The ground is the machine's memory: a square whose area is total RAM.
//! Every process, kernel or user, owns a plot whose area is its share of
//! the memory in use; the kernel's own memory (caches, slabs) is a plot of
//! its own, and free memory is open, empty ground. Plots are cut by an
//! ordered binary treemap, families staying together and groups in a stable
//! but unsorted order, so the map reads like a city grown over time rather
//! than a table.
//!
//! On each plot stands a tower: its height is what the process is doing
//! (recent CPU, plus a little for its memory), so the skyline is the
//! system's activity at a glance.
//!
//! Pure functions over the model (no Bevy systems), so the layout is
//! unit-testable and identical on every platform.

use bevy::math::{Vec2, Vec3};
use bw_model::{ProcKey, Realm, Snapshot};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Vertical pitch of interior rows (process interiors).
pub const LEVEL_H: f32 = 0.2;
/// Gap left round each group of plots (a family, the kernel).
const STREET: f32 = 0.22;
/// Gap left round each plot inside a group: crowded, but not touching.
const ALLEY: f32 = 0.05;
/// The smallest plot side, so even an idle helper can be clicked.
const MIN_PLOT_MB: f32 = 2.0;

/// Kernel subsystems, each a district behind the Wall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subsystem {
    Scheduler,
    Memory,
    Storage,
    Core,
    Network,
    Drivers,
    Interrupts,
}

impl Subsystem {
    pub const ALL: [Subsystem; 7] = [
        Subsystem::Scheduler,
        Subsystem::Memory,
        Subsystem::Storage,
        Subsystem::Core,
        Subsystem::Network,
        Subsystem::Drivers,
        Subsystem::Interrupts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Subsystem::Scheduler => "SCHEDULER",
            Subsystem::Memory => "MEMORY",
            Subsystem::Storage => "STORAGE",
            Subsystem::Core => "KERNEL CORE",
            Subsystem::Network => "NETWORK",
            Subsystem::Drivers => "DRIVERS",
            Subsystem::Interrupts => "INTERRUPTS",
        }
    }

    /// Best-effort classification of a kernel-side process by name.
    /// Covers Linux kthread names and the Windows/macOS pseudo-processes.
    pub fn classify(name: &str) -> Subsystem {
        let n = name.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|x| n.contains(x));
        if has(&["kthreadd", "kernel_task", "secure system"]) || n == "system" {
            Subsystem::Core
        } else if has(&["irq/", "softirq", "interrupt"]) {
            Subsystem::Interrupts
        } else if has(&[
            "kswapd",
            "kcompactd",
            "khugepaged",
            "oom_reaper",
            "ksmd",
            "zswap",
            "memory compression",
            "registry",
            "mm_percpu",
        ]) {
            Subsystem::Memory
        } else if has(&[
            "jbd2",
            "kblockd",
            "nvme",
            "scsi",
            "writeback",
            "flush",
            "blk",
            "md",
            "dm-",
            "xfs",
            "btrfs",
            "ext4",
            "loop",
            "ata_",
        ]) {
            Subsystem::Storage
        } else if has(&["napi", "wg-", "ipv6", "netns", "net", "wlan", "iwl", "rtw"]) {
            Subsystem::Network
        } else if has(&[
            "card", "i915", "amdgpu", "nvidia", "drm", "usb", "hid", "acpi", "kthrotld", "audit",
        ]) {
            Subsystem::Drivers
        } else {
            Subsystem::Scheduler
        }
    }
}

/// Tower height for a plot of side `side`: an idle process is a squat block
/// as tall as half its width, a busy one a tower several times its width.
/// Proportions, not absolute height, carry the activity, so big and small
/// processes read the same way.
pub fn height(side: f32, cpu_avg: f32) -> f32 {
    let cpu = (cpu_avg / 100.0).clamp(0.0, 1.5);
    side * (0.45 + 3.2 * cpu.sqrt()) + 0.1
}

/// What a non-process plot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// Free memory: open, empty ground.
    Free,
    /// Memory the kernel holds itself (caches, slabs, page tables).
    KernelMemory,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: BlockKind,
    pub min: Vec2,
    pub max: Vec2,
    pub bytes: u64,
}

/// One process tower on its plot.
#[derive(Clone, Debug)]
pub struct Column {
    pub key: ProcKey,
    /// The tower's footprint on the floor (x, z).
    pub min: Vec2,
    pub max: Vec2,
    pub tall: f32,
    pub realm: Realm,
}

impl Column {
    pub fn height(&self) -> f32 {
        self.tall
    }

    /// Center of the footprint, on the floor.
    pub fn base(&self) -> Vec3 {
        let c = (self.min + self.max) * 0.5;
        Vec3::new(c.x, 0.0, c.y)
    }

    pub fn top(&self) -> Vec3 {
        self.base() + Vec3::Y * self.tall
    }

    pub fn center(&self) -> Vec3 {
        self.base() + Vec3::Y * self.tall * 0.5
    }

    /// Half of the footprint's width (x) and depth (z).
    pub fn half(&self) -> Vec2 {
        (self.max - self.min) * 0.5
    }
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub columns: Vec<Column>,
    pub blocks: Vec<Block>,
    index: HashMap<ProcKey, usize>,
    /// The RAM square on the floor (x, z).
    pub min: Vec2,
    pub max: Vec2,
    /// Half the RAM square's side, for camera framing.
    pub extent: f32,
}

/// Something to place: a process or a block, with its weight in MB.
#[derive(Clone, Copy, Debug)]
enum Item {
    Proc(ProcKey),
    Block(BlockKind),
}

/// Ordered binary treemap: split the list where the weight halves, cut the
/// rectangle across its longer side in proportion, recurse. Keeps the given
/// order (so families stay together) and gives reasonable aspect ratios.
fn treemap<T: Copy>(items: &[(T, f32)], min: Vec2, max: Vec2, out: &mut Vec<(T, Vec2, Vec2)>) {
    match items {
        [] => {}
        [(t, _)] => out.push((*t, min, max)),
        _ => {
            let total: f32 = items.iter().map(|i| i.1).sum::<f32>().max(1e-6);
            let mut acc = 0.0;
            let mut k = 1;
            let mut best = f32::MAX;
            for (i, it) in items.iter().enumerate().take(items.len() - 1) {
                acc += it.1;
                let d = (acc - total / 2.0).abs();
                if d < best {
                    best = d;
                    k = i + 1;
                }
            }
            let f = items[..k].iter().map(|i| i.1).sum::<f32>() / total;
            let size = max - min;
            if size.x >= size.y {
                let x = min.x + size.x * f;
                treemap(&items[..k], min, Vec2::new(x, max.y), out);
                treemap(&items[k..], Vec2::new(x, min.y), max, out);
            } else {
                let y = min.y + size.y * f;
                treemap(&items[..k], min, Vec2::new(max.x, y), out);
                treemap(&items[k..], Vec2::new(min.x, y), max, out);
            }
        }
    }
}

/// Shrink a rectangle by `d` on every side, never below a sliver.
fn inset(min: Vec2, max: Vec2, d: f32) -> (Vec2, Vec2) {
    let size = max - min;
    let d = d.min(size.x * 0.3).min(size.y * 0.3);
    (min + Vec2::splat(d), max - Vec2::splat(d))
}

/// A stable hash, so group order is fixed but not sorted.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Layout {
    /// Lay out the map. `cpu` holds recent CPU samples per process; towers
    /// are as tall as the average of the last few, so the skyline is calm.
    pub fn build(s: &Snapshot, cpu: &HashMap<ProcKey, VecDeque<f32>>) -> Layout {
        let mb = |b: u64| b as f32 / 1_048_576.0;
        let total = mb(s.system.mem_total).max(256.0);
        let used = mb(s.system.mem_used).clamp(0.0, total);

        // Process trees: the dominant root's children each head a family,
        // as does every other root; the kernel is one more group.
        let mut children: BTreeMap<ProcKey, Vec<ProcKey>> = BTreeMap::new();
        let mut roots = Vec::new();
        let mut kernel = Vec::new();
        for p in s.processes.values() {
            if p.realm == Realm::Kernel {
                kernel.push(p.key);
                continue;
            }
            match p
                .parent
                .filter(|pp| s.processes.get(pp).is_some_and(|q| q.realm == Realm::User))
            {
                Some(pp) => children.entry(pp).or_default().push(p.key),
                None => roots.push(p.key),
            }
        }
        roots.sort();
        kernel.sort();
        for v in children.values_mut() {
            v.sort();
        }
        let dfs = |root: ProcKey| {
            let mut out = Vec::new();
            let mut st = vec![root];
            while let Some(k) = st.pop() {
                out.push(k);
                if let Some(kids) = children.get(&k) {
                    st.extend(kids.iter().rev());
                }
            }
            out
        };
        let count: usize = roots.iter().map(|r| dfs(*r).len()).sum();
        let center = roots
            .iter()
            .copied()
            .max_by_key(|r| dfs(*r).len())
            .filter(|r| dfs(*r).len() * 10 >= count * 3);
        let mut heads: Vec<ProcKey> = Vec::new();
        if let Some(c) = center {
            heads.extend(children.get(&c).into_iter().flatten());
        }
        heads.extend(roots.iter().filter(|r| Some(**r) != center));

        // Groups of items, each with a stable sort key.
        let mut groups: Vec<(u64, Vec<Item>)> = Vec::new();
        let mut loose: Vec<Item> = center.map(Item::Proc).into_iter().collect();
        for h in heads {
            let fam: Vec<Item> = dfs(h).into_iter().map(Item::Proc).collect();
            if fam.len() <= 2 {
                loose.extend(fam);
            } else {
                groups.push((mix(h.pid as u64 ^ (h.start_time << 20)), fam));
            }
        }
        if !loose.is_empty() {
            groups.push((mix(1), loose));
        }
        // Weights: processes by resident memory, scaled so that together with
        // the kernel's own memory they fill exactly the memory in use.
        let rss: f32 = s
            .processes
            .values()
            .map(|p| mb(p.mem_bytes).max(MIN_PLOT_MB))
            .sum();
        let scale = if rss > used { used / rss } else { 1.0 };
        let kernel_mem = (used - rss * scale).max(0.0);
        let mut kitems: Vec<Item> = kernel.iter().copied().map(Item::Proc).collect();
        // The kernel's own memory gets a plot when there is any to speak of.
        if kernel_mem >= 16.0 {
            kitems.push(Item::Block(BlockKind::KernelMemory));
        }
        if !kitems.is_empty() {
            groups.push((mix(2), kitems));
        }
        groups.push((mix(3), vec![Item::Block(BlockKind::Free)]));
        groups.sort_by_key(|g| g.0);
        let free = (total - used).max(0.0);
        let weight = |it: &Item| match it {
            Item::Proc(k) => mb(s.processes[k].mem_bytes).max(MIN_PLOT_MB) * scale,
            Item::Block(BlockKind::KernelMemory) => kernel_mem.max(MIN_PLOT_MB),
            Item::Block(BlockKind::Free) => free.max(MIN_PLOT_MB),
        };

        // The RAM square: 10 units a side per sqrt(GB).
        let side = 10.0 * (total / 1024.0).sqrt().max(1.0);
        let (min, max) = (Vec2::splat(-side / 2.0), Vec2::splat(side / 2.0));
        let gw: Vec<(usize, f32)> = groups
            .iter()
            .enumerate()
            .map(|(i, g)| (i, g.1.iter().map(weight).sum()))
            .collect();
        let mut grects = Vec::new();
        treemap(&gw, min, max, &mut grects);

        let mut columns = Vec::new();
        let mut blocks = Vec::new();
        for (gi, gmin, gmax) in grects {
            let items: Vec<(Item, f32)> = groups[gi].1.iter().map(|it| (*it, weight(it))).collect();
            let (gmin, gmax) = inset(gmin, gmax, STREET);
            let mut rects = Vec::new();
            treemap(&items, gmin, gmax, &mut rects);
            for (it, rmin, rmax) in rects {
                match it {
                    Item::Proc(k) => {
                        let (a, b) = inset(rmin, rmax, ALLEY);
                        let p = &s.processes[&k];
                        let recent = cpu
                            .get(&k)
                            .filter(|h| !h.is_empty())
                            .map(|h| {
                                let n = h.len().min(6);
                                h.iter().rev().take(n).sum::<f32>() / n as f32
                            })
                            .unwrap_or(p.cpu_pct);
                        let plot = ((b - a).x * (b - a).y).sqrt();
                        columns.push(Column {
                            key: k,
                            min: a,
                            max: b,
                            tall: height(plot, recent).min(side * 0.6),
                            realm: p.realm,
                        });
                    }
                    Item::Block(kind) => blocks.push(Block {
                        kind,
                        min: rmin,
                        max: rmax,
                        bytes: match kind {
                            BlockKind::Free => (free * 1_048_576.0) as u64,
                            BlockKind::KernelMemory => (kernel_mem * 1_048_576.0) as u64,
                        },
                    }),
                }
            }
        }
        let index = columns
            .iter()
            .enumerate()
            .map(|(i, c)| (c.key, i))
            .collect();
        Layout {
            columns,
            blocks,
            index,
            min,
            max,
            extent: side / 2.0,
        }
    }

    /// Base of the i-th storage volume: in a row just outside the RAM
    /// square's near edge.
    pub fn volume_base(&self, i: usize) -> Vec3 {
        Vec3::new(self.min.x + 1.5 + i as f32 * 2.5, 0.0, self.max.y + 2.5)
    }

    pub fn column(&self, k: &ProcKey) -> Option<&Column> {
        self.index.get(k).map(|i| &self.columns[*i])
    }

    pub fn column_index(&self, k: &ProcKey) -> Option<usize> {
        self.index.get(k).copied()
    }

    /// Tower tops, keyed by process (used for labels and camera follow).
    pub fn positions(&self, out: &mut HashMap<ProcKey, Vec3>) {
        out.clear();
        out.extend(self.columns.iter().map(|c| (c.key, c.top())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bw_platform::Collector;
    use bw_source::DemoWorld;

    fn overlap(a: &Column, b: &Column) -> bool {
        a.min.x < b.max.x - 1e-4
            && b.min.x < a.max.x - 1e-4
            && a.min.y < b.max.y - 1e-4
            && b.min.y < a.max.y - 1e-4
    }

    #[test]
    fn every_process_has_its_own_plot() {
        let s = DemoWorld::new(1).sample();
        let l = Layout::build(&s, &HashMap::new());
        assert_eq!(l.columns.len(), s.processes.len());
        for (i, a) in l.columns.iter().enumerate() {
            assert!(a.min.cmplt(a.max).all(), "empty plot");
            assert!(a.min.cmpge(l.min).all() && a.max.cmple(l.max).all());
            for b in &l.columns[i + 1..] {
                assert!(!overlap(a, b), "plots overlap");
            }
        }
        // Free memory is open ground.
        assert!(l.blocks.iter().any(|b| b.kind == BlockKind::Free));
    }

    #[test]
    fn area_is_memory() {
        let s = DemoWorld::new(1).sample();
        let l = Layout::build(&s, &HashMap::new());
        let area = |c: &Column| (c.max - c.min).x * (c.max - c.min).y;
        let big = l
            .columns
            .iter()
            .max_by_key(|c| s.processes[&c.key].mem_bytes)
            .unwrap();
        let small = l
            .columns
            .iter()
            .min_by_key(|c| s.processes[&c.key].mem_bytes)
            .unwrap();
        assert!(area(big) > area(small) * 10.0);
    }

    #[test]
    fn small_churn_keeps_the_map_steady() {
        let mut w = DemoWorld::new(1);
        let a = Layout::build(&w.sample(), &HashMap::new());
        let b = Layout::build(&w.sample(), &HashMap::new());
        let moved = a
            .columns
            .iter()
            .filter(|c| {
                b.column(&c.key)
                    .is_some_and(|d| d.base().distance(c.base()) > 1.0)
            })
            .count();
        assert!(
            moved * 4 < a.columns.len(),
            "{moved} of {} towers moved",
            a.columns.len()
        );
    }

    #[test]
    fn classify_kernel_names() {
        assert_eq!(Subsystem::classify("kswapd0"), Subsystem::Memory);
        assert_eq!(Subsystem::classify("jbd2/nvme0n1p2-8"), Subsystem::Storage);
        assert_eq!(
            Subsystem::classify("irq/131-iwlwifi"),
            Subsystem::Interrupts
        );
        assert_eq!(Subsystem::classify("System"), Subsystem::Core);
        assert_eq!(Subsystem::classify("Memory Compression"), Subsystem::Memory);
        assert_eq!(Subsystem::classify("kernel_task"), Subsystem::Core);
        assert_eq!(Subsystem::classify("migration/0"), Subsystem::Scheduler);
    }
}
