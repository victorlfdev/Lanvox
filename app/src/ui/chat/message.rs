use super::*;

pub fn message_row(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    m: &Message,
    grouped: bool,
) -> egui::Response {
    ui.add_space(if grouped {
        0.0
    } else {
        app.settings.chat_group_spacing.clamp(0.0, 32.0)
    });
    let details = app
        .view
        .as_ref()
        .and_then(|v| v.details(m.id))
        .cloned()
        .unwrap_or_default();
    let mentioned = details.mentions.contains(&app.me());
    let reply = app.view.as_ref().and_then(|v| v.reply_preview(&details));
    let mut open_reply = None;
    let hover_bg = ui.painter().add(egui::Shape::Noop);
    // The row-wide click sense must be registered BEFORE the content: egui gives clicks to the
    // widget added last, so adding it afterwards swallowed every click on the video poster,
    // images and buttons inside the message. The previous frame's rect is used for it.
    let row_id = egui::Id::unique(("message-row", m.id));
    let known_rect: Option<egui::Rect> = ui.data(|d| d.get_temp(row_id));
    let early_row = known_rect.map(|rect| ui.interact(rect, row_id, egui::Sense::click()));
    let frame_response = Frame::new()
        .inner_margin(Margin::symmetric(16, 3))
        .fill(
            if app
                .workspace_ui
                .highlighted
                .is_some_and(|(id, at)| id == m.id && at.elapsed().as_secs_f32() < 3.0)
            {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
                theme::ACCENT.gamma_multiply(0.35)
            } else if mentioned {
                theme::ACCENT.gamma_multiply(0.22)
            } else {
                egui::Color32::TRANSPARENT
            },
        )
        .show(ui, |ui| {
            if let (Some(reply), Some(id)) = (&reply, details.reply_to) {
                let label = if reply.deleted {
                    "Em resposta a uma mensagem removida".to_owned()
                } else {
                    format!(
                        "Em resposta a {}: {}",
                        reply.author_name,
                        mention_text(app, &reply.text.chars().take(90).collect::<String>())
                    )
                };
                let r = ui.add(
                    egui::Label::new(RichText::new(label).small().color(theme::TEXT_MUTED)).sense(
                        if reply.deleted {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::click()
                        },
                    ),
                );
                if !reply.deleted && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    open_reply = Some(id);
                }
            } else if let Some(id) = details.reply_to {
                // Older host: no preview and the original is outside the local cache.
                ui.label(
                    RichText::new(format!("Em resposta à mensagem {id}"))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if !grouped {
                    let texture = app.avatar_texture(ui.ctx(), m.author);
                    if Avatar::new(&m.author_name, AvatarSize::Md)
                        .user(m.author)
                        .color(m.author_color)
                        .texture(texture.as_ref())
                        .show(ui)
                        .clicked()
                    {
                        app.open_profile(m.author);
                    }
                } else {
                    ui.add_space(40.0);
                }
                // item_spacing.x já separa avatar e conteúdo.
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 12.0).max(60.0));
                    ui.spacing_mut().item_spacing.y = 4.0;
                    if !grouped {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if ui
                                .link(RichText::new(&m.author_name).strong().color(
                                    color32(m.author_color).lerp_to_gamma(theme::TEXT_BRIGHT, 0.15),
                                ))
                                .clicked()
                            {
                                app.open_profile(m.author);
                            }
                            ui.label(
                                RichText::new(time_label(m.time))
                                    .size(12.0)
                                    .color(theme::TEXT_MUTED),
                            );
                            if details.edited.is_some() {
                                ui.label(RichText::new("editada").small());
                            }
                            if details.pinned {
                                ui.label(RichText::new("Fixada").small().color(theme::YELLOW));
                            }
                        });
                    }
                    let editing = app
                        .workspace_ui
                        .editing
                        .as_ref()
                        .is_some_and(|(id, _)| *id == m.id);
                    if editing {
                        let mut text = app
                            .workspace_ui
                            .editing
                            .as_ref()
                            .map(|(_, text)| text.clone())
                            .unwrap_or_default();
                        let id = egui::Id::unique(("edit-inline", m.id));
                        let first = ui.ctx().data_mut(|d| {
                            let key = id.with("started");
                            if d.get_temp::<bool>(key).unwrap_or(false) {
                                false
                            } else {
                                d.insert_temp(key, true);
                                true
                            }
                        });
                        let response = ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .id(id)
                                .desired_width(f32::INFINITY)
                                .desired_rows(2)
                                .char_limit(MAX_MESSAGE_CHARS),
                        );
                        if first {
                            response.request_focus();
                        }
                        let escape =
                            response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
                        let enter = response.has_focus()
                            && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                        let mut save = enter;
                        let mut cancel = escape;
                        ui.horizontal(|ui| {
                            save |= ui
                                .add_enabled(!text.trim().is_empty(), egui::Button::new("Salvar"))
                                .clicked();
                            cancel |= ui.button("Cancelar").clicked();
                            ui.label(
                                RichText::new("Ctrl+Enter salva · Esc cancela")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                        if save && !text.trim().is_empty() {
                            app.workspace(WorkspaceCommand::Edit {
                                message: m.id,
                                text,
                            });
                            app.workspace_ui.editing = None;
                        } else if cancel {
                            app.workspace_ui.editing = None;
                        } else {
                            app.workspace_ui.editing = Some((m.id, text));
                        }
                        if save || cancel {
                            ui.ctx().data_mut(|d| d.remove::<bool>(id.with("started")));
                        }
                    }
                    let shown = mention_text(app, &m.text);
                    if !editing
                        && app.view.as_ref().is_none_or(|v| {
                            !v.workspace
                                .community
                                .polls
                                .iter()
                                .any(|p| p.message == m.id)
                        })
                    {
                        text_content(app, ui, &shown, m.channel);
                    }
                    poll_row(app, ui, m.id);
                    for url in crate::ui::video::video_links(&m.text) {
                        ui.add_space(4.0);
                        crate::ui::video::link(app, ui, &url);
                    }
                    for attachment in &details.attachments {
                        ui.add_space(4.0);
                        if crate::ui::video::is_video_attachment(attachment) {
                            crate::ui::video::attachment(app, ui, attachment);
                        }
                        if crate::ui::video::is_audio_attachment(attachment) {
                            crate::ui::video::audio_attachment(app, ui, attachment);
                        }
                        if attachment.media_type.starts_with("image/") {
                            if let Some(texture) = app.attachment_texture(ui.ctx(), attachment) {
                                let size = texture.size_vec2();
                                let scale = (ui.available_width().min(480.0) / size.x).min(1.0);
                                if ui
                                    .add(
                                        egui::Image::new((texture.id(), size * scale))
                                            .corner_radius(6)
                                            .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text("Abrir imagem")
                                    .clicked()
                                {
                                    app.workspace_ui.gallery = Some(attachment.id);
                                    app.workspace_ui.gallery_zoom = 1.0;
                                }
                            } else if attachment.size <= MAX_INLINE_ATTACHMENT_BYTES as u64
                                && ui
                                    .button(format!("Ver imagem: {}", attachment.name))
                                    .clicked()
                                && app.workspace_ui.preview_requests.insert(attachment.id)
                            {
                                app.workspace(WorkspaceCommand::Download {
                                    attachment: attachment.id,
                                });
                            }
                        }
                        // Vídeos baixam pelo menu "…"; os demais anexos mantêm o botão.
                        if !crate::ui::video::is_video_attachment(attachment)
                            && ui
                                .button(format!(
                                    "Baixar {} · {:.1} KiB",
                                    attachment.name,
                                    attachment.size as f64 / 1024.0
                                ))
                                .clicked()
                        {
                            app.download_attachment(attachment);
                        }
                    }
                    if !details.reactions.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for reaction in &details.reactions {
                                let mine = reaction.users.contains(&app.me());
                                let custom = reaction
                                    .emoji
                                    .strip_prefix("custom:")
                                    .and_then(|id| id.parse::<u64>().ok())
                                    .and_then(|id| custom_emoji_texture(app, ui.ctx(), id));
                                let button = if let Some(texture) = custom {
                                    egui::Button::image_and_text(
                                        egui::Image::new((texture.id(), vec2(20.0, 20.0))),
                                        reaction.users.len().to_string(),
                                    )
                                } else {
                                    egui::Button::new(format!(
                                        "{} {}",
                                        reaction.emoji,
                                        reaction.users.len()
                                    ))
                                };
                                if ui
                                    .add(
                                        button
                                            .fill(if mine {
                                                theme::ACCENT.gamma_multiply(0.3)
                                            } else {
                                                theme::color(theme::SIDEBAR)
                                            })
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                if mine {
                                                    theme::ACCENT
                                                } else {
                                                    theme::color(theme::SIDEBAR)
                                                },
                                            ))
                                            .corner_radius(8),
                                    )
                                    .clicked()
                                {
                                    app.workspace(WorkspaceCommand::React {
                                        message: m.id,
                                        emoji: reaction.emoji.clone(),
                                    });
                                }
                            }
                        });
                    }
                });
            });
        })
        .response;
    let row_rect = frame_response.rect;
    ui.data_mut(|d| d.insert_temp(row_id, row_rect));
    let response = match early_row {
        Some(response) => response,
        None => frame_response.interact(egui::Sense::click()),
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!(
                "Mensagem de {}: {}",
                m.author_name,
                mention_text(app, &m.text)
            ),
        )
    });
    if response.has_focus() {
        ui.painter().rect_stroke(
            row_rect,
            4.0,
            egui::Stroke::new(2.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Ações da mensagem")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.menu_button("Abrir ações", |ui| message_actions(app, ui, m, &details));
        });
    }
    if !mentioned && ui.rect_contains_pointer(row_rect) {
        ui.painter().set(
            hover_bg,
            egui::Shape::rect_filled(row_rect, 0.0, theme::color(theme::MESSAGE_HOVER)),
        );
    }
    if let Some(id) = open_reply {
        jump_to_message(app, m.channel, id);
    }
    response.context_menu(|ui| message_actions(app, ui, m, &details));
    message_toolbar(app, ui, m, &details, row_rect);
    response
}

/// Floating action bar shown at the top-right of the hovered message.
pub(super) fn message_toolbar(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    m: &Message,
    details: &MessageDetails,
    rect: egui::Rect,
) {
    if has_blocking_surface(app, ui.ctx()) {
        return;
    }
    const BTN: f32 = 28.0;
    let size = vec2(BTN * 4.0 + 12.0, BTN + 6.0);
    let clip = ui.clip_rect();
    if !clip.intersects(rect) || clip.width() < size.x || clip.height() < size.y {
        return;
    }
    let pos = egui::pos2(
        (rect.right() - size.x - 8.0).clamp(clip.left(), clip.right() - size.x),
        (rect.top() - 14.0).clamp(clip.top(), clip.bottom() - size.y),
    );
    let bar_rect = egui::Rect::from_min_size(pos, size);
    let ctx = ui.ctx().clone();
    let key = egui::Id::unique("message-toolbar-owner");
    let pointer = ctx.pointer_latest_pos();
    let over = ui.is_rect_visible(rect)
        && pointer.is_some_and(|p| rect.contains(p) || bar_rect.contains(p))
        && ctx
            .layer_id_at(pointer.unwrap())
            .is_none_or(|l| l == ui.layer_id() || l.id == egui::Id::unique(("message-bar", m.id)));
    if over {
        ctx.data_mut(|d| d.insert_temp(key, m.id));
    }
    let owner = ctx.data(|d| d.get_temp::<MessageId>(key));
    if !(over || (owner == Some(m.id) && egui::Popup::is_any_open(&ctx))) {
        return;
    }
    egui::Area::new(egui::Id::unique(("message-bar", m.id)))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(&ctx, |ui| {
            ui.set_clip_rect(clip);
            Frame::new()
                .fill(theme::color(theme::SIDEBAR))
                .stroke(egui::Stroke::new(
                    1.0,
                    theme::TEXT_MUTED.gamma_multiply(0.4),
                ))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(3, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let react =
                            widgets::icon_button(ui, Icon::Smile, BTN, theme::TEXT, "Reagir");
                        egui::Popup::menu(&react).show(|ui| {
                            if let Some(emoji) = emoji_picker(app, ui, m.channel, true) {
                                app.workspace(WorkspaceCommand::React {
                                    message: m.id,
                                    emoji,
                                });
                                ui.close();
                            }
                        });
                        if widgets::icon_button(ui, Icon::Reply, BTN, theme::TEXT, "Responder")
                            .clicked()
                        {
                            app.workspace_ui.reply_to = Some(m.id);
                        }
                        let can_delete = m.author == app.me()
                            || app
                                .view
                                .as_ref()
                                .and_then(|v| v.channel(m.channel))
                                .is_some_and(|c| {
                                    app.view.as_ref().is_some_and(|v| {
                                        v.has_permission(c.id, permission::MANAGE_MESSAGES)
                                    })
                                });
                        if can_delete
                            && widgets::icon_button(ui, Icon::Trash, BTN, theme::RED, "Excluir")
                                .clicked()
                        {
                            app.workspace_ui.deleting = Some(m.id);
                        }
                        let more =
                            widgets::icon_button(ui, Icon::More, BTN, theme::TEXT, "Mais ações");
                        egui::Popup::menu(&more).show(|ui| message_actions(app, ui, m, details));
                    });
                });
        });
}

pub(super) fn message_actions(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    m: &Message,
    details: &MessageDetails,
) {
    {
        if ui.button("Responder").clicked() {
            app.workspace_ui.reply_to = Some(m.id);
            ui.close();
        }
        if ui.button("Copiar texto").clicked() {
            ui.ctx().copy_text(m.text.clone());
            ui.close();
        }
        for attachment in &details.attachments {
            if crate::ui::video::is_video_attachment(attachment)
                && ui
                    .button(format!(
                        "Baixar {} · {:.1} KiB",
                        attachment.name,
                        attachment.size as f64 / 1024.0
                    ))
                    .clicked()
            {
                app.download_attachment(attachment);
                ui.close();
            }
        }
        ui.menu_button("Reagir", |ui| {
            if let Some(emoji) = emoji_picker(app, ui, m.channel, true) {
                app.workspace(WorkspaceCommand::React {
                    message: m.id,
                    emoji,
                });
                ui.close();
            }
        });
        if ui.button("Encaminhar…").clicked() {
            app.workspace_ui.forwarding = Some(m.clone());
            app.workspace_ui.forwarding_target = None;
            app.workspace_ui.forwarding_note.clear();
            ui.close();
        }
        if ui.button("Copiar link da mensagem").clicked() {
            ui.ctx().copy_text(format!(
                "lanvox://{}/message/{}/{}",
                app.current_host().split('?').next().unwrap_or(""),
                m.channel,
                m.id
            ));
            ui.close();
        }
        if ui.button("Marcar como não lida").clicked() {
            app.mark_unread(m.channel, m.id);
            ui.close();
        }
        if ui.button("Criar discussão…").clicked() {
            app.workspace_ui.thread_from = Some(m.clone());
            app.workspace_ui.thread_name.clear();
            ui.close();
        }
        if ui
            .button(if details.pinned { "Desafixar" } else { "Fixar" })
            .clicked()
        {
            app.workspace(WorkspaceCommand::Pin {
                message: m.id,
                pinned: !details.pinned,
            });
            ui.close();
        }
        if m.author == app.me() && ui.button("Editar").clicked() {
            app.workspace_ui.editing = Some((m.id, m.text.clone()));
            ui.close();
        }
        let admin = app
            .view
            .as_ref()
            .and_then(|v| v.channel(m.channel))
            .is_some_and(|c| {
                app.view
                    .as_ref()
                    .is_some_and(|v| v.has_permission(c.id, permission::MANAGE_MESSAGES))
            });
        if (m.author == app.me() || admin) && ui.button("Excluir…").clicked() {
            app.workspace_ui.deleting = Some(m.id);
            ui.close();
        }
    }
}

/// `<@id>` references shown as highlighted `@name`, resolved by identity.
pub(super) fn mention_text(app: &LanvoxApp, text: &str) -> String {
    crate::mentions::display(text, |id| {
        app.view
            .as_ref()
            .and_then(|v| v.users.get(&id))
            .map(|u| u.name.clone())
    })
}

/// Scroll to a message of this channel, fetching the page around it when it is not cached.
pub(super) fn jump_to_message(app: &mut LanvoxApp, channel: ChannelId, id: MessageId) {
    let cached = app
        .view
        .as_ref()
        .and_then(|v| v.messages.get(&channel))
        .is_some_and(|list| list.iter().any(|m| m.id == id));
    if cached {
        app.workspace_ui.history_anchor = Some(id);
    } else {
        app.open_message(&Message {
            id,
            channel,
            author: 0,
            author_name: String::new(),
            author_color: 0,
            time: 0,
            text: String::new(),
        });
    }
}

pub(super) fn text_content(app: &mut LanvoxApp, ui: &mut egui::Ui, text: &str, channel: ChannelId) {
    let guild = app
        .view
        .as_ref()
        .and_then(|v| v.channel(channel))
        .map(|c| c.guild);
    let emojis: Vec<_> = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .emojis
                .iter()
                .filter(|e| Some(e.guild) == guild)
                .map(|e| (e.id, e.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let images = emojis
        .into_iter()
        .filter_map(|(id, name)| {
            custom_emoji_texture(app, ui.ctx(), id).map(|image| (id, name, image))
        })
        .collect();
    ui.scope(|ui| {
        let size = app.settings.chat_text_size.clamp(12.0, 24.0);
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(size));
        crate::ui::markdown::render_custom(ui, text, images);
    });
}
