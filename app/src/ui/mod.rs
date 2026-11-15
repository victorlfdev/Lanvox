//! The interface, split by area of the window.

pub mod avatar;
pub mod call;
pub mod chat;
pub mod community_ui;
pub mod components;
pub mod details;
pub mod friends;
pub mod home;
pub mod icons;
pub mod markdown;
pub mod members;
pub mod modals;
pub mod quick_switcher;
pub mod sidebar;
pub mod theme;
pub mod video;
pub mod widgets;
pub mod workspace;

use egui::{Frame, Panel};

use crate::app::LanvoxApp;
use crate::model::ChannelKind;

pub fn root(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if ctx.input(|input| input.key_pressed(egui::Key::F1)) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("help-open"), true));
    }
    theme::configure(
        &ctx,
        app.settings.ui_scale,
        app.settings.ui_density,
        app.settings.theme,
    );

    if !app.connection_status.is_empty() && app.view.is_some() {
        Panel::top("connection-status")
            .frame(
                Frame::new()
                    .fill(theme::color(theme::PANEL))
                    .inner_margin(8),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(app.connection_status.clone())
                            .color(theme::color(theme::YELLOW)),
                    );
                    if app.connecting || app.reconnect_at.is_some() {
                        ui.spinner();
                    }
                    if ui.button("Reconectar agora").clicked() {
                        if let Some(address) = app.reconnect_address.clone() {
                            app.connect(&ctx, address);
                        }
                    }
                    if ui.button("Sair").clicked() {
                        app.disconnect();
                    }
                });
            });
    }
    if app.fullscreen {
        if let Some(key) = app.focused {
            egui::CentralPanel::default()
                .frame(Frame::new().fill(theme::color(theme::CALL_BG)))
                .show(ui, |ui| {
                    call::stream_stage(app, ui, key, true);
                });
            modals::show(app, &ctx);
            return;
        }
        app.fullscreen = false;
    }

    Panel::left("rail")
        .exact_size(theme::RAIL_W)
        .resizable(false)
        .frame(Frame::new().fill(theme::color(theme::RAIL)))
        .show(ui, |ui| sidebar::rail(app, ui));

    if !app.connected() {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::color(theme::CHAT)))
            .show(ui, |ui| home::show(app, ui));
    } else {
        let channels_panel = Panel::left("channels")
            .default_size(app.settings.channel_width.clamp(200.0, 360.0))
            .min_size(200.0)
            .max_size(360.0)
            .resizable(true)
            .frame(Frame::new().fill(theme::color(theme::SIDEBAR)))
            .show(ui, |ui| sidebar::channels(app, ui));
        app.settings.channel_width = channels_panel.response.rect.width();
        if ui.input(|i| i.pointer.any_released()) {
            app.settings.save();
        }

        // A call that belongs to a conversation is shown inside that conversation.
        if let Some(conv) = app
            .selected
            .and_then(|id| app.view.as_ref()?.conversation_of_call(id))
        {
            app.selected = Some(conv);
        }
        let selected_kind = app.selected.and_then(|id| {
            app.view
                .as_ref()
                .and_then(|v| v.channel(id))
                .map(|c| c.kind)
        });
        match selected_kind {
            None if app.home => {
                details::show(app, ui);
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(theme::color(theme::CHAT)))
                    .show(ui, |ui| friends::show(app, ui));
            }
            Some(ChannelKind::Voice) => {
                details::show(app, ui);
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(theme::color(theme::CALL_BG)))
                    .show(ui, |ui| call::show(app, ui));
            }
            _ => {
                chat::header_panel(app, ui);
                community_ui::threads_panel(app, ui);
                let linked_call = app
                    .selected
                    .and_then(|id| app.view.as_ref()?.call_of_conversation(id).map(|c| c.id));
                if let Some(call_id) = linked_call {
                    call::embedded(app, ui, call_id);
                }
                let details_visible = details::show(app, ui);
                if !details_visible && app.show_members && ui.available_width() > 650.0 {
                    let members_panel = Panel::right("members")
                        .default_size(app.settings.member_width.clamp(190.0, 320.0))
                        .min_size(190.0)
                        .max_size(320.0)
                        .resizable(true)
                        .frame(Frame::new().fill(theme::color(theme::SIDEBAR)))
                        .show(ui, |ui| members::show(app, ui));
                    app.settings.member_width = members_panel.response.rect.width();
                }
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(theme::color(theme::CHAT)))
                    .show(ui, |ui| {
                        if !community_ui::forum(app, ui) {
                            chat::show(app, ui);
                        }
                    });
            }
        }
    }

    modals::show(app, &ctx);
    quick_switcher::show(app, &ctx);
    workspace::show(app, &ctx);
    chat::extras(app, &ctx);
    call::popout(app, &ctx);
    friends::ring_popup(app, &ctx);
    modals::notice(app, &ctx);
    help(app, &ctx);
}

fn help(app: &LanvoxApp, ctx: &egui::Context) {
    let id = egui::Id::unique("help-open");
    if !ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    let result = components::modal(
        ctx,
        "help",
        "Ajuda e atalhos",
        components::ModalOptions::new(490.0).secondary("Fechar"),
        |ui| {
            ui.label("Encontre conversas, use a voz e compartilhe sua tela.");
            ui.add_space(8.0);
            egui::Grid::new("help-shortcuts")
                .spacing([20.0, 8.0])
                .show(ui, |ui| {
                    for (key, action) in [
                        ("Ctrl+K", "Encontrar conversa ou canal"),
                        ("Enter / Shift+Enter", "Enviar / nova linha"),
                        ("↑ no campo vazio", "Editar a última mensagem"),
                        ("Ctrl+Enter / Esc", "Salvar / cancelar edição"),
                        ("F1", "Abrir esta ajuda"),
                    ] {
                        ui.strong(key);
                        ui.label(action);
                        ui.end_row();
                    }
                    ui.strong(app.settings.mute_hotkey.label());
                    ui.label("Silenciar microfone");
                    ui.end_row();
                    ui.strong(app.settings.deafen_hotkey.label());
                    ui.label("Silenciar áudio");
                    ui.end_row();
                });
            ui.add_space(12.0);
            ui.label("Para entrar em um host, cole o convite recebido. Confirme a identidade do host com o administrador por um canal confiável.");
            ui.label(crate::branding::ADMIN_VISIBILITY_NOTICE);
            ui.label("Para acessar pela VPN, use o endereço da VPN; a descoberta automática procura apenas na rede local.");
        },
    );
    if result.cancelled() {
        ctx.data_mut(|data| data.insert_temp(id, false));
    }
}
