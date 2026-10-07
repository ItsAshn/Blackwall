//! A deterministic synthetic workstation.
//!
//! Used for screenshots, demos on machines with few processes (containers,
//! CI) and render tests. It is clearly labelled `source = "demo"` everywhere
//! it appears, so it is never mistaken for real telemetry.

use bw_model::{
    Capabilities, FdInfo, FdKind, HostInfo, Interface, MemRegion, Owner, ProcKey, ProcState,
    Process, ProcessDetail, Realm, RegionKind, Snapshot, SystemStats, ThreadInfo, Volume,
};
use bw_platform::Collector;
use std::collections::BTreeMap;

const GB: u64 = 1 << 30;
const MB: u64 = 1 << 20;

/// Tiny deterministic PRNG (xorshift64*), so the demo needs no dependencies.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

struct Spec {
    pid: u32,
    parent: Option<u32>,
    name: &'static str,
    realm: Realm,
    owner: Owner,
    mem: u64,
    /// Baseline CPU % and how much it swings.
    cpu: f32,
    swing: f32,
    state: ProcState,
}

pub struct DemoWorld {
    rng: Rng,
    tick: u64,
    specs: Vec<Spec>,
    next_pid: u32,
    /// Short-lived compiler processes that come and go.
    transient: Vec<(u32, u64)>,
    /// The latest sample, for synthesizing process internals.
    last: Option<Snapshot>,
    /// The Web Content process with a leak to find.
    leaky: Option<ProcKey>,
}

impl DemoWorld {
    pub fn new(seed: u64) -> Self {
        let mut w = DemoWorld {
            rng: Rng(seed | 1),
            tick: 0,
            specs: Vec::new(),
            next_pid: 300,
            transient: vec![],
            last: None,
            leaky: None,
        };
        w.build();
        w
    }

    fn add(
        &mut self,
        parent: Option<u32>,
        name: &'static str,
        realm: Realm,
        owner: Owner,
        mem: u64,
        cpu: f32,
        swing: f32,
    ) -> u32 {
        let pid = self.next_pid;
        self.next_pid += 1 + (self.rng.next() % 37) as u32;
        self.specs.push(Spec {
            pid,
            parent,
            name,
            realm,
            owner,
            mem,
            cpu,
            swing,
            state: ProcState::Sleeping,
        });
        pid
    }

    fn build(&mut self) {
        use Owner::*;
        use Realm::*;
        self.specs.push(Spec {
            pid: 1,
            parent: None,
            name: "systemd",
            realm: User,
            owner: System,
            mem: 14 * MB,
            cpu: 0.1,
            swing: 0.2,
            state: ProcState::Sleeping,
        });
        self.specs.push(Spec {
            pid: 2,
            parent: None,
            name: "kthreadd",
            realm: Kernel,
            owner: System,
            mem: 0,
            cpu: 0.0,
            swing: 0.0,
            state: ProcState::Sleeping,
        });
        for (i, n) in [
            "kworker/0:1-events",
            "kworker/1:2-mm_percpu_wq",
            "ksoftirqd/0",
            "ksoftirqd/1",
            "rcu_preempt",
            "migration/0",
            "migration/1",
            "kswapd0",
            "jbd2/nvme0n1p2-8",
            "kcompactd0",
            "khugepaged",
            "irq/128-nvme0q1",
            "irq/131-iwlwifi",
            "kworker/u16:3-flush-259:0",
            "kworker/2:0-events_freezable",
            "kworker/3:1H-kblockd",
            "watchdogd",
            "kauditd",
            "oom_reaper",
            "writeback",
            "kblockd",
            "nvme-wq",
            "card0-crtc0",
            "kworker/u17:0-i915_flip",
        ]
        .iter()
        .enumerate()
        {
            let busy = if i % 5 == 0 { 2.0 } else { 0.2 };
            self.add(Some(2), n, Kernel, System, 0, busy, busy * 2.0);
        }
        let sysd = 1;
        for n in [
            "systemd-journald",
            "systemd-udevd",
            "systemd-resolved",
            "systemd-timesyncd",
            "systemd-logind",
            "dbus-daemon",
            "NetworkManager",
            "wpa_supplicant",
            "polkitd",
            "cupsd",
            "bluetoothd",
            "avahi-daemon",
            "thermald",
            "cron",
            "sshd",
            "chronyd",
            "rtkit-daemon",
            "upowerd",
        ] {
            let mem = 6 * MB + (self.rng.next() % 30) * MB;
            self.add(Some(sysd), n, User, System, mem, 0.1, 0.4);
        }
        let pg = self.add(Some(sysd), "postgres", User, System, 180 * MB, 0.5, 1.0);
        for n in [
            "postgres: checkpointer",
            "postgres: background writer",
            "postgres: walwriter",
            "postgres: autovacuum launcher",
            "postgres: app app 10.0.0.12(51522) idle",
            "postgres: app app 10.0.0.12(51530) SELECT",
        ] {
            self.add(Some(pg), n, User, System, 40 * MB, 0.3, 4.0);
        }
        let dockerd = self.add(Some(sysd), "dockerd", User, System, 95 * MB, 0.4, 0.8);
        let ctd = self.add(Some(dockerd), "containerd", User, System, 60 * MB, 0.2, 0.5);
        for _ in 0..2 {
            let shim = self.add(
                Some(ctd),
                "containerd-shim-runc-v2",
                User,
                System,
                12 * MB,
                0.0,
                0.1,
            );
            let ng = self.add(Some(shim), "nginx", User, System, 8 * MB, 0.1, 0.2);
            for _ in 0..3 {
                self.add(Some(ng), "nginx: worker", User, OtherUser, 9 * MB, 0.3, 1.5);
            }
        }
        let redis_shim = self.add(
            Some(ctd),
            "containerd-shim-runc-v2",
            User,
            System,
            12 * MB,
            0.0,
            0.1,
        );
        self.add(
            Some(redis_shim),
            "redis-server",
            User,
            OtherUser,
            120 * MB,
            0.6,
            1.2,
        );

        let gdm = self.add(Some(sysd), "gdm", User, System, 10 * MB, 0.0, 0.1);
        let user_sd = self.add(
            Some(sysd),
            "systemd --user",
            User,
            CurrentUser,
            12 * MB,
            0.0,
            0.1,
        );
        self.add(Some(gdm), "Xwayland", User, CurrentUser, 90 * MB, 1.0, 2.0);
        let shell = self.add(
            Some(user_sd),
            "gnome-shell",
            User,
            CurrentUser,
            420 * MB,
            6.0,
            8.0,
        );
        for n in [
            "pipewire",
            "wireplumber",
            "pipewire-pulse",
            "gnome-keyring-d",
            "gvfsd",
            "xdg-desktop-portal",
            "tracker-miner-f",
            "evolution-alarm",
            "ibus-daemon",
            "gsd-power",
            "gsd-media-keys",
            "gsd-color",
        ] {
            let mem = 15 * MB + (self.rng.next() % 40) * MB;
            self.add(Some(user_sd), n, User, CurrentUser, mem, 0.2, 0.6);
        }
        let ff = self.add(
            Some(shell),
            "firefox",
            User,
            CurrentUser,
            780 * MB,
            4.0,
            10.0,
        );
        for i in 0..14 {
            let name = if i % 4 == 0 {
                "Isolated Web Co"
            } else {
                "Web Content"
            };
            let mem = 80 * MB + (self.rng.next() % 500) * MB;
            self.add(
                Some(ff),
                name,
                User,
                CurrentUser,
                mem,
                1.0,
                if i == 3 { 45.0 } else { 3.0 },
            );
        }
        self.add(
            Some(ff),
            "RDD Process",
            User,
            CurrentUser,
            60 * MB,
            0.5,
            2.0,
        );
        self.add(
            Some(ff),
            "Socket Process",
            User,
            CurrentUser,
            30 * MB,
            0.3,
            1.0,
        );
        let code = self.add(Some(shell), "code", User, CurrentUser, 350 * MB, 2.0, 4.0);
        for n in [
            "code --type=zygote",
            "code --type=gpu-process",
            "code --type=utility (network)",
            "code: extensionHost",
            "code: fileWatcher",
            "code --type=renderer",
        ] {
            let mem = 120 * MB + (self.rng.next() % 300) * MB;
            self.add(Some(code), n, User, CurrentUser, mem, 1.0, 4.0);
        }
        let ra = self.add(
            Some(code),
            "rust-analyzer",
            User,
            CurrentUser,
            2 * GB + 300 * MB,
            8.0,
            30.0,
        );
        self.add(
            Some(ra),
            "rust-analyzer-proc-macro-srv",
            User,
            CurrentUser,
            180 * MB,
            0.5,
            4.0,
        );
        let term = self.add(Some(shell), "kitty", User, CurrentUser, 110 * MB, 0.8, 2.0);
        let zsh = self.add(Some(term), "zsh", User, CurrentUser, 8 * MB, 0.0, 0.1);
        let cargo = self.add(Some(zsh), "cargo", User, CurrentUser, 140 * MB, 3.0, 3.0);
        self.next_pid = self.next_pid.max(cargo + 1);
        let zsh2 = self.add(Some(term), "zsh", User, CurrentUser, 8 * MB, 0.0, 0.1);
        let rsync = self.add(Some(zsh2), "rsync", User, CurrentUser, 24 * MB, 2.0, 1.0);
        if let Some(s) = self.specs.iter_mut().find(|s| s.pid == rsync) {
            s.state = ProcState::DiskWait;
        }
        let steam = self.add(Some(shell), "steam", User, CurrentUser, 410 * MB, 1.0, 2.0);
        for _ in 0..3 {
            let z = self.add(
                Some(steam),
                "steamwebhelper",
                User,
                CurrentUser,
                0,
                0.0,
                0.0,
            );
            if let Some(s) = self.specs.iter_mut().find(|s| s.pid == z) {
                s.state = ProcState::Zombie;
            }
        }
        let spotify = self.add(
            Some(shell),
            "spotify",
            User,
            CurrentUser,
            380 * MB,
            2.0,
            3.0,
        );
        for _ in 0..4 {
            self.add(
                Some(spotify),
                "spotify --type=renderer",
                User,
                CurrentUser,
                90 * MB,
                0.5,
                1.5,
            );
        }
        self.add(
            Some(shell),
            "nautilus",
            User,
            CurrentUser,
            140 * MB,
            0.1,
            0.3,
        );
        self.add(
            Some(shell),
            "discord",
            User,
            CurrentUser,
            520 * MB,
            1.0,
            3.0,
        );
        // The cargo PID is reused below for transient rustc children.
        self.transient.push((cargo, 0));
    }

    fn cargo_pid(&self) -> u32 {
        self.transient.first().map(|t| t.0).unwrap_or(0)
    }
}

impl Collector for DemoWorld {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kernel_threads: true,
            process_io: true,
            load_average: true,
            pressure_stall: true,
            process_detail: true,
            ..Default::default()
        }
    }

    fn detail(&mut self, key: ProcKey) -> Option<ProcessDetail> {
        let snap = self.last.as_ref()?;
        let p = snap.processes.get(&key)?;
        Some(demo_detail(p, self.tick, Some(key) == self.leaky))
    }

    fn sample(&mut self) -> Snapshot {
        let s = self.sample_inner();
        if self.leaky.is_none() {
            // The fourth Web Content process (the one with the runaway thread) leaks.
            self.leaky = s
                .processes
                .values()
                .filter(|p| p.name == "Web Content")
                .nth(2)
                .map(|p| p.key);
        }
        self.last = Some(s.clone());
        s
    }
}

impl DemoWorld {
    fn sample_inner(&mut self) -> Snapshot {
        self.tick += 1;
        let t = self.tick as f32;
        let boot = 1_760_000_000u64;
        let mut processes = BTreeMap::new();
        let mut total_cpu = 0.0;
        let mut mem_used = 0;
        for (i, s) in self.specs.iter().enumerate() {
            let phase = i as f32 * 1.7;
            let wave = ((t * 0.21 + phase).sin() * 0.5 + 0.5).powf(3.0);
            let cpu = if s.state == ProcState::Zombie {
                0.0
            } else {
                (s.cpu + s.swing * wave + self.rng.f() * s.swing * 0.2).max(0.0)
            };
            total_cpu += cpu;
            mem_used += s.mem;
            let key = ProcKey {
                pid: s.pid,
                start_time: boot + s.pid as u64,
            };
            processes.insert(
                key,
                demo_proc(
                    key,
                    s.parent.map(|p| ProcKey {
                        pid: p,
                        start_time: boot + p as u64,
                    }),
                    s.name,
                    s.realm,
                    s.owner,
                    s.state,
                    cpu,
                    s.mem,
                    (wave * 3.0 * MB as f32) as u64,
                ),
            );
        }
        // Transient compiler jobs: a rolling set of rustc processes under cargo.
        let cargo = self.cargo_pid();
        let cargo_key = ProcKey {
            pid: cargo,
            start_time: boot + cargo as u64,
        };
        for j in 0..6u64 {
            let generation = (self.tick + j * 3) / 6;
            if (generation + j).is_multiple_of(3) {
                continue;
            }
            let pid = 40_000 + (generation * 7 + j) as u32 % 20_000;
            let key = ProcKey {
                pid,
                start_time: boot + 100_000 + generation * 10 + j,
            };
            let cpu = 60.0 + self.rng.f() * 40.0;
            total_cpu += cpu;
            processes.insert(
                key,
                demo_proc(
                    key,
                    Some(cargo_key),
                    "rustc",
                    Realm::User,
                    Owner::CurrentUser,
                    ProcState::Running,
                    cpu,
                    300 * MB + (j * 90) * MB,
                    12 * MB,
                ),
            );
        }

        let cpus = 16;
        let cpu_pct = (total_cpu / cpus as f32).min(100.0);
        let pressure = (0.18 + 0.15 * (t * 0.07).sin() + cpu_pct / 250.0).clamp(0.0, 1.0);
        Snapshot {
            time_ms: (boot + 90_000 + self.tick) * 1000,
            host: HostInfo {
                hostname: "demo-workstation".into(),
                os: "Demo Linux 26.04".into(),
                kernel: "6.18.0-demo".into(),
                arch: "x86_64".into(),
                cpu_count: cpus,
                source: "demo".into(),
            },
            caps: self.capabilities(),
            system: SystemStats {
                cpu_pct,
                per_cpu_pct: (0..cpus)
                    .map(|c| ((t * 0.3 + c as f32).sin() * 0.5 + 0.5) * cpu_pct * 1.6)
                    .map(|v| v.min(100.0))
                    .collect(),
                mem_total: 32 * GB,
                mem_used: mem_used.min(31 * GB),
                swap_total: 8 * GB,
                swap_used: 600 * MB,
                load_avg: Some([cpu_pct as f64 / 6.0, 2.4, 2.1]),
                uptime_secs: 90_000 + self.tick,
                kernel_pressure: pressure,
            },
            processes,
            volumes: vec![
                Volume {
                    mount: "/".into(),
                    name: "nvme0n1p2".into(),
                    fs: "ext4".into(),
                    total_bytes: 476 * GB,
                    available_bytes: 61 * GB,
                    removable: false,
                    read_bytes: 40 * MB,
                    written_bytes: 12 * MB,
                },
                Volume {
                    mount: "/home".into(),
                    name: "nvme1n1p1".into(),
                    fs: "btrfs".into(),
                    total_bytes: 1863 * GB,
                    available_bytes: 1100 * GB,
                    removable: false,
                    read_bytes: 8 * MB,
                    written_bytes: 30 * MB,
                },
                Volume {
                    mount: "/media/backup".into(),
                    name: "sda1".into(),
                    fs: "exfat".into(),
                    total_bytes: 931 * GB,
                    available_bytes: 22 * GB,
                    removable: true,
                    read_bytes: 0,
                    written_bytes: 55 * MB,
                },
            ],
            interfaces: vec![
                Interface {
                    name: "wlp0s20f3".into(),
                    rx_bytes: 2 * MB,
                    tx_bytes: 300 * 1024,
                },
                Interface {
                    name: "docker0".into(),
                    rx_bytes: 80 * 1024,
                    tx_bytes: 60 * 1024,
                },
            ],
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn demo_proc(
    key: ProcKey,
    parent: Option<ProcKey>,
    name: &str,
    realm: Realm,
    owner: Owner,
    state: ProcState,
    cpu: f32,
    mem: u64,
    io: u64,
) -> Process {
    Process {
        key,
        parent,
        name: name.into(),
        exe: (realm == Realm::User)
            .then(|| format!("/usr/bin/{}", name.split([' ', ':']).next().unwrap_or(name))),
        cmd: vec![name.into()],
        realm,
        owner,
        user: Some(
            match owner {
                Owner::System => "root",
                Owner::CurrentUser => "ashn",
                Owner::OtherUser => "www-data",
                Owner::Unknown => "?",
            }
            .into(),
        ),
        state,
        cpu_pct: cpu,
        mem_bytes: mem,
        virt_bytes: mem * 3,
        io_read_bytes: io,
        io_write_bytes: io / 3,
        threads: Some(1 + (mem / (64 * MB)) as u32),
        restricted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_deterministic_and_tree_shaped() {
        let mut a = DemoWorld::new(7);
        let mut b = DemoWorld::new(7);
        let (sa, sb) = (a.sample(), b.sample());
        assert_eq!(sa, sb);
        assert!(sa.processes.len() > 120);
        // Every parent reference resolves.
        for p in sa.processes.values() {
            if let Some(pp) = p.parent {
                assert!(
                    sa.processes.contains_key(&pp),
                    "{} has dangling parent",
                    p.name
                );
            }
        }
        // Churn: later samples differ in membership.
        for _ in 0..6 {
            a.sample();
        }
        let later = a.sample();
        assert_ne!(
            sa.processes.keys().collect::<Vec<_>>(),
            later.processes.keys().collect::<Vec<_>>()
        );
    }
}

/// Plausible internals for a demo process. Deterministic per process and
/// tick, with planted anomalies: rsync's main thread stuck in IO wait, one
/// Web Content process leaking heap and descriptors, a spinning JS helper.
fn demo_detail(p: &Process, tick: u64, leaky: bool) -> ProcessDetail {
    let mut rng = Rng((p.key.pid as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let base = p
        .name
        .split([' ', ':'])
        .next()
        .unwrap_or(&p.name)
        .to_string();
    let names: Vec<String> = match base.as_str() {
        "firefox" | "Web" | "Isolated" => {
            let mut v: Vec<String> = [
                "MainThread",
                "IPC I/O Child",
                "Timer",
                "Socket Thread",
                "ImageIO",
                "Compositor",
                "DOM Worker",
                "MediaDecoder",
                "StyleThread#0",
                "StyleThread#1",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            v.extend((0..6).map(|i| format!("JS Helper #{i}")));
            v
        }
        "rust-analyzer" => std::iter::once("main".to_string())
            .chain((0..14).map(|i| format!("Worker {i}")))
            .chain(["Vfs", "Flycheck", "LspServer"].map(String::from))
            .collect(),
        "rustc" => std::iter::once("rustc".to_string())
            .chain((0..8).map(|i| format!("opt cgu.{i:02}")))
            .collect(),
        "code" | "spotify" | "discord" | "steam" => std::iter::once(base.clone())
            .chain(
                [
                    "Chrome_ChildIOT",
                    "ThreadPoolForeg",
                    "ThreadPoolForeg",
                    "CompositorTileW",
                    "VideoFrameCompo",
                    "NetworkService",
                ]
                .map(String::from),
            )
            .collect(),
        "gnome-shell" => [
            "gnome-shell",
            "gmain",
            "gdbus",
            "pool-gnome-shel",
            "JS Helper",
            "KMS thread",
        ]
        .map(String::from)
        .to_vec(),
        "postgres" | "rsync" | "zsh" | "nginx" | "cargo" => vec![base.clone()],
        _ => {
            let n = p.threads.unwrap_or(1).clamp(1, 6) as usize;
            std::iter::once(base.clone())
                .chain((1..n).map(|i| format!("{base}-worker-{i}")))
                .collect()
        }
    };
    let n = names.len().max(1);
    let threads: Vec<ThreadInfo> = names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let share = if i == 0 { 0.4 } else { 0.6 / n as f32 };
            let mut state = if p.state == ProcState::Zombie {
                ProcState::Zombie
            } else if rng.f() < 0.15 {
                ProcState::Running
            } else {
                ProcState::Sleeping
            };
            let mut cpu = p.cpu_pct * share * (0.5 + rng.f());
            if p.state == ProcState::DiskWait && i == 0 {
                state = ProcState::DiskWait;
            }
            if leaky && name == "JS Helper #3" {
                state = ProcState::Running;
                cpu = 96.0 + rng.f() * 3.0;
            }
            ThreadInfo {
                tid: p.key.pid + i as u32,
                name,
                state,
                cpu_pct: cpu,
            }
        })
        .collect();

    const MB: u64 = 1 << 20;
    let mem = p.mem_bytes.max(4 * MB);
    let libs: &[(&str, u64)] = match base.as_str() {
        "firefox" | "Web" | "Isolated" => &[
            ("libxul.so", 140),
            ("libnss3.so", 4),
            ("libgtk-3.so.0", 8),
            ("libmozsqlite3.so", 2),
            ("libc.so.6", 2),
            ("libm.so.6", 1),
            ("libstdc++.so.6", 3),
            ("libfreetype.so.6", 1),
        ],
        "rustc" | "cargo" | "rust-analyzer" => &[
            ("librustc_driver.so", 120),
            ("libLLVM.so.19", 110),
            ("libc.so.6", 2),
            ("libstdc++.so.6", 3),
            ("libz.so.1", 1),
        ],
        "code" | "spotify" | "discord" | "steam" => &[
            ("libffmpeg.so", 3),
            ("libvk_swiftshader.so", 6),
            ("libnss3.so", 4),
            ("libgtk-3.so.0", 8),
            ("libc.so.6", 2),
        ],
        _ => &[
            ("libc.so.6", 2),
            ("libm.so.6", 1),
            ("libssl.so.3", 1),
            ("libcrypto.so.3", 5),
        ],
    };
    let growth = if leaky { tick * 6 * MB } else { 0 };
    let mut regions = vec![
        MemRegion {
            kind: RegionKind::Code,
            label: base.clone(),
            size_bytes: (2 + rng.next() % 24) * MB,
        },
        MemRegion {
            kind: RegionKind::Heap,
            label: "[heap]".into(),
            size_bytes: mem * 35 / 100 + growth,
        },
        MemRegion {
            kind: RegionKind::Anonymous,
            label: "anonymous".into(),
            size_bytes: mem * 55 / 100,
        },
        MemRegion {
            kind: RegionKind::Stack,
            label: "[stack]".into(),
            size_bytes: 8 * MB * n as u64,
        },
        MemRegion {
            kind: RegionKind::Kernel,
            label: "[vdso]".into(),
            size_bytes: 8 << 10,
        },
    ];
    regions.extend(libs.iter().map(|(l, m)| MemRegion {
        kind: RegionKind::Library,
        label: l.to_string(),
        size_bytes: m * MB,
    }));
    for f in ["fonts.cache-1", "locale-archive"] {
        regions.push(MemRegion {
            kind: RegionKind::File,
            label: f.into(),
            size_bytes: (1 + rng.next() % 12) * MB,
        });
    }

    let (files, sockets, pipes): (u32, u32, u32) = match base.as_str() {
        "firefox" => (90, 60, 40),
        "Web" | "Isolated" => (24, 6, 14),
        "postgres" => (30, 12, 2),
        "nginx" => (6, 24, 2),
        "rsync" => (12, 0, 2),
        "rust-analyzer" | "code" => (60, 8, 10),
        _ => (4 + (rng.next() % 6) as u32, (rng.next() % 3) as u32, 2),
    };
    let leak = if leaky { (tick * 4) as u32 } else { 0 };
    let mut fds = vec![
        FdInfo {
            fd: 0,
            kind: FdKind::Device,
            target: "/dev/null".into(),
        },
        FdInfo {
            fd: 1,
            kind: FdKind::Device,
            target: "/dev/pts/0".into(),
        },
        FdInfo {
            fd: 2,
            kind: FdKind::Device,
            target: "/dev/pts/0".into(),
        },
    ];
    let mut fd = 3;
    let file_names = [
        "/home/ashn/.cache/index",
        "/usr/share/fonts/NotoSans.ttf",
        "/var/lib/data.db",
        "/home/ashn/.config/settings.json",
        "/tmp/session.lock",
    ];
    for i in 0..files + leak {
        let target = if i >= files {
            format!("/home/ashn/.cache/blob-{i:04}.tmp")
        } else {
            file_names[i as usize % file_names.len()].to_string()
        };
        fds.push(FdInfo {
            fd,
            kind: FdKind::File,
            target,
        });
        fd += 1;
    }
    for i in 0..sockets {
        fds.push(FdInfo {
            fd,
            kind: FdKind::Socket,
            target: format!("socket:[{}]", 40_000 + p.key.pid * 7 + i),
        });
        fd += 1;
    }
    for i in 0..pipes {
        let kind = if i % 3 == 0 {
            FdKind::Event
        } else {
            FdKind::Pipe
        };
        let target = if kind == FdKind::Event {
            "anon_inode:[eventfd]".into()
        } else {
            format!("pipe:[{}]", 90_000 + p.key.pid + i)
        };
        fds.push(FdInfo { fd, kind, target });
        fd += 1;
    }
    let fd_count = fds.len() as u32;
    fds.truncate(512);
    ProcessDetail {
        key: Some(p.key),
        time_ms: tick * 1000,
        threads,
        regions,
        fds,
        fd_count,
        restricted: false,
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    #[test]
    fn planted_anomalies_are_findable() {
        let mut w = DemoWorld::new(7);
        let s = w.sample();
        let rsync = s
            .processes
            .values()
            .find(|p| p.name == "rsync")
            .unwrap()
            .key;
        let d = w.detail(rsync).unwrap();
        assert_eq!(
            d.threads[0].state,
            ProcState::DiskWait,
            "rsync main thread stuck in IO"
        );

        let leaky = w.leaky.expect("a leaky process was chosen");
        let a = w.detail(leaky).unwrap();
        w.sample();
        w.sample();
        let b = w.detail(leaky).unwrap();
        let heap = |d: &ProcessDetail| {
            d.regions
                .iter()
                .find(|r| r.kind == RegionKind::Heap)
                .unwrap()
                .size_bytes
        };
        assert!(heap(&b) > heap(&a), "heap grows");
        assert!(b.fd_count > a.fd_count, "descriptors climb");
        assert!(
            b.threads.iter().any(|t| t.cpu_pct > 90.0),
            "a spinning thread"
        );
    }
}
