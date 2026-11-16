//! Pop-up windows: the screen-share picker, settings, creating a channel, and
//! short notices.

use std::sync::atomic::Ordering;

use std::time::Duration;

use egui::{
    pos2, vec2, Align, Align2, Color32, FontId, Frame, Layout, Margin, Rect, RichText, Sense,
    Stroke,
};

use super::{theme, widgets};

use crate::app::{LanvoxApp, Modal, SettingsTab};

use crate::media::audio::InputMode;

use crate::media::screen::{Resolution, SourceId, FRAME_RATES, TEXT_PRESET};

use crate::model::*;

use crate::settings::PttKey;

mod settings;
mod share_picker;
mod voice_settings;
use settings::*;
use share_picker::*;
use voice_settings::*;

pub fn show(app: &mut LanvoxApp, ctx: &egui::Context) {
    match app.modal.clone() {
        Modal::None => {}
        Modal::ConnectHost { address } => connect_host(app, ctx, address),
        Modal::JoinInvite { invite } => join_invite(app, ctx, invite),
        Modal::SharePicker => share_picker(app, ctx),
        Modal::Settings(tab) => settings(app, ctx, tab),
        Modal::CreateChannel {
            kind,
            name,
            category,
        } => create_channel(app, ctx, kind, name, category),
        Modal::CreateCategory { name } => category_name(app, ctx, None, name),
        Modal::RenameCategory { category, name } => category_name(app, ctx, Some(category), name),
        Modal::DeleteCategory { category } => delete_category(app, ctx, category),
        Modal::CreateGuild { name } => create_guild(app, ctx, name),
    }
}

use super::components;

fn pill(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(13.0), Color32::WHITE);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 24.0, 30.0), Sense::click());
    let bg = if on {
        theme::color(theme::ACTION_FILL)
    } else if resp.hovered() {
        theme::color(theme::SELECTED)
    } else {
        theme::color(theme::INPUT)
    };
    ui.painter().rect_filled(rect, 15.0, bg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, text));
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            15,
            Stroke::new(2.0, theme::TEXT_BRIGHT),
            egui::StrokeKind::Outside,
        );
    }
    resp
}

fn connect_host(app: &mut LanvoxApp, ctx: &egui::Context, mut address: String) {
    let before = address.clone();
    let mut enter = false;
    let result = components::modal(
        ctx,
        "connect-host",
        "Abrir outro host",
        components::ModalOptions::new(480.0)
            .primary("Conectar")
            .enabled(!address.trim().is_empty() && !app.connecting)
            .secondary("Cancelar"),
        |ui| {
            ui.label("Entre em outro grupo mantendo os hosts anteriores conectados.");
            let response = components::text_field(
                ui,
                &mut address,
                "Convite lanvox:// ou endereço do host",
                8192,
            );
            if address != before {
                app.load_host_preferences(&address);
            }
            enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.collapsing("Acesso manual", |ui| {
                ui.label("Token do host");
                ui.add(
                    egui::TextEdit::singleline(&mut app.settings.access_token)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
            });
            if app.connecting {
                ui.spinner();
                ui.label(&app.connection_status);
            }
            if let Some(error) = &app.join_error {
                ui.colored_label(theme::color(theme::YELLOW), error);
            }
        },
    );
    if result.cancelled() {
        app.modal = Modal::None;
    } else if result.primary || enter {
        if address.starts_with("lanvox://") {
            app.stage_invite(address);
        } else {
            app.connect(ctx, address);
            if app.connecting {
                app.modal = Modal::None;
            }
        }
    } else {
        app.modal = Modal::ConnectHost { address };
    }
}

fn join_invite(app: &mut LanvoxApp, ctx: &egui::Context, invite: String) {
    let parsed = crate::invites::parse(&invite);
    let result = components::modal(
        ctx,
        "join-invite",
        "Entrar por convite",
        components::ModalOptions::new(480.0)
            .primary("Entrar")
            .enabled(parsed.is_ok())
            .secondary("Cancelar"),
        |ui| match &parsed {
            Ok(value) => {
                ui.label("Você recebeu um convite para este host:");
                ui.label(RichText::new(&value.address).strong().size(20.0));
                ui.label("Confira o destino antes de continuar.");
                ui.collapsing("Identidade do host", |ui| {
                    ui.label(
                        RichText::new(value.fingerprint.as_deref().unwrap_or("Não informada"))
                            .monospace(),
                    );
                });
            }
            Err(error) => {
                ui.colored_label(theme::color(theme::YELLOW), error.to_string());
            }
        },
    );
    if result.primary {
        app.accept_invite(ctx, invite);
    } else if result.cancelled() {
        app.modal = Modal::None;
    }
}

fn create_channel(
    app: &mut LanvoxApp,
    ctx: &egui::Context,
    kind: ChannelKind,
    mut name: String,
    category: Option<CategoryId>,
) {
    let mut kind = kind;
    let mut done = false;
    let category_name = category.and_then(|id| {
        app.view
            .as_ref()?
            .workspace
            .categories
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
    });
    let subtitle = category_name.map(|n| format!("Na categoria {n}"));
    let mut options = components::ModalOptions::new(420.0);
    if let Some(text) = &subtitle {
        options = options.subtitle(text);
    }
    let result = components::modal(
        ctx,
        "create-channel",
        "Criar canal",
        options.primary("Criar").secondary("Cancelar"),
        |ui| {
            components::form_row(ui, "Tipo de canal", |ui| {
                ui.radio_value(
                    &mut kind,
                    ChannelKind::Text,
                    "Texto — conversar por mensagens",
                );
                ui.radio_value(
                    &mut kind,
                    ChannelKind::Voice,
                    "Voz — conversar, usar câmera e compartilhar tela",
                );
            });
            components::form_row(ui, "Nome do canal", |ui| {
                let hint = if kind == ChannelKind::Text {
                    "novo-canal"
                } else {
                    "Nova sala de voz"
                };
                let r = components::text_field(ui, &mut name, hint, 40);
                r.request_focus();
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
            });
        },
    );
    done |= result.primary;
    if done && !name.trim().is_empty() {
        app.create_channel(name.trim().to_string(), kind, category);
        app.modal = Modal::None;
    } else if result.cancelled() {
        app.modal = Modal::None;
    } else {
        app.modal = Modal::CreateChannel {
            kind,
            name,
            category,
        };
    }
}

/// Creates (`category` None) or renames a category.
fn category_name(
    app: &mut LanvoxApp,
    ctx: &egui::Context,
    category: Option<CategoryId>,
    mut name: String,
) {
    let mut done = false;
    let (title, action) = if category.is_some() {
        ("Renomear categoria", "Salvar")
    } else {
        ("Criar categoria", "Criar")
    };
    let result = components::modal(
        ctx,
        "category-name",
        title,
        components::ModalOptions::new(420.0)
            .subtitle("Categorias agrupam canais de texto e de voz na barra lateral.")
            .primary(action)
            .secondary("Cancelar"),
        |ui| {
            components::form_row(ui, "Nome da categoria", |ui| {
                let r = components::text_field(ui, &mut name, "Nova categoria", 50);
                r.request_focus();
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
            });
        },
    );
    done |= result.primary;
    if done && !name.trim().is_empty() {
        let name = name.trim().to_string();
        app.category_command(match category {
            Some(category) => WorkspaceCommand::RenameCategory { category, name },
            None => WorkspaceCommand::CreateCategory {
                guild: app.guild.unwrap_or(1),
                name,
            },
        });
        app.modal = Modal::None;
    } else if result.cancelled() {
        app.modal = Modal::None;
    } else {
        app.modal = match category {
            Some(category) => Modal::RenameCategory { category, name },
            None => Modal::CreateCategory { name },
        };
    }
}

fn delete_category(app: &mut LanvoxApp, ctx: &egui::Context, category: CategoryId) {
    let name = app
        .view
        .as_ref()
        .and_then(|v| v.workspace.categories.iter().find(|c| c.id == category))
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let result = components::modal(
        ctx,
        "delete-category",
        "Excluir categoria",
        components::ModalOptions::new(420.0)
            .primary("Excluir")
            .danger()
            .secondary("Cancelar"),
        |ui| {
            ui.label(
                RichText::new(format!(
                    "A categoria \"{name}\" será excluída. Os canais dela não são apagados: eles passam para o topo da lista, sem categoria."
                ))
                .color(theme::color(theme::TEXT)),
            );
        },
    );
    if result.primary {
        app.category_command(WorkspaceCommand::DeleteCategory { category });
        app.modal = Modal::None;
    } else if result.cancelled() {
        app.modal = Modal::None;
    }
}

fn create_guild(app: &mut LanvoxApp, ctx: &egui::Context, mut name: String) {
    let mut done = false;
    let result = components::modal(
        ctx,
        "create-guild",
        "Criar comunidade",
        components::ModalOptions::new(420.0)
            .subtitle(
                "Você será o proprietário. Convide membros para acessar os canais de texto e voz.",
            )
            .primary("Criar")
            .secondary("Cancelar"),
        |ui| {
            components::form_row(ui, "Nome da comunidade", |ui| {
                let r = components::text_field(ui, &mut name, "Minha comunidade", 40);
                r.request_focus();
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
            });
        },
    );
    done |= result.primary;
    if done && !name.trim().is_empty() {
        app.send(crate::protocol::ClientMsg::CreateGuild {
            name: name.trim().to_string(),
        });
        app.modal = Modal::None;
    } else if result.cancelled() {
        app.modal = Modal::None;
    } else {
        app.modal = Modal::CreateGuild { name };
    }
}

/// A short message at the bottom of the window for a few seconds.
pub fn notice(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some((text, at)) = app.notice.clone() else {
        return;
    };
    if at.elapsed() > Duration::from_secs(6) {
        app.notice = app
            .notice_queue
            .pop_front()
            .map(|text| (text, std::time::Instant::now()));
        return;
    }
    ctx.request_repaint_after(Duration::from_millis(250));
    egui::Area::new(egui::Id::unique("notice"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -80.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            Frame::new()
                .fill(theme::color(theme::RAIL))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.label(RichText::new(text).color(theme::color(theme::TEXT_BRIGHT)));
                });
        });
}
