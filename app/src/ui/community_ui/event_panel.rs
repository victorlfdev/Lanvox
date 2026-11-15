use super::*;

pub fn events(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    let can_manage = app
        .view
        .as_ref()
        .is_some_and(|v| v.guild_permissions(guild) & permission::MANAGE_GUILD != 0);
    let events = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .events
                .iter()
                .filter(|e| e.guild == guild)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if events.is_empty() {
        ui.label("Não há eventos agendados nesta comunidade.");
    }
    for event in events {
        ui.separator();
        ui.label(RichText::new(&event.name).strong().size(18.0));
        ui.label(format!(
            "{} · {}",
            format_time(event.starts_at),
            if event.cancelled {
                "Cancelado"
            } else if event.ends_at.is_some_and(|end| end <= now_secs()) {
                "Encerrado"
            } else if event.starts_at <= now_secs() {
                "Em andamento"
            } else {
                "Agendado"
            }
        ));
        if !event.description.is_empty() {
            ui.label(&event.description);
        }
        ui.horizontal_wrapped(|ui| {
            let interested = event.interested.contains(&app.me());
            if ui
                .selectable_label(
                    interested,
                    format!(
                        "{} · {} interessados",
                        if interested {
                            "Tenho interesse"
                        } else {
                            "Participar"
                        },
                        event.interested.len()
                    ),
                )
                .clicked()
            {
                command(
                    app,
                    CommunityCommand::RespondEvent {
                        event: event.id,
                        interested: !interested,
                    },
                );
            }
            if let Some(channel) = event.channel {
                if ui.button("Abrir canal").clicked() {
                    app.select_channel(channel);
                }
            }
            if can_manage && ui.button("Editar").clicked() {
                app.workspace_ui.extras.event = event.clone();
                app.workspace_ui.extras.event_date = local_date(event.starts_at);
                app.workspace_ui.extras.event_end =
                    event.ends_at.map(local_date).unwrap_or_default();
            }
        });
    }
    if !can_manage {
        return;
    }
    ui.add_space(16.0);
    ui.collapsing("Criar ou editar evento", |ui| {
        if ui.button("Novo evento").clicked() {
            app.workspace_ui.extras.event = ScheduledEvent {
                guild,
                ..Default::default()
            };
            app.workspace_ui.extras.event_date = local_date(now_secs() + 3600);
            app.workspace_ui.extras.event_end.clear();
        }
        let mut event = app.workspace_ui.extras.event.clone();
        event.guild = guild;
        components::form_row(ui, "Nome", |ui| {
            components::text_field(ui, &mut event.name, "Partida, encontro ou conversa", 100);
        });
        components::form_row(ui, "Descrição", |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut event.description)
                    .desired_rows(3)
                    .char_limit(2000)
                    .desired_width(f32::INFINITY),
            );
        });
        ui.label("Início · horário local (AAAA-MM-DD HH:MM)");
        components::text_field(
            ui,
            &mut app.workspace_ui.extras.event_date,
            "2026-11-20 20:00",
            16,
        );
        ui.label("Fim opcional · horário local");
        components::text_field(
            ui,
            &mut app.workspace_ui.extras.event_end,
            "AAAA-MM-DD HH:MM",
            16,
        );
        let starts = parse_local_date(&app.workspace_ui.extras.event_date);
        let ends = if app.workspace_ui.extras.event_end.trim().is_empty() {
            None
        } else {
            parse_local_date(&app.workspace_ui.extras.event_end)
        };
        let dates_valid = starts.is_some()
            && (app.workspace_ui.extras.event_end.trim().is_empty()
                || ends.zip(starts).is_some_and(|(e, s)| e > s));
        if !dates_valid {
            ui.colored_label(
                theme::color(theme::YELLOW),
                "Informe horários válidos. O fim deve ser posterior ao início.",
            );
        }
        let channels = app
            .view
            .as_ref()
            .map(|v| {
                v.channels
                    .iter()
                    .filter(|c| c.guild == guild)
                    .map(|c| (c.id, c.name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("event-channel")
            .selected_text(
                event
                    .channel
                    .and_then(|id| channels.iter().find(|c| c.0 == id).map(|c| c.1.clone()))
                    .unwrap_or_else(|| "Sem canal vinculado".into()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut event.channel, None, "Sem canal vinculado");
                for (id, name) in channels {
                    ui.selectable_value(&mut event.channel, Some(id), name);
                }
            });
        ui.checkbox(&mut event.cancelled, "Evento cancelado");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    dates_valid && !event.name.trim().is_empty(),
                    egui::Button::new("Salvar evento").fill(theme::color(theme::ACCENT)),
                )
                .clicked()
            {
                event.starts_at = starts.unwrap_or(0);
                event.ends_at = ends;
                command(
                    app,
                    CommunityCommand::UpsertEvent {
                        event: event.clone(),
                    },
                );
            }
            if event.id != 0 && ui.button("Excluir evento…").clicked() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::unique(("confirm-event-delete", guild)),
                        Some((event.id, event.name.clone())),
                    )
                });
            }
        });
        let confirm_key = egui::Id::unique(("confirm-event-delete", guild));
        let confirmation = ui
            .ctx()
            .data(|d| d.get_temp::<Option<(u64, String)>>(confirm_key))
            .flatten();
        if let Some((id, name)) = confirmation {
            ui.colored_label(
                theme::YELLOW,
                format!("Excluir o evento {name}? Esta ação não pode ser desfeita."),
            );
            ui.horizontal_wrapped(|ui| {
                if components::danger_button(ui, "Confirmar exclusão").clicked() {
                    command(app, CommunityCommand::DeleteEvent { guild, id });
                    event = ScheduledEvent::default();
                    ui.ctx()
                        .data_mut(|d| d.remove::<Option<(u64, String)>>(confirm_key));
                }
                if ui.button("Cancelar").clicked() {
                    ui.ctx()
                        .data_mut(|d| d.remove::<Option<(u64, String)>>(confirm_key));
                }
            });
        }
        app.workspace_ui.extras.event = event;
    });
}

pub(super) fn local_date(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub(super) fn parse_local_date(text: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M")
        .ok()?
        .and_local_timezone(chrono::Local)
        .single()
        .map(|d| d.timestamp())
}

pub(super) fn style_name(style: ChannelStyle) -> &'static str {
    match style {
        ChannelStyle::Standard => "Texto padrão",
        ChannelStyle::Forum => "Fórum",
        ChannelStyle::Announcement => "Anúncios",
        ChannelStyle::Stage => "Palco",
    }
}

pub(super) fn format_time(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d/%m %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "Sem prazo".into())
}
