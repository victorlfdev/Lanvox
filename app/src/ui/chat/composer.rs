use super::*;

pub(super) fn composer(app: &mut LanvoxApp, ui: &mut egui::Ui, ch: &Channel) {
    let can_write = app.demo || app.view.as_ref().is_some_and(|v| v.can_send(ch.id));
    if !can_write {
        if app
            .voice_note
            .as_ref()
            .is_some_and(|note| note.channel == ch.id)
        {
            app.voice_note = None;
        }
        ui.label(RichText::new("Você não pode enviar mensagens aqui. Verifique suas permissões ou se a discussão foi arquivada.").small().color(theme::TEXT_MUTED));
        return;
    }
    let batch_pending = app.workspace_ui.pending_batches.contains_key(&ch.id);
    let can_attach = !batch_pending
        && (app.demo
            || app
                .view
                .as_ref()
                .is_some_and(|v| v.has_permission(ch.id, permission::ATTACH)));
    let dropped = ui.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .collect::<Vec<_>>()
    });
    if !dropped.is_empty() && can_attach && !has_blocking_surface(app, ui.ctx()) {
        app.stage_paths(ch.id, dropped);
    }
    if ui.input(|i| !i.raw.hovered_files.is_empty()) {
        ui.label(RichText::new("Solte seus arquivos aqui para anexar").color(theme::ACCENT));
    }
    voice_note_controls(app, ui, ch.id, can_attach);
    staged_files(app, ui, ch.id);
    let typing = app
        .view
        .as_ref()
        .map(|v| v.typing_in(ch.id))
        .unwrap_or_default();
    if let Some(reply) = app.workspace_ui.reply_to {
        let preview = app.view.as_ref().and_then(|v| {
            v.reply_preview(&MessageDetails {
                reply_to: Some(reply),
                ..Default::default()
            })
        });
        let label = match preview {
            Some(p) if !p.deleted => format!(
                "Respondendo a {}: {}",
                p.author_name,
                mention_text(app, &p.text.chars().take(80).collect::<String>())
            ),
            // Replying starts from a cached message, so a miss means it was removed.
            _ => "Respondendo a uma mensagem removida".to_owned(),
        };
        ui.horizontal(|ui| {
            if ui
                .add(egui::Label::new(label).sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                jump_to_message(app, ch.id, reply);
            }
            if ui.button("Cancelar resposta").clicked() {
                app.workspace_ui.reply_to = None;
            }
        });
    }
    let attachments = app
        .workspace_ui
        .attachments
        .get(&ch.id)
        .cloned()
        .unwrap_or_default();
    for attachment in attachments {
        ui.horizontal(|ui| {
            ui.label(format!("Anexo: {}", attachment.name));
            if ui.small_button("Remover").clicked() {
                if let Some(files) = app.workspace_ui.attachments.get_mut(&ch.id) {
                    files.retain(|f| f.id != attachment.id);
                }
            }
        });
    }
    let online = app.demo || app.client.as_ref().is_some_and(|c| c.is_alive());
    let mut send = false;
    let mut changed = false;
    let mut attach = false;
    let mut record_voice = false;
    Frame::new()
        .fill(theme::color(theme::INPUT))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let add = widgets::icon_button(
                    ui,
                    Icon::PlusCircle,
                    30.0,
                    theme::TEXT_MUTED,
                    "Adicionar à mensagem",
                );
                egui::Popup::menu(&add).show(|ui| {
                    if ui
                        .add_enabled(
                            online && can_attach && app.file_pending.is_none(),
                            egui::Button::new("Anexar arquivo…"),
                        )
                        .clicked()
                    {
                        attach = true;
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            online
                                && can_attach
                                && !app.demo
                                && app.voice_note.is_none()
                                && !app
                                    .audio_shared
                                    .muted
                                    .load(std::sync::atomic::Ordering::Relaxed)
                                && !app
                                    .audio_shared
                                    .deafened
                                    .load(std::sync::atomic::Ordering::Relaxed),
                            egui::Button::new("Gravar mensagem de voz…"),
                        )
                        .on_hover_text("Grave, ouça e escolha anexar antes de enviar")
                        .clicked()
                    {
                        record_voice = true;
                        ui.close();
                    }
                    if ui
                        .add_enabled(online, egui::Button::new("Criar enquete…"))
                        .clicked()
                    {
                        app.workspace_ui.poll_channel = Some(ch.id);
                        app.workspace_ui.poll_question.clear();
                        app.workspace_ui.poll_options.clear();
                        app.workspace_ui.poll_multiple = false;
                        ui.close();
                    }
                });
                let width = (ui.available_width() - 82.0).max(60.0);
                let compose_id = egui::Id::unique(("compose", ch.id));
                let members = app
                    .view
                    .as_ref()
                    .map(|v| v.channel_members(ch.id))
                    .unwrap_or_default();
                let hits = mention_hits(app, ch.id, &members);
                let mut picked = None;
                if !hits.is_empty() {
                    // Consumed before the editor sees them, so Enter/Tab/arrows drive the list.
                    let (down, up, accept, esc) = ui.input_mut(|i| {
                        (
                            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                                | i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                        )
                    });
                    let sel = &mut app.workspace_ui.mention_sel;
                    *sel = (*sel).min(hits.len() - 1);
                    if down {
                        *sel = (*sel + 1) % hits.len();
                    }
                    if up {
                        *sel = (*sel + hits.len() - 1) % hits.len();
                    }
                    if accept {
                        picked = Some(hits[*sel].1.clone());
                    }
                    if esc {
                        app.workspace_ui.mention_cursor = None;
                    }
                }
                if let Some(name) = picked {
                    accept_mention(app, ui.ctx(), compose_id, ch.id, &name);
                    app.workspace_ui.mention_sel = 0;
                }
                let text = app.compose.entry(ch.id).or_default();
                let output = ui
                    .add_enabled_ui(!batch_pending, |ui| {
                        egui::TextEdit::multiline(text)
                            .id(compose_id)
                            .hint_text(format!("Conversar em {}", ch.name))
                            .desired_rows(1)
                            .desired_width(width)
                            .char_limit(MAX_MESSAGE_CHARS)
                            .frame(egui::Frame::NONE)
                            .return_key(Some(egui::KeyboardShortcut::new(
                                egui::Modifiers::SHIFT,
                                egui::Key::Enter,
                            )))
                            .show(ui)
                    })
                    .inner;
                let response = output.response;
                if response.has_focus()
                    && can_attach
                    && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::V))
                {
                    app.paste_image(ch.id);
                }
                if response.has_focus()
                    && app.compose.get(&ch.id).is_none_or(|t| t.is_empty())
                    && hits.is_empty()
                    && ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
                {
                    if let Some(last) = app
                        .view
                        .as_ref()
                        .and_then(|v| v.messages.get(&ch.id))
                        .and_then(|list| list.iter().rev().find(|m| m.author == app.me()))
                        .cloned()
                    {
                        app.workspace_ui.editing = Some((last.id, last.text));
                    }
                }
                app.workspace_ui.mention_cursor = if response.has_focus() {
                    output.cursor_range.map(|r| r.primary.index.0)
                } else if app.workspace_ui.mention_cursor.is_some() && !hits.is_empty() {
                    app.workspace_ui.mention_cursor // a click on the list is in flight
                } else {
                    None
                };
                if !hits.is_empty() {
                    if let Some(name) = mention_popup(ui, app, response.rect, &hits) {
                        accept_mention(app, ui.ctx(), compose_id, ch.id, &name);
                        ui.memory_mut(|m| m.request_focus(compose_id));
                    }
                }
                changed = response.changed();
                send = response.has_focus()
                    && hits.is_empty()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                let has_text = app
                    .compose
                    .get(&ch.id)
                    .is_some_and(|t| !t.trim().is_empty());
                let emoji_button = ui
                    .add_enabled_ui(!batch_pending, |ui| {
                        widgets::icon_button(
                            ui,
                            Icon::Smile,
                            28.0,
                            theme::TEXT_MUTED,
                            "Adicionar emoji",
                        )
                    })
                    .inner;
                egui::Popup::menu(&emoji_button).show(|ui| {
                    if let Some(emoji) = emoji_picker(app, ui, ch.id, false) {
                        let text = app.compose.entry(ch.id).or_default();
                        if text.chars().count() + emoji.chars().count() <= MAX_MESSAGE_CHARS {
                            text.push_str(&emoji);
                        }
                        ui.close();
                    }
                });
                send |= ui
                    .add_enabled_ui(
                        online
                            && app.voice_note.is_none()
                            && !app
                                .workspace_ui
                                .uploads
                                .values()
                                .any(|channel| *channel == ch.id),
                        |ui| {
                            widgets::icon_button(
                                ui,
                                Icon::Send,
                                30.0,
                                if has_text {
                                    theme::ACCENT
                                } else {
                                    theme::TEXT_FAINT
                                },
                                "Enviar",
                            )
                        },
                    )
                    .inner
                    .clicked();
            });
        });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if !online {
            ui.label(
                RichText::new("Sem conexão · rascunho preservado neste dispositivo")
                    .small()
                    .color(theme::YELLOW),
            );
        } else if !typing.is_empty() {
            ui.label(RichText::new(format!("{} digitando…", typing.join(", "))).small());
        } else {
            ui.label(
                RichText::new(
                    "Enter envia · Shift+Enter quebra a linha · botão direito abre ações",
                )
                .size(11.0)
                .color(theme::TEXT_MUTED),
            );
        }
    });
    if changed {
        app.typed(ch.id);
    }
    if attach {
        app.pick_attachment(ch.id);
    }
    if record_voice {
        match crate::media::audio::voice_note::Recorder::start(
            app.settings.input_device.clone(),
            app.audio_shared.clone(),
        ) {
            Ok(recorder) => {
                app.voice_note = Some(crate::app::VoiceNote {
                    host: app.current_host(),
                    channel: ch.id,
                    recorder: Some(recorder),
                    bytes: None,
                    preview: None,
                })
            }
            Err(error) => app.notify(error),
        }
    }
    if send && online && app.voice_note.is_none() {
        let count = app.workspace_ui.staged.get(&ch.id).map_or(0, Vec::len);
        if count > 0 {
            app.begin_attachment_batch(ch.id);
        } else if !app
            .workspace_ui
            .uploads
            .values()
            .any(|channel| *channel == ch.id)
        {
            app.send_message(ch.id);
        }
    }
}

/// Recording is local and requires a separate preview/attach action before sending.
fn voice_note_controls(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    channel: ChannelId,
    can_attach: bool,
) {
    use crate::media::player::{PlayState, VideoPlayer, VideoSource};
    let Some(mut note) = app.voice_note.take() else {
        return;
    };
    if note.channel != channel || note.host != app.current_host() || !can_attach {
        // Dropping an active recorder immediately stops its microphone worker.
        return;
    }
    let mut discard = false;
    let mut finish = false;
    let mut attach = false;
    Frame::new()
        .fill(theme::color(theme::SIDEBAR))
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            if let Some(recorder) = &note.recorder {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(if recorder.ready() {
                            "Gravando mensagem de voz"
                        } else {
                            "Abrindo microfone…"
                        })
                        .strong(),
                    );
                    ui.label(format!(
                        "{:02}:{:02} / 02:00",
                        recorder.duration_seconds() / 60,
                        recorder.duration_seconds() % 60
                    ));
                    if ui
                        .add_enabled(recorder.ready(), egui::Button::new("Parar e ouvir"))
                        .clicked()
                    {
                        finish = true;
                    }
                    if ui.button("Cancelar gravação").clicked() {
                        discard = true;
                    }
                });
                if recorder.finished() {
                    finish = true;
                }
                ui.label(
                    RichText::new("O áudio fica neste dispositivo até você anexar e enviar.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(150));
            } else if let Some(bytes) = &note.bytes {
                let duration =
                    bytes.len().saturating_sub(44) / (crate::media::audio::RATE as usize * 2);
                ui.label(
                    RichText::new(format!(
                        "Mensagem de voz · {:02}:{:02}",
                        duration / 60,
                        duration % 60
                    ))
                    .strong(),
                );
                ui.horizontal_wrapped(|ui| {
                    let label = match note.preview.as_ref().map(|p| p.state()) {
                        Some(PlayState::Playing) => "Pausar",
                        Some(PlayState::Loading) => "Carregando…",
                        _ => "Ouvir prévia",
                    };
                    if ui.button(label).clicked() {
                        if let Some(player) = &note.preview {
                            if player.state() == PlayState::Failed {
                                note.preview = Some(VideoPlayer::start(VideoSource::Bytes {
                                    name: "mensagem-de-voz.wav".into(),
                                    data: bytes.clone(),
                                }));
                            } else {
                                player.toggle();
                            }
                        } else {
                            note.preview = Some(VideoPlayer::start(VideoSource::Bytes {
                                name: "mensagem-de-voz.wav".into(),
                                data: bytes.clone(),
                            }));
                        }
                    }
                    if ui.button("Anexar gravação").clicked() {
                        attach = true;
                    }
                    if ui.button("Descartar").clicked() {
                        discard = true;
                    }
                });
                if let Some(player) = &note.preview {
                    player.touch();
                    if player.state() == PlayState::Failed {
                        ui.label(
                            RichText::new(format!(
                                "Não foi possível reproduzir a prévia: {}",
                                player.error()
                            ))
                            .color(theme::YELLOW),
                        );
                    } else if matches!(player.state(), PlayState::Loading | PlayState::Playing) {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(150));
                    }
                }
                ui.label(
                    RichText::new("Anexe a gravação e envie a mensagem quando estiver pronta.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
        });
    if discard {
        return;
    }
    if finish {
        if let Some(recorder) = note.recorder.take() {
            match recorder.finish() {
                Ok(bytes) => note.bytes = Some(bytes),
                Err(error) => {
                    app.notify(error);
                    return;
                }
            }
        }
    }
    if attach {
        if let Some(bytes) = &note.bytes {
            let staged_bytes: usize = app
                .workspace_ui
                .staged
                .values()
                .flatten()
                .map(|file| file.bytes.len())
                .sum();
            let count = app.workspace_ui.staged.get(&channel).map_or(0, Vec::len)
                + app
                    .workspace_ui
                    .attachments
                    .get(&channel)
                    .map_or(0, Vec::len)
                + app
                    .workspace_ui
                    .uploads
                    .values()
                    .filter(|ch| **ch == channel)
                    .count();
            if count >= 10 || staged_bytes.saturating_add(bytes.len()) > 2 * MAX_ATTACHMENT_BYTES {
                app.notify("Não há espaço para anexar a gravação. Envie ou remova outros arquivos primeiro.");
                app.voice_note = Some(note);
                return;
            }
        }
        if let Some(bytes) = note.bytes.take() {
            app.stage_attachment(
                channel,
                format!("mensagem-de-voz-{:08x}.wav", rand::random::<u32>()),
                bytes,
            );
        }
        return;
    }
    app.voice_note = Some(note);
}

pub(super) fn staged_files(app: &mut LanvoxApp, ui: &mut egui::Ui, channel: ChannelId) {
    if app.workspace_ui.pending_batches.contains_key(&channel) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Preparando mensagem e anexos…")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            if ui.small_button("Cancelar preparação").clicked() {
                app.workspace_ui.pending_batches.remove(&channel);
                app.workspace_ui.send_after_upload.remove(&channel);
                let active: Vec<_> = app
                    .workspace_ui
                    .uploads
                    .iter()
                    .filter(|(_, ch)| **ch == channel)
                    .map(|(operation, _)| operation.clone())
                    .collect();
                for operation in active {
                    app.cancel_upload(operation);
                }
            }
        });
    }
    let mut remove = None;
    if let Some(files) = app.workspace_ui.staged.get_mut(&channel) {
        if !files.is_empty() {
            egui::ScrollArea::horizontal()
                .id_salt(("staged", channel))
                .max_height(145.0)
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for (index, file) in files.iter_mut().enumerate() {
                            Frame::new()
                                .fill(theme::color(theme::SIDEBAR))
                                .corner_radius(6)
                                .inner_margin(8)
                                .show(ui, |ui| {
                                    ui.set_width(150.0);
                                    if file.preview.is_none() {
                                        file.preview = image_texture(
                                            ui.ctx(),
                                            &format!("draft-{channel}-{index}-{}", file.name),
                                            &file.bytes,
                                            Some(140),
                                        );
                                    }
                                    if let Some(texture) = &file.preview {
                                        let size = texture.size_vec2();
                                        let scale = (140.0 / size.x).min(70.0 / size.y).min(1.0);
                                        ui.add(
                                            egui::Image::new((texture.id(), size * scale))
                                                .corner_radius(4),
                                        );
                                    }
                                    ui.add(egui::Label::new(&file.name).truncate())
                                        .on_hover_text(&file.name);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{:.1} KiB",
                                                file.bytes.len() as f64 / 1024.0
                                            ))
                                            .small()
                                            .color(theme::TEXT_MUTED),
                                        );
                                        if ui.small_button("Remover").clicked() {
                                            remove = Some(index);
                                        }
                                    });
                                });
                        }
                    });
                });
            ui.label(
                RichText::new("Os arquivos serão enviados com sua mensagem.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        }
    }
    if let Some(index) = remove {
        if let Some(files) = app.workspace_ui.staged.get_mut(&channel) {
            let file = files.remove(index);
            app.remove_attachment_draft(&file.operation);
        }
    }
    let uploads: Vec<_> = app
        .workspace_ui
        .uploads
        .iter()
        .filter(|(_, ch)| **ch == channel)
        .map(|(op, _)| {
            (
                op.clone(),
                app.workspace_ui
                    .upload_names
                    .get(op)
                    .cloned()
                    .unwrap_or_else(|| "Arquivo".into()),
            )
        })
        .collect();
    for (operation, name) in uploads {
        ui.horizontal(|ui| {
            ui.spinner();
            if app.workspace_ui.cancelled_uploads.contains(&operation) {
                ui.label(format!("{name} retirado · aguardando confirmação…"));
            } else {
                match app.upload_fraction(&operation) {
                    Some(fraction) => {
                        let progress = app.workspace_ui.chunk_uploads.get(&operation);
                        let size = progress.map(|p| format!("{:.1} / {:.1} MiB · ", p.acked as f64 / 1_048_576.0, p.total as f64 / 1_048_576.0)).unwrap_or_default();
                        ui.label(format!("Enviando {name} · {size}{:.0}%", fraction * 100.0))
                    }
                    None => ui.label(format!("Enviando {name}…")),
                };
                if ui
                    .small_button("Retirar do envio")
                    .on_hover_text("Cancela o envio em blocos; arquivos menores em trânsito são ignorados quando o host confirmar.")
                    .clicked()
                {
                    app.cancel_upload(operation);
                }
            }
        });
    }
}

pub(super) fn pending_rows(app: &mut LanvoxApp, ui: &mut egui::Ui, channel: ChannelId) {
    let host = app.current_host();
    let pending: Vec<_> = app
        .settings
        .pending
        .iter()
        .filter(|p| p.host == host && p.identity == app.local_identity && p.channel == channel)
        .cloned()
        .collect();
    for message in pending {
        Frame::new()
            .inner_margin(Margin::symmetric(70, 8))
            .show(ui, |ui| {
                ui.label(RichText::new(&message.text).color(theme::TEXT_MUTED));
                if let Some(error) = &message.error {
                    ui.label(RichText::new(error).small().color(theme::YELLOW));
                    ui.horizontal(|ui| {
                        if ui.button("Tentar novamente").clicked() {
                            app.retry_pending(&message.operation);
                        }
                        if ui.button("Voltar ao rascunho").clicked() {
                            app.compose
                                .entry(channel)
                                .or_default()
                                .push_str(&message.text);
                            app.settings
                                .pending
                                .retain(|p| p.operation != message.operation);
                            app.save_drafts();
                        }
                    });
                } else {
                    ui.label(
                        RichText::new("Enviando · aguardando gravação no servidor")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
            });
    }
}

/// Members matching the `@query` at the caret, if the composer is showing one.
pub(super) fn mention_hits(
    app: &LanvoxApp,
    channel: ChannelId,
    members: &[(UserId, String)],
) -> Vec<(UserId, String)> {
    let (Some(cursor), Some(text)) = (app.workspace_ui.mention_cursor, app.compose.get(&channel))
    else {
        return Vec::new();
    };
    let end = text
        .char_indices()
        .nth(cursor)
        .map_or(text.len(), |(b, _)| b);
    match crate::mentions::active_query(&text[..end]) {
        Some((_, query)) => crate::mentions::suggestions(members, query),
        None => Vec::new(),
    }
}

/// Replace the `@query` before the caret with `@name ` and move the caret after it.
pub(super) fn accept_mention(
    app: &mut LanvoxApp,
    ctx: &egui::Context,
    id: egui::Id,
    channel: ChannelId,
    name: &str,
) {
    let Some(cursor) = app.workspace_ui.mention_cursor else {
        return;
    };
    let text = app.compose.entry(channel).or_default();
    let end = text
        .char_indices()
        .nth(cursor)
        .map_or(text.len(), |(b, _)| b);
    let Some((at, _)) = crate::mentions::active_query(&text[..end]) else {
        return;
    };
    let insert = format!("@{name} ");
    text.replace_range(at..end, &insert);
    let caret = text[..at].chars().count() + insert.chars().count();
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(caret),
        )));
    egui::TextEdit::store_state(ctx, id, state);
    app.workspace_ui.mention_cursor = Some(caret);
}

/// The suggestion list above the composer; returns the name clicked, if any.
pub(super) fn mention_popup(
    ui: &mut egui::Ui,
    app: &mut LanvoxApp,
    anchor: egui::Rect,
    hits: &[(UserId, String)],
) -> Option<String> {
    let mut clicked = None;
    egui::Area::new(egui::Id::unique("mention-popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(anchor.left_top() - vec2(0.0, 8.0))
        .show(ui.ctx(), |ui| {
            Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                for (n, (_, name)) in hits.iter().enumerate() {
                    let r =
                        ui.selectable_label(n == app.workspace_ui.mention_sel, format!("@{name}"));
                    // Pressing (not releasing) keeps the editor focused while picking.
                    if r.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
                        clicked = Some(name.clone());
                    }
                }
            });
        });
    clicked
}
