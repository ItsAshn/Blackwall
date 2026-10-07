//! The cyberpunk theme, shared by every panel. One place to change the look,
//! with high-contrast colors chosen to stay legible over bright bloom.

use bevy_egui::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke};

pub const CYAN: Color32 = Color32::from_rgb(90, 170, 255);
/// Health colors, matching the scene (bw_scene::health_color).
pub const BLUE: Color32 = Color32::from_rgb(70, 150, 255);
pub const VIOLET: Color32 = Color32::from_rgb(190, 90, 255);
pub const INDIGO: Color32 = Color32::from_rgb(130, 110, 255);
pub const RED: Color32 = Color32::from_rgb(255, 45, 60);
pub const AMBER: Color32 = Color32::from_rgb(255, 176, 40);
pub const GREEN: Color32 = Color32::from_rgb(60, 255, 150);
pub const TEXT: Color32 = Color32::from_rgb(205, 215, 235);
pub const DIM: Color32 = Color32::from_rgb(120, 130, 160);

/// Green → amber → red for a 0..1 load.
pub fn heat(x: f32) -> Color32 {
    match x {
        x if x >= 0.9 => RED,
        x if x >= 0.7 => AMBER,
        _ => GREEN,
    }
}

pub fn bar_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(5, 3, 14, 215))
        .inner_margin(Margin::symmetric(10, 6))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(40, 220, 255, 60),
        ))
}

pub fn side_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(5, 3, 14, 200))
        .inner_margin(Margin::same(10))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(40, 220, 255, 45),
        ))
}

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgba_unmultiplied(5, 3, 14, 210);
    v.window_fill = Color32::from_rgba_unmultiplied(8, 5, 20, 240);
    v.window_stroke = Stroke::new(1.0, CYAN);
    v.window_corner_radius = CornerRadius::same(2);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = CYAN;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(40, 220, 255, 70);
    v.selection.stroke = Stroke::new(1.0, CYAN);
    v.extreme_bg_color = Color32::from_rgba_unmultiplied(0, 0, 0, 160);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(2);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_rgba_unmultiplied(40, 220, 255, 18);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgba_unmultiplied(40, 220, 255, 45);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, CYAN);
    v.widgets.active.weak_bg_fill = Color32::from_rgba_unmultiplied(255, 40, 70, 80);
    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        // Monospace everywhere: it reads like a terminal and aligns numbers.
        for f in s.text_styles.values_mut() {
            f.family = egui::FontFamily::Monospace;
        }
        s.spacing.item_spacing = egui::vec2(6.0, 3.0);
    });
}
