//! Small reusable pieces: avatars, icon buttons, section labels.

use egui::{vec2, Align2, Color32, FontId, Rect, Response, Sense, Stroke, Ui};

use super::icons::{self, Icon};
use super::theme;

/// A round avatar with the first letter of the name, a green ring while speaking
/// and an optional online dot.
pub fn avatar_at(
    ui: &Ui,
    rect: Rect,
    name: &str,
    color: Color32,
    speaking: bool,
    status: Option<bool>,
) {
    let p = ui.painter();
    let c = rect.center();
    let radius = rect.width() / 2.0;
    p.circle_filled(c, radius, color);
    let letter: String = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    p.text(
        c,
        Align2::CENTER_CENTER,
        letter,
        FontId::proportional(radius * 0.95),
        Color32::WHITE,
    );
    if speaking {
        p.circle_stroke(c, radius + 1.5, Stroke::new(2.5, theme::GREEN));
    }
    if let Some(online) = status {
        let dot = c + vec2(radius * 0.72, radius * 0.72);
        let dr = (radius * 0.32).max(4.0);
        p.circle_filled(dot, dr + 2.0, theme::color(theme::SIDEBAR));
        if online {
            p.circle_filled(dot, dr, theme::GREEN);
        } else {
            p.circle_filled(dot, dr, theme::TEXT_FAINT);
            p.circle_filled(dot, dr * 0.45, theme::color(theme::SIDEBAR));
        }
    }
}

pub fn avatar(
    ui: &mut Ui,
    size: f32,
    name: &str,
    color: Color32,
    speaking: bool,
    status: Option<bool>,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    avatar_at(ui, rect, name, color, speaking, status);
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!("Perfil de {name}"),
        )
    });
    resp
}

/// A square icon button with hover background and tooltip.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, color: Color32, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, theme::color(theme::HOVER));
    }
    let col = if resp.hovered() {
        theme::TEXT_BRIGHT.lerp_to_gamma(color, 0.4)
    } else {
        color
    };
    icons::paint(ui.painter(), rect.shrink(size * 0.18), icon, col);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            4,
            Stroke::new(2.0, theme::TEXT_BRIGHT),
            egui::StrokeKind::Outside,
        );
    }
    resp.on_hover_text(tip)
}

/// A call-control button (rounded rectangle).
pub fn round_button(ui: &mut Ui, icon: Icon, size: f32, bg: Color32, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size * 1.2, size * 0.85), Sense::click());
    let fill = if resp.hovered() {
        bg.lerp_to_gamma(Color32::WHITE, 0.12)
    } else {
        bg
    };
    ui.painter().rect_filled(rect, size * 0.28, fill);
    let side = size * 0.42;
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(side, side)),
        icon,
        Color32::WHITE,
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            size * 0.3,
            Stroke::new(2.0, theme::TEXT_BRIGHT),
            egui::StrokeKind::Outside,
        );
    }
    resp.on_hover_text(tip)
}

/// Upper-case grey label used above lists.
pub fn section_label(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.to_uppercase())
                .size(12.0)
                .strong()
                .color(theme::TEXT_MUTED),
        )
        .sense(Sense::click())
        .selectable(false),
    )
}

/// A filled accent button.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
            .fill(theme::ACCENT)
            .corner_radius(4)
            .min_size(vec2(96.0, 34.0)),
    )
}

/// Opens an http(s) link in the default browser. egui's own `open_url` does
/// nothing here because eframe is built without its `webbrowser` feature.
pub fn open_url(url: &str) {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return;
    }
    #[cfg(windows)]
    {
        let wide = |s: &str| {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        let (verb, target) = (wide("open"), wide(url));
        unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            );
        }
    }
}

/// A clickable link label that really opens the browser.
pub fn link(ui: &mut Ui, text: &str, url: &str) {
    if ui.link(text).on_hover_text(url).clicked() {
        open_url(url);
    }
}
