//! Linux network endpoints and firewall rules.
//!
//! Listening ports and connections come from `/proc/net/{tcp,tcp6,udp,udp6}`
//! (readable by anyone); sockets are matched to processes through
//! `/proc/<pid>/fd` (only our own processes unless elevated). Firewall rules
//! come from `firewall-cmd`, `ufw` or `nft`; the last two need root, so they
//! are read through `pkexec` when the user asks for it.

use bw_model::{
    Connection, Firewall, FirewallAction, FirewallRule, FirewallStatus, Listener, NetState,
    ProcKey, Proto,
};
use std::collections::HashMap;
use std::process::Command;

/// A row of `/proc/net/*`.
#[derive(Debug, PartialEq)]
pub struct SockRow {
    pub local: (String, u16),
    pub remote: (String, u16),
    pub state: u8,
    pub inode: u64,
}

/// Parse a hex address as the kernel prints it: IPv4 is one little-endian
/// 32-bit word, IPv6 four of them.
fn parse_addr(s: &str) -> Option<(String, u16)> {
    let (a, p) = s.split_once(':')?;
    let port = u16::from_str_radix(p, 16).ok()?;
    let words: Vec<u32> = (0..a.len() / 8)
        .map(|i| u32::from_str_radix(&a[i * 8..i * 8 + 8], 16))
        .collect::<Result<_, _>>()
        .ok()?;
    let addr = match words.as_slice() {
        [w] => std::net::Ipv4Addr::from(w.to_le_bytes()).to_string(),
        [a, b, c, d] => {
            let mut bytes = [0u8; 16];
            for (i, w) in [a, b, c, d].iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            let v6 = std::net::Ipv6Addr::from(bytes);
            match v6.to_ipv4_mapped() {
                Some(v4) => v4.to_string(),
                None => v6.to_string(),
            }
        }
        _ => return None,
    };
    Some((addr, port))
}

pub fn parse_proc_net(text: &str) -> Vec<SockRow> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 {
                return None;
            }
            Some(SockRow {
                local: parse_addr(f[1])?,
                remote: parse_addr(f[2])?,
                state: u8::from_str_radix(f[3], 16).ok()?,
                inode: f[9].parse().ok()?,
            })
        })
        .collect()
}

const TCP_ESTABLISHED: u8 = 0x01;
const TCP_LISTEN: u8 = 0x0A;
const UDP_UNCONNECTED: u8 = 0x07;

fn loopback(addr: &str) -> bool {
    addr.starts_with("127.") || addr == "::1"
}

fn unspecified(addr: &str) -> bool {
    addr == "0.0.0.0" || addr == "::"
}

/// Socket inode → owning pid, for every process whose descriptors we can read.
fn socket_owners() -> HashMap<u64, u32> {
    let mut out = HashMap::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return out;
    };
    for e in procs.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(t) = std::fs::read_link(fd.path())
                && let Some(n) = t
                    .to_str()
                    .and_then(|s| s.strip_prefix("socket:["))
                    .and_then(|s| s.strip_suffix(']'))
                    .and_then(|s| s.parse().ok())
            {
                out.insert(n, pid);
            }
        }
    }
    out
}

/// Cached socket owners: scanning every process's descriptors is the
/// expensive part, so it is redone every few samples.
#[derive(Default)]
pub struct NetCache {
    owners: HashMap<u64, u32>,
    age: u32,
}

pub fn net(cache: &mut NetCache, keys: &HashMap<u32, ProcKey>) -> NetState {
    if cache.age == 0 {
        cache.owners = socket_owners();
    }
    cache.age = (cache.age + 1) % 5;
    let read = |f: &str| std::fs::read_to_string(format!("/proc/net/{f}")).unwrap_or_default();
    let mut listening = Vec::new();
    let mut socks = Vec::new();
    for (f, proto) in [
        ("tcp", Proto::Tcp),
        ("tcp6", Proto::Tcp),
        ("udp", Proto::Udp),
        ("udp6", Proto::Udp),
    ] {
        for r in parse_proc_net(&read(f)) {
            let process = cache
                .owners
                .get(&r.inode)
                .and_then(|p| keys.get(p))
                .copied();
            let listens = match proto {
                Proto::Tcp => r.state == TCP_LISTEN,
                Proto::Udp => r.state == UDP_UNCONNECTED && r.remote.1 == 0,
            };
            if listens {
                if !listening
                    .iter()
                    .any(|l: &Listener| l.port == r.local.1 && l.proto == proto)
                {
                    listening.push(Listener {
                        proto,
                        port: r.local.1,
                        exposed: !loopback(&r.local.0),
                        addr: r.local.0.clone(),
                        process,
                    });
                } else if let Some(l) = listening
                    .iter_mut()
                    .find(|l| l.port == r.local.1 && l.proto == proto)
                {
                    // Bound on IPv4 and IPv6: exposed if either is.
                    l.exposed |= !loopback(&r.local.0);
                    l.process = l.process.or(process);
                }
            } else if proto == Proto::Tcp && r.state == TCP_ESTABLISHED {
                socks.push((proto, r, process));
            }
        }
    }
    let connections = socks
        .into_iter()
        .filter(|(_, r, _)| !(loopback(&r.remote.0) || unspecified(&r.remote.0)))
        .map(|(proto, r, process)| Connection {
            proto,
            local_port: r.local.1,
            outbound: !listening.iter().any(|l| l.port == r.local.1),
            remote_addr: r.remote.0,
            remote_port: r.remote.1,
            process,
        })
        .collect();
    NetState {
        listening,
        connections,
        firewall: Firewall::default(),
    }
}

/// Run a command; its stdout if it succeeded.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn have(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| d.join(cmd).is_file())
            || ["/usr/sbin", "/sbin"]
                .iter()
                .any(|d| std::path::Path::new(d).join(cmd).is_file())
    })
}

fn parse_action(s: &str) -> Option<FirewallAction> {
    match s.to_ascii_lowercase().as_str() {
        "allow" | "accept" => Some(FirewallAction::Allow),
        "deny" | "drop" | "reject" | "limit" => Some(FirewallAction::Deny),
        _ => None,
    }
}

/// `ufw status verbose`.
pub fn parse_ufw(text: &str) -> Firewall {
    let mut fw = Firewall {
        backend: "ufw".into(),
        ..Default::default()
    };
    for l in text.lines() {
        let l = l.trim();
        if let Some(s) = l.strip_prefix("Status:") {
            fw.status = if s.trim() == "active" {
                FirewallStatus::Active
            } else {
                FirewallStatus::Inactive
            };
        } else if let Some(d) = l.strip_prefix("Default:") {
            fw.inbound_default = d
                .split(',')
                .find(|p| p.contains("incoming"))
                .and_then(|p| p.split_whitespace().next())
                .and_then(parse_action);
        } else if l.contains(" IN ") || l.ends_with(" IN") || l.contains("ALLOW") {
            let mut f = l.split_whitespace();
            let (Some(to), Some(action)) = (f.next(), f.next()) else {
                continue;
            };
            let Some(action) = parse_action(action) else {
                continue;
            };
            let (ports, proto) = match to.split_once('/') {
                Some((p, pr)) => (p, Some(if pr == "udp" { Proto::Udp } else { Proto::Tcp })),
                None => (to, None),
            };
            for p in ports.split(',') {
                let port = p.split(':').next().and_then(|p| p.parse().ok());
                if port.is_some() || p == "Anywhere" {
                    fw.rules.push(FirewallRule {
                        port,
                        proto,
                        action,
                        text: l.to_string(),
                    });
                }
            }
        }
    }
    fw
}

/// `nft list ruleset`: the input chains' policy and their port rules.
pub fn parse_nft(text: &str) -> Firewall {
    let mut fw = Firewall {
        backend: "nftables".into(),
        status: FirewallStatus::Inactive,
        ..Default::default()
    };
    let mut in_input = false;
    for l in text.lines().map(str::trim) {
        if l.starts_with("chain ") {
            in_input = false;
        }
        if l.contains("hook input") {
            in_input = true;
            fw.status = FirewallStatus::Active;
            if let Some(p) = l.split("policy").nth(1) {
                fw.inbound_default = p
                    .split(|c: char| !c.is_alphanumeric())
                    .find(|w| !w.is_empty())
                    .and_then(parse_action);
            }
            continue;
        }
        if !in_input || !l.contains("dport") {
            continue;
        }
        let proto = if l.starts_with("udp") || l.contains(" udp ") {
            Some(Proto::Udp)
        } else if l.starts_with("tcp") || l.contains(" tcp ") {
            Some(Proto::Tcp)
        } else {
            None
        };
        let Some(action) = l.split_whitespace().rev().find_map(parse_action) else {
            continue;
        };
        let after = l.split("dport").nth(1).unwrap_or("");
        let spec: String = if let Some(set) = after.trim_start().strip_prefix('{') {
            set.split('}').next().unwrap_or("").to_string()
        } else {
            after.split_whitespace().next().unwrap_or("").to_string()
        };
        for p in spec.split(',') {
            if let Ok(port) = p.trim().split('-').next().unwrap_or("").parse() {
                fw.rules.push(FirewallRule {
                    port: Some(port),
                    proto,
                    action,
                    text: l.to_string(),
                });
            }
        }
    }
    fw
}

/// Well-known firewalld service names → port.
fn firewalld_service(name: &str) -> Option<(u16, Proto)> {
    Some(match name {
        "ssh" => (22, Proto::Tcp),
        "http" => (80, Proto::Tcp),
        "https" => (443, Proto::Tcp),
        "dhcpv6-client" => (546, Proto::Udp),
        "mdns" => (5353, Proto::Udp),
        "samba-client" => (137, Proto::Udp),
        "cockpit" => (9090, Proto::Tcp),
        "kdeconnect" => (1716, Proto::Tcp),
        _ => return None,
    })
}

/// `firewall-cmd --list-all`.
pub fn parse_firewalld(text: &str) -> Firewall {
    let mut fw = Firewall {
        backend: "firewalld".into(),
        status: FirewallStatus::Active,
        inbound_default: Some(FirewallAction::Deny),
        ..Default::default()
    };
    for l in text.lines().map(str::trim) {
        if let Some(t) = l.strip_prefix("target:")
            && t.trim().eq_ignore_ascii_case("ACCEPT")
        {
            fw.inbound_default = Some(FirewallAction::Allow);
        }
        if let Some(s) = l.strip_prefix("services:") {
            for name in s.split_whitespace() {
                if let Some((port, proto)) = firewalld_service(name) {
                    fw.rules.push(FirewallRule {
                        port: Some(port),
                        proto: Some(proto),
                        action: FirewallAction::Allow,
                        text: format!("service {name}"),
                    });
                }
            }
        }
        if let Some(s) = l.strip_prefix("ports:") {
            for spec in s.split_whitespace() {
                if let Some((p, pr)) = spec.split_once('/')
                    && let Ok(port) = p.split('-').next().unwrap_or("").parse()
                {
                    fw.rules.push(FirewallRule {
                        port: Some(port),
                        proto: Some(if pr == "udp" { Proto::Udp } else { Proto::Tcp }),
                        action: FirewallAction::Allow,
                        text: format!("port {spec}"),
                    });
                }
            }
        }
    }
    fw
}

/// Read the firewall. Without `elevate` only what an ordinary user may see
/// (firewalld usually answers); with it, `ufw` or `nft` through `pkexec`,
/// which shows the system's own admin prompt.
pub fn firewall(elevate: bool) -> Option<Firewall> {
    if have("firewall-cmd")
        && run("firewall-cmd", &["--state"]).is_some_and(|s| s.trim() == "running")
        && let Some(t) = run("firewall-cmd", &["--list-all"])
    {
        return Some(parse_firewalld(&t));
    }
    if have("ufw") {
        if let Some(t) = run("ufw", &["status", "verbose"]) {
            return Some(parse_ufw(&t));
        }
        if elevate && let Some(t) = run("pkexec", &["ufw", "status", "verbose"]) {
            return Some(parse_ufw(&t));
        }
    }
    if have("nft") {
        if let Some(t) = run("nft", &["list", "ruleset"]) {
            return Some(parse_nft(&t));
        }
        if elevate && let Some(t) = run("pkexec", &["nft", "list", "ruleset"]) {
            return Some(parse_nft(&t));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_net_rows() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0CEA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 31312 1 0000000000000000 100 0 0 10 0
   1: 0F02000A:D2B4 22D8B85D:01BB 01 00000000:00000000 02:00000A2C 00000000  1000        0 98765 2 0000000000000000 20 4 30 10 -1";
        let r = parse_proc_net(t);
        assert_eq!(r[0].local, ("127.0.0.1".into(), 3306));
        assert_eq!(r[0].state, TCP_LISTEN);
        assert_eq!(r[1].remote, ("93.184.216.34".into(), 443));
        assert_eq!(r[1].inode, 98765);
        let v6 = parse_addr("00000000000000000000000000000000:0016").unwrap();
        assert_eq!(v6, ("::".into(), 22));
    }

    #[test]
    fn ufw() {
        let fw = parse_ufw(
            "Status: active\nLogging: on (low)\nDefault: deny (incoming), allow (outgoing), disabled (routed)\n\nTo                         Action      From\n--                         ------      ----\n22/tcp                     ALLOW IN    Anywhere\n80,443/tcp                 ALLOW IN    Anywhere\n3000                       DENY IN     Anywhere\n",
        );
        assert_eq!(fw.status, FirewallStatus::Active);
        assert_eq!(fw.verdict(22, Proto::Tcp), Some(FirewallAction::Allow));
        assert_eq!(fw.verdict(443, Proto::Tcp), Some(FirewallAction::Allow));
        assert_eq!(fw.verdict(3000, Proto::Tcp), Some(FirewallAction::Deny));
        assert_eq!(fw.verdict(5432, Proto::Tcp), Some(FirewallAction::Deny));
    }

    #[test]
    fn nft() {
        let fw = parse_nft(
            "table inet filter {\n\tchain input {\n\t\ttype filter hook input priority filter; policy drop;\n\t\tct state established,related accept\n\t\ttcp dport 22 accept\n\t\ttcp dport { 80, 443 } accept\n\t\tudp dport 53 drop\n\t}\n\tchain output {\n\t\ttype filter hook output priority filter; policy accept;\n\t\ttcp dport 25 drop\n\t}\n}\n",
        );
        assert_eq!(fw.inbound_default, Some(FirewallAction::Deny));
        assert_eq!(fw.verdict(22, Proto::Tcp), Some(FirewallAction::Allow));
        assert_eq!(fw.verdict(80, Proto::Tcp), Some(FirewallAction::Allow));
        assert_eq!(fw.verdict(53, Proto::Udp), Some(FirewallAction::Deny));
        // Output-chain rules are not inbound rules.
        assert!(fw.rules.iter().all(|r| r.port != Some(25)));
    }

    #[test]
    fn firewalld() {
        let fw = parse_firewalld(
            "public (active)\n  target: default\n  services: dhcpv6-client ssh\n  ports: 8080/tcp 60000-60010/udp\n",
        );
        assert_eq!(fw.verdict(22, Proto::Tcp), Some(FirewallAction::Allow));
        assert_eq!(fw.verdict(8080, Proto::Tcp), Some(FirewallAction::Allow));
        assert_eq!(fw.verdict(3000, Proto::Tcp), Some(FirewallAction::Deny));
    }
}
