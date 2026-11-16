use super::*;

/// The sidebar of the Home screen: Friends, and your private call.
pub(super) fn home_list(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
    let selected = app.selected.is_none();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), "Amigos"));
    if selected {
        ui.painter()
            .rect_filled(rect, 4.0, theme::color(theme::SELECTED));
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, theme::color(theme::HOVER));
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4,
            Stroke::new(2.0, theme::TEXT_BRIGHT),
            egui::StrokeKind::Inside,
        );
    }
    icons::paint(
        ui.painter(),
        Rect::from_min_size(
            pos2(rect.left() + 10.0, rect.center().y - 11.0),
            vec2(22.0, 22.0),
        ),
        Icon::Users,
        theme::color(theme::TEXT),
    );
    ui.painter().text(
        pos2(rect.left() + 44.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Amigos",
        FontId::proportional(15.5),
        if selected {
            theme::color(theme::TEXT_BRIGHT)
        } else {
            theme::color(theme::TEXT_MUTED)
        },
    );
    if resp.clicked() {
        app.show_friends();
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        widgets::section_label(ui, "Mensagens diretas");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(
                ui,
                Icon::Plus,
                28.0,
                theme::color(theme::TEXT_MUTED),
                "Novo grupo",
            )
            .clicked()
            {
                app.workspace_ui.group_open = true;
            }
        });
    });
    ui.add_space(2.0);
    let conversations: Vec<_> = app
        .view
        .as_ref()
        .map(|v| v.workspace.conversations.clone())
        .unwrap_or_default();
    if conversations.is_empty() {
        ui.label(
            RichText::new("Abra Mensagem na lista de amigos.")
                .small()
                .color(theme::color(theme::TEXT_MUTED)),
        );
    }
    for conversation in conversations {
        let unread = app
            .view
            .as_ref()
            .and_then(|v| v.unread.get(&conversation.channel))
            .copied()
            .unwrap_or(0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
        let selected = app.selected == Some(conversation.channel);
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                ui.is_enabled(),
                format!(
                    "Conversa {}{}",
                    conversation.name,
                    if unread > 0 {
                        format!(", {unread} não lidas")
                    } else {
                        String::new()
                    }
                ),
            )
        });
        if selected {
            ui.painter()
                .rect_filled(rect, 4.0, theme::color(theme::SELECTED));
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, theme::color(theme::HOVER));
        }
        if resp.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                4,
                Stroke::new(2.0, theme::TEXT_BRIGHT),
                egui::StrokeKind::Inside,
            );
        }
        let av =
            Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(32.0, 32.0));
        let peer = if conversation.members.len() == 2 {
            app.view.as_ref().and_then(|v| {
                conversation
                    .members
                    .iter()
                    .find(|id| **id != v.me)
                    .and_then(|id| v.users.get(id))
                    .map(|u| (u.id, u.color))
            })
        } else {
            None
        };
        let texture = if let Some((id, _)) = peer {
            app.avatar_texture(ui.ctx(), id)
        } else {
            let bytes = app
                .view
                .as_ref()
                .and_then(|v| {
                    v.workspace
                        .community
                        .conversation_visuals
                        .iter()
                        .find(|v| v.channel == conversation.channel)
                })
                .map(|v| v.icon.clone())
                .unwrap_or_default();
            crate::ui::community_ui::image_texture(
                ui.ctx(),
                &format!("group-icon-{}", conversation.channel),
                &bytes,
            )
        };
        let mut dm_avatar =
            Avatar::new(&conversation.name, AvatarSize::Sm).texture(texture.as_ref());
        if let Some((id, color)) = peer {
            dm_avatar = dm_avatar
                .user(id)
                .color(color)
                .presence(PresenceState::of_user(app, id));
        } else {
            dm_avatar = dm_avatar.color(0x0e9488);
        }
        dm_avatar.paint(ui, av);
        let name_col = if selected || unread > 0 || resp.hovered() {
            theme::color(theme::TEXT_BRIGHT)
        } else {
            theme::color(theme::TEXT_MUTED)
        };
        let in_call = app.view.as_ref().and_then(|v| {
            let call = v.call_of_conversation(conversation.channel)?;
            Some(v.in_voice(call.id).len())
        });
        let call_live = in_call.is_some_and(|n| n > 0);
        ui.painter()
            .with_clip_rect(Rect::from_min_max(
                pos2(rect.left() + 50.0, rect.top()),
                pos2(
                    rect.right() - if unread > 0 { 28.0 } else { 4.0 },
                    rect.bottom(),
                ),
            ))
            .text(
                pos2(
                    rect.left() + 50.0,
                    rect.center().y - if call_live { 7.0 } else { 0.0 },
                ),
                Align2::LEFT_CENTER,
                &conversation.name,
                FontId::proportional(15.0),
                name_col,
            );
        if call_live {
            ui.painter().text(
                pos2(rect.left() + 50.0, rect.center().y + 9.0),
                Align2::LEFT_CENTER,
                "Em uma chamada",
                FontId::proportional(11.5),
                theme::color(theme::GREEN),
            );
        }
        if unread > 0 {
            let badge = Rect::from_center_size(
                pos2(rect.right() - 16.0, rect.center().y),
                vec2(18.0, 16.0),
            );
            ui.painter()
                .rect_filled(badge, 8.0, theme::color(theme::RED));
            ui.painter().text(
                badge.center(),
                Align2::CENTER_CENTER,
                unread.to_string(),
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        if resp.clicked() {
            app.select_channel(conversation.channel);
        }
        resp.on_hover_text(&conversation.name);
    }
    let calls: Vec<(ChannelId, GuildId, String)> = app
        .view
        .as_ref()
        .map(|v| {
            v.my_voice()
                .channel
                .and_then(|id| v.channel(id))
                .map(|c| vec![(c.id, c.guild, c.name.clone())])
                .unwrap_or_default()
        })
        .unwrap_or_default();
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        widgets::section_label(ui, "Sua chamada");
    });
    if calls.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new("Você não está em uma chamada ou sala de voz.")
                        .size(12.5)
                        .color(theme::color(theme::TEXT_FAINT)),
                )
                .wrap(),
            );
        });
    }
    for (id, guild, name) in calls {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                ui.is_enabled(),
                format!("Voz conectada: {name}"),
            )
        });
        let sel = app.selected == Some(id);
        if sel {
            ui.painter()
                .rect_filled(rect, 4.0, theme::color(theme::SELECTED));
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, theme::color(theme::HOVER));
        }
        if resp.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                4,
                Stroke::new(2.0, theme::TEXT_BRIGHT),
                egui::StrokeKind::Inside,
            );
        }
        icons::paint(
            ui.painter(),
            Rect::from_min_size(
                pos2(rect.left() + 10.0, rect.center().y - 10.0),
                vec2(20.0, 20.0),
            ),
            Icon::Speaker,
            theme::color(theme::GREEN),
        );
        ui.painter().text(
            pos2(rect.left() + 40.0, rect.center().y),
            Align2::LEFT_CENTER,
            &name,
            FontId::proportional(14.5),
            theme::color(theme::TEXT_BRIGHT),
        );
        if resp.clicked() {
            if guild != 0 {
                app.home = false;
                app.guild = Some(guild);
            }
            app.select_channel(id);
        }
        voice_members(app, ui, id);
    }
}
