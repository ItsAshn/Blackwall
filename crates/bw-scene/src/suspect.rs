//! Suspicious behaviour, as rules over what the source reports. Kept apart
//! from health: health is "is it broken", suspicion is "is it what it
//! claims to be". Four families:
//!
//! * **Network:** listening on every interface at a port nothing usually
//!   uses; connecting to a public address on an unusual port.
//! * **Lineage:** running from /tmp or a deleted file; a user process named
//!   like a kernel thread; a shell or downloader spawned by a server or a
//!   browser.
//! * **Resource:** a core saturated for a minute by something that is not a
//!   compiler or similar; memory that only ever climbs.
//! * **Persistence:** autostart entries, units and scheduled jobs added in
//!   the last week; scheduled commands that download and run code.
//!
//! Pure functions over the model, so the rules are unit-tested.

use bw_model::{Process, Realm, Service, ServiceKind, Snapshot};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Network,
    Lineage,
    Resource,
    Persistence,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Network => "network",
            Family::Lineage => "lineage",
            Family::Resource => "resource",
            Family::Persistence => "persistence",
        }
    }
}

/// What a suspicion is about.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Process(bw_model::ProcKey),
    /// A service, by its location (unique).
    Service(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suspicion {
    pub target: Target,
    pub family: Family,
    pub text: String,
    /// 1 (odd) to 3 (very likely wrong).
    pub weight: u8,
}

/// Ports people commonly expose on purpose.
const USUAL_LISTEN: [u16; 18] = [
    22, 53, 80, 443, 631, 5353, 1900, 3389, 5900, 8080, 8443, 3000, 5000, 8000, 1716, 137, 138, 139,
];
/// Ports outbound traffic commonly goes to.
const USUAL_OUT: [u16; 22] = [
    80, 443, 53, 853, 22, 993, 995, 465, 587, 143, 110, 123, 4070, 5228, 3478, 19302, 8443, 9418,
    5222, 1194, 51820, 8080,
];
const SHELLS: [&str; 13] = [
    "sh", "bash", "dash", "zsh", "fish", "curl", "wget", "nc", "ncat", "socat", "python",
    "python3", "perl",
];
const SERVERS: [&str; 14] = [
    "nginx",
    "apache2",
    "httpd",
    "php-fpm",
    "node",
    "java",
    "postgres",
    "mysqld",
    "redis-server",
    "firefox",
    "chrome",
    "chromium",
    "Web Content",
    "sshd",
];
const KERNEL_LIKE: [&str; 6] = [
    "kworker",
    "ksoftirqd",
    "kthreadd",
    "migration/",
    "rcu_",
    "kswapd",
];
/// Programs expected to saturate cores.
const HEAVY: [&str; 14] = [
    "rustc", "cargo", "cc1", "cc1plus", "gcc", "clang", "ld", "lld", "ffmpeg", "blender", "make",
    "ninja", "javac", "go",
];

fn base(name: &str) -> &str {
    name.split([' ', ':']).next().unwrap_or(name)
}

fn public_ip(ip: &str) -> bool {
    !(ip.starts_with("10.")
        || ip.starts_with("192.168.")
        || ip.starts_with("127.")
        || ip.starts_with("169.254.")
        || (ip.starts_with("172.")
            && ip
                .split('.')
                .nth(1)
                .and_then(|o| o.parse::<u8>().ok())
                .is_some_and(|o| (16..32).contains(&o)))
        || ip.starts_with("fe80")
        || ip.starts_with("fd")
        || ip == "::1")
}

/// Recent history the rules need, kept across samples.
#[derive(Default)]
pub struct Baseline {
    /// Samples seen.
    pub samples: u64,
    /// Listening ports seen in the first samples: newer ones stand out.
    pub ports: Vec<u16>,
}

impl Baseline {
    pub fn observe(&mut self, s: &Snapshot) {
        self.samples += 1;
        if self.samples <= 3 {
            for l in &s.net.listening {
                if !self.ports.contains(&l.port) {
                    self.ports.push(l.port);
                }
            }
        }
    }
}

fn lineage(p: &Process, s: &Snapshot, out: &mut Vec<Suspicion>) {
    let t = Target::Process(p.key);
    if let Some(exe) = &p.exe {
        if ["/tmp/", "/dev/shm/", "/var/tmp/"]
            .iter()
            .any(|d| exe.starts_with(d))
        {
            out.push(Suspicion {
                target: t.clone(),
                family: Family::Lineage,
                text: format!("runs from {exe}"),
                weight: 3,
            });
        }
        if exe.ends_with("(deleted)") {
            out.push(Suspicion {
                target: t.clone(),
                family: Family::Lineage,
                text: "its program was deleted from disk".into(),
                weight: 3,
            });
        }
    }
    if p.realm == Realm::User && KERNEL_LIKE.iter().any(|k| p.name.starts_with(k)) {
        out.push(Suspicion {
            target: t.clone(),
            family: Family::Lineage,
            text: format!("a user process named like a kernel thread ({})", p.name),
            weight: 3,
        });
    }
    if SHELLS.contains(&base(&p.name))
        && let Some(parent) = p.parent.and_then(|k| s.processes.get(&k))
        && SERVERS.contains(&base(&parent.name))
        && base(&parent.name) != "sshd"
    {
        out.push(Suspicion {
            target: t,
            family: Family::Lineage,
            text: format!("{} spawned by {}", p.name, parent.name),
            weight: 3,
        });
    }
}

fn resource(
    p: &Process,
    cpu: Option<&VecDeque<f32>>,
    mem: Option<&VecDeque<f32>>,
    out: &mut Vec<Suspicion>,
) {
    let t = Target::Process(p.key);
    if let Some(h) = cpu
        && h.len() >= 20
        && h.iter().rev().take(20).all(|c| *c >= 85.0)
        && !HEAVY.contains(&base(&p.name))
    {
        out.push(Suspicion {
            target: t.clone(),
            family: Family::Resource,
            text: format!(
                "a core saturated for {} samples",
                h.iter().rev().take_while(|c| **c >= 85.0).count()
            ),
            weight: 2,
        });
    }
    if let Some(h) = mem
        && h.len() >= 30
    {
        let w: Vec<f32> = h.iter().rev().take(30).rev().copied().collect();
        if w.windows(2).all(|x| x[1] >= x[0]) && w[29] > w[0] * 1.25 && w[29] > 50.0 * 1_048_576.0 {
            out.push(Suspicion {
                target: t,
                family: Family::Resource,
                text: format!("memory only climbs: +{:.0}%", (w[29] / w[0] - 1.0) * 100.0),
                weight: 1,
            });
        }
    }
}

fn network(s: &Snapshot, base: &Baseline, out: &mut Vec<Suspicion>) {
    for l in &s.net.listening {
        let Some(k) = l.process else { continue };
        if !l.exposed {
            continue;
        }
        let new = base.samples > 3 && !base.ports.contains(&l.port);
        if !USUAL_LISTEN.contains(&l.port) && (l.port < 1024 || l.port > 10_000 || new) {
            out.push(Suspicion {
                target: Target::Process(k),
                family: Family::Network,
                text: format!("listening to the world on :{}", l.port),
                weight: 2,
            });
        } else if new {
            out.push(Suspicion {
                target: Target::Process(k),
                family: Family::Network,
                text: format!("started listening on :{}", l.port),
                weight: 1,
            });
        }
    }
    for c in s.net.connections.iter().filter(|c| c.outbound) {
        let Some(k) = c.process else { continue };
        if public_ip(&c.remote_addr) && !USUAL_OUT.contains(&c.remote_port) {
            out.push(Suspicion {
                target: Target::Process(k),
                family: Family::Network,
                text: format!("talks to {}:{}", c.remote_addr, c.remote_port),
                weight: 2,
            });
        }
    }
}

fn persistence(services: &[Service], now: u64, out: &mut Vec<Suspicion>) {
    const WEEK: u64 = 7 * 86_400;
    for sv in services {
        let t = Target::Service(sv.location.clone());
        let lower = sv.location.to_ascii_lowercase();
        if (lower.contains("curl") || lower.contains("wget"))
            && (lower.contains("| sh") || lower.contains("|sh") || lower.contains("| bash"))
        {
            out.push(Suspicion {
                target: t.clone(),
                family: Family::Persistence,
                text: "downloads code and runs it".into(),
                weight: 3,
            });
        }
        let starts_itself = matches!(
            sv.kind,
            ServiceKind::Autostart
                | ServiceKind::Scheduled
                | ServiceKind::SystemUnit
                | ServiceKind::UserUnit
        );
        if starts_itself && sv.changed.is_some_and(|c| now.saturating_sub(c) < WEEK) {
            out.push(Suspicion {
                target: t,
                family: Family::Persistence,
                text: format!("new {} (added this week)", sv.kind.label()),
                weight: 1,
            });
        }
    }
}

/// Every suspicion in the current state.
pub fn assess(
    s: &Snapshot,
    services: &[Service],
    cpu: &HashMap<bw_model::ProcKey, VecDeque<f32>>,
    mem: &HashMap<bw_model::ProcKey, VecDeque<f32>>,
    base: &Baseline,
) -> Vec<Suspicion> {
    let mut out = Vec::new();
    for p in s.processes.values() {
        lineage(p, s, &mut out);
        resource(p, cpu.get(&p.key), mem.get(&p.key), &mut out);
    }
    network(s, base, &mut out);
    persistence(services, s.time_ms / 1000, &mut out);
    out.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.target.cmp(&b.target)));
    out
}

/// Total weight per target, for ranking ("most suspicious first").
pub fn by_target(list: &[Suspicion]) -> Vec<(Target, u32)> {
    let mut m: HashMap<&Target, u32> = HashMap::new();
    for s in list {
        *m.entry(&s.target).or_default() += s.weight as u32;
    }
    let mut v: Vec<(Target, u32)> = m.into_iter().map(|(t, w)| (t.clone(), w)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use bw_platform::Collector;
    use bw_source::{DemoWorld, demo_services};

    #[test]
    fn demo_plants_are_found() {
        let mut w = DemoWorld::new(1);
        let mut base = Baseline::default();
        let mut cpu: HashMap<_, VecDeque<f32>> = HashMap::new();
        let mut s = w.sample();
        for _ in 0..25 {
            s = w.sample();
            base.observe(&s);
            for p in s.processes.values() {
                cpu.entry(p.key).or_default().push_back(p.cpu_pct);
            }
        }
        let services = demo_services(s.time_ms / 1000);
        let found = assess(&s, &services, &cpu, &HashMap::new(), &base);
        let has = |f: Family, needle: &str| {
            found
                .iter()
                .any(|x| x.family == f && x.text.contains(needle))
        };
        assert!(has(Family::Lineage, "/tmp/"), "{found:#?}");
        assert!(has(Family::Lineage, "kernel thread"));
        assert!(has(Family::Lineage, "spawned by nginx"));
        assert!(has(Family::Network, ":31337"));
        assert!(has(Family::Network, "45.142.212.61:4444"));
        assert!(has(Family::Resource, "saturated"));
        assert!(has(Family::Persistence, "downloads code"));
        assert!(has(Family::Persistence, "new autostart"));
        // Ordinary things are not flagged: compilers, browsers on 443, sshd.
        for x in &found {
            if let Target::Process(k) = &x.target {
                let name = &s.processes[k].name;
                assert!(
                    !["rustc", "firefox", "sshd", "Web Content"].contains(&name.as_str()),
                    "{name}: {}",
                    x.text
                );
            }
        }
        let top = by_target(&found);
        let Target::Process(k) = &top[0].0 else {
            panic!("a process should top the list")
        };
        assert_eq!(s.processes[k].name, "kworker/u8:3");
    }
}
