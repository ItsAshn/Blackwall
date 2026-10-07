//! Blackwall's HUD: dense, readable egui panels over the 3D scene (PLAN §6.3).
//!
//! * top bar: host, source, machine vitals, Wall pressure
//! * left: searchable process list
//! * right: inspector for the selection
//! * bottom: system sparklines, legend, key hints
//! * world labels projected from the scene
//!
//! Character shortcuts use *logical* keys via egui so they follow the user's
//! keyboard layout; Cmd on macOS / Ctrl elsewhere is `Modifiers::command`.

mod theme;

use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use bw_model::{Health, ProcState, Process, Realm};
use bw_scene::camera::{InputBlock, MainCamera, OrbitCam};
use bw_scene::{
    LabelKind, Machine, ProcEntities, ProcNode, Quality, SceneLayout, SceneSettings, Selection,
    Shown, Tier, WorldLabel,
};
use egui::{Color32, Pos2, RichText, Stroke, Ui, UiBuilder, vec2};
use theme::*;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiState>()
            .add_systems(PreUpdate, block_input)
            .add_systems(EguiPrimaryContextPass, hud);
    }
}

#[derive(Resource, Debug)]
pub struct UiState {
    pub search: String,
    pub show_list: bool,
    pub show_inspector: bool,
    pub show_help: bool,
    /// Hide every panel (labels too) for an unobstructed view.
    pub hidden: bool,
    focus_search: bool,
    styled: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            search: String::new(),
            show_list: true,
            show_inspector: true,
            show_help: false,
            hidden: false,
            focus_search: false,
            styled: false,
        }
    }
}

/// Tell the scene which input egui has claimed this frame.
fn block_input(wants: Option<Res<EguiWantsInput>>, mut block: ResMut<InputBlock>) {
    if let Some(w) = wants {
        block.pointer = w.wants_any_pointer_input();
        block.keyboard = w.wants_any_keyboard_input();
    }
}

pub fn fmt_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn fmt_uptime(s: u64) -> String {
    let (d, h, m) = (s / 86_400, s / 3600 % 24, s / 60 % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else {
        format!("{h}h {m:02}m")
    }
}

#[allow(clippy::too_many_arguments)]
fn hud(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<UiState>,
    m: Res<Machine>,
    mut sel: ResMut<Selection>,
    mut settings: ResMut<SceneSettings>,
    mut quality: ResMut<Quality>,
    mut cam: ResMut<OrbitCam>,
    sl: Res<SceneLayout>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    labels: Query<(&WorldLabel, &GlobalTransform, &InheritedVisibility)>,
    nodes: Query<(&ProcNode, &Shown)>,
    ents: Res<ProcEntities>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !ui_state.styled {
        theme::apply(ctx);
        ui_state.styled = true;
    }
    let sel_changed = sel.is_changed();
    shortcuts(ctx, &mut ui_state, &mut settings, &mut sel, &m);

    let mut root = Ui::new(
        ctx.clone(),
        "bw-root".into(),
        UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    if ui_state.hidden {
        if settings.show_labels
            && let Ok((camera, cam_tf)) = camera.single()
        {
            world_labels(&root, camera, cam_tf, &m, &sel, &labels, &nodes, &ents);
        }
        return Ok(());
    }

    top_bar(&mut root, &m, &mut quality, &mut settings, &mut ui_state);
    bottom_bar(&mut root, &m, &settings);
    if ui_state.show_list {
        process_list(&mut root, &m, &mut sel, &mut ui_state, sel_changed);
    }
    if ui_state.show_inspector {
        inspector(&mut root, &m, &mut sel, &sl);
    }
    // Labels go in the space the panels left, so they never show through them.
    if settings.show_labels
        && let Ok((camera, cam_tf)) = camera.single()
    {
        world_labels(&root, camera, cam_tf, &m, &sel, &labels, &nodes, &ents);
    }
    if ui_state.show_help {
        help_window(ctx, &mut ui_state);
    }
    if !m.received {
        egui::Area::new("connecting".into())
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(RichText::new("JACKING IN…").size(28.0).color(CYAN).strong());
            });
    }
    let _ = &mut cam;
    Ok(())
}

fn shortcuts(
    ctx: &egui::Context,
    st: &mut UiState,
    settings: &mut SceneSettings,
    sel: &mut Selection,
    m: &Machine,
) {
    let typing = ctx.egui_wants_keyboard_input();
    ctx.input(|i| {
        if i.modifiers.command && i.key_pressed(egui::Key::K) {
            st.focus_search = true;
            st.show_list = true;
        }
        if typing {
            return;
        }
        if i.key_pressed(egui::Key::Slash) {
            st.focus_search = true;
            st.show_list = true;
        }
        if i.key_pressed(egui::Key::Questionmark) || i.key_pressed(egui::Key::F1) {
            st.show_help = !st.show_help;
        }
        if i.key_pressed(egui::Key::H) {
            st.hidden = !st.hidden;
        }
        if i.key_pressed(egui::Key::L) {
            st.show_list = !st.show_list;
        }
        if i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::I) {
            st.show_inspector = !st.show_inspector;
        }
        if i.key_pressed(egui::Key::Num1) {
            settings.show_links = !settings.show_links;
        }
        if i.key_pressed(egui::Key::Num2) {
            settings.show_kernel = !settings.show_kernel;
        }
        if i.key_pressed(egui::Key::Num3) {
            settings.show_streams = !settings.show_streams;
        }
        if i.key_pressed(egui::Key::Num4) {
            settings.show_labels = !settings.show_labels;
        }
        if i.key_pressed(egui::Key::Num5) {
            settings.issues_only = !settings.issues_only;
        }
        if i.key_pressed(egui::Key::M) {
            settings.reduced_motion = !settings.reduced_motion;
        }
    });
    // Drop a stale selection (process exited).
    if sel
        .key
        .is_some_and(|k| m.received && !m.snapshot.processes.contains_key(&k))
    {
        sel.key = None;
    }
}

fn top_bar(
    root: &mut Ui,
    m: &Machine,
    quality: &mut Quality,
    settings: &mut SceneSettings,
    st: &mut UiState,
) {
    egui::Panel::top("top").frame(bar_frame()).show(root, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("◢ BLACKWALL").size(18.0).strong().color(CYAN));
            let h = &m.snapshot.host;
            let (badge, color) = match h.source.as_str() {
                "live" => ("● LIVE", GREEN),
                "demo" => ("◆ DEMO DATA", AMBER),
                other => (other, DIM),
            };
            ui.label(RichText::new(badge).strong().color(color));
            ui.label(RichText::new(&h.hostname).color(DIM)).on_hover_text(format!("{}\nkernel {}\n{} · {} CPUs", h.os, h.kernel, h.arch, h.cpu_count));
            ui.separator();
            let s = &m.snapshot.system;
            stat(ui, "CPU", &format!("{:.0}%", s.cpu_pct), heat(s.cpu_pct / 100.0));

            let mem = s.mem_used as f32 / s.mem_total.max(1) as f32;
            stat(ui, "MEM", &format!("{} / {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total)), heat(mem));
            if s.swap_total > 0 {
                stat(ui, "SWAP", &fmt_bytes(s.swap_used), heat(s.swap_used as f32 / s.swap_total as f32));
            }
            let procs = m.snapshot.processes.values().filter(|p| p.realm == Realm::User).count();
            let kern = m.snapshot.processes.len() - procs;
            stat(ui, "PROCS", &format!("{procs} · {kern} kern"), TEXT).on_hover_text(format!("{procs} user-space processes, {kern} kernel-side"));
            if let Some(l) = s.load_avg {
                stat(ui, "LOAD", &format!("{:.1}", l[0]), TEXT).on_hover_text(format!("load average 1/5/15 min: {:.2} {:.2} {:.2}", l[0], l[1], l[2]));
            }
            stat(ui, "UP", &fmt_uptime(s.uptime_secs), DIM);
            ui.separator();
            ui.label(RichText::new("WALL").color(DIM).small());
            meter(ui, s.kernel_pressure, 90.0, heat(s.kernel_pressure));
            let src = if m.snapshot.caps.pressure_stall { "PSI" } else { "est." };
            ui.label(RichText::new(src).color(DIM).small()).on_hover_text(
                "Kernel pressure drives the Wall's turbulence.\nPSI = Linux pressure-stall information; est. = estimated from CPU and memory on this platform.",
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("?").on_hover_text("Shortcuts (?)").clicked() {
                    st.show_help = !st.show_help;
                }
                ui.menu_button("VIEW", |ui| {
                    ui.checkbox(&mut settings.show_links, "1  Tree links");
                    ui.checkbox(&mut settings.show_kernel, "2  Kernel side");
                    ui.checkbox(&mut settings.show_streams, "3  Wall streams");
                    ui.checkbox(&mut settings.show_labels, "4  Labels");
                    ui.checkbox(&mut settings.issues_only, "5  Issues only");
                    ui.checkbox(&mut settings.reduced_motion, "M  Reduced motion");
                    ui.separator();
                    ui.checkbox(&mut st.show_list, "L  Process list");
                    ui.checkbox(&mut st.show_inspector, "I  Inspector");
                });
                let label = format!("{}{} · {:.0} fps", quality.tier.label(), if quality.auto { " (auto)" } else { "" }, 1000.0 / quality.frame_ms.max(0.1));
                ui.menu_button(label, |ui| {
                    ui.label(RichText::new(format!("{} · {}", quality.adapter, quality.backend)).color(DIM).small());
                    if quality.software {
                        ui.label(RichText::new("Software renderer detected: limited to LOW").color(AMBER).small());
                    }
                    if ui.radio(quality.auto, "Automatic").clicked() {
                        quality.set_manual(None);
                    }
                    for t in Tier::ALL {
                        if ui.radio(!quality.auto && quality.tier == t, t.label()).clicked() {
                            quality.set_manual(Some(t));
                        }
                    }
                });
            });
        });
    });
}

fn bottom_bar(root: &mut Ui, m: &Machine, settings: &SceneSettings) {
    egui::Panel::bottom("bottom").frame(bar_frame()).show(root, |ui| {
        ui.horizontal(|ui| {
            let hist = &m.sys_history;
            let series = |i: usize| hist.iter().map(|h| h[i]).collect::<Vec<_>>();
            spark_labeled(ui, "CPU", &series(0), 100.0, CYAN);
            spark_labeled(ui, "MEM", &series(1), 100.0, INDIGO);
            spark_labeled(ui, "WALL", &series(2).iter().map(|v| v * 100.0).collect::<Vec<_>>(), 100.0, RED);
            ui.separator();
            legend(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let hints = "↑↓←→ walk tree · Tab busiest · / search · drag orbit · WASD pan · F follow · Home overview · H hide HUD · ? help";
                ui.label(RichText::new(hints).color(DIM).small());
                if settings.issues_only {
                    ui.label(RichText::new("ISSUES ONLY").color(AMBER).strong().small());
                }
            });
        });
    });
}

fn legend(ui: &mut Ui) {
    let item = |ui: &mut Ui, c: Color32, text: &str| {
        ui.label(RichText::new("■").color(c).strong());
        ui.label(RichText::new(text).color(DIM).small());
    };
    item(ui, BLUE, "healthy");
    item(ui, VIOLET, "watch");
    item(ui, RED, "issue");
    item(ui, INDIGO, "kernel");
    ui.label(
        RichText::new("height & density = memory · brightness & rising pulses = CPU")
            .color(DIM)
            .small(),
    );
}

/// List glyph: color is health only, matching the scene. Kernel threads get
/// a different glyph so they stay distinguishable without color.
pub fn glyph(p: &Process) -> (&'static str, Color32) {
    let g = if p.realm == Realm::Kernel {
        "▲"
    } else {
        "■"
    };
    let c = match (p.health(), p.realm) {
        (Health::Critical, _) => RED,
        (Health::Warning, _) => VIOLET,
        (Health::Healthy, Realm::Kernel) => INDIGO,
        (Health::Healthy, Realm::User) => BLUE,
    };
    (g, c)
}

fn process_list(
    root: &mut Ui,
    m: &Machine,
    sel: &mut Selection,
    st: &mut UiState,
    sel_changed: bool,
) {
    egui::Panel::left("procs")
        .frame(side_frame())
        .default_size(300.0)
        .resizable(true)
        .show(root, |ui| {
            ui.label(RichText::new("PROCESSES").color(CYAN).strong());
            let edit = ui.add(
                egui::TextEdit::singleline(&mut st.search)
                    .hint_text("search name, pid, user  ( / )")
                    .desired_width(f32::INFINITY),
            );
            if st.focus_search {
                edit.request_focus();
                st.focus_search = false;
            }
            if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                edit.surrender_focus();
            }
            let q = st.search.to_lowercase();
            let mut rows: Vec<&Process> = m
                .snapshot
                .processes
                .values()
                .filter(|p| {
                    q.is_empty()
                        || p.name.to_lowercase().contains(&q)
                        || p.key.pid.to_string() == q
                        || p.user.as_deref().is_some_and(|u| u.to_lowercase() == q)
                })
                .collect();
            rows.sort_by(|a, b| {
                b.cpu_pct
                    .total_cmp(&a.cpu_pct)
                    .then(b.mem_bytes.cmp(&a.mem_bytes))
            });
            // Enter in the search box jumps to the top match.
            if edit.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some(p) = rows.first()
            {
                sel.key = Some(p.key);
                sel.follow = true;
            }
            ui.label(
                RichText::new(format!("{} shown · sorted by CPU", rows.len()))
                    .color(DIM)
                    .small(),
            );
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                18.0,
                rows.len(),
                |ui, range| {
                    for p in &rows[range] {
                        let selected = sel.key == Some(p.key);
                        let (g, c) = glyph(p);
                        let resp = ui
                            .horizontal(|ui| {
                                ui.set_min_height(18.0);
                                ui.label(RichText::new(g).color(c));
                                let name = RichText::new(truncate(&p.name, 19))
                                    .color(if selected { Color32::WHITE } else { TEXT });
                                let r = ui.add(
                                    egui::Button::selectable(selected, name)
                                        .frame_when_inactive(false),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(fmt_bytes(p.mem_bytes))
                                                .color(DIM)
                                                .small(),
                                        );
                                        ui.label(
                                            RichText::new(format!("{:>5.1}%", p.cpu_pct))
                                                .color(heat(p.cpu_pct / 100.0))
                                                .small(),
                                        );
                                    },
                                );
                                r
                            })
                            .inner;
                        if resp.clicked() {
                            sel.key = Some(p.key);
                            sel.follow = true;
                        }
                        if selected && sel_changed {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                },
            );
        });
}

fn inspector(root: &mut Ui, m: &Machine, sel: &mut Selection, sl: &SceneLayout) {
    egui::Panel::right("inspector").frame(side_frame()).default_size(330.0).resizable(true).show(root, |ui| {
        let Some(p) = sel.key.and_then(|k| m.snapshot.processes.get(&k)) else {
            ui.label(RichText::new("INSPECTOR").color(CYAN).strong());
            ui.add_space(8.0);
            ui.label(RichText::new("Nothing selected.").color(TEXT));
            ui.label(RichText::new("Click an entity, pick one from the list, or press ↓ / Tab to start walking the tree.").color(DIM));
            return;
        };
        let (g, c) = glyph(p);
        ui.horizontal(|ui| {
            ui.label(RichText::new(g).color(c).size(20.0));
            ui.label(RichText::new(&p.name).size(18.0).strong().color(Color32::WHITE));
        });
        let realm = match p.realm {
            Realm::Kernel => format!("kernel side · {}", sl.layout.subsystem_of(&p.key).map(|s| s.label()).unwrap_or("kernel")),
            Realm::User => "deep space (user)".into(),
        };
        ui.label(RichText::new(realm).color(DIM).small());
        ui.separator();
        egui::Grid::new("facts").num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
            let row = |ui: &mut Ui, k: &str, v: String, c: Color32| {
                ui.label(RichText::new(k).color(DIM));
                ui.label(RichText::new(v).color(c));
                ui.end_row();
            };
            row(ui, "PID", p.key.pid.to_string(), TEXT);
            row(ui, "USER", p.user.clone().unwrap_or_else(|| "?".into()), TEXT);
            let (state, sc) = match p.state {
                ProcState::Running => ("running", GREEN),
                ProcState::Sleeping => ("sleeping", TEXT),
                ProcState::DiskWait => ("uninterruptible IO wait", RED),
                ProcState::Stopped => ("stopped", RED),
                ProcState::Zombie => ("zombie (exited, not reaped)", RED),
                ProcState::Idle => ("idle", DIM),
                ProcState::Unknown => ("unknown", DIM),
            };
            row(ui, "STATE", state.into(), sc);
            row(ui, "CPU", format!("{:.1}%", p.cpu_pct), heat(p.cpu_pct / 100.0));
            row(ui, "MEMORY", format!("{} rss · {} virt", fmt_bytes(p.mem_bytes), fmt_bytes(p.virt_bytes)), TEXT);
            if let Some(t) = p.threads {
                row(ui, "THREADS", t.to_string(), TEXT);
            }
            if m.snapshot.caps.process_io {
                row(ui, "DISK IO", format!("↓ {}/s  ↑ {}/s", fmt_bytes(p.io_read_bytes), fmt_bytes(p.io_write_bytes)), TEXT);
            }
        });
        if let Some(reason) = p.health_reason() {
            let c = if p.health() == Health::Critical { RED } else { VIOLET };
            ui.label(RichText::new(format!("⚠ {reason}")).color(c).strong());
        }
        if p.restricted {
            ui.label(RichText::new("🔒 Details restricted by the OS. Elevated read access arrives with the ICE helper.").color(AMBER).small());
        }
        ui.add_space(6.0);
        if let Some(h) = m.cpu_history.get(&p.key) {
            let v: Vec<f32> = h.iter().copied().collect();
            let max = v.iter().copied().fold(10.0, f32::max);
            spark_labeled(ui, "CPU", &v, max, CYAN);
        }
        if let Some(h) = m.mem_history.get(&p.key) {
            let v: Vec<f32> = h.iter().copied().collect();
            let max = v.iter().copied().fold(1.0, f32::max) * 1.2;
            spark_labeled(ui, "MEM", &v, max, INDIGO);
        }
        ui.separator();
        if let Some(exe) = &p.exe {
            ui.label(RichText::new("EXECUTABLE").color(DIM).small());
            ui.label(RichText::new(exe).color(TEXT).monospace().small());
        }
        if !p.cmd.is_empty() {
            ui.label(RichText::new("COMMAND").color(DIM).small());
            ui.label(RichText::new(truncate(&p.cmd.join(" "), 300)).color(TEXT).monospace().small());
        }
        ui.separator();
        if let Some(parent) = p.parent.and_then(|k| m.snapshot.processes.get(&k)) {
            ui.label(RichText::new("PARENT  ↑").color(DIM).small());
            let (g, c) = glyph(parent);
            if ui.button(RichText::new(format!("{g} {}  ({})", parent.name, parent.key.pid)).color(c)).clicked() {
                sel.last_child.insert(parent.key, p.key);
                sel.key = Some(parent.key);
                sel.follow = true;
            }
        }
        let mut kids: Vec<&Process> = m.snapshot.processes.values().filter(|c| c.parent == Some(p.key)).collect();
        if !kids.is_empty() {
            kids.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
            ui.label(RichText::new(format!("CHILDREN  ↓  ({})", kids.len())).color(DIM).small());
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for k in kids {
                    let (g, c) = glyph(k);
                    if ui.button(RichText::new(format!("{g} {}  {:.1}%", truncate(&k.name, 28), k.cpu_pct)).color(c)).clicked() {
                        sel.key = Some(k.key);
                        sel.follow = true;
                    }
                }
            });
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let follow = if sel.follow { "camera following · F to release" } else { "camera free · F to follow" };
            ui.label(RichText::new(follow).color(DIM).small());
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn world_labels(
    root: &Ui,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    m: &Machine,
    sel: &Selection,
    labels: &Query<(&WorldLabel, &GlobalTransform, &InheritedVisibility)>,
    nodes: &Query<(&ProcNode, &Shown)>,
    ents: &ProcEntities,
) {
    let painter = root
        .painter()
        .with_clip_rect(root.available_rect_before_wrap());
    let painter = &painter;
    let font = egui::FontId::monospace(12.0);
    let mut placed: Vec<egui::Rect> = Vec::new();
    let project = |p: Vec3| {
        camera
            .world_to_viewport(cam_tf, p)
            .ok()
            .map(|v| Pos2::new(v.x, v.y))
    };
    for (label, tf, vis) in labels {
        if !vis.get() {
            continue;
        }
        let (offset, color) = match label.kind {
            LabelKind::Subsystem => (Vec3::new(0.0, 0.6, 0.0), INDIGO),
            LabelKind::Volume => (Vec3::new(0.0, 0.5, 0.0), TEXT),
        };
        if let Some(p) = project(tf.translation() + offset) {
            let galley = painter.layout_no_wrap(label.text.clone(), font.clone(), color);
            let rect = egui::Align2::CENTER_BOTTOM
                .anchor_size(p, galley.size())
                .expand(2.0);
            if placed.iter().any(|r| r.intersects(rect)) {
                continue;
            }
            placed.push(rect);
            painter.galley(rect.min + vec2(2.0, 2.0), galley, color);
        }
    }
    // Name the busiest processes, plus the selected and hovered ones.
    let mut top: Vec<&Process> = m
        .snapshot
        .processes
        .values()
        .filter(|p| p.realm == Realm::User)
        .collect();
    top.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
    let keys: Vec<_> = top.iter().take(8).map(|p| p.key).collect();
    // Selected and hovered first, so they win any overlap.
    let mut ordered: Vec<_> = sel.key.into_iter().chain(sel.hovered).collect();
    ordered.extend(keys);
    for k in ordered {
        let Some((_, shown)) = ents.0.get(&k).and_then(|e| nodes.get(*e).ok()) else {
            continue;
        };
        let Some(p) = m.snapshot.processes.get(&k) else {
            continue;
        };
        let Some(pos) = project(shown.pos + Vec3::Y * (shown.radius + 0.4)) else {
            continue;
        };
        let strong = Some(k) == sel.key || Some(k) == sel.hovered;
        let text = format!("{}  {:.0}%", truncate(&p.name, 22), p.cpu_pct);
        let galley = painter.layout_no_wrap(
            text,
            font.clone(),
            if strong { Color32::WHITE } else { TEXT },
        );
        let rect = egui::Align2::CENTER_BOTTOM
            .anchor_size(pos, galley.size())
            .expand(3.0);
        if placed.iter().any(|r| r.intersects(rect)) {
            continue;
        }
        placed.push(rect);
        painter.rect_filled(
            rect,
            2.0,
            Color32::from_rgba_unmultiplied(4, 3, 12, if strong { 220 } else { 150 }),
        );
        if strong {
            painter.rect_stroke(rect, 2.0, Stroke::new(1.0, CYAN), egui::StrokeKind::Outside);
        }
        painter.galley(rect.min + vec2(3.0, 3.0), galley, TEXT);
    }
}

fn help_window(ctx: &egui::Context, st: &mut UiState) {
    let mut open = true;
    egui::Window::new("SHORTCUTS")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Grid::new("keys")
                .num_columns(2)
                .spacing(vec2(18.0, 4.0))
                .show(ui, |ui| {
                    for (k, v) in [
                        ("↑ / ↓", "parent / child"),
                        ("← / →", "previous / next sibling"),
                        ("Tab", "cycle the busiest processes"),
                        ("Esc", "clear selection"),
                        ("/  or  Ctrl/⌘+K", "search"),
                        ("Enter / I", "toggle inspector"),
                        ("L", "toggle process list"),
                        ("H", "hide the whole HUD"),
                        ("Drag · Shift+arrows", "orbit"),
                        ("Right-drag · WASD · R/Shift+F", "pan"),
                        ("Wheel · PgUp/PgDn · +/-", "zoom"),
                        ("Pinch · two-finger drag", "zoom · pan (touch)"),
                        ("F", "follow selection on/off"),
                        ("Home", "overview"),
                        (
                            "1 2 3 4 5",
                            "links · kernel · streams · labels · issues-only",
                        ),
                        ("M", "reduced motion"),
                    ] {
                        ui.label(RichText::new(k).color(CYAN).monospace());
                        ui.label(RichText::new(v).color(TEXT));
                        ui.end_row();
                    }
                });
        });
    if !open {
        st.show_help = false;
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn stat(ui: &mut Ui, k: &str, v: &str, c: Color32) -> egui::Response {
    ui.label(RichText::new(k).color(DIM).small());
    ui.label(RichText::new(v).color(c).strong())
}

fn meter(ui: &mut Ui, frac: f32, width: f32, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 10.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        rect,
        1.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, 18),
    );
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    p.rect_filled(fill, 1.0, c);
}

fn spark_labeled(ui: &mut Ui, label: &str, v: &[f32], max: f32, c: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(DIM).small());
        sparkline(ui, v, max, c, vec2(110.0, 22.0));
        if let Some(last) = v.last() {
            let txt = if max > 100.0 {
                fmt_bytes(*last as u64)
            } else {
                format!("{last:.0}%")
            };
            ui.label(RichText::new(txt).color(c).small());
        }
    });
}

fn sparkline(ui: &mut Ui, v: &[f32], max: f32, c: Color32, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 8));
    if v.len() < 2 {
        return;
    }
    let n = bw_scene::HISTORY_LEN.max(v.len()) as f32;
    let start = n - v.len() as f32;
    let pts: Vec<Pos2> = v
        .iter()
        .enumerate()
        .map(|(i, y)| {
            Pos2::new(
                rect.left() + (start + i as f32) / (n - 1.0) * rect.width(),
                rect.bottom() - (y / max.max(1e-6)).clamp(0.0, 1.0) * rect.height(),
            )
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(1.4, c)));
}
