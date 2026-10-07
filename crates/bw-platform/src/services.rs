//! Services that exist on the machine whether or not they run: containers
//! (Docker, Podman) and project folders that define services (a compose
//! file, a Procfile, a unit file, a package with a start script). Works the
//! same on every OS; per-OS units (systemd, launchd, Windows services) come
//! from the `os` module.
//!
//! The point is the ones nobody has run in a long time: an old project's
//! compose stack, a container that exited a year ago.

use bw_model::{Service, ServiceKind, ServiceState};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// RFC 3339 timestamp ("2024-08-01T12:34:56.123Z", "…+02:00") to Unix
/// seconds. Docker's "0001-01-01T00:00:00Z" (never) is `None`.
pub fn parse_rfc3339(s: &str) -> Option<u64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if y < 1971 {
        return None;
    }
    let rest = &s[19..];
    let tz = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match tz.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let oh: i64 = tz.get(1..3)?.parse().ok()?;
            let om: i64 = tz.get(4..6).and_then(|x| x.parse().ok()).unwrap_or(0);
            sign * (oh * 3600 + om * 60)
        }
        _ => 0,
    };
    let t = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset;
    u64::try_from(t).ok()
}

pub fn mtime(p: &Path) -> Option<u64> {
    std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// A container, with the compose project it belongs to (if any).
pub struct ContainerInfo {
    pub service: Service,
    pub project_dir: Option<String>,
}

/// Containers from Docker and Podman, running or not.
pub fn containers() -> Vec<ContainerInfo> {
    let mut out = Vec::new();
    for tool in ["docker", "podman"] {
        let Some(list) = run(
            tool,
            &[
                "ps",
                "-a",
                "--format",
                "{{.Names}}\t{{.State}}\t{{.Label \"com.docker.compose.project.working_dir\"}}",
            ],
        ) else {
            continue;
        };
        let rows: Vec<(String, String, String)> = list
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                Some((
                    f.next()?.to_string(),
                    f.next().unwrap_or("").to_string(),
                    f.next().unwrap_or("").to_string(),
                ))
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let mut args = vec![
            "inspect",
            "-f",
            "{{.Name}}\t{{.State.StartedAt}}\t{{.State.FinishedAt}}\t{{.HostConfig.RestartPolicy.Name}}\t{{.Created}}",
        ];
        args.extend(names.iter());
        let inspect = run(tool, &args).unwrap_or_default();
        for (name, state, dir) in rows {
            let row = inspect
                .lines()
                .find(|l| l.trim_start_matches('/').starts_with(&format!("{name}\t")));
            let f: Vec<&str> = row.map(|r| r.split('\t').collect()).unwrap_or_default();
            let started = f.get(1).and_then(|x| parse_rfc3339(x));
            let finished = f.get(2).and_then(|x| parse_rfc3339(x));
            let restart = f.get(3).copied().unwrap_or("");
            let created = f.get(4).and_then(|x| parse_rfc3339(x));
            let running = state == "running";
            out.push(ContainerInfo {
                service: Service {
                    kind: ServiceKind::Container,
                    name: name.clone(),
                    location: format!("{tool}: {name}"),
                    state: if running {
                        ServiceState::Running
                    } else if state == "dead" {
                        ServiceState::Failed
                    } else {
                        ServiceState::Idle
                    },
                    enabled: !restart.is_empty() && restart != "no",
                    last_active: if running { None } else { finished.max(started) },
                    changed: created,
                },
                project_dir: (!dir.is_empty()).then_some(dir),
            });
        }
    }
    out
}

/// Files that mark a folder as defining services.
const MARKERS: [&str; 7] = [
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
    "Procfile",
    "Dockerfile",
    "fly.toml",
];

/// Folders never worth descending into.
fn skip(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules"
                | "target"
                | "venv"
                | "env"
                | "__pycache__"
                | "dist"
                | "build"
                | "Library"
                | "AppData"
                | "snap"
                | "go"
                | "Downloads"
                | "Pictures"
                | "Music"
                | "Videos"
        )
}

/// Whether `dir` defines services, and how ("compose", "Procfile"…).
fn project_kind(dir: &Path) -> Option<&'static str> {
    for m in MARKERS {
        if dir.join(m).is_file() {
            return Some(match m {
                "Procfile" => "Procfile",
                "Dockerfile" => "Dockerfile",
                "fly.toml" => "fly.io app",
                _ => "compose",
            });
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "service") {
                return Some("unit file");
            }
        }
    }
    let pkg = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let scripts = pkg.split("\"scripts\"").nth(1)?;
    let block = scripts.split('}').next()?;
    ["\"start\"", "\"serve\"", "\"dev\""]
        .iter()
        .any(|k| block.contains(k))
        .then_some("node app")
}

/// Most recent change among a folder's top-level entries: when it was last
/// worked on.
fn last_touched(dir: &Path) -> Option<u64> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.') || e.file_name() == ".env")
        .filter_map(|e| mtime(&e.path()))
        .max()
}

/// Project folders under `root` (three levels deep) that define services.
pub fn projects(root: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > 5000 {
            break;
        }
        if depth > 0
            && let Some(kind) = project_kind(&dir)
        {
            out.push((dir, kind));
            continue;
        }
        if depth >= 3 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !skip(&name) && e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    out.sort();
    out
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Containers and project folders, with each project's last run taken from
/// its compose containers when there are any.
pub fn common() -> Vec<Service> {
    let containers = containers();
    let mut out: Vec<Service> = Vec::new();
    if let Some(h) = home() {
        for (dir, kind) in projects(&h) {
            let d = dir.to_string_lossy().into_owned();
            let mine: Vec<&ContainerInfo> = containers
                .iter()
                .filter(|c| c.project_dir.as_deref() == Some(d.as_str()))
                .collect();
            let running = mine
                .iter()
                .any(|c| c.service.state == ServiceState::Running);
            out.push(Service {
                kind: ServiceKind::Project,
                name: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| d.clone()),
                location: format!("{d} ({kind})"),
                state: if running {
                    ServiceState::Running
                } else {
                    ServiceState::Idle
                },
                enabled: false,
                last_active: mine.iter().filter_map(|c| c.service.last_active).max(),
                changed: last_touched(&dir),
            });
        }
    }
    out.extend(containers.into_iter().map(|c| c.service));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-02T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2001-09-09T01:46:40Z"), Some(1_000_000_000));
        assert_eq!(
            parse_rfc3339("2001-09-09T03:46:40.123456789+02:00"),
            Some(1_000_000_000)
        );
        assert_eq!(parse_rfc3339("0001-01-01T00:00:00Z"), None);
    }

    #[test]
    fn finds_projects() {
        let root = std::env::temp_dir().join(format!("bw-proj-{}", std::process::id()));
        let p = root.join("code/odysseus");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("docker-compose.yml"), "services: {}\n").unwrap();
        std::fs::create_dir_all(root.join("code/odysseus/node_modules/x")).unwrap();
        let web = root.join("code/site");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(
            web.join("package.json"),
            "{\"scripts\": {\"dev\": \"vite\"}}",
        )
        .unwrap();
        let found = projects(&root);
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found
                .iter()
                .any(|(d, k)| d.ends_with("odysseus") && *k == "compose")
        );
        assert!(
            found
                .iter()
                .any(|(d, k)| d.ends_with("site") && *k == "node app")
        );
    }
}
