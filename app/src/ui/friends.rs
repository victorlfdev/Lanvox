//! The Home screen: friends, requests, one-on-one and group calls, and the
//! incoming-call popup.

use egui::{vec2, Align, Align2, Color32, Frame, Layout, Margin, RichText};

use super::avatar::{Avatar, AvatarSize, PresenceState};
use super::{theme, widgets};
use crate::app::LanvoxApp;
use crate::model::*;
use crate::protocol::ClientMsg;

struct Row {
    id: UserId,
    name: String,
    color: u32,
    state: PresenceState,
    status: FriendStatus,
    in_call: bool,
    avatar: Option<egui::TextureHandle>,
}

pub fn show(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let tab_id = egui::Id::unique("friends-tab");
    let mut tab = ui.ctx().data(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
    let group_id = egui::Id::unique("friends-group-call");
    let mut group_mode = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(group_id))
        .unwrap_or(false);
    let Some(view) = &app.view else { return };
    let mut rows: Vec<Row> = view
        .friends
        .iter()
        .map(|f| Row {
            id: f.user,
            name: view.user_name(f.user),
            color: view.user_color(f.user),
            state: PresenceState::in_view(view, f.user),
            status: f.status,
            avatar: None,
            in_call: view
                .voice
                .get(&f.user)
                .map(|v| v.channel.is_some())
                .unwrap_or(false),
        })
        .collect();
    if tab == 3 {
        rows = view
            .users
            .values()
            .filter(|u| app.is_blocked(u.id))
            .map(|u| Row {
                id: u.id,
                name: u.name.clone(),
                color: u.color,
                state: PresenceState::in_view(view, u.id),
                status: FriendStatus::Friend,
                in_call: false,
                avatar: None,
            })
            .collect();
    } else {
        rows.retain(|r| !app.is_blocked(r.id));
    }
    rows.sort_by(|a, b| {
        b.state
            .is_present()
            .cmp(&a.state.is_present())
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    for row in &mut rows {
        row.avatar = app.avatar_texture(ui.ctx(), row.id);
    }
    let filter = app.friend_filter.to_lowercase();
    rows.retain(|r| {
        (filter.is_empty() || r.name.to_lowercase().contains(&filter))
            && (tab != 0 || r.state.is_present() || r.status != FriendStatus::Friend)
    });
    let incoming: Vec<&Row> = rows
        .iter()
        .filter(|r| r.status == FriendStatus::Incoming)
        .collect();
    let outgoing: Vec<&Row> = rows
        .iter()
        .filter(|r| r.status == FriendStatus::Outgoing)
        .collect();
    let friends: Vec<&Row> = rows
        .iter()
        .filter(|r| r.status == FriendStatus::Friend)
        .collect();

    let mut send_request = false;
    let mut respond: Option<(UserId, bool)> = None;
    let mut call_one: Option<UserId> = None;
    let mut call_group = false;
    let mut message_one: Option<(UserId, String)> = None;
    let mut profile_one = None;
    let mut disconnect = false;
    let mut toggle_pick: Option<UserId> = None;

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(24, 20))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Amigos")
                                .size(24.0)
                                .strong()
                                .color(theme::color(theme::TEXT_BRIGHT)),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Sair do host").clicked() {
                                disconnect = true;
                            }
                        });
                    });
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        for (value, label) in [
                            (0, "Disponíveis"),
                            (1, "Todos"),
                            (2, "Pendentes"),
                            (3, "Bloqueados"),
                            (4, "Adicionar amigo"),
                        ] {
                            ui.selectable_value(&mut tab, value, label);
                        }
                    });
                    ui.add_space(10.0);
                    if tab <= 3 { ui.horizontal_wrapped(|ui| {
                        let filter_w = 220.0_f32.min(ui.available_width());
                        ui.add(
                            egui::TextEdit::singleline(&mut app.friend_filter)
                                .hint_text("Buscar amigos")
                                .desired_width(filter_w),
                        );
                        if tab <= 1 && ui
                            .selectable_label(group_mode, "Chamada em grupo")
                            .clicked()
                        {
                            group_mode = !group_mode;
                            if !group_mode {
                                app.group_pick.clear();
                            }
                        }
                        if tab <= 1 && ui.button("Novo grupo").clicked() {
                            app.workspace_ui.group_open = true;
                        }
                        if tab <= 1 && ui.button("Convites e comunidade").clicked() {
                            app.open_guild_settings(app.guild, 0);
                        }
                    }); }
                    ui.add_space(12.0);

                    if tab == 4 {
                        widgets::section_label(ui, "Adicionar amigo");
                        ui.horizontal_wrapped(|ui| {
                            let input_w = 300.0_f32.min(ui.available_width());
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut app.friend_input)
                                    .hint_text("Nome da pessoa neste host")
                                    .desired_width(input_w)
                                    .char_limit(32)
                                    .margin(Margin::symmetric(10, 8)),
                            );
                            let enter =
                                r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if (widgets::primary_button(ui, "Enviar solicitação").clicked()
                                || enter)
                                && !app.friend_input.trim().is_empty()
                            {
                                send_request = true;
                            }
                        });
                        ui.add_space(16.0);
                    }
                    if tab == 2 && (!incoming.is_empty() || !outgoing.is_empty()) {
                        widgets::section_label(
                            ui,
                            &format!("Pendentes — {}", incoming.len() + outgoing.len()),
                        );
                        for r in &incoming {
                            row(
                                ui,
                                r,
                                false,
                                false,
                                |ui| {
                                    // Right-to-left: the secondary menu goes first so
                                    // "Aceitar" stays next to the name.
                                    ui.menu_button("Mais", |ui| {
                                        if ui.button("Recusar").clicked() {
                                            respond = Some((r.id, false));
                                            ui.close();
                                        }
                                    });
                                    if widgets::primary_button(ui, "Aceitar").clicked() {
                                        respond = Some((r.id, true));
                                    }
                                },
                                "Quer ser seu amigo",
                            );
                        }
                        for r in &outgoing {
                            row(
                                ui,
                                r,
                                false,
                                false,
                                |ui| {
                                    if ui.button("Cancelar").clicked() {
                                        respond = Some((r.id, false));
                                    }
                                },
                                "Solicitação enviada",
                            );
                        }
                        ui.add_space(16.0);
                    }

                    if tab == 2 && incoming.is_empty() && outgoing.is_empty() {
                        ui.label(if filter.is_empty() { "Você não tem solicitações pendentes." } else { "Nenhuma solicitação corresponde à busca." });
                    }
                    if tab <= 1 || tab == 3 {
                        widgets::section_label(
                            ui,
                            &format!(
                                "{} — {}",
                                if tab == 3 { "Bloqueados" } else { "Amigos" },
                                friends.len()
                            ),
                        );
                        if friends.is_empty() {
                            ui.label(
                                RichText::new(
                                    if !filter.is_empty() {
                                        "Nenhuma pessoa corresponde à busca."
                                    } else if tab == 3 {
                                        "Você não bloqueou ninguém neste host."
                                    } else if tab == 0 {
                                        "Nenhum amigo está disponível agora. Veja todos na aba Todos."
                                    } else {
                                        "Ainda não há amigos. Use Adicionar amigo para enviar uma solicitação."
                                    },
                                )
                                .color(theme::color(theme::TEXT_MUTED)),
                            );
                        }
                        for r in &friends {
                            let picked = app.group_pick.contains(&r.id);
                            let sub = if r.in_call {
                                "Em chamada"
                            } else {
                                r.state.label()
                            };
                            let clicked = row(
                                ui,
                                r,
                                group_mode && tab != 3,
                                picked,
                                |ui| {
                                    if tab == 3 {
                                        if ui.button("Desbloquear").clicked() {
                                            app.set_blocked(r.id, false);
                                        }
                                        return;
                                    }
                                    ui.menu_button("Mais", |ui| {
                                        if ui.button("Perfil").clicked() {
                                            profile_one = Some(r.id);
                                            ui.close();
                                        }
                                        if ui.button("Bloquear pessoa").clicked() {
                                            app.set_blocked(r.id, true);
                                            ui.close();
                                        }
                                        if ui.button("Remover amigo").clicked() {
                                            respond = Some((r.id, false));
                                            ui.close();
                                        }
                                    });
                                    if ui
                                        .add_enabled(r.state.is_present(), egui::Button::new("Chamar"))
                                        .on_hover_text(if r.state.is_present() { "Iniciar uma chamada" } else { "A pessoa precisa estar disponível para receber uma chamada" })
                                        .clicked()
                                    {
                                        call_one = Some(r.id);
                                    }
                                    if ui.button("Mensagem").clicked() {
                                        message_one = Some((r.id, r.name.clone()));
                                    }
                                },
                                sub,
                            );
                            if clicked {
                                toggle_pick = Some(r.id);
                            }
                        }

                        if group_mode && !app.group_pick.is_empty() {
                            ui.add_space(16.0);
                            let n = app.group_pick.len();
                            let label = if n == 1 {
                                "Chamar o amigo selecionado".to_string()
                            } else {
                                format!("Chamar os {n} amigos selecionados")
                            };
                            if widgets::primary_button(ui, &label).clicked() {
                                call_group = true;
                            }
                            ui.label(
                                RichText::new("Marque as pessoas que deseja chamar.")
                                    .size(12.0)
                                    .color(theme::color(theme::TEXT_FAINT)),
                            );
                        } else if group_mode && friends.len() > 1 {
                            ui.add_space(12.0);
                            ui.label(
                                RichText::new(
                                    "Marque dois ou mais amigos para iniciar uma chamada em grupo.",
                                )
                                .size(12.0)
                                .color(theme::color(theme::TEXT_FAINT)),
                            );
                        }
                    }
                });
        });
    ui.ctx().data_mut(|d| {
        d.insert_temp(tab_id, tab);
        d.insert_temp(group_id, group_mode);
    });

    if let Some(u) = toggle_pick {
        if !app.group_pick.remove(&u) {
            app.group_pick.insert(u);
        }
    }
    if send_request {
        let name = std::mem::take(&mut app.friend_input);
        app.send(ClientMsg::FriendRequest { name });
    }
    if let Some((user, accept)) = respond {
        app.send(ClientMsg::FriendRespond { user, accept });
    }
    if let Some((user, name)) = message_one {
        app.open_conversation(name, vec![user], true);
    }
    if let Some(user) = profile_one {
        app.open_profile(user);
    }
    if let Some(u) = call_one {
        app.start_call(vec![u]);
    }
    if call_group {
        let members: Vec<UserId> = app.group_pick.iter().copied().collect();
        app.start_call(members);
    }
    if disconnect {
        app.disconnect();
    }
}

/// One person's row. Returns true when the selection circle was clicked.
fn row(
    ui: &mut egui::Ui,
    r: &Row,
    selectable: bool,
    picked: bool,
    buttons: impl FnOnce(&mut egui::Ui),
    sub: &str,
) -> bool {
    let mut clicked = false;
    Frame::new()
        .fill(theme::color(theme::SIDEBAR))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(720.0));
            ui.horizontal(|ui| {
                // Buttons claim the right edge first; the name gets the rest and truncates.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    buttons(ui);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if selectable {
                            let mut selected = picked;
                            let response = ui
                                .checkbox(&mut selected, "")
                                .on_hover_text("Selecionar para chamada em grupo");
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::CheckBox,
                                    ui.is_enabled(),
                                    picked,
                                    format!("Selecionar {} para chamada", r.name),
                                )
                            });
                            clicked = response.changed();
                        }
                        Avatar::new(&r.name, AvatarSize::Md)
                            .user(r.id)
                            .color(r.color)
                            .texture(r.avatar.as_ref())
                            .presence(r.state)
                            .show(ui);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&r.name)
                                        .strong()
                                        .size(16.0)
                                        .color(theme::color(theme::TEXT_BRIGHT)),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(sub)
                                        .size(12.5)
                                        .color(theme::color(theme::TEXT_MUTED)),
                                )
                                .truncate(),
                            );
                        });
                    });
                });
            });
        });
    ui.add_space(4.0);
    clicked
}

/// A popup at the top of the window for every call that is ringing.
pub fn ring_popup(app: &mut LanvoxApp, ctx: &egui::Context) {
    let ringing = app.ringing();
    if ringing.is_empty() {
        return;
    }
    let mut accept = None;
    let mut decline = None;
    egui::Area::new(egui::Id::unique("ring-popup"))
        .anchor(Align2::CENTER_TOP, vec2(0.0, 16.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            for call in &ringing {
                let who = app
                    .view
                    .as_ref()
                    .map(|v| v.user_name(call.host))
                    .unwrap_or_default();
                let group = call.members.len() > 2;
                let host_texture = app.avatar_texture(ctx, call.host);
                Frame::new()
                    .fill(theme::color(theme::RAIL))
                    .corner_radius(10)
                    .stroke(egui::Stroke::new(2.0, theme::color(theme::GREEN)))
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let color = app
                                .view
                                .as_ref()
                                .map(|v| v.user_color(call.host))
                                .unwrap_or(0x5b6ee1);
                            Avatar::new(&who, AvatarSize::Lg)
                                .user(call.host)
                                .color(color)
                                .texture(host_texture.as_ref())
                                .speaking(true)
                                .show(ui);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(
                                    RichText::new(&who)
                                        .strong()
                                        .size(16.0)
                                        .color(theme::color(theme::TEXT_BRIGHT)),
                                );
                                ui.label(
                                    RichText::new(if group {
                                        format!(
                                            "Chamada de grupo recebida ({} pessoas)",
                                            call.members.len()
                                        )
                                    } else {
                                        "Chamada recebida…".into()
                                    })
                                    .color(theme::color(theme::TEXT_MUTED)),
                                );
                            });
                            ui.add_space(16.0);
                            if widgets::primary_button(ui, "Aceitar").clicked() {
                                accept = Some(call.id);
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Recusar").color(Color32::WHITE),
                                    )
                                    .fill(theme::color(theme::RED))
                                    .min_size(vec2(88.0, 34.0)),
                                )
                                .clicked()
                            {
                                decline = Some(call.id);
                            }
                        });
                    });
                ui.add_space(6.0);
            }
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    if let Some(id) = accept {
        app.join_call(id);
    }
    if let Some(id) = decline {
        app.decline_call(id);
    }
}
