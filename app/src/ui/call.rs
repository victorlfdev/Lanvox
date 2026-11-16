//! A voice channel: people tiles, the stream stage, call controls and the
//! pop-out stream window.

use std::sync::atomic::Ordering;

use egui::{
    pos2, vec2, Align, Align2, Color32, FontId, Frame, Layout, Margin, Panel, Rect, RichText,
    Sense, Stroke,
};

use super::icons::{self, Icon};

use super::{theme, widgets};

use crate::app::LanvoxApp;

use crate::media::video::StreamKey;

use crate::model::*;
mod stage;
mod tiles;
pub use stage::*;
pub use tiles::*;

/// Minimum size of the pop-out window.
const POPOUT_MIN: [f32; 2] = [320.0, 180.0];

/// Whether the selected conversation's messages are on screen. A voice channel
/// only shows its chat while the call side panel is open.
pub fn chat_is_visible(kind: Option<ChannelKind>, call_chat_open: bool) -> bool {
    kind != Some(ChannelKind::Voice) || call_chat_open
}

/// Text for the unread badge on the call chat button; `None` when nothing is unread.
pub fn unread_badge(count: u32) -> Option<String> {
    match count {
        0 => None,
        1..=9 => Some(count.to_string()),
        _ => Some("9+".to_string()),
    }
}

/// Width of the call chat side panel for the space available; keeps the stage usable at small sizes.
fn chat_panel_width(available: f32) -> f32 {
    (available * 0.38).clamp(240.0, 340.0)
}

pub fn show(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let Some(channel) = app.selected else { return };
    let Some(ch) = app.view.as_ref().and_then(|v| v.channel(channel)).cloned() else {
        return;
    };
    let in_this_call = app.view.as_ref().and_then(|v| v.my_voice().channel) == Some(channel);

    Panel::top("call-header")
        .exact_size(theme::HEADER_H)
        .frame(
            Frame::new()
                .fill(theme::color(theme::CALL_BG))
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Speaker, theme::TEXT_MUTED);
                ui.add(
                    egui::Label::new(
                        RichText::new(&ch.name)
                            .strong()
                            .size(16.0)
                            .color(theme::TEXT_BRIGHT),
                    )
                    .truncate(),
                )
                .on_hover_text(&ch.name);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    add_people_menu(app, ui, channel)
                });
            });
        });

    if !in_this_call {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::color(theme::CALL_BG)))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 2.0 - 60.0);
                    ui.label(
                        RichText::new(&ch.name)
                            .size(24.0)
                            .strong()
                            .color(theme::TEXT_BRIGHT),
                    );
                    let n = app
                        .view
                        .as_ref()
                        .map(|v| v.in_voice(channel).len())
                        .unwrap_or(0);
                    ui.label(
                        RichText::new(if n == 0 {
                            "Ainda não há ninguém aqui.".to_string()
                        } else {
                            format!("{n} na chamada")
                        })
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(12.0);
                    if widgets::primary_button(ui, "Entrar na voz").clicked() {
                        app.join_voice(channel);
                    }
                });
            });
        return;
    }

    let is_stage = app.view.as_ref().is_some_and(|v| {
        v.workspace
            .community
            .policies
            .iter()
            .any(|p| p.channel == channel && p.style == ChannelStyle::Stage)
    });
    if is_stage {
        let loaded = app
            .view
            .as_ref()
            .is_some_and(|v| v.stage_states.contains_key(&channel));
        let key = egui::Id::unique(("stage-status-request", channel));
        let now = std::time::Instant::now();
        let due = ui
            .ctx()
            .data(|d| d.get_temp::<std::time::Instant>(key))
            .is_none_or(|last| now.duration_since(last).as_secs() >= 2);
        if !loaded && due {
            ui.ctx().data_mut(|d| d.insert_temp(key, now));
            app.workspace(WorkspaceCommand::Community(CommunityCommand::StageStatus {
                channel,
            }));
        }
        Panel::top("stage-participation")
            .frame(
                Frame::new()
                    .fill(theme::color(theme::PANEL))
                    .inner_margin(8),
            )
            .show(ui, |ui| stage_participation(app, ui, channel));
    }

    Panel::bottom("call-controls")
        .exact_size(84.0)
        .frame(Frame::new().fill(theme::color(theme::CALL_BG)))
        .show(ui, |ui| controls(app, ui, Some(channel)));

    if app.call_chat_open {
        Panel::right("call-chat")
            .exact_size(chat_panel_width(ui.available_width()))
            .resizable(false)
            .frame(Frame::new().fill(theme::color(theme::CHAT)))
            .show(ui, |ui| {
                Panel::top("call-chat-title")
                    .exact_size(36.0)
                    .frame(
                        Frame::new()
                            .fill(theme::color(theme::CHAT))
                            .inner_margin(Margin::symmetric(12, 0)),
                    )
                    .show(ui, |ui| {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.label(
                                RichText::new("Chat da chamada")
                                    .strong()
                                    .color(theme::TEXT_BRIGHT),
                            );
                        });
                    });
                // Reuses the channel chat (history, list, composer) for the selected voice channel.
                super::chat::show(app, ui);
            });
    }

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(theme::color(theme::CALL_BG))
                .inner_margin(Margin::same(12)),
        )
        .show(ui, |ui| {
            if let Some(key) = app.focused {
                let strip_h = 110.0;
                let stage = Rect::from_min_max(
                    ui.max_rect().min,
                    pos2(
                        ui.max_rect().right(),
                        ui.max_rect().bottom() - strip_h - 8.0,
                    ),
                );
                let mut stage_ui = ui.new_child(egui::UiBuilder::new().max_rect(stage));
                stream_stage(app, &mut stage_ui, key, false);
                let strip = Rect::from_min_max(
                    pos2(ui.max_rect().left(), stage.bottom() + 8.0),
                    ui.max_rect().max,
                );
                let mut strip_ui = ui.new_child(egui::UiBuilder::new().max_rect(strip));
                tiles(app, &mut strip_ui, channel, true);
            } else {
                tiles(app, ui, channel, false);
            }
        });
}

fn stage_participation(app: &mut LanvoxApp, ui: &mut egui::Ui, channel: ChannelId) {
    let Some(view) = app.view.as_ref() else {
        return;
    };
    let me = view.me;
    let manager = view.has_permission(channel, permission::MANAGE_MESSAGES);
    let state = view.stage_states.get(&channel).cloned();
    let people: Vec<_> = view
        .in_voice(channel)
        .iter()
        .map(|id| (*id, view.user_name(*id)))
        .collect();
    let Some((speakers, requests)) = state else {
        ui.label("Carregando participantes do palco…");
        return;
    };
    let can_speak = manager || speakers.contains(&me);
    ui.horizontal_wrapped(|ui| {
        ui.label(if can_speak {
            "Você pode falar no palco"
        } else {
            "Você está na audiência"
        });
        if !can_speak {
            let requested = requests.contains(&me);
            if ui
                .button(if requested {
                    "Cancelar pedido"
                } else {
                    "Pedir para falar"
                })
                .clicked()
            {
                app.workspace(WorkspaceCommand::Community(
                    CommunityCommand::StageRequest {
                        channel,
                        requested: !requested,
                    },
                ));
            }
        }
        if manager {
            ui.menu_button(
                format!("Participantes · {} pedidos", requests.len()),
                |ui| {
                    for (user, name) in &people {
                        if *user == me {
                            continue;
                        }
                        let allowed = speakers.contains(user);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "{}{}",
                                name,
                                if requests.contains(user) {
                                    " · pediu para falar"
                                } else {
                                    ""
                                }
                            ));
                            if ui
                                .button(if allowed {
                                    "Mover para audiência"
                                } else {
                                    "Convidar a falar"
                                })
                                .clicked()
                            {
                                app.workspace(WorkspaceCommand::Community(
                                    CommunityCommand::StageSpeaker {
                                        channel,
                                        user: *user,
                                        allowed: !allowed,
                                    },
                                ));
                                ui.close();
                            }
                        });
                    }
                },
            );
        }
    });
}

/// "Add friends" menu for a private call: invite more friends into it.
fn add_people_menu(app: &mut LanvoxApp, ui: &mut egui::Ui, channel: ChannelId) {
    let Some(view) = &app.view else { return };
    let Some(call) = view.calls.get(&channel) else {
        return;
    };
    let candidates: Vec<(UserId, String)> = view
        .friends
        .iter()
        .filter(|f| f.status == FriendStatus::Friend && !call.members.contains(&f.user))
        .filter(|f| view.users.get(&f.user).map(|u| u.online).unwrap_or(false))
        .map(|f| (f.user, view.user_name(f.user)))
        .collect();
    let mut invite = None;
    ui.menu_button("Adicionar amigos à chamada", |ui| {
        if candidates.is_empty() {
            ui.label("Não há outros amigos disponíveis.");
        }
        for (id, name) in &candidates {
            if ui.button(name).clicked() {
                invite = Some(*id);
                ui.close();
            }
        }
    });
    if let Some(user) = invite {
        app.send(crate::protocol::ClientMsg::InviteToCall {
            call: channel,
            user,
        });
    }
}

enum TileKind {
    Person(UserId),
    Stream(UserId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_hidden_when_nothing_unread() {
        assert_eq!(unread_badge(0), None);
        assert_eq!(unread_badge(3).as_deref(), Some("3"));
        assert_eq!(unread_badge(9).as_deref(), Some("9"));
        assert_eq!(unread_badge(10).as_deref(), Some("9+"));
    }

    #[test]
    fn voice_chat_visible_only_when_panel_open() {
        assert!(!chat_is_visible(Some(ChannelKind::Voice), false));
        assert!(chat_is_visible(Some(ChannelKind::Voice), true));
        assert!(chat_is_visible(Some(ChannelKind::Text), false));
        assert!(chat_is_visible(None, false));
    }

    #[test]
    fn chat_panel_width_is_clamped() {
        assert_eq!(chat_panel_width(500.0), 240.0);
        assert_eq!(chat_panel_width(800.0), 304.0);
        assert_eq!(chat_panel_width(2000.0), 340.0);
    }
}
