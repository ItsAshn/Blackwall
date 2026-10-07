//! Linux services: the systemd units the admin or user added (not the
//! distribution's own), login autostart entries and cron jobs.
//!
//! systemd only remembers activity since boot; for units that have not run
//! this boot the journal says when they last did.

use crate::services::mtime;
use bw_model::{Service, ServiceKind, ServiceState};
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn boot_time() -> Option<u64> {
    std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

/// Unit files defined in `dir` itself (not enablement symlinks into the
/// distribution's units).
fn unit_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x == "service" || x == "timer")
        })
        .collect();
    v.sort();
    v.truncate(40);
    v
}

/// `systemctl show` output for several units: one map per unit.
pub fn parse_show(text: &str) -> Vec<std::collections::HashMap<String, String>> {
    text.split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .map(|b| {
            b.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .collect()
}

/// The last time the journal saw the unit, if we may read it.
fn journal_last(unit: &str, user: bool) -> Option<u64> {
    let mut args = vec![];
    if user {
        args.push("--user");
    }
    args.extend([
        "-u",
        unit,
        "-n",
        "1",
        "-o",
        "short-unix",
        "--no-pager",
        "-q",
    ]);
    let out = run("journalctl", &args)?;
    out.lines()
        .last()?
        .split_whitespace()
        .next()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn units(dir: &Path, user: bool) -> Vec<Service> {
    let files = unit_files(dir);
    if files.is_empty() {
        return vec![];
    }
    let names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let mut args: Vec<&str> = vec![];
    if user {
        args.push("--user");
    }
    args.extend([
        "show",
        "-p",
        "Id,ActiveState,UnitFileState,ActiveEnterTimestampMonotonic,InactiveEnterTimestampMonotonic",
    ]);
    args.extend(names.iter().map(String::as_str));
    let shown = run("systemctl", &args)
        .map(|t| parse_show(&t))
        .unwrap_or_default();
    let boot = boot_time().unwrap_or(0);
    names
        .iter()
        .zip(&files)
        .map(|(name, path)| {
            let m = shown
                .iter()
                .find(|m| m.get("Id").is_some_and(|i| i == name));
            let get = |k: &str| m.and_then(|m| m.get(k)).map(String::as_str).unwrap_or("");
            let mono = |k: &str| get(k).parse::<u64>().ok().filter(|v| *v > 0);
            let active = get("ActiveState");
            let state = match active {
                "active" | "activating" | "reloading" => ServiceState::Running,
                "failed" => ServiceState::Failed,
                _ => ServiceState::Idle,
            };
            let this_boot = mono("InactiveEnterTimestampMonotonic")
                .max(mono("ActiveEnterTimestampMonotonic"))
                .map(|us| boot + us / 1_000_000);
            Service {
                kind: if name.ends_with(".timer") {
                    ServiceKind::Scheduled
                } else if user {
                    ServiceKind::UserUnit
                } else {
                    ServiceKind::SystemUnit
                },
                name: name.trim_end_matches(".service").to_string(),
                location: path.to_string_lossy().into_owned(),
                state,
                enabled: get("UnitFileState") == "enabled",
                last_active: if state == ServiceState::Running {
                    None
                } else {
                    this_boot.or_else(|| journal_last(name, user))
                },
                changed: mtime(path),
            }
        })
        .collect()
}

fn desktop_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("Name="))
        .map(str::to_string)
}

fn autostart(home: &Path) -> Vec<Service> {
    let Ok(rd) = std::fs::read_dir(home.join(".config/autostart")) else {
        return vec![];
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "desktop"))
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            if text.lines().any(|l| l.trim() == "Hidden=true") {
                return None;
            }
            Some(Service {
                kind: ServiceKind::Autostart,
                name: desktop_name(&text).unwrap_or_else(|| {
                    p.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                location: p.to_string_lossy().into_owned(),
                state: ServiceState::Idle,
                enabled: true,
                last_active: None,
                changed: mtime(&p),
            })
        })
        .collect()
}

/// The command of a crontab line (after its five schedule fields or @word).
pub fn cron_command(line: &str) -> Option<String> {
    let l = line.trim();
    if l.is_empty() || l.starts_with('#') || l.contains('=') && !l.contains(' ') {
        return None;
    }
    let skip = if l.starts_with('@') { 1 } else { 5 };
    let cmd: Vec<&str> = l.split_whitespace().skip(skip).collect();
    (!cmd.is_empty()).then(|| cmd.join(" "))
}

fn cron() -> Vec<Service> {
    let mut out = Vec::new();
    for l in run("crontab", &["-l"]).unwrap_or_default().lines() {
        if let Some(cmd) = cron_command(l) {
            out.push(Service {
                kind: ServiceKind::Scheduled,
                name: cmd.chars().take(48).collect(),
                location: format!("crontab: {}", l.trim()),
                state: ServiceState::Idle,
                enabled: true,
                last_active: None,
                changed: None,
            });
        }
    }
    if let Ok(rd) = std::fs::read_dir("/etc/cron.d") {
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            out.push(Service {
                kind: ServiceKind::Scheduled,
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                location: p.to_string_lossy().into_owned(),
                state: ServiceState::Idle,
                enabled: true,
                last_active: None,
                changed: mtime(&p),
            });
        }
    }
    out
}

pub fn services() -> Vec<Service> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut out = units(Path::new("/etc/systemd/system"), false);
    if let Some(h) = &home {
        out.extend(units(&h.join(".config/systemd/user"), true));
        out.extend(autostart(h));
    }
    out.extend(cron());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_blocks() {
        let m = parse_show(
            "Id=odysseus-api.service\nActiveState=inactive\nUnitFileState=disabled\nActiveEnterTimestampMonotonic=0\n\nId=backup.timer\nActiveState=active\n",
        );
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["ActiveState"], "inactive");
        assert_eq!(m[1]["Id"], "backup.timer");
    }

    #[test]
    fn crontab_lines() {
        assert_eq!(
            cron_command("*/5 * * * * /home/me/bin/sync.sh --quiet").as_deref(),
            Some("/home/me/bin/sync.sh --quiet")
        );
        assert_eq!(
            cron_command("@reboot curl x | sh").as_deref(),
            Some("curl x | sh")
        );
        assert_eq!(cron_command("# comment"), None);
        assert_eq!(cron_command("MAILTO=me"), None);
    }
}
