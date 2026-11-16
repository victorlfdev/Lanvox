//! Colours and sizes of the interface.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};
use std::sync::atomic::{AtomicU8, Ordering};

static PALETTE: AtomicU8 = AtomicU8::new(0);

/// Maps semantic tokens to the selected theme; status/accent colors remain stable.
pub fn color(base: Color32) -> Color32 {
    let selected = PALETTE.load(Ordering::Relaxed);
    let surfaces = [
        RAIL,
        PANEL,
        SIDEBAR,
        CHAT,
        INPUT,
        HOVER,
        SELECTED,
        DIVIDER,
        MESSAGE_HOVER,
        TILE_BG,
    ];
    let Some(index) = surfaces.iter().position(|c| *c == base) else {
        return base;
    };
    let values = match selected {
        1 => [
            0x0d0e10, 0x111214, 0x17181b, 0x1c1d21, 0x27282e, 0x303139, 0x3c3e47, 0x111214,
            0x23242a, 0x17181b,
        ],
        2 => [
            0x202225, 0x292b2f, 0x2f3136, 0x36393f, 0x40444b, 0x41454c, 0x4f545c, 0x292b2f,
            0x32353b, 0x202225,
        ],
        _ => return base,
    };
    let rgb = values[index];
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Changes appearance only when a preference changes, including in secondary viewports.
pub fn configure(ctx: &egui::Context, scale: f32, density: u8, palette: u8) {
    let state = (
        scale.clamp(0.8, 1.4).to_bits(),
        density.min(2),
        palette.min(2),
    );
    let id = egui::Id::unique("appearance-settings");
    if ctx.data(|d| d.get_temp::<(u32, u8, u8)>(id)) == Some(state) {
        return;
    }
    PALETTE.store(state.2, Ordering::Relaxed);
    ctx.set_zoom_factor(f32::from_bits(state.0));
    apply_style(ctx);
    ctx.all_styles_mut(|style| {
        let spacing = match state.1 {
            0 => 3.0,
            2 => 9.0,
            _ => 6.0,
        };
        style.spacing.item_spacing = egui::vec2(8.0, spacing);
        style.spacing.button_padding = egui::vec2(12.0, spacing);
        style.spacing.interact_size.y = match state.1 {
            0 => 22.0,
            2 => 32.0,
            _ => 27.0,
        };
    });
    ctx.data_mut(|d| d.insert_temp(id, state));
}

/// Surface colours of the default dark theme.
pub const SIDEBAR: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x31);
pub const RAIL: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x22);
pub const CHAT: Color32 = Color32::from_rgb(0x31, 0x33, 0x38);
pub const PANEL: Color32 = Color32::from_rgb(0x23, 0x24, 0x28);
pub const INPUT: Color32 = Color32::from_rgb(0x38, 0x3a, 0x40);
pub const HOVER: Color32 = Color32::from_rgb(0x35, 0x37, 0x3c);
pub const SELECTED: Color32 = Color32::from_rgb(0x40, 0x42, 0x49);
pub const CALL_BG: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const TILE_BG: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x22);

pub const TEXT: Color32 = Color32::from_rgb(0xdb, 0xde, 0xe1);
pub const TEXT_BRIGHT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0xa8, 0xad, 0xb5);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0xa8, 0xad, 0xb5);

pub const ACCENT: Color32 = Color32::from_rgb(0x2f, 0xc7, 0xb6);
pub const ACTION_FILL: Color32 = Color32::from_rgb(0x06, 0x67, 0x5f);
pub const GREEN: Color32 = Color32::from_rgb(0x23, 0xa5, 0x59);
pub const RED: Color32 = Color32::from_rgb(0xd4, 0x2e, 0x38);
/// Thin divider / header shadow line.
pub const DIVIDER: Color32 = Color32::from_rgb(0x26, 0x27, 0x2b);
/// Row background of a hovered chat message.
pub const MESSAGE_HOVER: Color32 = Color32::from_rgb(0x2e, 0x30, 0x35);
pub const LINK: Color32 = Color32::from_rgb(0x00, 0xa8, 0xfc);
pub const YELLOW: Color32 = Color32::from_rgb(0xfa, 0xa6, 0x1a);

pub const RAIL_W: f32 = 72.0;
pub const SIDEBAR_W: f32 = 240.0;
pub const MEMBERS_W: f32 = 240.0;
pub const HEADER_H: f32 = 48.0;
pub const USER_PANEL_H: f32 = 56.0;

pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    fn luminance(c: Color32) -> f32 {
        let linear = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    }
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Retain member-chosen hues while making text readable on the active surface.
pub fn readable_text(chosen: Color32, background: Color32) -> Color32 {
    for step in 0..=20 {
        let candidate = chosen.lerp_to_gamma(TEXT_BRIGHT, step as f32 / 20.0);
        if contrast_ratio(candidate, background) >= 4.5 {
            return candidate;
        }
    }
    TEXT_BRIGHT
}

pub fn on_color(background: Color32) -> Color32 {
    if contrast_ratio(Color32::BLACK, background) > contrast_ratio(Color32::WHITE, background) {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

pub fn r(v: u8) -> CornerRadius {
    CornerRadius::same(v)
}

/// Use the system emoji font (when present) as a fallback so reactions are not boxes.
fn load_emoji_font(ctx: &egui::Context) {
    let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\seguiemj.ttf") else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(body) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts.font_data.insert(
            "lanvox-body".into(),
            std::sync::Arc::new(egui::FontData::from_owned(body)),
        );
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "lanvox-body".into());
    }
    fonts.font_data.insert(
        "system-emoji".into(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("system-emoji".into());
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    load_emoji_font(ctx);
    apply_style(ctx);
}

fn apply_style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(20.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(14.0, FontFamily::Monospace),
            ),
            (
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(12.0, FontFamily::Proportional),
            ),
        ]
        .into();
        let v = &mut style.visuals;
        v.panel_fill = color(CHAT);
        v.window_fill = color(CHAT);
        v.extreme_bg_color = color(INPUT);
        v.faint_bg_color = color(SIDEBAR);
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = LINK;
        v.selection.bg_fill = ACTION_FILL;
        v.selection.stroke = Stroke::new(1.0, TEXT_BRIGHT);
        v.window_corner_radius = r(8);
        v.menu_corner_radius = r(6);
        v.popup_shadow = egui::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(110),
        };
        v.window_shadow = v.popup_shadow;
        v.widgets.noninteractive.corner_radius = r(4);
        v.window_stroke = Stroke::NONE;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, DIVIDER);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x4e, 0x50, 0x58);
        v.widgets.inactive.bg_fill = color(INPUT);
        v.widgets.inactive.corner_radius = r(4);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x6d, 0x6f, 0x78);
        v.widgets.hovered.bg_fill = color(SELECTED);
        v.widgets.hovered.corner_radius = r(4);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(0x80, 0x82, 0x8b);
        v.widgets.active.bg_fill = ACTION_FILL;
        v.widgets.active.corner_radius = r(4);
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.hovered.bg_stroke = Stroke::NONE;
        v.widgets.active.bg_stroke = Stroke::NONE;
        v.widgets.open.weak_bg_fill = color(SELECTED);
        v.widgets.open.bg_fill = color(SELECTED);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_BRIGHT);
        v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT_BRIGHT);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.scroll.floating = true;
    });
}
