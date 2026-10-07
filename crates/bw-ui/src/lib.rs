//! Blackwall's HUD: words floating on pure black over the scene (design
//! system: HUD). No panels, boxes or dividers; regions are made by position
//! and space, and the scene fades to black beneath them.
//!
//! * top: the wordmark, machine vitals, Wall pressure
//! * left: searchable process list
//! * right: inspector for the selection
//! * bottom: dotted sparklines, legend, key hints
//! * world labels projected from the scene
//!
//! Character shortcuts use *logical* keys via egui so they follow the user's
//! keyboard layout; Cmd on macOS / Ctrl elsewhere is `Modifiers::command`.

mod quiet;
mod theme;

use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use bw_model::{Health, ProcState, Process, Realm};
use bw_scene::camera::{InputBlock, MainCamera};
use bw_scene::{
    FloorInfo, LabelKind, Machine, ProcEntities, ProcNode, Quality, SceneLayout, SceneSettings,
    Selection, Shown, Tier, WorldLabel,
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
    /// Machine vitals along the top, sparklines along the bottom (V).
    pub show_vitals: bool,
    /// The search overlay (/).
    search_open: bool,
    focus_search: bool,
    styled: bool,
    /// Last frame's HUD column sizes, for the fades painted beneath them.
    left_w: f32,
    right_w: f32,
    top_h: f32,
    bottom_h: f32,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            search: String::new(),
            show_list: false,
            show_inspector: false,
            show_help: false,
            hidden: false,
            show_vitals: false,
            search_open: false,
            focus_search: false,
            styled: false,
            left_w: 0.0,
            right_w: 0.0,
            top_h: 0.0,
            bottom_h: 0.0,
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

fn label(text: &str) -> RichText {
    RichText::new(text).size(10.0).color(SIGNAL_DIM)
}

fn value(text: impl Into<String>) -> RichText {
    RichText::new(text).font(semibold(13.0)).color(SIGNAL)
}

#[allow(clippy::too_many_arguments)]
fn hud(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<UiState>,
    m: Res<Machine>,
    mut sel: ResMut<Selection>,
    mut settings: ResMut<SceneSettings>,
    mut quality: ResMut<Quality>,
    sl: Res<SceneLayout>,
    floor: Res<FloorInfo>,
    jack: Res<bw_scene::camera::JackIn>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    labels: Query<(&WorldLabel, &GlobalTransform, &InheritedVisibility)>,
    nodes: Query<(&ProcNode, &Shown)>,
    ents: Res<ProcEntities>,
    mut ex: ResMut<bw_scene::explore::Explore>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !ui_state.styled {
        // Fonts installed now take effect on egui's next pass; draw nothing
        // until then, or the custom families aren't bound yet.
        theme::apply(ctx);
        ui_state.styled = true;
        return Ok(());
    }
    let sel_changed = sel.is_changed();
    shortcuts(ctx, &mut ui_state, &mut settings, &mut sel, &m);

    let screen = ctx.viewport_rect();
    let mut root = Ui::new(
        ctx.clone(),
        "bw-root".into(),
        UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(screen),
    );
    macro_rules! draw_labels {
        ($root:expr) => {
            if settings.show_labels {
                if let Ok((camera, cam_tf)) = camera.single() {
                    world_labels($root, camera, cam_tf, &m, &sel, &labels, &nodes, &ents);
                }
            }
        };
    }

    // Nothing but the rain while jacking in.
    if jack.running() {
        return Ok(());
    }
    let cam = camera.single().ok();
    if ui_state.hidden {
        quiet::fade_overlay(ctx, &ex);
        return Ok(());
    }

    // Summoned columns; the scene fades to black beneath each.
    let p = root.painter().clone();
    let reach = 70.0;
    if ui_state.show_vitals {
        if ui_state.top_h > 0.0 {
            fade(
                &p,
                egui::Rect::from_min_size(
                    screen.min,
                    vec2(screen.width(), ui_state.top_h + reach * 0.5),
                ),
                Edge::Top,
            );
        }
        if ui_state.bottom_h > 0.0 {
            fade(
                &p,
                egui::Rect::from_min_max(
                    Pos2::new(
                        screen.left(),
                        screen.bottom() - ui_state.bottom_h - reach * 0.5,
                    ),
                    screen.max,
                ),
                Edge::Bottom,
            );
        }
        ui_state.top_h = top_bar(&mut root, &m, &mut quality, &mut settings, &mut ui_state);
        ui_state.bottom_h = bottom_bar(&mut root, &m, &settings, &floor);
    } else {
        quiet::breadcrumb(ctx, &m, &sel, &ex);
    }
    if ui_state.show_list {
        if ui_state.left_w > 0.0 {
            fade(
                &p,
                egui::Rect::from_min_size(
                    screen.min,
                    vec2(ui_state.left_w + reach, screen.height()),
                ),
                Edge::Left,
            );
        }
        ui_state.left_w = process_list(&mut root, &m, &mut sel, &mut ui_state, sel_changed);
    }
    if ui_state.show_inspector {
        if ui_state.right_w > 0.0 {
            fade(
                &p,
                egui::Rect::from_min_max(
                    Pos2::new(screen.right() - ui_state.right_w - reach, screen.top()),
                    screen.max,
                ),
                Edge::Right,
            );
        }
        ui_state.right_w = if ex.inside().is_some() {
            element_inspector(&mut root, &m, &mut ex)
        } else {
            inspector(&mut root, &m, &mut sel, &sl)
        };
    }
    if ex.inside().is_none() {
        draw_labels!(&root);
    }
    if let Some((camera, cam_tf)) = cam {
        quiet::whisper(ctx, camera, cam_tf, &m, &sel, &ex, &sl);
        quiet::compass(ctx, camera, cam_tf, &ex);
    }
    if !ui_state.show_vitals {
        quiet::hint(ctx, &ex, &sel);
    }
    quiet::search(ctx, &mut ui_state, &m, &mut sel, &mut ex);
    if ui_state.show_help {
        help_window(ctx, &mut ui_state);
    }
    if !m.received {
        egui::Area::new("connecting".into())
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("JACKING IN")
                        .font(display(28.0))
                        .color(WALL_CALM),
                );
            });
    }
    quiet::fade_overlay(ctx, &ex);
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
            st.search_open = true;
            st.focus_search = true;
        }
        if typing {
            return;
        }
        if i.key_pressed(egui::Key::Slash) {
            st.search_open = true;
            st.focus_search = true;
        }
        if i.key_pressed(egui::Key::V) {
            st.show_vitals = !st.show_vitals;
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
        if i.key_pressed(egui::Key::I) {
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

fn stat(ui: &mut Ui, k: &str, v: impl Into<String>) -> egui::Response {
    ui.label(label(k));
    ui.label(value(v))
}

/// The Wall's color for a pressure, matching the rain (and the meter).
fn pressure_color(p: f32) -> Color32 {
    match p {
        p if p < 0.35 => OK,
        p if p < 0.9 => WALL_CALM,
        _ => WALL_HOT,
    }
}

fn top_bar(
    root: &mut Ui,
    m: &Machine,
    quality: &mut Quality,
    settings: &mut SceneSettings,
    st: &mut UiState,
) -> f32 {
    egui::Panel::top("top")
        .frame(bare_frame(14, 10))
        .show_separator_line(false)
        .show(root, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(150.0, 22.0), egui::Sense::hover());
                glow_text(ui.painter(), r.left_center(), egui::Align2::LEFT_CENTER, "BLACKWALL", display(20.0), SIGNAL, OK);
                let h = &m.snapshot.host;
                match h.source.as_str() {
                    "live" => ui.label(label("LIVE")),
                    "demo" => ui.label(RichText::new("DEMO DATA").font(semibold(11.0)).color(SIGNAL)),
                    other => ui.label(label(other)),
                };
                ui.label(RichText::new(&h.hostname).size(11.0).color(SIGNAL_DIM)).on_hover_text(format!("{}\nkernel {}\n{} · {} CPUs", h.os, h.kernel, h.arch, h.cpu_count));
                ui.add_space(12.0);
                let s = &m.snapshot.system;
                stat(ui, "CPU", format!("{:.0}%", s.cpu_pct));
                stat(ui, "MEM", format!("{} / {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total)));
                if s.swap_total > 0 {
                    stat(ui, "SWAP", fmt_bytes(s.swap_used));
                }
                let procs = m.snapshot.processes.values().filter(|p| p.realm == Realm::User).count();
                let kern = m.snapshot.processes.len() - procs;
                stat(ui, "PROCS", format!("{procs} · {kern} kern")).on_hover_text(format!("{procs} user-space processes, {kern} kernel-side"));
                if let Some(l) = s.load_avg {
                    stat(ui, "LOAD", format!("{:.1}", l[0])).on_hover_text(format!("load average 1/5/15 min: {:.2} {:.2} {:.2}", l[0], l[1], l[2]));
                }
                ui.label(label("UP"));
                ui.label(RichText::new(fmt_uptime(s.uptime_secs)).size(13.0).color(SIGNAL_DIM));
                ui.add_space(12.0);
                ui.label(label("WALL"));
                dot_meter(ui, s.kernel_pressure, 20, pressure_color(s.kernel_pressure));
                let pc = if s.kernel_pressure >= 0.9 { ISSUE } else { SIGNAL };
                ui.label(RichText::new(format!("{:.0}%", s.kernel_pressure * 100.0)).font(semibold(13.0)).color(pc));
                let src = if m.snapshot.caps.pressure_stall { "PSI" } else { "est." };
                ui.label(label(src)).on_hover_text(
                    "Kernel pressure drives the Wall's rain.\nPSI = Linux pressure-stall information; est. = estimated from CPU and memory on this platform.",
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(RichText::new("?").color(SIGNAL)).on_hover_text("Shortcuts (?)").clicked() {
                        st.show_help = !st.show_help;
                    }
                    ui.menu_button(RichText::new("VIEW").size(11.0).color(SIGNAL_DIM), |ui| {
                        ui.checkbox(&mut settings.show_links, "1  All family lines");
                        ui.checkbox(&mut settings.show_kernel, "2  Kernel side");
                        ui.checkbox(&mut settings.show_streams, "3  Streams");
                        ui.checkbox(&mut settings.show_labels, "4  Labels");
                        ui.checkbox(&mut settings.issues_only, "5  Issues only");
                        ui.checkbox(&mut settings.reduced_motion, "M  Reduced motion");
                        ui.add_space(4.0);
                        ui.checkbox(&mut st.show_list, "L  Process list");
                        ui.checkbox(&mut st.show_inspector, "I  Inspector");
                    });
                    let q = format!("{}{} · {:.0} FPS", quality.tier.label(), if quality.auto { " AUTO" } else { "" }, 1000.0 / quality.frame_ms.max(0.1));
                    ui.menu_button(RichText::new(q).size(11.0).color(SIGNAL_DIM), |ui| {
                        ui.label(label(&format!("{} · {}", quality.adapter, quality.backend)));
                        if quality.software {
                            ui.label(RichText::new("Software renderer: limited to LOW").size(10.0).color(SIGNAL));
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
        })
        .response
        .rect
        .height()
}

fn bottom_bar(root: &mut Ui, m: &Machine, settings: &SceneSettings, floor: &FloorInfo) -> f32 {
    egui::Panel::bottom("bottom")
        .frame(bare_frame(14, 10))
        .show_separator_line(false)
        .show(root, |ui| {
            ui.horizontal(|ui| {
                let hist = &m.sys_history;
                let series = |i: usize, k: f32| hist.iter().map(|h| h[i] * k).collect::<Vec<_>>();
                spark_labeled(ui, "CPU", &series(0, 1.0), 100.0, OK);
                spark_labeled(ui, "MEM", &series(1, 1.0), 100.0, KERNEL);
                spark_labeled(ui, "WALL", &series(2, 100.0), 100.0, WALL_HOT);
                ui.add_space(12.0);
                legend(ui, floor);
            });
            ui.horizontal(|ui| {
                if settings.issues_only {
                    ui.label(RichText::new("ISSUES ONLY").font(semibold(10.0)).color(SIGNAL));
                }
                let hints = "drag pan · wheel zoom · right-drag turn · click choose · zoom in to open · Tab busiest · N next issue · / search · ? help";
                ui.label(RichText::new(hints).size(10.0).color(SIGNAL_DIM));
            });
        })
        .response
        .rect
        .height()
}

fn legend(ui: &mut Ui, floor: &FloorInfo) {
    for (g, c, text) in [
        (Glyph::Square, OK, "healthy"),
        (Glyph::Diamond, WATCH, "watch"),
        (Glyph::Cross, ISSUE, "issue"),
        (Glyph::Triangle, KERNEL, "kernel"),
    ] {
        glyph(ui, g, c);
        ui.label(RichText::new(text).size(10.0).color(SIGNAL_DIM));
    }
    let floor_text = if floor.mb_per_dot > 0 {
        format!(" · floor: 1 dot = {} MB RAM", floor.mb_per_dot)
    } else {
        String::new()
    };
    ui.label(
        RichText::new(format!("height = memory · light = CPU{floor_text}"))
            .size(10.0)
            .color(SIGNAL_DIM),
    );
}

/// A process's CPU in the list and inspector: health color when not healthy.
fn cpu_color(p: &Process) -> Color32 {
    match p.health() {
        Health::Critical => ISSUE,
        Health::Warning => WATCH,
        Health::Healthy => SIGNAL,
    }
}

fn process_list(
    root: &mut Ui,
    m: &Machine,
    sel: &mut Selection,
    st: &mut UiState,
    sel_changed: bool,
) -> f32 {
    egui::Panel::left("procs")
        .frame(bare_frame(14, 8))
        .show_separator_line(false)
        .default_size(330.0)
        .resizable(true)
        .show(root, |ui| {
            ui.label(label("PROCESSES"));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut st.search)
                    .hint_text(RichText::new("search name, pid, user  ( / )").color(SIGNAL_DIM))
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE),
            );
            // The search field is a line of dots, not a box.
            let r = edit.rect;
            let line_c = if edit.has_focus() { SIGNAL } else { DOT_OFF };
            let mut x = r.left();
            while x < r.right() {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(Pos2::new(x, r.bottom() + 1.0), vec2(2.0, 1.0)),
                    0.0,
                    line_c,
                );
                x += 4.0;
            }
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
                RichText::new(format!("{} shown · by CPU", rows.len()))
                    .size(10.0)
                    .color(SIGNAL_DIM),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                18.0,
                rows.len(),
                |ui, range| {
                    for p in &rows[range] {
                        let selected = sel.key == Some(p.key);
                        let resp = ui
                            .horizontal(|ui| {
                                ui.set_min_height(18.0);
                                let (g, c) = glyph_of(p);
                                glyph(ui, g, c);
                                let name = RichText::new(truncate(&p.name, 17))
                                    .size(11.0)
                                    .color(if selected { SELECT } else { SIGNAL });
                                let r = ui.add(
                                    egui::Button::selectable(false, name)
                                        .frame_when_inactive(false),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(fmt_bytes(p.mem_bytes))
                                                .size(10.0)
                                                .color(SIGNAL_DIM),
                                        );
                                        ui.label(
                                            RichText::new(format!("{:>5.1}%", p.cpu_pct))
                                                .size(10.0)
                                                .color(cpu_color(p)),
                                        );
                                    },
                                );
                                r
                            })
                            .inner;
                        if selected {
                            ui.painter().rect_stroke(
                                resp.rect.expand(1.0),
                                0.0,
                                Stroke::new(1.0, SELECT),
                                egui::StrokeKind::Outside,
                            );
                        }
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
        })
        .response
        .rect
        .width()
}

fn inspector(root: &mut Ui, m: &Machine, sel: &mut Selection, _sl: &SceneLayout) -> f32 {
    egui::Panel::right("inspector")
        .frame(bare_frame(14, 8))
        .show_separator_line(false)
        .default_size(330.0)
        .resizable(true)
        .show(root, |ui| {
            let Some(p) = sel.key.and_then(|k| m.snapshot.processes.get(&k)) else {
                ui.label(label("INSPECTOR"));
                ui.add_space(8.0);
                ui.label(RichText::new("Nothing selected.").size(12.0).color(SIGNAL));
                ui.label(RichText::new("Click a column, pick one from the list, or press ↓ or Tab to start walking the tree.").size(11.0).color(SIGNAL_DIM));
                return;
            };
            ui.horizontal(|ui| {
                let (g, c) = glyph_of(p);
                glyph(ui, g, c);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::hover());
                glow_text(ui.painter(), r.left_center(), egui::Align2::LEFT_CENTER, &truncate(&p.name, 24), semibold(17.0), SELECT, OK);
            });
            let realm = match p.realm {
                Realm::Kernel => format!("kernel · {}", bw_scene::Subsystem::classify(&p.name).label().to_lowercase()),
                Realm::User => "deep space (user)".into(),
            };
            ui.label(RichText::new(realm).size(10.0).color(SIGNAL_DIM));
            ui.add_space(8.0);
            egui::Grid::new("facts").num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                let row = |ui: &mut Ui, k: &str, v: String, c: Color32| {
                    ui.label(label(k));
                    ui.label(RichText::new(v).size(12.0).color(c));
                    ui.end_row();
                };
                row(ui, "PID", p.key.pid.to_string(), SIGNAL);
                row(ui, "USER", p.user.clone().unwrap_or_else(|| "?".into()), SIGNAL);
                let (state, sc) = match p.state {
                    ProcState::Running => ("running", SIGNAL),
                    ProcState::Sleeping => ("sleeping", SIGNAL),
                    ProcState::DiskWait => ("uninterruptible IO wait", ISSUE),
                    ProcState::Stopped => ("stopped", ISSUE),
                    ProcState::Zombie => ("zombie (exited, not reaped)", ISSUE),
                    ProcState::Idle => ("idle", SIGNAL_DIM),
                    ProcState::Unknown => ("unknown", SIGNAL_DIM),
                };
                row(ui, "STATE", state.into(), sc);
                row(ui, "CPU", format!("{:.1}%", p.cpu_pct), cpu_color(p));
                row(ui, "MEMORY", format!("{} rss · {} virt", fmt_bytes(p.mem_bytes), fmt_bytes(p.virt_bytes)), SIGNAL);
                if let Some(t) = p.threads {
                    row(ui, "THREADS", t.to_string(), SIGNAL);
                }
                if m.snapshot.caps.process_io {
                    row(ui, "DISK IO", format!("↓ {}/s  ↑ {}/s", fmt_bytes(p.io_read_bytes), fmt_bytes(p.io_write_bytes)), SIGNAL);
                }
            });
            if let Some(reason) = p.health_reason() {
                let (g, c) = glyph_of(p);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    glyph(ui, g, c);
                    ui.label(RichText::new(reason).font(semibold(12.0)).color(c));
                });
            }
            if p.restricted {
                ui.label(RichText::new("Details restricted by the OS. Elevated read access arrives with the ICE helper.").size(10.0).color(SIGNAL_DIM));
            }
            ui.add_space(8.0);
            if let Some(h) = m.cpu_history.get(&p.key) {
                let v: Vec<f32> = h.iter().copied().collect();
                let max = v.iter().copied().fold(10.0, f32::max);
                let c = if p.health() == Health::Healthy { OK } else { cpu_color(p) };
                spark_labeled(ui, "CPU", &v, max, c);
            }
            if let Some(h) = m.mem_history.get(&p.key) {
                let v: Vec<f32> = h.iter().copied().collect();
                let max = v.iter().copied().fold(1.0, f32::max) * 1.2;
                spark_labeled(ui, "MEM", &v, max, KERNEL);
            }
            ui.add_space(8.0);
            if let Some(exe) = &p.exe {
                ui.label(label("EXECUTABLE"));
                ui.label(RichText::new(exe).size(10.0).color(SIGNAL));
            }
            if !p.cmd.is_empty() {
                ui.label(label("COMMAND"));
                ui.label(RichText::new(truncate(&p.cmd.join(" "), 300)).size(10.0).color(SIGNAL));
            }
            ui.add_space(8.0);
            if let Some(parent) = p.parent.and_then(|k| m.snapshot.processes.get(&k)) {
                ui.label(label("PARENT ↑"));
                if family_button(ui, parent, &format!("{}  ({})", parent.name, parent.key.pid)) {
                    sel.last_child.insert(parent.key, p.key);
                    sel.key = Some(parent.key);
                    sel.follow = true;
                }
            }
            let mut kids: Vec<&Process> = m.snapshot.processes.values().filter(|c| c.parent == Some(p.key)).collect();
            if !kids.is_empty() {
                kids.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
                ui.label(label(&format!("CHILDREN ↓  ({})", kids.len())));
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for k in kids {
                        if family_button(ui, k, &format!("{}  {:.1}%", truncate(&k.name, 28), k.cpu_pct)) {
                            sel.key = Some(k.key);
                            sel.follow = true;
                        }
                    }
                });
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                let follow = if sel.follow { "camera following · F to release" } else { "camera free · F to follow" };
                ui.label(RichText::new(follow).size(10.0).color(SIGNAL_DIM));
            });
        })
        .response
        .rect
        .width()
}

/// A parent or child: its glyph, then its name as a text button.
fn family_button(ui: &mut Ui, p: &Process, text: &str) -> bool {
    ui.horizontal(|ui| {
        let (g, c) = glyph_of(p);
        glyph(ui, g, c);
        ui.add(egui::Button::new(RichText::new(text).size(12.0).color(SIGNAL)).frame(false))
            .clicked()
    })
    .inner
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
    let font = egui::FontId::monospace(11.0);
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
            LabelKind::Subsystem => (Vec3::new(0.0, 0.6, 0.0), KERNEL),
            LabelKind::Volume => (Vec3::new(0.0, 0.5, 0.0), SIGNAL_DIM),
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
    // Process names come from the whisper (quiet.rs): one at a time, never a crowd.
    let _ = (m, sel, nodes, ents, &mut placed);
}

fn help_window(ctx: &egui::Context, st: &mut UiState) {
    let mut open = true;
    egui::Window::new(RichText::new("SHORTCUTS").size(11.0).color(SIGNAL_DIM))
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
                        ("Drag · WASD · arrows", "pan the map"),
                        ("Wheel · pinch · +/-", "zoom toward the cursor"),
                        ("Right-drag · Q / E", "turn (right-drag up/down tilts)"),
                        ("Click", "choose a tower"),
                        ("Click again · Enter · zoom in", "open it"),
                        ("Zoom out · Esc", "close it, then clear the choice"),
                        ("↑↓ / ←→ inside", "floors and slabs / pipes"),
                        ("Tab", "cycle the busiest processes"),
                        ("N", "next issue; inside: next anomaly"),
                        ("Home", "the whole map"),
                        ("/  or  Ctrl/⌘+K", "search"),
                        ("I · L · H", "inspector · process list · hide HUD"),
                        (
                            "1 2 3 4 5",
                            "family cables · kernel · IO conduits · labels · issues only",
                        ),
                        ("M", "reduced motion"),
                        ("Any key during start-up", "skip the arrival"),
                    ] {
                        ui.label(RichText::new(k).size(11.0).color(SIGNAL));
                        ui.label(RichText::new(v).size(11.0).color(SIGNAL_DIM));
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

fn spark_labeled(ui: &mut Ui, name: &str, v: &[f32], max: f32, c: Color32) {
    ui.horizontal(|ui| {
        ui.label(label(name));
        dot_sparkline(ui, v, max, c, vec2(110.0, 22.0), bw_scene::HISTORY_LEN);
        if let Some(last) = v.last() {
            let txt = if max > 100.0 {
                fmt_bytes(*last as u64)
            } else {
                format!("{last:.0}%")
            };
            ui.label(RichText::new(txt).size(10.0).color(c));
        }
    });
}

/// The inspector inside a process: the chosen part, then the anomalies.
fn element_inspector(root: &mut Ui, m: &Machine, ex: &mut bw_scene::explore::Explore) -> f32 {
    use bw_scene::interior::ElementKind;
    egui::Panel::right("inspector")
        .frame(bare_frame(14, 8))
        .show_separator_line(false)
        .default_size(330.0)
        .resizable(true)
        .show(root, |ui| {
            let Some(p) = ex.inside().and_then(|k| m.snapshot.processes.get(&k)) else {
                return;
            };
            ui.label(label("INSIDE"));
            ui.label(RichText::new(&p.name).font(semibold(16.0)).color(SELECT));
            if let Some(d) = &ex.detail {
                ui.label(
                    RichText::new(format!(
                        "{} threads · {} regions · {} descriptors",
                        d.threads.len(),
                        d.regions.len(),
                        d.fd_count
                    ))
                    .size(10.0)
                    .color(SIGNAL_DIM),
                );
                if d.restricted {
                    ui.label(
                        RichText::new("Some of it is hidden by the OS (another user's process).")
                            .size(10.0)
                            .color(SIGNAL_DIM),
                    );
                }
            } else {
                ui.label(
                    RichText::new("reading its internals…")
                        .size(10.0)
                        .color(SIGNAL_DIM),
                );
            }
            ui.add_space(10.0);
            if let (Some(e), Some(d)) = (
                ex.element
                    .and_then(|i| ex.interior.elements.get(i))
                    .cloned(),
                ex.detail.clone(),
            ) {
                let row = |ui: &mut Ui, k: &str, v: String| {
                    ui.label(label(k));
                    ui.label(RichText::new(v).size(12.0).color(SIGNAL));
                    ui.end_row();
                };
                egui::Grid::new("element")
                    .num_columns(2)
                    .spacing(vec2(12.0, 4.0))
                    .show(ui, |ui| match &e.kind {
                        ElementKind::Floor { thread } => {
                            let t = &d.threads[*thread];
                            row(ui, "THREAD", t.name.clone());
                            row(ui, "TID", t.tid.to_string());
                            row(ui, "STATE", format!("{:?}", t.state).to_lowercase());
                            row(ui, "CPU", format!("{:.1}%", t.cpu_pct));
                        }
                        ElementKind::Stratum { region } => {
                            let r = &d.regions[*region];
                            row(ui, "MEMORY", r.label.clone());
                            row(ui, "KIND", format!("{:?}", r.kind).to_lowercase());
                            row(ui, "SIZE", fmt_bytes(r.size_bytes));
                        }
                        ElementKind::Conduit { fd } => {
                            let f = &d.fds[*fd];
                            row(ui, "DESCRIPTOR", f.fd.to_string());
                            row(ui, "KIND", format!("{:?}", f.kind).to_lowercase());
                            row(ui, "TARGET", truncate(&f.target, 60));
                        }
                        ElementKind::Satellite { child } => {
                            if let Some(c) = m.snapshot.processes.get(child) {
                                row(ui, "CHILD", c.name.clone());
                                row(ui, "PID", c.key.pid.to_string());
                                row(ui, "CPU", format!("{:.1}%", c.cpu_pct));
                                row(ui, "MEMORY", fmt_bytes(c.mem_bytes));
                            }
                        }
                    });
                if let Some(a) = &e.anomaly {
                    ui.add_space(4.0);
                    ui.label(RichText::new(a).font(semibold(12.0)).color(
                        if e.health == Health::Critical {
                            ISSUE
                        } else {
                            WATCH
                        },
                    ));
                }
                ui.add_space(10.0);
            }
            ui.label(label(&format!("ANOMALIES ({})", ex.anomalies.len())));
            let list: Vec<(usize, String, Health)> = ex
                .anomalies
                .iter()
                .filter_map(|a| {
                    ex.interior
                        .elements
                        .iter()
                        .position(|e| e.kind == a.element)
                        .map(|i| (i, a.text.clone(), a.health))
                })
                .collect();
            if list.is_empty() {
                ui.label(
                    RichText::new("none found yet; growth needs a few samples")
                        .size(10.0)
                        .color(SIGNAL_DIM),
                );
            }
            for (i, text, h) in list {
                let c = if h == Health::Critical { ISSUE } else { WATCH };
                if ui
                    .add(
                        egui::Button::new(RichText::new(truncate(&text, 44)).size(11.0).color(c))
                            .frame(false),
                    )
                    .clicked()
                {
                    ex.choose(Some(i));
                }
            }
        })
        .response
        .rect
        .width()
}
