//! Colours and visuals (the legacy css/style.css `:root` palette).

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};
use ed_core::Level;

pub const BG: Color32 = Color32::from_rgb(0x07, 0x0a, 0x10);
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(0x0d, 0x12, 0x1b, 0xf2);
pub const CARD: Color32 = Color32::from_rgb(0x13, 0x1a, 0x26);
pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xed, 0xf3);
pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x96, 0xa8);
pub const ACCENT: Color32 = Color32::from_rgb(0xf4, 0xb9, 0x4a);
pub const ALERT: Color32 = Color32::from_rgb(0xff, 0x6b, 0x5e);
pub const WATCH: Color32 = Color32::from_rgb(0xf4, 0xb9, 0x4a);
pub const OK: Color32 = Color32::from_rgb(0x7b, 0xd8, 0x8f);
pub const WATER: Color32 = Color32::from_rgb(0x5c, 0xc8, 0xf0);
pub const VIOLET: Color32 = Color32::from_rgb(0xc9, 0xa7, 0xff);
pub const GREY: Color32 = Color32::from_rgb(0x9a, 0xa6, 0xb8);

pub fn level(l: Level) -> Color32 {
    match l {
        Level::Alert => ALERT,
        Level::Watch => WATCH,
        Level::Ok => OK,
    }
}

/// Marker colour per point kind.
pub fn kind(k: &str) -> Color32 {
    match k {
        "subsidence" => ALERT,
        "water" | "flood" => WATER,
        "crop" => OK,
        "urban" => VIOLET,
        _ => GREY,
    }
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn install(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_rgb(0x0d, 0x12, 0x1b);
    v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0e, 0x15);
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0xf4, 0xb9, 0x4a, 0x55);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(14);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(24));
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_white_alpha(10);
    v.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(22);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}
