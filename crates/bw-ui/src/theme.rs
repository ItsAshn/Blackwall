//! The HUD's look, from the design system: words floating on pure black,
//! health shown as painted glyphs and dot meters, Doto for the name and
//! Martian Mono for everything else. Colors come from `bw_scene::palette`.

use bevy_egui::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, Pos2, Rect,
    Stroke, Ui, epaint, vec2,
};
use bw_model::{Health, Process, Realm};
use bw_scene::palette as p;
use std::sync::Arc;

const fn c(hex: u32) -> Color32 {
    let [r, g, b] = p::srgb8(hex);
    Color32::from_rgb(r, g, b)
}

pub const VOID: Color32 = c(p::VOID);
pub const SIGNAL: Color32 = c(p::SIGNAL);
pub const SIGNAL_DIM: Color32 = c(p::SIGNAL_DIM);
pub const SELECT: Color32 = c(p::SELECT);
pub const OK: Color32 = c(p::HEALTH_OK);
pub const WATCH: Color32 = c(p::HEALTH_WATCH);
pub const ISSUE: Color32 = c(p::HEALTH_ISSUE);
pub const KERNEL: Color32 = c(p::KERNEL);
pub const WALL_CALM: Color32 = c(p::WALL_CALM);
pub const WALL_HOT: Color32 = c(p::WALL_HOT);
pub const DOT_OFF: Color32 = c(p::DOT_OFF);

pub const DISPLAY: &str = "display";
pub const SEMIBOLD: &str = "semibold";

/// Regions of the HUD have no fill and no border: only position and space.
pub fn bare_frame(x: i8, y: i8) -> Frame {
    Frame::new()
        .fill(Color32::TRANSPARENT)
        .inner_margin(Margin::symmetric(x, y))
        .stroke(Stroke::NONE)
}

pub fn display(size: f32) -> egui::FontId {
    egui::FontId::new(size, FontFamily::Name(DISPLAY.into()))
}

pub fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn apply(ctx: &egui::Context) {
    // Bundled faces (SIL OFL, see assets/fonts) so text is identical on every OS.
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        &mut fonts,
        "MartianMono",
        include_bytes!("../assets/fonts/MartianMono-Regular.ttf"),
    );
    add(
        &mut fonts,
        "MartianMono-SemiBold",
        include_bytes!("../assets/fonts/MartianMono-SemiBold.ttf"),
    );
    add(
        &mut fonts,
        "Doto",
        include_bytes!("../assets/fonts/Doto-Black.ttf"),
    );
    // egui's own fonts stay behind as fallbacks for symbols Martian Mono lacks.
    let fallback = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let chain = |first: &[&str]| {
        first
            .iter()
            .map(|s| s.to_string())
            .chain(fallback.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Monospace, chain(&["MartianMono"]));
    fonts
        .families
        .insert(FontFamily::Proportional, chain(&["MartianMono"]));
    fonts.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        chain(&["MartianMono-SemiBold", "MartianMono"]),
    );
    fonts.families.insert(
        FontFamily::Name(DISPLAY.into()),
        chain(&["Doto", "MartianMono"]),
    );
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = VOID;
    v.window_stroke = Stroke::new(1.0, SIGNAL_DIM);
    v.window_corner_radius = CornerRadius::ZERO;
    v.window_shadow = epaint::Shadow::NONE;
    v.popup_shadow = epaint::Shadow::NONE;
    v.override_text_color = Some(SIGNAL);
    v.hyperlink_color = SIGNAL;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(79, 195, 255, 60);
    v.selection.stroke = Stroke::new(1.0, SELECT);
    v.extreme_bg_color = VOID;
    v.faint_bg_color = VOID;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::ZERO;
        w.bg_fill = Color32::TRANSPARENT;
        w.weak_bg_fill = Color32::TRANSPARENT;
        w.bg_stroke = Stroke::NONE;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, SIGNAL);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, SIGNAL);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, SELECT);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, SIGNAL_DIM);
    v.widgets.active.fg_stroke = Stroke::new(1.0, SELECT);
    v.widgets.active.bg_stroke = Stroke::new(1.0, SELECT);
    v.widgets.open.bg_stroke = Stroke::new(1.0, SIGNAL_DIM);
    // The text cursor and focus ring are solid signal white.
    v.text_cursor.stroke = Stroke::new(1.5, SIGNAL);
    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        for f in s.text_styles.values_mut() {
            f.family = FontFamily::Monospace;
        }
        s.spacing.item_spacing = vec2(8.0, 4.0);
        s.spacing.button_padding = vec2(4.0, 1.0);
    });
}

/// The health glyph's shape: one per state, so lists read without color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// ■ healthy
    Square,
    /// ◆ worth watching
    Diamond,
    /// ✕ issue
    Cross,
    /// ▲ kernel thread
    Triangle,
}

pub fn glyph_of(p: &Process) -> (Glyph, Color32) {
    match (p.health(), p.realm) {
        (Health::Critical, _) => (Glyph::Cross, ISSUE),
        (Health::Warning, _) => (Glyph::Diamond, WATCH),
        (Health::Healthy, Realm::Kernel) => (Glyph::Triangle, KERNEL),
        (Health::Healthy, Realm::User) => (Glyph::Square, OK),
    }
}

/// Paint a glyph centered in `r`.
pub fn paint_glyph(painter: &egui::Painter, r: Rect, g: Glyph, color: Color32) {
    let ctr = r.center();
    let s = r.height().min(r.width()) * 0.32;
    match g {
        Glyph::Square => {
            painter.rect_filled(
                Rect::from_center_size(ctr, vec2(s * 1.6, s * 1.6)),
                0.0,
                color,
            );
        }
        Glyph::Diamond => {
            let pts = vec![
                ctr + vec2(0.0, -s * 1.15),
                ctr + vec2(s * 1.15, 0.0),
                ctr + vec2(0.0, s * 1.15),
                ctr + vec2(-s * 1.15, 0.0),
            ];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Glyph::Cross => {
            let st = Stroke::new(s * 0.55, color);
            painter.line_segment([ctr + vec2(-s, -s), ctr + vec2(s, s)], st);
            painter.line_segment([ctr + vec2(-s, s), ctr + vec2(s, -s)], st);
        }
        Glyph::Triangle => {
            let pts = vec![
                ctr + vec2(0.0, -s * 1.1),
                ctr + vec2(s * 1.15, s * 0.9),
                ctr + vec2(-s * 1.15, s * 0.9),
            ];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
    }
}

/// A glyph laid out inline, the size of a line of body text.
pub fn glyph(ui: &mut Ui, g: Glyph, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(12.0, 14.0), egui::Sense::hover());
    paint_glyph(ui.painter(), r, g, color);
}

/// A meter made of dots: lit in `color`, unlit in `DOT_OFF`.
pub fn dot_meter(ui: &mut Ui, frac: f32, dots: usize, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(dots as f32 * 6.0, 10.0), egui::Sense::hover());
    let lit = (frac.clamp(0.0, 1.0) * dots as f32).round() as usize;
    for i in 0..dots {
        let x = r.left() + i as f32 * 6.0 + 1.0;
        let d = Rect::from_min_size(Pos2::new(x, r.center().y - 2.0), vec2(4.0, 4.0));
        if i < lit {
            ui.painter()
                .rect_filled(d.expand(1.5), 0.0, color.gamma_multiply(0.18));
            ui.painter().rect_filled(d, 0.0, color);
        } else {
            ui.painter().rect_filled(d, 0.0, DOT_OFF);
        }
    }
}

/// A sparkline as one dot per sample; the newest dot burns white.
pub fn dot_sparkline(
    ui: &mut Ui,
    v: &[f32],
    max: f32,
    color: Color32,
    size: egui::Vec2,
    history: usize,
) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if v.is_empty() {
        return;
    }
    let n = history.max(v.len()) as f32;
    let start = n - v.len() as f32;
    let painter = ui.painter();
    for (i, y) in v.iter().enumerate() {
        let x = rect.left() + (start + i as f32) / (n - 1.0).max(1.0) * (rect.width() - 2.0);
        let y = rect.bottom() - 2.0 - (y / max.max(1e-6)).clamp(0.0, 1.0) * (rect.height() - 3.0);
        let last = i + 1 == v.len();
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x, y), vec2(1.6, 1.6)),
            0.0,
            if last { SELECT } else { color },
        );
    }
}

/// Text with a soft halo, like the scene's glowing dots (design system:
/// `glow-signal`). egui has no text shadows, so the halo is the text itself,
/// faint, drawn at small offsets beneath.
pub fn glow_text(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: egui::Align2,
    text: &str,
    font: egui::FontId,
    color: Color32,
    halo: Color32,
) {
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        painter.text(
            pos + vec2(dx, dy),
            anchor,
            text,
            font.clone(),
            halo.gamma_multiply(0.22),
        );
    }
    painter.text(pos, anchor, text, font, color);
}

/// Which edge of a HUD column is darkest.
#[derive(Clone, Copy)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// Darken the scene toward the void beneath a HUD column, so words stay
/// readable without a panel: black at `edge`, clear at the opposite side.
pub fn fade(painter: &egui::Painter, rect: Rect, edge: Edge) {
    let mut mesh = epaint::Mesh::default();
    let black = |a: f32| Color32::from_black_alpha((a * 255.0) as u8);
    let (d, z) = (0.9, 0.0);
    // Corners in order: left-top, right-top, right-bottom, left-bottom.
    let a = match edge {
        Edge::Left => [d, z, z, d],
        Edge::Right => [z, d, d, z],
        Edge::Top => [d, d, z, z],
        Edge::Bottom => [z, z, d, d],
    };
    for (pos, alpha) in [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ]
    .into_iter()
    .zip(a)
    {
        mesh.colored_vertex(pos, black(alpha));
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}
