# Blackwall — Project Plan

> A cyberpunk system visualizer. User space is **Deep Space**: a dark void where processes drift as living entities. The **Blackwall** is the kernel/user boundary: a vast luminous barrier, with the kernel's machinery humming behind it. You can drift through it as ambient art, then drill into any entity for real, actionable diagnostics.

---

## 1. Requirements (from the discovery interview)

| Topic | Decision |
|---|---|
| Purpose | **Balanced**: an immersive view that is also a real diagnostics tool you can drill into |
| Platforms | Linux, Windows, macOS. Fleet / remote servers come **later**, but the protocol is designed for them from day one |
| Visual form | **Hybrid**: a 3D scene plus a flat, readable HUD overlay |
| Stack | **Rust** backend, web frontend, packaged as a **Tauri** desktop app |
| Metaphor | **Wall = kernel/user boundary**. Kernel and drivers sit behind the Wall. User-space processes float in Deep Space in front of it. Syscalls, IO and interrupts are streams crossing it |
| Data layers (v1) | Processes and tree, storage, network, issues/health. All four are required |
| Control | **Full control** (kill/suspend/renice, services, firewall), so it needs a real privilege and security design |
| Detection | **Rules plus learned per-machine baselines** (anomaly / novelty detection) |
| History | **Timeline plus replay**: scrub back in time and replay incidents |
| Immersion | Sound design and a screensaver mode. **Full keyboard and touch support**, but as plain, accessible UX rather than a netrunner theme |
| Performance | **Adaptive quality tiers**: detect the GPU, scale effects automatically, with a manual override |
| Team / pace | Solo side project, so the plan is a series of small, shippable milestones |

---

## 2. Choosing the 3D stack

The constraint that settles this is **Tauri's webviews**. Tauri renders with the OS webview: WebView2 (Chromium) on Windows, WKWebView (Safari) on macOS, and **WebKitGTK on Linux**. Windows and recent macOS can do WebGPU. WebKitGTK on Linux cannot be relied on for WebGPU, and its WebGL performance has historically trailed the other two. So whatever we choose must:

1. run on **WebGL2 as a first-class path**, and use WebGPU where it exists
2. handle about 1,000–5,000 live entities through instancing
3. leave room for a rich, accessible, text-heavy HUD

| Option | Verdict |
|---|---|
| **Three.js (`WebGPURenderer` + TSL) + Svelte 5 HUD** | ✅ **Chosen.** One renderer that uses WebGPU when available and **falls back to WebGL2 automatically** (default since r171). TSL shaders compile to both WGSL and GLSL, so the glitch, bloom and hologram shaders are written once. It has by far the largest ecosystem. The scene is managed directly in code (not through React), which suits high-frequency data. Svelte 5's fine-grained reactivity keeps the HUD cheap when hundreds of values update every second |
| Three.js + React Three Fiber | Strong runner-up with a great ecosystem. But React reconciliation for a scene whose data changes every tick is overhead, and we would end up bypassing it with direct refs anyway. Choose it only if you already know React well |
| Babylon.js | A capable, batteries-included engine with a GUI and inspector. Its HUD toolkit is canvas-based and weaker for text and accessibility than DOM, and its ecosystem is much smaller |
| PlayCanvas | Editor-centric and cloud-oriented, a poor fit for a code-driven data visualization |
| Bevy (native Rust, no webview) | Tempting: one language, native GPU, no webview risk. But its UI story (text, forms, accessibility, touch, IME) is far behind the DOM, and we would lose Tauri and the shared web frontend for a future fleet UI. **Fallback plan only**, if Milestone 0 proves WebKitGTK can't hit the frame budget |

**Frontend:** Vite + TypeScript + Svelte 5 for the HUD, with Three.js `WebGPURenderer` and TSL for the scene, post-processing through Three's node-based pipeline (bloom, chromatic aberration, scanlines, glitch), and the Web Audio API for sound.

**Backend:** Rust workspace with Tauri v2 and Tokio, using Tauri `ipc::Channel` for streaming.

---

## 3. Architecture

```
┌──────────────────────── Tauri app (unprivileged) ─────────────────────────┐
│  Frontend (webview)                                                        │
│   ├─ Scene (Three.js)   ← world-state store ← delta decoder               │
│   ├─ HUD (Svelte)       ← same store (inspector, alerts, timeline, search)│
│   └─ Audio (WebAudio)   ← derived load / alert signals                    │
│                    ▲ ipc::Channel (snapshot + deltas, MessagePack)        │
│  Rust core ────────┴──────────────────────────────────────────────────────│
│   ├─ Source trait: LiveSource | ReplaySource | (later) RemoteSource       │
│   ├─ collector/   per-OS probes → normalized model                        │
│   ├─ detect/      rules engine + baselines → Issues                       │
│   ├─ store/       SQLite time-series + events (downsampled tiers)         │
│   └─ actions/     client → privileged helper (signed IPC)                 │
└────────────────────────────────────────────────────────────────────────────┘
                         │ local socket / named pipe, authenticated
┌────────────────────────▼───────────────────────────────────────────────────┐
│  blackwall-ice (privileged helper)  — allowlisted ops, audit log, no UI    │
│  Linux: systemd unit + polkit │ Windows: service │ macOS: SMAppService     │
└────────────────────────────────────────────────────────────────────────────┘
```

### 3.1 Key design principles
- **One normalized model, many sources.** The UI never knows whether data comes from live probes, a replay of the store, or (later) a remote agent. Fleet support then means adding a `RemoteSource`, not rewriting the app.
- **Snapshot + delta protocol.** On connect, send a full snapshot; then send deltas at about 1–4 Hz (configurable). Every entity has a stable ID (`host:pid:start_time`, because PIDs get reused). Types are defined once in Rust and exported to TypeScript (`ts-rs` or `specta`).
- **Capabilities, not assumptions.** Each OS probe advertises what it can see (`kernel_threads`, `per_proc_net`, `smart`, `syscall_rate`, …). The UI degrades gracefully and shows *"not visible on this platform"* instead of fake data.
- **The UI is never privileged.** All mutations go through the helper (§7).

### 3.2 Repository layout
```
blackwall/
├─ Cargo.toml                 # workspace
├─ crates/
│  ├─ bw-model/               # entity types, protocol, IDs, capabilities (no OS deps)
│  ├─ bw-collect/             # Collector trait + linux/ windows/ macos/ modules
│  ├─ bw-store/               # SQLite time-series, events, retention, replay cursor
│  ├─ bw-detect/              # rules (TOML) + baselines + issue lifecycle
│  ├─ bw-actions/             # action schema, client, audit types
│  └─ bw-ice/                 # privileged helper binary
├─ app/
│  ├─ src-tauri/              # Tauri shell, wires Source → Channel
│  └─ src/                    # Svelte + Three.js frontend
│     ├─ scene/  (layout, instancing, shaders/TSL, camera, quality tiers)
│     ├─ hud/    (panels, timeline, alerts, command palette, inspector)
│     ├─ input/  (keyboard focus model, touch gestures)
│     └─ audio/
├─ rules/                     # default detection rules (TOML)
└─ docs/
```

---

## 4. Data collection (per layer, per OS)

The baseline everywhere is the [`sysinfo`](https://crates.io/crates/sysinfo) crate for processes, CPU, memory, disks and network interfaces. Platform modules add depth on top of it.

| Layer | Linux (richest, built first) | Windows | macOS |
|---|---|---|---|
| **Processes + tree** | `procfs` crate: ppid, state (incl. **zombie / D-state**), threads, cgroup, start time, cmdline | `sysinfo` + Toolhelp/NtQuery, job objects | `libproc` (`proc_pidinfo`), `sysctl` |
| **Kernel side (behind the Wall)** | kthreads (children of `kthreadd`), `/proc/stat` (ctx switches, interrupts, softirqs), `/proc/pressure/*` (**PSI**), `/proc/vmstat`, loaded modules. Later: **eBPF via `aya`** for per-process syscall rates | PID 4 "System", DPC/ISR time, drivers list; later ETW (`ferrisetw`) | `kernel_task`, kexts, `host_statistics64` |
| **Storage** | `/proc/diskstats` (IO and latency), mounts, fill, SMART via `smartctl --json` (optional), inode exhaustion | `GetDiskFreeSpaceEx`, perf counters, SMART via WMI | IOKit stats, `smartctl` |
| **Network** | `netlink sock_diag` for sockets mapped to PIDs (`netstat2` crate), per-interface throughput | `GetExtendedTcpTable` (`netstat2`) | `libproc` fd/socket info (`netstat2`) |
| **Issues / events** | journald (`systemd-journal`), kmsg (**OOM kills**, segfaults, IO errors), failed systemd units, hwmon temps | Event Log (crash/WER, service failures), WMI temps where exposed | Unified log (`log stream --predicate`), crash reports dir |

Collection cadence comes in tiers: **fast** (1 Hz: CPU/mem/IO/net rates), **medium** (5 s: sockets, process tree diff), **slow** (60 s: SMART, disk scans, service states). The collector must stay below about 1–2% of one CPU core. Measure this in Milestone 1 and keep it as a CI benchmark.

---

## 5. The visual language

### 5.1 Spatial layout
- **The Blackwall**: a vast, slightly curved plane in the far background. It is a translucent hexagonal lattice with slow, flowing red-violet energy. Its **brightness and turbulence track kernel pressure** (PSI, iowait, softirq load). Under heavy pressure it ripples, and under critical pressure it **cracks and glitches**.
- **Behind the Wall (kernel space)**: dim, monolithic structures seen through the barrier, one per subsystem: *Scheduler*, *Memory Manager*, *VFS / Block layer*, *Network stack*, *Drivers / Modules*, *Interrupts*. Kernel threads are small sparks clustered around their subsystem.
- **Deep Space (user space)**: processes, laid out as a **process tree in orbital form**. Init/launchd/services.exe is a central "star"; services and session leaders orbit it, and children orbit their parents. Layout is a stable, damped force simulation so things don't jump around. New processes **materialize** and exiting ones **dissolve**.
- **Storage**: "data fortresses" anchored at the Wall's edge, one per physical disk, with volumes as rings around them. A ring's fill shows usage and its color shows health (SMART). IO appears as **particle streams from a process, through the Wall, to its disk**. Stream thickness is throughput, and stream color is latency.
- **Network**: interfaces are "gates" at the edge of space. Connections are beams from a process out to remote endpoints, which appear as distant star clusters grouped by subnet or ASN. Listening ports are small open "ports" on the entity's surface.
- **Crossing the Wall**: syscalls, IO, page faults and interrupts are shown as **data streams crossing the barrier**. This is the core of the metaphor. Linux gets the richest view (eBPF later); other OSes use the aggregate rates they expose.

### 5.2 Encoding a process (redundant: never color alone)
| Property | Visual channel |
|---|---|
| Memory (RSS) | Size |
| CPU % | Brightness and pulse rate |
| Owner (root/system / user / other user) | Shape (octahedron / sphere / icosahedron) and base hue |
| State: zombie | Grey hollow husk that doesn't pulse |
| State: uninterruptible IO (D) | Pulled toward the Wall, tethered to its disk |
| Anomaly / issue | Red glitch shader, corruption particles, and a HUD marker with an icon |
| Network active | Orbiting particles; beams when selected |

### 5.3 HUD (Svelte, DOM-based, readable)
- **Inspector** for the selected entity: real numbers, sparklines, open files and sockets, children, history, and an actions menu.
- **Alert feed**: the active issues list, sorted by severity; clicking an alert flies the camera to it.
- **Timeline** along the bottom edge: live/replay toggle, scrubber, and incident markers.
- **Search / command palette** (`/` or `Ctrl+K`): find a process by name or PID, jump to a disk, run an action.
- **Layer toggles**: processes / storage / network / kernel / issues-only.
- **Legend** for all encodings, always one key away.

---

## 6. Detection: rules and baselines

### 6.1 Rules (declarative, TOML, shipped defaults plus user overrides)
```toml
[[rule]]
id = "disk.fill.critical"
when = "volume.used_pct > 95"
for = "2m"
severity = "critical"
message = "Volume {mount} is {used_pct}% full"
```
Defaults include: disk above 90% or 95% full, inode exhaustion, sustained CPU saturation, memory pressure (PSI), swap thrash, zombie accumulation, OOM kill events, failed services, SMART warnings, high IO latency, temperature thresholds, and processes stuck in D-state.

### 6.2 Baselines (learned on each machine)
- **Identity** = executable path + hash, not PID.
- Per identity: an **hour-of-week profile** of CPU, memory, IO and network, computed as streaming quantiles (t-digest or EWMA buckets). Flag readings above p99 of baseline for N minutes.
- **Novelty detection**, which is cheap and high-signal:
  - a known binary contacting a **never-before-seen** remote network or opening a **new listening port**
  - a new child executable spawned by a long-lived service
  - a brand-new binary running as root or SYSTEM
  - a new kernel module or driver loaded
- **Learning period**: anomalies are suppressed for the first 7 days on a machine (configurable), with a visible "learning" state in the HUD.
- Every issue has a lifecycle (*open → acknowledged → resolved / snoozed*) and an **explanation** ("CPU 4.2× its usual Tuesday-14:00 level").

---

## 7. Actions and security (full control, done safely)

Full control means Blackwall can damage the system. The design:

1. **Privilege separation.** The Tauri app is never elevated. `blackwall-ice` is a small privileged helper whose entire job is to run a **fixed allowlist of typed operations**: no shell, and no arbitrary commands.
2. **Authenticated local IPC.** Linux uses a Unix socket with `SO_PEERCRED` and a polkit authorization check per action class. Windows uses a named pipe with an ACL and client-process verification. macOS uses an XPC/SMAppService helper with code-signature checks on the client.
3. **Action tiers**:
   - *Tier 1 (process)*: kill (TERM, then KILL), suspend/resume, renice/priority, open file location. **v1 ships with these.**
   - *Tier 2 (services)*: start/stop/restart a systemd unit, Windows service or launchd job.
   - *Tier 3 (network)*: block a remote endpoint or a process's network access through a firewall rule (nftables / Windows Filtering Platform / pf). Every rule is tagged as Blackwall's and **auto-expires** unless pinned.
4. **Guardrails**: confirmation for every action (with typed confirmation for PID 1, kernel threads, the user's own session and Blackwall itself); a dry-run preview showing exactly what will happen; protected-process lists; rate limits.
5. **Audit log**: an append-only, hash-chained record of every action (who, what, when, result) in the store, which also shows on the timeline.
6. **Future fleet**: the same action schema over **mTLS**, with per-host authorization. Remote actions stay off by default.

---

## 8. History, timeline and replay

- **Store**: SQLite in WAL mode (`rusqlite`), one file per host.
  - `samples`: metric time series with **tiered downsampling**: 1 s resolution for 2 h, 10 s for 48 h, 1 min for 30 days, 15 min for 1 year. All of this is configurable, with a hard disk-size cap.
  - `entities`: lifetimes of processes, sockets and volumes (born/died).
  - `events`: issues, kernel events, actions. These are kept longer than metrics.
- **Replay** is a `ReplaySource` that rebuilds snapshots and deltas from the store, so the frontend renders history with **exactly the same code path** as live data. Scrub, play at 1×/10×/60×, and jump to the previous or next incident.
- Baselines (§6.2) are trained from this same store.
- Optional later: export to Prometheus/OpenMetrics.

---

## 9. Input: keyboard and touch (equal citizens)

**Keyboard** (every feature reachable without a mouse):
- A logical focus model that is independent of 3D picking. `Tab` cycles HUD regions; arrow keys move through the **process tree** (parent / child / sibling) and the camera follows the focused entity.
- `Enter` inspects, `A` opens actions, `/` searches, `Space` toggles live/replay, `[` `]` steps between incidents, `1–5` toggle layers, `?` shows the shortcut sheet.
- WASD/QE free-fly camera mode, toggled explicitly so it never steals text input.
- Visible focus rings in both the HUD and the scene (a holographic selection reticle).

**Touch:**
- One-finger orbit, two-finger pan, pinch to zoom, tap to select, long-press for actions, double-tap to focus.
- Touch targets of at least 44 px. Picking uses enlarged invisible hit volumes, so small entities are still tappable.
- The HUD adapts to tablet-sized windows: panels become bottom sheets.

**Accessibility:** reduced-motion mode (honors the OS setting: no glitch, no shake, slow camera), a high-contrast theme, color-blind-safe encodings (shape and pattern are always redundant with color), and a screen-reader-friendly **list view** of everything in the scene.

---

## 10. Sound design

- Synthesized with the Web Audio API, so no large audio assets are needed.
- **Ambient drone**: layered detuned oscillators and filtered noise. Filter cutoff and detune follow overall load, and a sub-bass layer follows kernel pressure.
- **Event cues**: soft chimes for process birth and death (rate-limited and aggregated), a distinct sting per severity, and a crack sound when the Wall glitches.
- Optional spatial audio: the selected entity hums from its position.
- **Off by default**, with a master volume, per-category mixing, and automatic mute when the window loses focus (configurable).

---

## 11. Screensaver mode

- After N minutes idle (or on demand, or launched as an OS screensaver host later), go fullscreen and start an **autopilot camera**: smooth spline tours between "points of interest" (the hottest process, busiest disk, newest connection, active issues), with slow drifts along the Wall.
- The HUD reduces to a minimal lower-third caption ("`firefox` — 2.1 GB — 14% CPU").
- Any input exits instantly, back to where you were.
- It respects quality tiers and drops to a low-power frame cap (e.g. 30 fps).

---

## 12. Adaptive quality tiers

| Tier | Target | Effects |
|---|---|---|
| Low | iGPU on WebKitGTK/WebGL2, battery | Instanced simple meshes, no post-FX, about 30 % particles, 30–60 fps |
| Medium | iGPU / WebGL2 | Bloom (half-res), basic glitch shader, LOD |
| High | dGPU or WebGPU | Full bloom, chromatic aberration, scanlines, volumetric Wall, more particles |
| Ultra | Strong dGPU + WebGPU | GPU-compute particles (TSL compute), higher-res post, per-entity shaders |

- **Initial tier** is chosen from the backend (WebGPU or WebGL2), the GPU renderer string, and the device pixel ratio.
- **Runtime governor**: if frame time stays above budget for 3 s, drop a tier; if it stays well under budget for 30 s, try a tier up. A manual override is available in settings.
- **Always on**: instancing (one draw call per entity type), frustum culling, LOD/impostors for distant entities, and capped DPR on high-DPI screens.

---

## 13. Milestones (solo, side project — each one is usable on its own)

Sizes: **S** ≈ a few evenings, **M** ≈ 2–3 weekends, **L** ≈ a month or more of side time.

| # | Milestone | Size | Done when… |
|---|---|---|---|
| **0** | **Tech spike / go-no-go** | S | Tauri + Three `WebGPURenderer` renders 3,000 instanced glowing entities with bloom at ≥ 60 fps on Windows and macOS and ≥ 30 fps on a Linux iGPU under WebKitGTK. If Linux fails badly, re-evaluate (Bevy fallback, or a Linux-specific low tier) |
| **1** | **Deep Space (processes)** | M | Linux collector via `sysinfo` + `procfs`; `bw-model` protocol (snapshot + delta → `ipc::Channel`); orbital process tree; inspector; keyboard focus model and search |
| **2** | **The Blackwall** | M | Wall shader driven by PSI/iowait; kernel-subsystem structures; kthreads; aggregate syscall/IO/interrupt streams crossing the Wall |
| **3** | **Storage + Network** | M | Disk fortresses and volume rings, IO streams; socket → process mapping, endpoint clusters, listening ports |
| **4** | **Memory: history & replay** | M | SQLite store with downsampling; `ReplaySource`; timeline scrubber; the same renderer for live and replay |
| **5** | **Issues: rules** | M | TOML rules engine, journald/kmsg/OOM/failed units, alert feed, glitch visuals, fly-to-issue |
| **6** | **Windows + macOS collectors** | L | Capability flags; parity for processes, storage, network and event sources; CI builds for all three |
| **7** | **Actions tier 1 + ICE helper** | L | Privileged helper on all three OSes, authenticated IPC, kill/suspend/renice, audit log, confirmations |
| **8** | **Baselines & anomalies** | M | Hour-of-week profiles, novelty detectors, learning period, explanations |
| **9** | **Immersion polish** | M | Sound design, screensaver/autopilot, quality governor, touch gestures, reduced-motion and list view |
| **10** | **Actions tiers 2–3** | M | Service control, firewall blocks with auto-expiry |
| **11** | **Fleet-ready** | L | Extract `bw-agent` (headless collector + store), `RemoteSource` over mTLS, a host switcher in the UI. A hub can come after this |
| Later | eBPF syscall flows (`aya`), Prometheus export, web-served UI, signed releases + auto-update | — | — |

Milestones 1–5 are **Linux-first** so that the visual and metaphor work isn't slowed down by three platforms at once. The `Collector` trait and capability flags keep the code ready for Milestone 6.

---

## 14. Risks and mitigations

| Risk | Mitigation |
|---|---|
| WebKitGTK (Linux) WebGL performance or quirks | Milestone 0 spike first; Low tier designed for it; Bevy fallback kept on the table |
| Visual clutter with 1,000+ processes | Aggregation: collapse sibling groups (e.g. browser renderer processes) into "swarms"; "issues only" and "top N" filters; semantic zoom |
| Collector overhead distorting the system it measures | Tiered cadences; diff-based updates; CPU-budget benchmark in CI |
| Full-control features being dangerous | Privilege-separated helper, allowlist, confirmations, audit log, protected list, auto-expiring firewall rules |
| Cross-platform signing and helper installation are painful | Defer to Milestone 7; read-only mode works with no installation step at all |
| Baseline false positives | Learning period, explanations, one-click "this is normal" feedback that updates the baseline |
| Scope creep in a side project | Every milestone ships something usable; "Later" bucket stays later |

---

## 15. Open questions for later

1. Name and branding of the helper (`blackwall-ice` is a placeholder).
2. Should a disk-usage "terrain" (large directories as landscape) be part of storage, or a separate deep-dive mode? It's expensive to scan.
3. Licensing (MIT/Apache-2.0 dual is the Rust norm).
4. Should replay files be exportable, so a captured incident can be shared and replayed on another machine?
5. Should the eventual fleet hub also serve the web UI (the same Svelte frontend), so it's reachable from a phone?
