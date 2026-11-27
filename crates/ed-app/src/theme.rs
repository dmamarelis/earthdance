//! Colours and visuals (the legacy css/style.css `:root` palette).

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};
use ed_core::Level;

pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x13);
pub const PANEL: Color32 = Color32::from_rgb(0x11, 0x15, 0x1c);
pub const CARD: Color32 = Color32::from_rgb(0x17, 0x1c, 0x25);
pub const TEXT: Color32 = Color32::from_rgb(0xdd, 0xe2, 0xea);
pub const MUTED: Color32 = Color32::from_rgb(0x87, 0x91, 0xa1);
pub const ACCENT: Color32 = Color32::from_rgb(0xc9, 0xa8, 0x6e);
pub const ALERT: Color32 = Color32::from_rgb(0xc8, 0x6f, 0x64);
pub const WATCH: Color32 = Color32::from_rgb(0xc9, 0x9f, 0x5e);
pub const OK: Color32 = Color32::from_rgb(0x7f, 0xa8, 0x88);
pub const WATER: Color32 = Color32::from_rgb(0x72, 0xa5, 0xbd);
pub const VIOLET: Color32 = Color32::from_rgb(0xa3, 0x98, 0xc4);
pub const GREY: Color32 = Color32::from_rgb(0x8c, 0x96, 0xa6);

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

/// Symbol fallback from the system (egui's bundled fonts lack ☁ ⚠ ✓ ⌂ ♫ …). Missing files are skipped.
fn symbol_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 4] = [
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/google-noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
    ];
    let mut fonts = egui::FontDefinitions::default();
    for (i, p) in CANDIDATES.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(p) {
            let name = format!("fallback{i}");
            fonts.font_data.insert(name.clone(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(name.clone());
            }
        }
    }
    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    symbol_fonts(ctx);
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0e, 0x15);
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0xc9, 0xa8, 0x6e, 0x40);
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
