# Blackwall — Project Plan (v2: all-Rust)

> A cyberpunk system visualizer. User space is **Deep Space**: a dark void where processes drift as living entities. The **Blackwall** is the kernel/user boundary: a vast luminous barrier, with the kernel's machinery humming behind it. You can drift through it as ambient art, then drill into any entity for real, actionable diagnostics.
>
> **v2 change:** The whole application is native Rust. Bevy renders the scene and egui provides the data panels. Tauri and the web frontend are gone. Cross-platform compatibility is now a first-class design constraint (§3).

---

## 1. Requirements (from the discovery interview)

| Topic | Decision |
|---|---|
| Purpose | **Balanced**: an immersive view that is also a real diagnostics tool you can drill into |
| Platforms | **Linux, Windows, macOS from day one** (x86_64 and ARM64). Fleet / remote servers come later; the protocol is designed for them now |
| Visual form | **Hybrid**: a 3D scene plus a flat, readable on-screen overlay (HUD) |
| Stack | **Rust for everything**: collector, renderer, UI, storage, privileged helper, future agent |
| Metaphor | **Wall = kernel/user boundary**. Syscalls, IO and interrupts are streams crossing it |
| Data layers (v1) | Processes and tree, storage, network, issues/health. All four are required |
| Control | **Full control** (kill/suspend/renice, services, firewall) with a privilege-separated design |
| Detection | **Rules plus learned per-machine baselines** |
| History | **Timeline plus replay** |
| Immersion | Sound design, screensaver mode, **full keyboard and touch support** (plain, accessible UX rather than a netrunner theme) |
| Performance | **Adaptive quality tiers** |
| Team / pace | Solo side project, so the plan is a series of small, shippable milestones |

---

## 2. Technology choices

### 2.1 Why all-Rust
- **Native graphics everywhere.** Bevy renders through `wgpu`, which drives Vulkan, DirectX 12, Metal, or OpenGL as a fallback. There is no browser engine underneath, so the v1 plan's biggest risk (WebKitGTK on Linux) is gone.
- **No bridge.** The collector writes straight into Bevy's entity system (ECS). There is no IPC channel, no serializing, and no Rust↔TypeScript type syncing.
- **ECS fits the data.** Thousands of entities whose values change every second is exactly what an entity system is built for.
- **One toolchain** across app, helper, agent, tests and CI.

### 2.2 The stack
| Concern | Choice | Notes |
|---|---|---|
| Engine / renderer | **Bevy** (pinned to the current stable minor, 0.19.x at the time of writing) | Built-in bloom, chromatic aberration, tonemapping, automatic instancing/batching, picking, custom WGSL shaders, compute |
| Data panels (HUD) | **`bevy_egui`** | Inspector, tables, alert feed, timeline, search, settings. Dense, fast to build, styleable |
| Cinematic overlays | **Bevy UI** (headless widgets / Feathers) | Selection labels, lower-thirds, boot sequence, screensaver captions |
| Audio | **`bevy_kira_audio`** + **`fundsp`** for the synthesized drone | Both run on `cpal`: WASAPI, CoreAudio, ALSA (PipeWire/Pulse through their ALSA layers) |
| Async / threads | `tokio` for collectors and IO; `crossbeam-channel` into ECS | The render loop never blocks on collection |
| System data | `sysinfo` (baseline) + per-OS crates (§5) | |
| Storage | `rusqlite` with the **`bundled`** feature | The same SQLite version on every OS, with no system library dependency |
| Networking (fleet later) | `quinn` (QUIC) + **`rustls`** | Pure Rust TLS: **no OpenSSL**, which avoids cross-platform build pain |
| Serialization | `serde` + `postcard` (wire format), TOML (config/rules) | |
| Paths | `directories` crate | Correct config/data/cache locations per OS |

**Bevy upgrade policy:** Bevy ships a breaking release every 3–4 months. Pin an exact minor version and keep `bevy_egui` and other plugins version-locked. Upgrade deliberately, at most twice a year, as its own small milestone. Keep engine-specific code inside the `bw-scene`/`bw-ui` crates so upgrades don't ripple into the collector or storage.

### 2.3 Alternatives considered
| Option | Why not |
|---|---|
| Tauri + Three.js (v1 plan) | Inconsistent GPU performance across OS webviews (especially WebKitGTK); two languages and an IPC bridge |
| Raw `wgpu` + `egui` | Maximum control, but we would rebuild the camera, scene graph, bloom, picking and asset handling that Bevy already provides |
| Fyrox | Capable engine, much smaller community |
| Makepad / Slint / Iced | Good Rust UI toolkits, weak for a 3D-first scene |

---

## 3. Cross-platform strategy

Cross-platform compatibility is built in from the start, not ported later.

### 3.1 Support matrix
| OS | Architectures | Minimum version (proposed) | GPU backend (primary → fallback) | Windowing |
|---|---|---|---|---|
| **Windows** | x86_64, ARM64 | Windows 10 21H2 / Windows 11 | DX12 → Vulkan → WARP (software) | winit (Win32) |
| **macOS** | Apple Silicon, Intel (universal2 binary) | macOS 12 Monterey | Metal | winit (AppKit) |
| **Linux** | x86_64, ARM64 | glibc ≥ 2.31 (Ubuntu 20.04 / Debian 11 era) | Vulkan → OpenGL 3.3 / GLES 3 → llvmpipe/lavapipe (software) | winit: **Wayland and X11** |

32-bit and big-endian platforms are out of scope.

### 3.2 Principles
1. **Every milestone ships on all three OSes.** Linux may get deeper probes first, but a milestone is not done until it builds, runs and is usable on Windows and macOS too, at least at the `sysinfo` baseline level.
2. **Capabilities, not assumptions.** Each platform backend reports a `Capabilities` struct (for example `kernel_threads`, `per_proc_net_bytes`, `smart`, `temps`, `service_control`, `per_app_firewall`). The UI shows *"not available on this platform"* or *"needs elevated access"* instead of hiding things or showing fake data. A **capability matrix** is generated from the code and published in the docs.
3. **One platform-abstraction crate.** OS-specific code lives only in `bw-platform/{linux,windows,macos}` behind traits, selected with `#[cfg(target_os)]`. Nothing else in the workspace uses `cfg(target_os)`; CI enforces this with a grep check.
4. **Pure-Rust dependencies by preference.** Use `rustls` instead of OpenSSL, bundled SQLite, and bundled fonts. Every system library we do need is listed per OS in `docs/BUILDING.md`.
5. **Read-only by default, elevated by choice.** Some data is hidden from unprivileged users on every OS (§5.2). The app works unprivileged; elevated *read* access comes through the same helper as actions (§7), opt-in.
6. **Normalize OS quirks at the edge.** Platform backends produce the same normalized model, and the scene, detection and storage never see OS differences.

### 3.3 Graphics compatibility
- **Backend selection:** wgpu's default order, with a `--backend` override and environment variable (`WGPU_BACKEND`).
- **The Low tier must run on the OpenGL/GLES backend.** No compute shaders, storage buffers or other features GL lacks are allowed in the Low tier.
- **Software renderers** (WARP, llvmpipe, lavapipe) are detected from the adapter info. When one is found: force the Low tier, cap at 30 fps, and show a one-time notice. This covers VMs, Remote Desktop and headless CI.
- **HiDPI:** honor winit's scale factor, including **fractional scaling on Wayland** and per-monitor DPI on Windows. Test at 100 %, 150 % and 200 %.
- **Shaders** are written in WGSL and must compile on every backend. CI compiles all shader permutations through `naga` for SPIR-V, MSL, HLSL and GLSL.

### 3.4 Input differences
| Input | Windows | macOS | Linux |
|---|---|---|---|
| Touchscreen | Yes (winit touch events) | No touchscreens | Yes (Wayland and X11 touch) |
| Trackpad gestures | Precision touchpad shows up as scroll | **Pinch and rotate gestures** (Bevy `PinchGesture`/`RotationGesture`) | Shows up as scroll (gesture support varies by compositor) |
| Primary modifier | Ctrl | **Cmd** | Ctrl |

- **Bindings:** use *logical* keys for character shortcuts (`/`, `?`) and *physical* keys for movement (WASD), so AZERTY and Dvorak work. Map the primary modifier per OS (Cmd on macOS, Ctrl elsewhere).
- **Gestures** (orbit, pan, pinch, long-press) live in one `bw-input` gesture recognizer that consumes raw touch points, so behavior is identical wherever touch exists.

### 3.5 Accessibility
- Bevy and egui both integrate **AccessKit**, which maps to UI Automation on Windows, NSAccessibility on macOS and AT-SPI on Linux. Milestone 0 confirms this works end to end with `bevy_egui` on all three.
- Reduced motion follows the OS setting where it can be read, with an in-app override.

### 3.6 Text and fonts
- **Fonts are bundled** (a monospace and a display face, plus Noto fallbacks for CJK and symbols), so rendering is identical on every OS.
- Process names and paths are not always valid UTF-8 (Linux bytes, Windows UTF-16 with unpaired surrogates). Store them raw and display them lossily. Search matches on the lossy form.

### 3.7 Packaging and distribution
| OS | Formats | Signing / notes |
|---|---|---|
| Windows | MSI (`cargo-wix`) and a portable zip; winget later | **Code-sign** (Authenticode, e.g. Azure Trusted Signing). An unsigned binary that inspects and kills processes is likely to be flagged by antivirus |
| macOS | `.app` in a DMG, universal2 | **Developer ID signing and notarization** (requires a paid Apple Developer account). The privileged helper (SMAppService) *requires* a signed app |
| Linux | AppImage, `.deb` (`cargo-deb`), `.rpm` (`cargo-generate-rpm`), AUR | **No Flatpak or Snap for v1**: their sandboxes hide host processes, which defeats a system monitor. Revisit later with a host-side helper |

- **Build glibc compatibility:** build Linux release binaries in an old-glibc container (or with `cargo-zigbuild` targeting glibc 2.31).
- **Updates:** at first, through package managers and GitHub Releases. A self-updater with signed manifests (e.g. minisign) comes later.

### 3.8 Special environments
- **Containers / WSL2:** detect them. Inside a container we only see its own process namespace; show a banner saying so. WSL2 sees the Linux VM, not the Windows host, so the banner points to the native Windows build.
- **Virtual machines:** expect software rendering (see §3.3).
- **Non-systemd Linux** (OpenRC, runit): service control is unavailable by capability, and issues fall back to kmsg and syslog.

### 3.9 CI matrix (GitHub Actions)
| Runner | Targets | Jobs |
|---|---|---|
| `ubuntu-latest` and `ubuntu-24.04-arm` | x86_64 / aarch64 `-unknown-linux-gnu` | fmt, clippy, tests, **headless render smoke test on lavapipe**, package |
| `windows-latest` and `windows-11-arm` | x86_64 / aarch64 `-pc-windows-msvc` | clippy, tests, **render smoke test on WARP**, MSI |
| `macos-latest` (arm64) | aarch64 + x86_64 → universal2 (`lipo`) | clippy, tests, render smoke test (Metal), DMG |

- **Render smoke test:** boot the app with a recorded fixture, render N frames offscreen, take a screenshot, and compare against a golden image with a tolerance.
- **Collector conformance suite:** the same test suite runs against every platform backend. It checks that our own PID is found, the parent chain reaches the root, at least one volume and one interface exist, and values are in sane ranges.
- **Fixture tests:** recorded `/proc` trees, Windows and macOS snapshots, and event logs, so parsing logic is tested on every OS regardless of the host.
- **Budget benchmark:** collector CPU use must stay under 1–2 % of one core on every OS.

---

## 4. Architecture

```
┌───────────────────────────── blackwall (single process, unprivileged) ─────────────────────────────┐
│                                                                                                     │
│  tokio runtime (background)                         Bevy App (main thread + render thread)          │
│  ┌──────────────────────────────┐   crossbeam      ┌──────────────────────────────────────────────┐ │
│  │ Source: Live | Replay |      │ ──deltas──────▶  │ ingest system → ECS components                │ │
│  │         (later) Remote       │                  │ layout · interpolation · LOD · effects       │ │
│  │  └─ bw-platform (per-OS)     │                  │ bw-scene (3D)  bw-ui (egui + Bevy UI)        │ │
│  │ bw-detect (rules+baselines)  │ ──issues──────▶  │ bw-input (keys, touch, gestures)             │ │
│  │ bw-store  (SQLite writer)    │ ◀──queries────── │ bw-audio (drone + cues)                      │ │
│  └──────────────────────────────┘                  └──────────────────────────────────────────────┘ │
│                 │  bw-actions client (typed requests)                                               │
└─────────────────┼───────────────────────────────────────────────────────────────────────────────────┘
                  ▼ authenticated local IPC (Unix socket / named pipe / XPC)
┌─────────────────────────────────────────────────────────────────────────────────────────────────────┐
│ blackwall-ice (privileged helper) — allowlisted actions + optional elevated READ probes + audit log │
│ Linux: systemd unit + polkit │ Windows: service │ macOS: SMAppService daemon                         │
└─────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 4.1 Key design points
- **Sources:** the same `Source` trait serves live data, replay of the store, and (later) remote agents. The ECS ingest system can't tell them apart.
- **Snapshot + delta model:** stable entity IDs of `host:pid:start_time`, because PIDs get reused. Deltas arrive at 1–4 Hz. Rendering runs at the display rate and **interpolates** values between samples, so motion stays smooth.
- **Rendering is decoupled from collection.** A slow probe (SMART, socket scan) can never drop frames.
- **Engine isolation:** only `bw-scene`, `bw-ui`, `bw-input` and `bw-audio` depend on Bevy. The model, platform, store, detection and actions crates are engine-free, which keeps Bevy upgrades cheap and makes the future headless agent trivial.

### 4.2 Workspace layout
```
blackwall/
├─ Cargo.toml                 # workspace, pinned Bevy version
├─ crates/
│  ├─ bw-model/               # entities, IDs, deltas, Capabilities (no OS / engine deps)
│  ├─ bw-platform/            # Collector + Actions traits
│  │  ├─ src/linux/  src/windows/  src/macos/
│  │  └─ tests/conformance.rs # same suite on every OS
│  ├─ bw-source/              # LiveSource, ReplaySource, (later) RemoteSource
│  ├─ bw-store/               # SQLite time-series, events, retention
│  ├─ bw-detect/              # rules (TOML), baselines, issue lifecycle
│  ├─ bw-actions/             # action schema, IPC client, audit types
│  ├─ bw-ice/                 # privileged helper binary
│  ├─ bw-scene/               # Bevy: layout, materials, WGSL shaders, camera, quality tiers
│  ├─ bw-ui/                  # bevy_egui panels + Bevy UI overlays, theme
│  ├─ bw-input/               # focus model, keymaps, gesture recognizer
│  ├─ bw-audio/               # synth drone, cues
│  └─ blackwall/              # the app binary: wires everything together
├─ assets/                    # bundled fonts, shaders, sounds (if any)
├─ rules/                     # default detection rules (TOML)
├─ fixtures/                  # recorded snapshots per OS for tests
├─ packaging/                 # wix/, macos/, linux/ (deb, rpm, AppImage)
└─ docs/
```

---

## 5. Data collection

### 5.1 Per layer, per OS
`sysinfo` is the shared baseline. The platform modules add depth on top of it.

| Layer | Linux | Windows | macOS |
|---|---|---|---|
| **Process tree** | `procfs`: ppid, state (zombie / D-state), threads, cgroup → container grouping | Toolhelp32 + `NtQueryInformationProcess`. **Parent PIDs are unreliable** (the parent may have exited and its PID been reused): validate that the parent's start time is earlier than the child's | `libproc`. Most apps have ppid = 1 (launchd): group helper processes by **responsible process** |
| **Kernel side (behind the Wall)** | kthreads (children of `kthreadd`), `/proc/stat` (context switches, interrupts, softirqs), **PSI** (`/proc/pressure`), `/proc/vmstat`, modules. Later: **eBPF via `aya`** for per-process syscall rates (root) | System (PID 4), Registry, Memory Compression, Secure System; drivers via `EnumDeviceDrivers`; DPC/interrupt time and context switches via **PDH** counters; ETW later (admin) | `kernel_task`, kexts and system extensions, `host_statistics64` (VM pressure, faults), context switches via `host_processor_info` |
| **Storage** | `/proc/diskstats`, mounts, inodes; SMART via `smartctl --json` if installed | Volumes API, PDH disk counters; health via `MSFT_StorageReliabilityCounter` (WMI) | IOKit block-storage statistics; SMART via `smartctl` if installed |
| **Network** | sockets → PID via `netstat2` (sock_diag); per-interface rates | `netstat2` (`GetExtendedTcpTable`/`UdpTable`) | `netstat2` (libproc) |
| **Per-process bandwidth** | eBPF (root) | ETW TCP/IP provider (admin) | `nettop`-style statistics (root, best effort) |
| **Issues / events** | journald, kmsg (**OOM**, segfaults, IO errors), failed systemd units | Event Log (crashes/WER, service failures, disk errors) | Unified log (`log stream`), `DiagnosticReports` crash files |
| **Temperatures** | hwmon (reliable) | Best effort (WMI thermal zones are often missing); **no kernel drivers** | Best effort via IOKit/SMC sensors |

**Collection cadence:** fast at 1 Hz (CPU, memory, IO and network rates), medium every 5 s (sockets, tree diff), slow every 60 s (SMART, services, temperatures).

### 5.2 Visibility without elevation
| OS | What an unprivileged user can't see |
|---|---|
| Linux | Other users' `/proc/<pid>/fd`, `io` and `environ` (depends on `hidepid`/ptrace scope); eBPF |
| Windows | Details of SYSTEM and other users' processes (path, command line, memory) without SeDebugPrivilege; ETW |
| macOS | Most `proc_pidinfo` details for other users' processes; anything blocked by SIP or TCC |

If the user opts in, `blackwall-ice` runs these probes elevated and streams **read-only** results to the app over the same authenticated IPC. Without it, the scene still shows those processes, marked *"restricted"*.

---

## 6. The visual language

### 6.1 Spatial layout
- **The Blackwall**: a vast, curved, translucent hexagonal-lattice barrier (a custom WGSL material). Its turbulence tracks kernel pressure (PSI on Linux; equivalent signals elsewhere). Under critical pressure it **cracks and glitches**.
- **Behind the Wall:** dim monoliths for the *Scheduler*, *Memory Manager*, *Storage/VFS*, *Network stack*, *Drivers* and *Interrupts*. Kernel threads and drivers are sparks clustered around them. A monolith with no data on the current OS is drawn as a faded silhouette labeled "no telemetry".
- **Deep Space:** the process tree in orbital form. Init/launchd/services is the central star; children orbit their parents. Layout uses a stable, damped force simulation that runs in an ECS system. New processes materialize and exiting ones dissolve.
- **Storage:** "data fortresses" at the Wall's edge, one per disk, with volumes as rings around them. IO is drawn as particle streams from a process, through the Wall, to its disk.
- **Network:** interfaces are gates at the edge of space, with beams out to distant clusters of remote endpoints.
- **Crossing the Wall:** syscalls, IO, page faults and interrupts are drawn as streams through the barrier. This is the core of the metaphor.

### 6.2 Encoding a process (redundant: never color alone)
| Property | Visual channel |
|---|---|
| Memory (RSS) | Size |
| CPU % | Brightness and pulse rate |
| Owner (system / current user / other) | Shape (octahedron / sphere / icosahedron) and base hue |
| Zombie | Grey hollow husk that doesn't pulse |
| Uninterruptible IO | Pulled toward the Wall, tethered to its disk |
| Restricted (no access) | Wireframe shell with a lock glyph |
| Issue / anomaly | Glitch shader, corruption particles, and a HUD marker with an icon |

**Scale:** sibling groups (e.g. 40 browser renderer processes) collapse into **swarms**, with semantic zoom to expand them. There are also "issues only" and "top N" filters.

### 6.3 HUD
- **egui:** inspector (numbers, sparklines, open files and sockets, children, history, actions), alert feed, timeline scrubber, search / command palette, settings, legend, and a **list view of everything in the scene** for accessibility.
- **Bevy UI:** world-anchored labels, selection reticle, lower-third captions, boot / "jack-in" intro.
- **One cyberpunk theme** applied to both (`egui::Visuals` plus Bevy UI styles), with a high-contrast variant.

---

## 7. Actions and security

1. **Privilege separation.** The app is never elevated. `blackwall-ice` runs a **fixed allowlist of typed operations**: no shell, and no arbitrary commands.
2. **Per-OS helper installation and authentication**:
   | OS | Helper | Who may connect |
   |---|---|---|
   | Linux | systemd system service, Unix socket | `SO_PEERCRED` plus a **polkit** check per action class. On non-systemd distros, a setuid-free fallback via `pkexec` per action |
   | Windows | Windows service, named pipe | Pipe ACL; verify the client process's image path and Authenticode signature |
   | macOS | **SMAppService** launch daemon, XPC | Code-signing requirement check on the connecting client |
3. **Action tiers and platform support**:
   | Tier | Action | Linux | Windows | macOS |
   |---|---|---|---|---|
   | 1 | Kill (graceful → force) | SIGTERM → SIGKILL | `WM_CLOSE`/`TerminateProcess` | SIGTERM → SIGKILL |
   | 1 | Suspend / resume | SIGSTOP / SIGCONT | `NtSuspendProcess` / `NtResumeProcess` | SIGSTOP / SIGCONT |
   | 1 | Priority | `setpriority` / ionice | `SetPriorityClass` | `setpriority` |
   | 1 | Reveal executable | xdg-open (directory) | Explorer `/select` | Finder reveal |
   | 2 | Service start/stop/restart | systemd via D-Bus (`zbus`) | Service Control Manager | `launchctl` (bootstrap/kickstart) |
   | 3 | Block a remote endpoint | **nftables**, in Blackwall's own table (coexists with ufw and firewalld) | **Windows Filtering Platform** / Firewall COM API | **pf anchor** |
   | 3 | Block a *process's* network | cgroup + nftables | WFP app-ID filter | ❌ needs a Network Extension (Apple entitlement): capability off |
4. **Guardrails:** confirm every action, with typed confirmation for critical targets (PID 1, kernel threads, the session leader, Blackwall itself); a dry-run preview; protected-process lists per OS; rate limits. Firewall rules created by Blackwall **auto-expire** unless pinned.
5. **Audit log:** append-only and hash-chained, written by the helper (so the app can't forge it), mirrored into the store, and shown on the timeline.
6. **Future fleet:** the same action schema over QUIC with mTLS and per-host authorization. Remote actions are off by default.

---

## 8. Detection: rules and baselines

- **Rules** (TOML, shipped defaults plus user overrides), for example `volume.used_pct > 95 for 2m → critical`. Defaults cover disk fill, inode exhaustion, sustained CPU saturation, memory pressure, swap thrash, zombies, OOM kills, failed services, SMART warnings, IO latency, temperatures and stuck processes.
- **Rules use the normalized model, so one rule works on every OS.** A rule that depends on a capability (e.g. PSI) declares it and is silently inactive where that capability is missing; the rules panel lists inactive rules and why.
- **Baselines:**
  - **Identity** = executable path plus hash.
  - Per identity: an **hour-of-week profile** of CPU, memory, IO and network using streaming quantiles. Flag readings above p99 sustained for N minutes.
  - **Novelty detectors:** a new remote network, a new listening port, a new child executable, a new binary running as root or SYSTEM, a new kernel module or driver.
  - **Learning period:** anomalies are suppressed for the first 7 days, with a visible "learning" state.
- **Issue lifecycle:** open → acknowledged → resolved / snoozed, with a plain-language **explanation** and one-click "this is normal" feedback that updates the baseline.

---

## 9. History, timeline and replay

- **Store:** SQLite in WAL mode (bundled), one database per host, in the OS-correct data directory (`directories` crate).
- **Tiered downsampling:** 1 s for 2 h → 10 s for 48 h → 1 min for 30 days → 15 min for 1 year, with a hard size cap. Entity lifetimes and events are kept longer.
- **`ReplaySource`** rebuilds snapshots and deltas from the store, so replay uses the **same rendering path** as live data. Scrub, play at 1×/10×/60×, and jump to the previous or next incident.
- **Exportable incident captures** (`.bwcap`: a postcard-encoded slice of the store) can be replayed on any OS. This also gives the cross-platform fixture tests their recorded data.

---

## 10. Input: keyboard and touch

**Keyboard** (every feature reachable without a mouse):
- A logical **focus model** that is independent of 3D picking. `Tab` cycles HUD regions; arrow keys walk the process tree (parent / child / sibling) and the camera follows the focus. This builds on Bevy's directional navigation.
- `Enter` inspects, `A` opens actions, `/` or `Mod+K` opens search, `Space` toggles live/replay, `[` `]` step between incidents, `1–5` toggle layers, `?` shows the shortcut sheet. `Mod` is Cmd on macOS and Ctrl elsewhere.
- WASD/QE free-fly mode, using physical keys so it is layout independent and toggled explicitly so it never steals text input.
- Keymaps can be remapped in a TOML file.

**Touch and gestures:**
- One finger orbits, two fingers pan, pinch zooms, tap selects, long-press opens actions, double-tap focuses.
- macOS trackpad pinch and rotate map to the same actions.
- Enlarged invisible picking volumes, so small entities stay tappable. HUD targets are at least 44 px. On narrow or tall windows, panels become bottom sheets.

**Accessibility:** reduced motion (OS setting plus override), high-contrast theme, color-blind-safe redundant encodings, an AccessKit-exposed list view, and adjustable UI scale independent of DPI.

---

## 11. Sound design

- Real-time synthesis with `fundsp`: detuned oscillator layers and filtered noise. Filter cutoff and detune follow overall load; a sub-bass layer follows kernel pressure.
- Event cues (birth/death chimes, rate-limited; one sting per severity; a "crack" when the Wall glitches) through `bevy_kira_audio`.
- Optional spatial hum from the selected entity.
- **Off by default**, with master and per-category volume and mute-on-unfocus.
- **Cross-platform:** everything goes through `cpal`. If no output device is present (servers, CI, some VMs), audio silently disables itself and never crashes the app. Device hot-swap (headphones plugged in) is handled by re-opening the stream.

---

## 12. Screensaver mode

- **In-app idle mode, on all OSes:** after N minutes without input, go borderless fullscreen and run an **autopilot camera** that tours points of interest (hottest process, busiest disk, newest connection, active issues). The HUD reduces to lower-third captions. Any input exits instantly.
- **System-wide idle detection** (optional): `GetLastInputInfo` on Windows; `CGEventSourceSecondsSinceLastEventType` on macOS; on Linux, `ext-idle-notify-v1` under Wayland or the XScreenSaver extension under X11.
- **Prevent display sleep** while the tour is running, using each OS's inhibit API (`SetThreadExecutionState` / IOPMAssertion / the `org.freedesktop.ScreenSaver` inhibit call). This is user-toggleable.
- **Native OS screensaver integration** comes later and is per-OS: a Windows `.scr` wrapper is easy (the same binary with `/s /p /c` arguments). A macOS `.saver` bundle and Linux xscreensaver hacks are much harder and out of scope for v1.
- Runs at the Low or Medium tier with a 30 fps cap to save power.

---

## 13. Adaptive quality tiers

| Tier | Typical target | Effects |
|---|---|---|
| **Low** | Software renderers, GL fallback, old iGPUs, battery | Instanced simple meshes, no post-processing, about 30 % particles, 30 fps cap. **Must work on GL/GLES** |
| **Medium** | Modern iGPU (Intel Xe, AMD APU, Apple M-series on battery) | Bloom, basic glitch shader, LOD |
| **High** | Discrete GPU or Apple M-series on power | Full bloom, chromatic aberration, scanlines, volumetric Wall, more particles |
| **Ultra** | Strong discrete GPU (Vulkan / DX12 / Metal) | GPU-compute particles, higher-res post-processing, per-entity shader effects |

- **Initial tier** comes from the wgpu adapter: backend, device type (discrete / integrated / CPU), vendor, limits, and the power state where the OS reports it.
- **Runtime governor:** drop a tier if frame time stays over budget for 3 s; try a tier up after 30 s well under budget. A manual override is in settings.
- **Always on:** automatic instancing, frustum culling, LOD/impostors, render scale below 1.0 on high-DPI screens at lower tiers, and reduced work when the window is unfocused or minimized.

---

## 14. Milestones (solo, side project — each one usable on its own, **on all three OSes**)

Sizes: **S** ≈ a few evenings, **M** ≈ 2–3 weekends, **L** ≈ a month or more of side time.

| # | Milestone | Size | Done when… |
|---|---|---|---|
| **0** | **Foundation spike + CI** | M | Workspace skeleton; CI matrix (§3.9) green on all 6 targets; Bevy renders 3,000 instanced glowing entities with bloom at 60 fps on real hardware for each OS, **and the Low tier runs on llvmpipe and WARP**; a `bevy_egui` panel with the cyberpunk theme; AccessKit confirmed; touch and pinch events confirmed. **Go/no-go on the egui look.** |
| **1** | **Deep Space: processes** | M | `bw-platform` baseline on all three OSes (`sysinfo` + parent validation); snapshot/delta ingest into ECS; orbital tree layout; swarms; inspector; keyboard focus model; search; conformance suite |
| **2** | **The Blackwall** | M | Wall shader driven by per-OS kernel-pressure signals; kernel monoliths with faded "no telemetry" variants; kthreads/drivers; cross-Wall streams from aggregate rates |
| **3** | **Storage + Network** | M | Fortresses, volume rings and IO streams; socket → process beams and endpoint clusters on all three OSes |
| **4** | **History & replay** | M | Bundled SQLite store with downsampling; `ReplaySource`; timeline; `.bwcap` export/import (and used as test fixtures) |
| **5** | **Issues: rules** | M | TOML rules engine with capability gating; journald/kmsg, Event Log, and unified log/crash-report readers; alert feed; glitch visuals; fly-to-issue |
| **6** | **Platform depth** | L | Deeper per-OS probes (PSI/vmstat, PDH, `host_statistics64`), SMART and temperatures where available, published capability matrix |
| **7** | **ICE helper + tier-1 actions** | L | Helper installed and authenticated on each OS (polkit / Windows service / SMAppService), elevated read probes, kill/suspend/priority/reveal, hash-chained audit log, signed builds |
| **8** | **Baselines & anomalies** | M | Hour-of-week profiles, novelty detectors, learning period, explanations, "this is normal" feedback |
| **9** | **Immersion polish** | M | Synthesized sound, screensaver with system-idle detection and sleep inhibit, quality governor, gesture recognizer, reduced-motion, list view |
| **10** | **Actions tiers 2–3** | M | Service control (systemd/SCM/launchd); firewall blocks (nftables/WFP/pf) with auto-expiry |
| **11** | **Packaging & release** | M | MSI + winget, notarized universal DMG, AppImage/deb/rpm/AUR, release automation |
| **12** | **Fleet-ready** | L | Headless `bw-agent` (engine-free crates only), `RemoteSource` over QUIC + mTLS, host switcher. A hub can come after this |
| Later | eBPF syscall flows, ETW deep tracing, Windows `.scr`, wasm/WebGPU viewer for fleet, self-updater | — | — |

Scheduled engine upgrades are slotted between milestones whenever a new Bevy minor version is out and `bevy_egui` supports it.

---

## 15. Risks and mitigations

| Risk | Mitigation |
|---|---|
| egui doesn't reach the desired cyberpunk polish | Go/no-go in Milestone 0; heavy theming plus Bevy UI overlays for the hero moments; panels behind a `bw-ui` boundary so they can be swapped |
| Bevy breaking changes | Pinned versions, engine isolated to four crates, deliberate upgrade milestones |
| GPU and driver diversity (old iGPUs, VMs, RDP, Wayland quirks) | GL-compatible Low tier, software-renderer detection, shader cross-compile in CI, render smoke tests on lavapipe and WARP |
| Platform data parity (some data missing or privileged on some OSes) | Capability flags, honest "not available / restricted" states, opt-in elevated read probes, capability-gated rules |
| Antivirus / Gatekeeper distrust of a process-killing tool | Code signing and notarization from Milestone 7; helper allowlist; clear docs |
| Paid signing requirements (Apple Developer, Windows certificate) | Unsigned dev builds until Milestone 7/11; budget for the certificates before the first public release |
| Linux distro fragmentation | Old-glibc builds, AppImage + deb/rpm, no hard systemd dependency, capability fallbacks |
| Collector overhead | Tiered cadences, diff-based updates, CI CPU-budget benchmark on all OSes |
| Visual clutter | Swarms, semantic zoom, filters, issues-only mode |
| Scope creep | Every milestone ships on all three OSes and is usable; the "Later" bucket stays later |

---

## 16. Open questions

1. Name of the helper (`blackwall-ice` is a placeholder).
2. A disk-usage "terrain" (large directories as landscape): part of the storage view, or a separate deep-dive mode? Scanning is expensive and slow on network drives.
3. License (MIT/Apache-2.0 dual is the Rust norm).
4. Are you willing to pay for an Apple Developer account and a Windows signing certificate? These are needed for the full-control features on macOS and for antivirus trust on Windows.
5. Is a WebAssembly/WebGPU viewer for the future fleet hub (phone access) worth keeping on the roadmap?
