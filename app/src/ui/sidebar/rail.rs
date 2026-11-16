use super::*;

pub fn rail(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    ui.add_space(12.0);
    egui::ScrollArea::vertical()
        .id_salt("community-rail")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                let home_selected = app.home || !app.connected();
                let home = rail_icon(ui, None, home_selected, |p, r, _| {
                    icons::paint(p, r.shrink(12.0), Icon::Home, Color32::WHITE)
                });
                if !app.ringing().is_empty() {
                    let c = home.rect.center() + vec2(18.0, -18.0);
                    ui.painter()
                        .circle_filled(c, 7.0, theme::color(theme::RAIL));
                    ui.painter().circle_filled(c, 5.0, theme::color(theme::RED));
                }
                home.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        ui.is_enabled(),
                        "Amigos e conversas",
                    )
                });
                let home = home.on_hover_text("Amigos e conversas");
                if home.clicked() && app.connected() {
                    app.show_friends();
                }
                let (sep, _) = ui.allocate_exact_size(vec2(32.0, 10.0), Sense::hover());
                ui.painter().line_segment(
                    [
                        pos2(sep.left(), sep.center().y),
                        pos2(sep.right(), sep.center().y),
                    ],
                    Stroke::new(2.0, theme::color(theme::HOVER)),
                );
                let guilds: Vec<Guild> = app
                    .view
                    .as_ref()
                    .map(|v| v.guilds.clone())
                    .unwrap_or_default();
                for g in guilds {
                    let selected = !app.home && app.guild == Some(g.id);
                    let initials: String = g
                        .name
                        .split_whitespace()
                        .filter_map(|w| w.chars().next())
                        .take(3)
                        .collect::<String>()
                        .to_uppercase();
                    let bytes = app
                        .view
                        .as_ref()
                        .and_then(|v| {
                            v.workspace
                                .community
                                .visuals
                                .iter()
                                .find(|v| v.guild == g.id)
                        })
                        .map(|v| v.icon.clone())
                        .unwrap_or_default();
                    let texture = crate::ui::community_ui::image_texture(
                        ui.ctx(),
                        &format!("guild-icon-{}", g.id),
                        &bytes,
                    );
                    let resp = rail_icon(
                        ui,
                        Some(theme::color(theme::ACTION_FILL)),
                        selected,
                        |p, r, radius| {
                            if let Some(t) = &texture {
                                // Follow the tile's rounded corners.
                                p.add(
                                    egui::epaint::RectShape::filled(r, radius, Color32::WHITE)
                                        .with_texture(
                                            t.id(),
                                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                        ),
                                );
                                return;
                            }
                            p.text(
                                r.center(),
                                Align2::CENTER_CENTER,
                                &initials,
                                FontId::proportional(16.0),
                                Color32::WHITE,
                            );
                        },
                    )
                    .on_hover_text(&g.name);
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            ui.is_enabled(),
                            format!("Comunidade {}", g.name),
                        )
                    });
                    let unread = app
                        .view
                        .as_ref()
                        .map(|v| {
                            v.channels
                                .iter()
                                .filter(|c| c.guild == g.id)
                                .map(|c| v.unread.get(&c.id).copied().unwrap_or(0))
                                .sum::<u32>()
                        })
                        .unwrap_or(0);
                    if unread > 0 {
                        let at = resp.rect.right_bottom() - vec2(8.0, 8.0);
                        ui.painter()
                            .circle_filled(at, 10.0, theme::color(theme::RED));
                        ui.painter().text(
                            at,
                            Align2::CENTER_CENTER,
                            if unread > 99 {
                                "99+".into()
                            } else {
                                unread.to_string()
                            },
                            FontId::proportional(10.0),
                            Color32::WHITE,
                        );
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Marcar comunidade como lida").clicked() {
                            let channels = app
                                .view
                                .as_ref()
                                .map(|v| {
                                    v.channels
                                        .iter()
                                        .filter(|c| c.guild == g.id)
                                        .map(|c| c.id)
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            for id in channels {
                                app.mark_all_read(id);
                            }
                            ui.close();
                        }
                        if ui.button("Notificações da comunidade").clicked() {
                            app.open_guild_settings(Some(g.id), 6);
                            ui.close();
                        }
                    });
                    if resp.clicked() {
                        app.home = false;
                        app.guild = Some(g.id);
                        app.scroll_to_bottom = 3;
                        if let Some(channel) =
                            crate::ui::quick_switcher::channel_for_guild(app, g.id)
                        {
                            app.select_channel(channel);
                        } else {
                            app.show_friends();
                            app.home = false;
                        }
                    }
                }
                if app.connected() {
                    let add = rail_icon(ui, None, false, |p, r, _| {
                        icons::paint(p, r.shrink(14.0), Icon::Plus, theme::color(theme::GREEN))
                    });
                    if add.on_hover_text("Criar comunidade").clicked() {
                        app.modal = Modal::CreateGuild {
                            name: String::new(),
                        };
                    }
                }
                ui.add_space(8.0);
                if ui
                    .button("?")
                    .on_hover_text("Ajuda e atalhos · F1")
                    .clicked()
                {
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(egui::Id::unique("help-open"), true));
                }
                ui.menu_button("Hosts", |ui| {
                    if app
                        .view
                        .as_ref()
                        .and_then(|v| v.my_voice().channel)
                        .is_some()
                    {
                        ui.label("Trocar de servidor encerra sua chamada atual.");
                        ui.separator();
                    }
                    for session in app.host_sessions() {
                        ui.horizontal(|ui| {
                            let label = format!(
                                "{}{} · {}{}",
                                if session.active { "● " } else { "" },
                                session.name,
                                if session.online {
                                    "online"
                                } else if session.reconnecting {
                                    "reconectando…"
                                } else {
                                    "offline"
                                },
                                if session.unread > 0 {
                                    format!(" · {}", session.unread)
                                } else {
                                    String::new()
                                }
                            );
                            let host_label = ui.selectable_label(session.active, label);
                            let host_label = if let Some(error) = &session.connection_error {
                                host_label.on_hover_text(error)
                            } else {
                                host_label
                            };
                            if host_label.clicked() {
                                app.switch_host(ui.ctx(), session.address.clone());
                                ui.close();
                            }
                            if !session.active && ui.small_button("Fechar").clicked() {
                                app.close_host_session(&session.address);
                                ui.close();
                            }
                        });
                    }
                    ui.separator();
                    for favorite in app.settings.favorites.clone() {
                        if ui.button(&favorite).clicked() {
                            app.switch_host(ui.ctx(), favorite);
                            ui.close();
                        }
                    }
                    if ui.button("Abrir outro host").clicked() {
                        app.modal = Modal::ConnectHost {
                            address: String::new(),
                        };
                        ui.close();
                    }
                });
                if app.connected()
                    && widgets::icon_button(
                        ui,
                        Icon::Pin,
                        34.0,
                        theme::color(theme::TEXT_MUTED),
                        "Caixa de entrada",
                    )
                    .clicked()
                {
                    app.workspace_ui.inbox_open = !app.workspace_ui.inbox_open;
                    if app.workspace_ui.inbox_open {
                        app.workspace_ui.inbox_requested = None;
                    }
                    app.workspace_ui.search_open = false;
                    app.workspace_ui.pinned_only = false;
                    app.workspace(WorkspaceCommand::Community(CommunityCommand::Unread));
                }
            });
        });
}

pub(super) fn rail_icon(
    ui: &mut egui::Ui,
    fill: Option<Color32>,
    selected: bool,
    paint: impl FnOnce(&egui::Painter, Rect, u8),
) -> egui::Response {
    let (outer, resp) = ui.allocate_exact_size(vec2(theme::RAIL_W, 56.0), Sense::click());
    let rect = Rect::from_center_size(outer.center(), vec2(48.0, 48.0));
    let hovered = resp.hovered();
    let radius: u8 = if selected || hovered { 14 } else { 18 };
    let bg = if selected || hovered {
        fill.unwrap_or(theme::color(theme::ACTION_FILL))
    } else {
        theme::color(theme::CHAT)
    };
    ui.painter().rect_filled(rect, radius, bg);
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            radius,
            Stroke::new(2.0, theme::TEXT_BRIGHT),
            egui::StrokeKind::Outside,
        );
    }
    paint(ui.painter(), rect, radius);
    if selected || hovered {
        let h = if selected { 40.0 } else { 20.0 };
        let pill =
            Rect::from_min_size(pos2(outer.left(), outer.center().y - h / 2.0), vec2(4.0, h));
        ui.painter().rect_filled(pill, 2.0, Color32::WHITE);
    }
    resp
}
