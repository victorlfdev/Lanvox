//! Shared building blocks for modals, floating windows, forms and profile cards.
//!
//! Everything here is presentation only: callers keep their own state and
//! decide what the returned flags mean.

use egui::{vec2, Align, Color32, Frame, Layout, Margin, Response, RichText, Ui};

use super::icons::Icon;
use super::{theme, widgets};

/// Smallest body height we allow (the interface is designed to fit 940x500).
const MIN_BODY_H: f32 = 120.0;
/// Vertical space the modal chrome (margins, title, footer, window padding) needs.
const CHROME_H: f32 = 165.0;

/// Width for a modal: the preferred width, shrunk to fit narrow windows.
pub fn modal_width(ctx: &egui::Context, preferred: f32) -> f32 {
    (ctx.content_rect().width() - 90.0).clamp(300.0_f32.min(preferred), preferred)
}

/// Height available to the scrollable body of a modal.
pub fn body_max_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - CHROME_H).clamp(MIN_BODY_H, 570.0)
}

pub fn modal_frame() -> Frame {
    Frame::new()
        .fill(theme::color(theme::CHAT))
        .corner_radius(8)
        .inner_margin(Margin::same(20))
}

// ---------------------------------------------------------------- buttons

/// Filled accent button for the main action.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(theme::color(theme::ACCENT))
            .corner_radius(4)
            .min_size(vec2(96.0, 34.0)),
    )
}

/// Filled red button for destructive actions.
pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(Color32::from_rgb(0xc4, 0x26, 0x2b))
            .corner_radius(4)
            .min_size(vec2(96.0, 34.0)),
    )
}

/// Neutral button for cancel and secondary actions.
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(theme::color(theme::TEXT_BRIGHT)))
            .fill(theme::color(theme::INPUT))
            .corner_radius(4)
            .min_size(vec2(80.0, 34.0)),
    )
}

// ---------------------------------------------------------------- forms

/// A label above a control.
pub fn form_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    widgets::section_label(ui, label);
    ui.add_space(2.0);
    let out = add(ui);
    ui.add_space(8.0);
    out
}

/// A label and a control on one line (for short controls such as numbers).
pub fn form_row_inline<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(theme::color(theme::TEXT_MUTED)));
        add(ui)
    })
    .inner
}

/// A full-width single-line text field with the standard inner margin.
pub fn text_field(ui: &mut Ui, value: &mut String, hint: &str, limit: usize) -> Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .char_limit(limit)
            .desired_width(f32::INFINITY)
            .margin(Margin::symmetric(10, 8)),
    )
}

// ---------------------------------------------------------------- modals

/// What the user did in a [`modal`] this frame.
#[derive(Default, Clone, Copy)]
pub struct ModalResult {
    pub primary: bool,
    pub secondary: bool,
    /// Close button, Escape or a click outside the modal.
    pub dismissed: bool,
}

impl ModalResult {
    /// The modal should go away without applying anything.
    pub fn cancelled(&self) -> bool {
        self.secondary || self.dismissed
    }
}

#[derive(Clone, Copy)]
pub struct ModalOptions<'a> {
    pub width: f32,
    pub subtitle: Option<&'a str>,
    pub primary: Option<&'a str>,
    pub primary_enabled: bool,
    pub primary_danger: bool,
    pub secondary: Option<&'a str>,
}

impl<'a> ModalOptions<'a> {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            subtitle: None,
            primary: None,
            primary_enabled: true,
            primary_danger: false,
            secondary: None,
        }
    }
    pub fn subtitle(mut self, text: &'a str) -> Self {
        self.subtitle = Some(text);
        self
    }
    pub fn primary(mut self, label: &'a str) -> Self {
        self.primary = Some(label);
        self
    }
    pub fn danger(mut self) -> Self {
        self.primary_danger = true;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.primary_enabled = enabled;
        self
    }
    pub fn secondary(mut self, label: &'a str) -> Self {
        self.secondary = Some(label);
        self
    }
}

/// Standard modal: title with close button, scrollable body, footer with
/// primary (right) and secondary actions. Width adapts to the window and the
/// body scrolls so it stays usable at 940x500.
pub fn modal(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    opts: ModalOptions<'_>,
    body: impl FnOnce(&mut Ui),
) -> ModalResult {
    let mut result = ModalResult::default();
    let width = modal_width(ctx, opts.width);
    let max_h = body_max_height(ctx);
    let response = egui::Modal::new(egui::Id::unique(id))
        .frame(modal_frame())
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .size(20.0)
                        .strong()
                        .color(theme::color(theme::TEXT_BRIGHT)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(
                        ui,
                        Icon::Close,
                        28.0,
                        theme::color(theme::TEXT),
                        "Fechar",
                    )
                    .clicked()
                    {
                        result.dismissed = true;
                    }
                });
            });
            if let Some(sub) = opts.subtitle {
                ui.label(
                    RichText::new(sub)
                        .size(13.0)
                        .color(theme::color(theme::TEXT_MUTED)),
                );
            }
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt("modal-body")
                .max_height(max_h)
                .auto_shrink([false, true])
                .show(ui, body);
            if opts.primary.is_some() || opts.secondary.is_some() {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(label) = opts.primary {
                        let clicked = ui
                            .add_enabled_ui(opts.primary_enabled, |ui| {
                                if opts.primary_danger {
                                    danger_button(ui, label)
                                } else {
                                    primary_button(ui, label)
                                }
                            })
                            .inner
                            .clicked();
                        result.primary |= clicked;
                    }
                    if let Some(label) = opts.secondary {
                        result.secondary |= secondary_button(ui, label).clicked();
                    }
                });
            }
        });
    result.dismissed |= response.should_close();
    result
}

// ---------------------------------------------------------------- profile

/// Round avatar with the initials of the name, used when no image is available.
pub fn initials_avatar(ui: &mut Ui, size: f32, name: &str) {
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(|c| c.to_uppercase())
        .collect();
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size / 2.0, theme::color(theme::ACCENT));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if initials.is_empty() {
            "?".to_string()
        } else {
            initials
        },
        egui::FontId::proportional(size * 0.4),
        Color32::WHITE,
    );
}

/// Profile card: header (avatar, name, optional subtitle) and biography.
/// `avatar` is the extension point for the avatar painter; pass
/// `|ui| initials_avatar(ui, 80.0, name)` for the fallback.
pub fn profile_card(
    ui: &mut Ui,
    name: &str,
    subtitle: Option<&str>,
    bio: Option<&str>,
    avatar: impl FnOnce(&mut Ui),
) {
    Frame::new()
        .fill(theme::color(theme::SIDEBAR))
        .corner_radius(8)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                avatar(ui);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(name)
                            .size(22.0)
                            .strong()
                            .color(theme::color(theme::TEXT_BRIGHT)),
                    );
                    if let Some(sub) = subtitle.filter(|s| !s.is_empty()) {
                        ui.label(RichText::new(sub).color(theme::color(theme::TEXT_MUTED)));
                    }
                });
            });
            if let Some(bio) = bio.filter(|b| !b.is_empty()) {
                ui.add_space(8.0);
                widgets::section_label(ui, "Sobre");
                ui.label(RichText::new(bio).color(theme::color(theme::TEXT)));
            }
        });
    ui.add_space(8.0);
}
