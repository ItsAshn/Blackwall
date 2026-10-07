//! The city layout: Deep Space as a grid of dot columns.
//!
//! Every process is a column standing on a dot-lattice floor. Each process
//! *tree* is a district (a block of columns in depth-first order, so families
//! stand together), districts are separated by streets, and the Blackwall
//! rises behind the city. Kernel threads stand in their own districts on the
//! far side of the Wall, grouped by subsystem.
//!
//! Pure functions over the model (no Bevy systems), so the layout is
//! unit-testable and identical on every platform.

use bevy::math::{Vec2, Vec3};
use bw_model::{ProcKey, Process, Realm, Snapshot};
use std::collections::{BTreeMap, HashMap};

/// Distance between neighbouring column slots.
pub const CELL: f32 = 1.0;
/// Vertical distance between dots in a column.
pub const LEVEL_H: f32 = 0.2;
/// Empty cells between districts.
const STREET: i32 = 1;

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

/// Dots stacked in a column: more memory, more dots (logarithmic).
pub fn levels(p: &Process) -> u32 {
    if p.realm == Realm::Kernel {
        return 4;
    }
    // Monoliths: twice the dots of before, so a big process towers over you.
    let mb = p.mem_bytes as f32 / 1_048_576.0;
    (4.0 + 10.0 * (1.0 + mb).log10()).round().clamp(4.0, 48.0) as u32
}

/// Side of the column's dot footprint: big processes are denser columns.
pub fn footprint(p: &Process) -> u32 {
    match p.mem_bytes {
        m if m >= 2 << 30 => 3,
        m if m >= 300 << 20 => 2,
        _ => 1,
    }
}

/// One process column.
#[derive(Clone, Debug)]
pub struct Column {
    pub key: ProcKey,
    /// Center of the column's base, on the floor.
    pub base: Vec3,
    pub footprint: u32,
    pub levels: u32,
    pub realm: Realm,
}

impl Column {
    pub fn height(&self) -> f32 {
        self.levels as f32 * LEVEL_H
    }

    pub fn top(&self) -> Vec3 {
        self.base + Vec3::Y * self.height()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub columns: Vec<Column>,
    index: HashMap<ProcKey, usize>,
    kernel_sub: HashMap<ProcKey, Subsystem>,
    districts: HashMap<Subsystem, Vec2>,
    /// User-city bounds on the floor (x, z).
    pub min: Vec2,
    pub max: Vec2,
    /// Rough radius of the city, for camera framing.
    pub extent: f32,
}

impl Layout {
    pub fn build(s: &Snapshot) -> Layout {
        let user = |k: &ProcKey| s.processes.get(k).is_some_and(|p| p.realm == Realm::User);
        let mut children: BTreeMap<ProcKey, Vec<ProcKey>> = BTreeMap::new();
        let mut roots = Vec::new();
        for p in s.processes.values().filter(|p| p.realm == Realm::User) {
            match p.parent.filter(|pp| user(pp)) {
                Some(pp) => children.entry(pp).or_default().push(p.key),
                None => roots.push(p.key),
            }
        }
        roots.sort();
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

        // Districts: the dominant root (init / launchd) gets a one-column
        // plaza, and each of its children's subtrees becomes a district, as
        // does every other root (orphans are common on Windows).
        let total: usize = roots.iter().map(|r| dfs(*r).len()).sum();
        let center = roots
            .iter()
            .copied()
            .max_by_key(|r| dfs(*r).len())
            .filter(|r| dfs(*r).len() * 10 >= total * 3);
        let mut district_roots: Vec<ProcKey> = Vec::new();
        if let Some(c) = center {
            district_roots.extend(children.get(&c).into_iter().flatten());
        }
        district_roots.extend(roots.iter().filter(|r| Some(**r) != center));
        district_roots.sort();

        let mut districts: Vec<Vec<ProcKey>> = Vec::new();
        // Tiny trees share one "suburb" block so the city isn't mostly streets.
        let mut suburb: Vec<ProcKey> = center.into_iter().collect();
        for r in district_roots {
            let d = dfs(r);
            if d.len() <= 2 {
                suburb.extend(d);
            } else {
                districts.push(d);
            }
        }
        if !suburb.is_empty() {
            districts.insert(0, suburb);
        }

        // Each district is a grid with ~25% spare capacity, so a few
        // processes coming and going don't reshape the whole city.
        let dims: Vec<(i32, i32)> = districts
            .iter()
            .map(|d| {
                let cap = (d.len() as f32 * 1.25).ceil() as i32 + 1;
                let w = (cap as f32).sqrt().ceil() as i32;
                (w, (cap + w - 1) / w)
            })
            .collect();
        let area: i32 = dims.iter().map(|(w, h)| (w + STREET) * (h + STREET)).sum();
        let row_w = ((area as f32 * 1.6).sqrt().ceil() as i32)
            .max(dims.iter().map(|d| d.0).max().unwrap_or(1));

        // Shelf packing, districts in stable (root key) order.
        let mut origin = Vec::with_capacity(dims.len());
        let (mut x, mut z, mut row_h) = (0, 0, 0);
        for &(w, h) in &dims {
            if x > 0 && x + w > row_w {
                x = 0;
                z += row_h + STREET;
                row_h = 0;
            }
            origin.push((x, z));
            x += w + STREET;
            row_h = row_h.max(h);
        }
        let city_w = row_w as f32 * CELL;
        let city_d = ((z + row_h) as f32 * CELL).max(CELL);

        let mut columns = Vec::new();
        for (di, d) in districts.iter().enumerate() {
            let (w, _) = dims[di];
            let (ox, oz) = origin[di];
            for (i, k) in d.iter().enumerate() {
                let (row, col) = (i as i32 / w, i as i32 % w);
                // Boustrophedon order keeps parents next to their children.
                let col = if row % 2 == 0 { col } else { w - 1 - col };
                let gx = (ox + col) as f32 * CELL - city_w / 2.0;
                // Row 0 is nearest the Wall; the city extends toward the viewer.
                let gz = (oz + row) as f32 * CELL - city_d / 2.0;
                let p = &s.processes[k];
                columns.push(Column {
                    key: *k,
                    base: Vec3::new(gx, 0.0, gz),
                    footprint: footprint(p),
                    levels: levels(p),
                    realm: Realm::User,
                });
            }
        }
        let min = Vec2::new(-city_w / 2.0, -city_d / 2.0);
        let max = Vec2::new(city_w / 2.0, city_d / 2.0);

        // Kernel districts behind the Wall, one per subsystem.
        let mut groups: BTreeMap<Subsystem, Vec<ProcKey>> =
            Subsystem::ALL.iter().map(|s| (*s, vec![])).collect();
        for p in s.processes.values().filter(|p| p.realm == Realm::Kernel) {
            groups
                .entry(Subsystem::classify(&p.name))
                .or_default()
                .push(p.key);
        }
        let wall_z = min.y - 3.0;
        let kdims: Vec<i32> = groups
            .values()
            .map(|v| (v.len().max(1) as f32).sqrt().ceil() as i32)
            .collect();
        let kwidth: i32 = kdims.iter().map(|w| w + 3).sum::<i32>() - 3;
        let mut kx = -(kwidth as f32) / 2.0 * CELL;
        let mut kernel_sub = HashMap::new();
        let mut district_pos = HashMap::new();
        for ((sub, keys), w) in groups.iter().zip(kdims) {
            let z0 = wall_z - 5.0;
            district_pos.insert(
                *sub,
                Vec2::new(
                    kx + (w - 1) as f32 * CELL / 2.0,
                    z0 - (w - 1) as f32 * CELL / 2.0,
                ),
            );
            for (i, k) in keys.iter().enumerate() {
                let (row, col) = (i as i32 / w, i as i32 % w);
                let p = &s.processes[k];
                columns.push(Column {
                    key: *k,
                    base: Vec3::new(kx + col as f32 * CELL, 0.0, z0 - row as f32 * CELL),
                    footprint: 1,
                    levels: levels(p),
                    realm: Realm::Kernel,
                });
                kernel_sub.insert(*k, *sub);
            }
            kx += (w + 3) as f32 * CELL;
        }

        let index = columns
            .iter()
            .enumerate()
            .map(|(i, c)| (c.key, i))
            .collect();
        let extent = (city_w.max(city_d) / 2.0).max(6.0);
        Layout {
            columns,
            index,
            kernel_sub,
            districts: district_pos,
            min,
            max,
            extent,
        }
    }

    /// Z of the Blackwall plane (just behind the city).
    pub fn wall_z(&self) -> f32 {
        self.min.y - 3.0
    }

    pub fn wall_size(&self) -> (f32, f32) {
        ((self.max.x - self.min.x) * 6.0 + 200.0, 34.0)
    }

    /// Center of a kernel subsystem's district, on the floor behind the Wall.
    pub fn district_pos(&self, s: Subsystem) -> Vec3 {
        let p = self
            .districts
            .get(&s)
            .copied()
            .unwrap_or(Vec2::new(0.0, self.wall_z() - 6.0));
        Vec3::new(p.x, 0.0, p.y)
    }

    /// Base of the i-th volume column: lined up in front of the Wall, left of the city.
    pub fn volume_base(&self, i: usize) -> Vec3 {
        Vec3::new(self.min.x - 3.0 - i as f32 * 2.0, 0.0, self.wall_z() + 1.5)
    }

    pub fn column(&self, k: &ProcKey) -> Option<&Column> {
        self.index.get(k).map(|i| &self.columns[*i])
    }

    pub fn column_index(&self, k: &ProcKey) -> Option<usize> {
        self.index.get(k).copied()
    }

    /// Column tops, keyed by process (used for labels and camera follow).
    pub fn positions(&self, out: &mut HashMap<ProcKey, Vec3>) {
        out.clear();
        out.extend(self.columns.iter().map(|c| (c.key, c.top())));
    }

    pub fn subsystem_of(&self, k: &ProcKey) -> Option<Subsystem> {
        self.kernel_sub.get(k).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bw_platform::Collector;
    use bw_source::DemoWorld;
    use std::collections::HashSet;

    #[test]
    fn every_process_has_its_own_column() {
        let s = DemoWorld::new(1).sample();
        let l = Layout::build(&s);
        assert_eq!(l.columns.len(), s.processes.len());
        let cells: HashSet<(i32, i32)> = l
            .columns
            .iter()
            .map(|c| {
                (
                    (c.base.x * 10.0).round() as i32,
                    (c.base.z * 10.0).round() as i32,
                )
            })
            .collect();
        assert_eq!(cells.len(), l.columns.len(), "two columns share a cell");
        // The city stands in front of the Wall; the kernel behind it.
        for c in &l.columns {
            match c.realm {
                Realm::User => assert!(c.base.z > l.wall_z()),
                Realm::Kernel => assert!(c.base.z < l.wall_z()),
            }
            assert!(c.base.is_finite());
        }
    }

    #[test]
    fn small_churn_keeps_most_columns_in_place() {
        let mut w = DemoWorld::new(1);
        let a = Layout::build(&w.sample());
        let b = Layout::build(&w.sample());
        let moved = a
            .columns
            .iter()
            .filter(|c| b.column(&c.key).is_some_and(|d| d.base != c.base))
            .count();
        assert!(
            moved * 4 < a.columns.len(),
            "{moved} of {} columns moved",
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
