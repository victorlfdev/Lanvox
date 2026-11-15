use super::*;

/// Forum channels present their posts; thread channels continue using the normal chat.
pub fn forum(app: &mut LanvoxApp, ui: &mut Ui) -> bool {
    let Some(channel) = app.selected else {
        return false;
    };
    let forum = app.view.as_ref().is_some_and(|v| {
        v.workspace
            .community
            .policies
            .iter()
            .any(|p| p.channel == channel && p.style == ChannelStyle::Forum)
    });
    if !forum {
        return false;
    }
    ui.add_space(16.0);
    ui.heading("Publicações do fórum");
    ui.label("Crie uma publicação ou encontre uma conversa existente.");
    let draft_key = egui::Id::unique(("forum-draft", channel));
    let mut draft = ui
        .ctx()
        .data(|d| d.get_temp::<String>(draft_key))
        .unwrap_or_default();
    if app
        .workspace_ui
        .forum_failed_draft
        .as_ref()
        .is_some_and(|(parent, _)| *parent == channel)
    {
        if let Some((_, recovered)) = app.workspace_ui.forum_failed_draft.take() {
            if !draft.is_empty() {
                draft.push_str("\n\n");
            }
            draft.push_str(&recovered);
            ui.colored_label(
                theme::YELLOW,
                "A discussão não foi criada. Seu texto foi recuperado abaixo.",
            );
        }
    }
    ui.horizontal_wrapped(|ui| {
        components::text_field(
            ui,
            &mut app.workspace_ui.extras.thread_name,
            "Título da discussão",
            100,
        );
    });
    ui.add(
        egui::TextEdit::multiline(&mut draft)
            .hint_text("Escreva a primeira mensagem (opcional)")
            .desired_width(f32::INFINITY)
            .desired_rows(3)
            .char_limit(MAX_MESSAGE_CHARS),
    );
    ui.label(
        RichText::new("O texto será preparado na nova discussão para você conferir e publicar.")
            .small()
            .color(theme::TEXT_MUTED),
    );
    if ui
        .add_enabled(
            !app.workspace_ui.extras.thread_name.trim().is_empty()
                && app.workspace_ui.forum_pending_draft.is_none(),
            egui::Button::new("Criar discussão").fill(theme::color(theme::ACCENT)),
        )
        .clicked()
    {
        if !draft.trim().is_empty() {
            app.workspace_ui.forum_pending_draft = Some((channel, draft.clone()));
        } else {
            app.workspace_ui.forum_pending_draft = None;
        }
        command(
            app,
            CommunityCommand::CreateThread {
                parent: channel,
                message: None,
                name: app.workspace_ui.extras.thread_name.clone(),
            },
        );
        app.workspace_ui.extras.thread_name.clear();
        draft.clear();
    }
    ui.ctx().data_mut(|d| d.insert_temp(draft_key, draft));
    components::text_field(
        ui,
        &mut app.workspace_ui.extras.forum_filter,
        "Buscar título ou tag",
        80,
    );
    let threads = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .threads
                .iter()
                .filter(|t| t.parent == channel)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let filter = app.workspace_ui.extras.forum_filter.to_lowercase();
    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut shown = 0;
        for thread in &threads {
            let name = app
                .view
                .as_ref()
                .and_then(|v| v.channel(thread.channel))
                .map(|c| c.name.clone())
                .unwrap_or_default();
            if !filter.is_empty()
                && !name.to_lowercase().contains(&filter)
                && !thread
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&filter))
            {
                continue;
            }
            shown += 1;
            let (author, latest) = app
                .view
                .as_ref()
                .map(|v| {
                    (
                        v.user_name(thread.creator),
                        v.messages
                            .get(&thread.channel)
                            .and_then(|m| m.last())
                            .cloned(),
                    )
                })
                .unwrap_or_default();
            ui.separator();
            if ui.link(RichText::new(name).strong().size(18.0)).clicked() {
                app.select_channel(thread.channel);
            }
            ui.label(
                RichText::new(format!("Criada por {author}"))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            if let Some(message) = latest {
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        message.author_name,
                        format_time(message.time)
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                ui.label(message.text.chars().take(160).collect::<String>());
            }
            ui.horizontal_wrapped(|ui| {
                for tag in &thread.tags {
                    ui.label(
                        RichText::new(tag)
                            .small()
                            .color(theme::color(theme::TEXT_MUTED)),
                    );
                }
                if thread.archived {
                    ui.label("Arquivada");
                }
            });
        }
        if shown == 0 {
            ui.add_space(24.0);
            ui.label(if threads.is_empty() {
                "Ainda não há discussões. Crie o primeiro assunto acima."
            } else {
                "Nenhuma discussão corresponde à busca."
            });
        }
    });
    true
}

pub fn threads_panel(app: &mut LanvoxApp, ui: &mut Ui) {
    let Some(channel) = app.selected else {
        return;
    };
    let current = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .community
                .threads
                .iter()
                .find(|t| t.channel == channel)
        })
        .cloned();
    let children = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .threads
                .iter()
                .filter(|t| t.parent == channel && !t.archived)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if current.is_none() && children.is_empty() {
        return;
    }
    egui::Panel::top("thread-context")
        .frame(
            egui::Frame::new()
                .fill(theme::color(theme::PANEL))
                .inner_margin(8),
        )
        .show(ui, |ui| {
            if let Some(thread) = current {
                if app.workspace_ui.extras.thread_tags_channel != Some(channel) {
                    app.workspace_ui.extras.thread_tags_channel = Some(channel);
                    app.workspace_ui.extras.thread_tags = thread.tags.join(", ");
                }
                ui.horizontal_wrapped(|ui| {
                    let parent = app
                        .view
                        .as_ref()
                        .and_then(|v| v.channel(thread.parent))
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    if ui.link(format!("Voltar a #{parent}")).clicked() {
                        app.select_channel(thread.parent);
                    }
                    if ui
                        .button(if thread.archived {
                            "Reabrir discussão"
                        } else {
                            "Arquivar discussão"
                        })
                        .clicked()
                    {
                        command(
                            app,
                            CommunityCommand::SetThreadArchived {
                                channel,
                                archived: !thread.archived,
                            },
                        );
                    }
                    ui.menu_button("Tags", |ui| {
                        components::text_field(
                            ui,
                            &mut app.workspace_ui.extras.thread_tags,
                            "Separe por vírgulas",
                            160,
                        );
                        if ui.button("Salvar tags").clicked() {
                            command(
                                app,
                                CommunityCommand::SetThreadTags {
                                    channel,
                                    tags: app
                                        .workspace_ui
                                        .extras
                                        .thread_tags
                                        .split(',')
                                        .map(|t| t.trim().to_owned())
                                        .filter(|t| !t.is_empty())
                                        .collect(),
                                },
                            );
                            ui.close();
                        }
                    });
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Discussões").strong());
                    for thread in children {
                        let name = app
                            .view
                            .as_ref()
                            .and_then(|v| v.channel(thread.channel))
                            .map(|c| c.name.clone())
                            .unwrap_or_default();
                        if ui.link(name).clicked() {
                            app.select_channel(thread.channel);
                        }
                    }
                });
            }
        });
}
