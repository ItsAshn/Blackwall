//! The quiet HUD: almost nothing on screen until it's asked for.
//!
//! * a breadcrumb of where you are (machine › process › element)
//! * a whisper beside whatever you're looking at: two or three lines
//! * a compass of chevrons at the screen's edge pointing at anomalies you
//!   can't see yet, so a hunt always has a lead
//! * one line of context hints
//! * the dive's fade to black
//! * a search summoned with `/`: processes outside, the process's own
//!   threads, libraries and descriptors inside

use crate::theme::*;
use crate::{UiState, fmt_bytes, truncate};
use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Pos2, RichText, vec2};
use bw_model::{Health, Process};
use bw_scene::explore::Explore;
use bw_scene::interior::ElementKind;
use bw_scene::{Machine, SceneLayout, Selection};

fn health_color(h: Health) -> Color32 {
    match h {
        Health::Critical => ISSUE,
        Health::Warning => WATCH,
        Health::Healthy => OK,
    }
}

fn project(camera: &Camera, cam_tf: &GlobalTransform, p: Vec3) -> Option<Pos2> {
    // Behind the camera counts as off-screen.
    let fwd = cam_tf.forward();
    if (p - cam_tf.translation()).dot(*fwd) <= 0.0 {
        return None;
    }
    camera
        .world_to_viewport(cam_tf, p)
        .ok()
        .map(|v| Pos2::new(v.x, v.y))
}

pub fn breadcrumb(ctx: &egui::Context, m: &Machine, sel: &Selection, ex: &Explore) {
    let painter = ctx.layer_painter(egui::LayerId::background());
    let mut x = 16.0;
    let y = 16.0;
    let r = painter.text(
        Pos2::new(x, y),
        egui::Align2::LEFT_TOP,
        "BLACKWALL",
        display(16.0),
        SIGNAL,
    );
    x = r.right() + 12.0;
    let mut crumbs: Vec<(String, Color32)> =
        vec![(m.snapshot.host.hostname.to_uppercase(), SIGNAL_DIM)];
    if m.snapshot.host.source == "demo" {
        crumbs[0].0.push_str(" · DEMO DATA");
    }
    let focus = ex.inside().or(sel.key);
    if let Some(p) = focus.and_then(|k| m.snapshot.processes.get(&k)) {
        crumbs.push((
            format!("{} ({})", p.name, p.key.pid),
            if ex.inside().is_some() {
                SIGNAL
            } else {
                SIGNAL_DIM
            },
        ));
    }
    if let Some(e) = ex.element.and_then(|i| ex.interior.elements.get(i)) {
        let what = match &e.kind {
            ElementKind::Floor { .. } => "thread",
            ElementKind::Stratum { .. } => "memory",
            ElementKind::Conduit { .. } => "descriptor",
            ElementKind::Satellite { .. } => "child",
        };
        crumbs.push((
            format!(
                "{what} {}",
                truncate(e.label.split(" · ").next().unwrap_or(""), 28)
            ),
            SIGNAL,
        ));
    }
    for (i, (c, col)) in crumbs.iter().enumerate() {
        if i > 0 {
            let r = painter.text(
                Pos2::new(x, y + 2.0),
                egui::Align2::LEFT_TOP,
                "›",
                egui::FontId::monospace(11.0),
                SIGNAL_DIM,
            );
            x = r.right() + 8.0;
        }
        let r = painter.text(
            Pos2::new(x, y + 2.0),
            egui::Align2::LEFT_TOP,
            c,
            egui::FontId::monospace(11.0),
            *col,
        );
        x = r.right() + 8.0;
    }
}

/// Lines of a whisper for a process tower at the machine level.
fn process_lines(p: &Process) -> Vec<(String, Color32)> {
    let mut v = vec![
        (p.name.clone(), SELECT),
        (
            format!(
                "{:.1}% CPU · {} · {} threads",
                p.cpu_pct,
                fmt_bytes(p.mem_bytes),
                p.threads.unwrap_or(1)
            ),
            SIGNAL_DIM,
        ),
    ];
    if let Some(r) = p.health_reason() {
        v.push((r.to_string(), health_color(p.health())));
    }
    v
}

/// Text cut out of the scene: a black halo behind it, so it stays readable
/// over bright dots (black is nothing, so the halo adds no grey).
fn cut_text(
    painter: &egui::Painter,
    pos: Pos2,
    text: &str,
    font: egui::FontId,
    color: Color32,
) -> egui::Rect {
    for (dx, dy) in [
        (-1.5, 0.0),
        (1.5, 0.0),
        (0.0, -1.5),
        (0.0, 1.5),
        (-1.0, -1.0),
        (1.0, 1.0),
        (-1.0, 1.0),
        (1.0, -1.0),
    ] {
        painter.text(
            pos + vec2(dx, dy),
            egui::Align2::LEFT_TOP,
            text,
            font.clone(),
            Color32::BLACK,
        );
    }
    painter.text(pos, egui::Align2::LEFT_TOP, text, font, color)
}

fn whisper_at(painter: &egui::Painter, anchor: Pos2, lines: &[(String, Color32)], strong: bool) {
    // A short tick from the anchor, then the lines to its right.
    let start = anchor + vec2(10.0, -10.0);
    painter.line_segment(
        [anchor, start],
        egui::Stroke::new(1.0, if strong { SELECT } else { SIGNAL_DIM }),
    );
    let mut y = start.y - 6.0;
    for (i, (text, color)) in lines.iter().enumerate() {
        let font = if i == 0 {
            semibold(if strong { 16.0 } else { 13.0 })
        } else {
            egui::FontId::monospace(12.0)
        };
        let r = cut_text(painter, Pos2::new(start.x + 4.0, y), text, font, *color);
        y = r.bottom() + 2.0;
    }
}

pub fn whisper(
    ctx: &egui::Context,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    m: &Machine,
    sel: &Selection,
    ex: &Explore,
    sl: &SceneLayout,
) {
    let painter = ctx.layer_painter(egui::LayerId::background());
    match ex.inside() {
        None => {
            let mut shown = Vec::new();
            for (k, strong) in [
                (sel.key, true),
                (sel.hovered.filter(|h| Some(*h) != sel.key), false),
            ] {
                let Some(k) = k else { continue };
                let (Some(p), Some(c)) = (m.snapshot.processes.get(&k), sl.layout.column(&k))
                else {
                    continue;
                };
                if let Some(pos) = project(camera, cam_tf, c.top() + Vec3::Y * 0.3) {
                    let mut lines = process_lines(p);
                    if !strong {
                        lines.truncate(2);
                    }
                    whisper_at(&painter, pos, &lines, strong);
                    shown.push(k);
                }
            }
        }
        Some(_) => {
            for (i, strong) in [
                (ex.element, true),
                (ex.hovered.filter(|h| Some(*h) != ex.element), false),
            ] {
                let Some(e) = i.and_then(|i| ex.interior.elements.get(i)) else {
                    continue;
                };
                let Some(pos) = project(camera, cam_tf, e.anchor) else {
                    continue;
                };
                let mut parts = e.label.split(" · ");
                let mut lines = vec![(
                    parts.next().unwrap_or("").to_string(),
                    if strong { SELECT } else { SIGNAL },
                )];
                let rest: Vec<&str> = parts.collect();
                if !rest.is_empty() {
                    lines.push((rest.join(" · "), SIGNAL_DIM));
                }
                if let Some(a) = &e.anomaly {
                    lines.push((a.clone(), health_color(e.health)));
                }
                if strong && matches!(e.kind, ElementKind::Satellite { .. }) {
                    lines.push(("Enter or click again to dive in".into(), SIGNAL_DIM));
                }
                whisper_at(&painter, pos, &lines, strong);
            }
        }
    }
}

/// Chevrons at the screen's edge toward anomalies outside the view.
pub fn compass(ctx: &egui::Context, camera: &Camera, cam_tf: &GlobalTransform, ex: &Explore) {
    if ex.inside().is_none() {
        return;
    }
    let screen = ctx.viewport_rect().shrink(28.0);
    let center = screen.center();
    let painter = ctx.layer_painter(egui::LayerId::background());
    for a in &ex.anomalies {
        let Some(e) = ex.interior.elements.iter().find(|e| e.kind == a.element) else {
            continue;
        };
        if Some(&a.element)
            == ex
                .element
                .and_then(|i| ex.interior.elements.get(i))
                .map(|e| &e.kind)
        {
            continue;
        }
        // Direction in screen space, from the camera's view of the anchor.
        let local = cam_tf.affine().inverse().transform_point3(e.anchor);
        let mut dir = vec2(local.x, -local.y);
        let behind = local.z > 0.0;
        if behind {
            dir = -dir;
        }
        if let Some(p) = project(camera, cam_tf, e.anchor).filter(|p| screen.contains(*p)) {
            // On screen: a small marker ring instead.
            painter.rect_stroke(
                egui::Rect::from_center_size(p, vec2(14.0, 14.0)),
                0.0,
                egui::Stroke::new(1.0, health_color(a.health)),
                egui::StrokeKind::Outside,
            );
            continue;
        }
        let d = if dir.length() < 1e-3 {
            vec2(0.0, 1.0)
        } else {
            dir.normalized()
        };
        let t = (screen.width() / 2.0 / d.x.abs().max(1e-3))
            .min(screen.height() / 2.0 / d.y.abs().max(1e-3));
        let tip = center + d * t;
        let side = vec2(-d.y, d.x) * 6.0;
        let pts = vec![tip, tip - d * 10.0 + side, tip - d * 10.0 - side];
        painter.add(egui::Shape::convex_polygon(
            pts,
            health_color(a.health),
            egui::Stroke::NONE,
        ));
    }
}

pub fn hint(ctx: &egui::Context, ex: &Explore, sel: &Selection) {
    let screen = ctx.viewport_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    let text = match ex.inside() {
        Some(_) => {
            let n = ex.anomalies.len();
            let lead = match n {
                0 => "no anomalies found".to_string(),
                1 => "1 anomaly · N to visit".to_string(),
                n => format!("{n} anomalies · N to visit"),
            };
            format!("{lead}   ↑↓ floors and slabs · ←→ pipes · zoom out or Esc to close")
        }
        None if sel.key.is_some() => {
            "zoom in, Enter or click again to open · drag to pan · N next issue · Esc back"
                .to_string()
        }
        None => "drag to pan · scroll to zoom · click a tower · N next issue · ? help".to_string(),
    };
    let font = egui::FontId::monospace(10.0);
    let w = painter
        .layout_no_wrap(text.clone(), font.clone(), SIGNAL_DIM)
        .rect
        .width();
    cut_text(
        &painter,
        Pos2::new(screen.center().x - w / 2.0, screen.bottom() - 30.0),
        &text,
        font,
        SIGNAL_DIM,
    );
}

pub fn fade_overlay(ctx: &egui::Context, ex: &Explore) {
    if ex.fade <= 0.0 {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("bw-dive-fade"),
    ));
    painter.rect_filled(
        ctx.viewport_rect(),
        0.0,
        Color32::from_black_alpha((ex.fade * 255.0) as u8),
    );
}

/// The search overlay. Outside: processes. Inside: the process's own parts.
pub fn search(
    ctx: &egui::Context,
    st: &mut UiState,
    m: &Machine,
    sel: &mut Selection,
    ex: &mut Explore,
) {
    if !st.search_open {
        return;
    }
    let mut close = false;
    egui::Area::new("bw-search".into())
        .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 120.0))
        .show(ctx, |ui| {
            ui.set_width(460.0);
            let hint = if ex.inside().is_some() {
                "search threads, libraries, files, sockets"
            } else {
                "search processes by name, pid or user"
            };
            let edit = ui.add(
                egui::TextEdit::singleline(&mut st.search)
                    .hint_text(RichText::new(hint).color(SIGNAL_DIM))
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE)
                    .font(egui::FontId::monospace(15.0)),
            );
            if st.focus_search {
                edit.request_focus();
                st.focus_search = false;
            }
            // A dotted underline instead of a box.
            let r = edit.rect;
            let mut x = r.left();
            while x < r.right() {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(Pos2::new(x, r.bottom() + 2.0), vec2(2.0, 1.0)),
                    0.0,
                    SIGNAL,
                );
                x += 4.0;
            }
            ui.add_space(8.0);
            let q = st.search.to_lowercase();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
            if q.is_empty() {
                return;
            }
            match ex.inside() {
                Some(_) => {
                    let hits: Vec<usize> = ex
                        .interior
                        .elements
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| {
                            e.label.to_lowercase().contains(&q)
                                || e.anomaly
                                    .as_deref()
                                    .is_some_and(|a| a.to_lowercase().contains(&q))
                        })
                        .map(|(i, _)| i)
                        .take(8)
                        .collect();
                    for (n, i) in hits.iter().enumerate() {
                        let e = &ex.interior.elements[*i];
                        let resp = ui.add(
                            egui::Button::new(
                                RichText::new(truncate(&e.label, 52))
                                    .size(12.0)
                                    .color(health_color_text(e.health)),
                            )
                            .frame(false),
                        );
                        if resp.clicked() || (enter && n == 0) {
                            ex.choose(Some(*i));
                            close = true;
                        }
                    }
                }
                None => {
                    let mut hits: Vec<&Process> = m
                        .snapshot
                        .processes
                        .values()
                        .filter(|p| {
                            p.name.to_lowercase().contains(&q)
                                || p.key.pid.to_string() == q
                                || p.user.as_deref().is_some_and(|u| u.to_lowercase() == q)
                        })
                        .collect();
                    hits.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
                    for (n, p) in hits.iter().take(8).enumerate() {
                        let resp = ui
                            .horizontal(|ui| {
                                let (g, c) = glyph_of(p);
                                glyph(ui, g, c);
                                ui.add(
                                    egui::Button::new(
                                        RichText::new(format!(
                                            "{}  {:.1}%  {}",
                                            truncate(&p.name, 30),
                                            p.cpu_pct,
                                            fmt_bytes(p.mem_bytes)
                                        ))
                                        .size(12.0)
                                        .color(SIGNAL),
                                    )
                                    .frame(false),
                                )
                            })
                            .inner;
                        if resp.clicked() || (enter && n == 0) {
                            sel.key = Some(p.key);
                            sel.follow = true;
                            close = true;
                        }
                    }
                }
            }
        });
    if close {
        st.search_open = false;
        st.search.clear();
    }
}

fn health_color_text(h: Health) -> Color32 {
    match h {
        Health::Healthy => SIGNAL,
        other => health_color(other),
    }
}
