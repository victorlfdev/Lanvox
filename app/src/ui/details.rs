//! Conversation context: searchable history, pinned messages and attention inbox.
use super::{components, icons::Icon, theme, widgets};
use crate::{app::LanvoxApp, model::*};
use egui::{RichText, Ui};

pub fn show(app: &mut LanvoxApp, ui: &mut Ui) -> bool {
    let searching = app.workspace_ui.search_open;
    let inbox = app.workspace_ui.inbox_open;
    if !searching && !inbox && !app.workspace_ui.pinned_only {
        return false;
    }
    if ui.available_width() < 850.0 {
        let ctx = ui.ctx().clone();
        let result = components::modal(
            &ctx,
            "conversation-details-overlay",
            if searching {
                "Buscar mensagens"
            } else if inbox {
                "Caixa de entrada"
            } else {
                "Mensagens fixadas"
            },
            components::ModalOptions::new(540.0).secondary("Voltar à conversa"),
            |ui| content(app, ui, searching, inbox),
        );
        if result.cancelled() {
            close(app);
        }
        return true;
    }
    let width = (ui.available_width() * 0.43).clamp(280.0, 410.0);
    egui::Panel::right("conversation-details")
        .default_size(width)
        .min_size(260.0)
        .max_size(480.0)
        .resizable(true)
        .frame(
            egui::Frame::new()
                .fill(theme::color(theme::SIDEBAR))
                .inner_margin(14),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if searching {
                        "Buscar mensagens"
                    } else if inbox {
                        "Caixa de entrada"
                    } else {
                        "Mensagens fixadas"
                    })
                    .strong()
                    .size(18.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(
                        ui,
                        Icon::Close,
                        26.0,
                        theme::color(theme::TEXT_MUTED),
                        "Fechar painel",
                    )
                    .clicked()
                    {
                        close(app);
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("details-content")
                .show(ui, |ui| content(app, ui, searching, inbox));
        });
    true
}

fn close(app: &mut LanvoxApp) {
    app.workspace_ui.search_open = false;
    app.workspace_ui.inbox_open = false;
    app.workspace_ui.inbox_requested = None;
    app.workspace_ui.pinned_only = false;
}

fn content(app: &mut LanvoxApp, ui: &mut Ui, searching: bool, inbox: bool) {
    if searching {
        search(app, ui);
    } else if inbox {
        inbox_content(app, ui);
    } else {
        let messages = app
            .selected
            .and_then(|c| app.workspace_ui.pinned_messages.get(&c))
            .cloned()
            .unwrap_or_default();
        if messages.is_empty() {
            ui.label("Ainda não há mensagens fixadas. Abra as ações de uma mensagem para fixá-la.");
        }
        result_rows(app, ui, &messages, "pinned-list");
    }
}

fn search(app: &mut LanvoxApp, ui: &mut Ui) {
    let scope = format!(
        "{}|{}",
        app.local_identity,
        crate::settings::host_address(&app.current_host())
    );
    let after_key = format!("{scope}|search-after");
    let before_key = format!("{scope}|search-before");
    let signature = format!("{:?}", app.workspace_ui.search_request);
    if app.workspace_ui.extras.search_signature != signature {
        app.workspace_ui.extras.search_draft = Some(app.workspace_ui.search_request.clone());
        app.workspace_ui.extras.search_signature = signature;
    }
    let mut request = app
        .workspace_ui
        .extras
        .search_draft
        .clone()
        .unwrap_or_else(|| app.workspace_ui.search_request.clone());
    let response = ui.add(
        egui::TextEdit::singleline(&mut request.query)
            .hint_text("Palavras da mensagem")
            .desired_width(f32::INFINITY),
    );
    let mut submit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.collapsing("Filtros e ordenação", |ui| {
        let channels: Vec<_> = app
            .view
            .as_ref()
            .map(|v| v.channels.iter().map(|c| (c.id, c.name.clone())).collect())
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("search-channel")
            .selected_text(
                request
                    .channel
                    .and_then(|id| channels.iter().find(|c| c.0 == id).map(|c| c.1.clone()))
                    .unwrap_or_else(|| "Todas as conversas".into()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut request.channel, None, "Todas as conversas");
                for (id, name) in &channels {
                    ui.selectable_value(&mut request.channel, Some(*id), name);
                }
            });
        let mut users: Vec<_> = app
            .view
            .as_ref()
            .map(|v| v.users.values().map(|u| (u.id, u.name.clone())).collect())
            .unwrap_or_default();
        users.sort_by_key(|u| u.1.to_lowercase());
        user_filter(
            ui,
            "Enviado por",
            "search-author",
            &mut request.author,
            &users,
        );
        user_filter(
            ui,
            "Menciona",
            "search-mention",
            &mut request.mentions,
            &users,
        );
        date_filter(ui, "A partir de", &mut request.after, &after_key, false);
        date_filter(ui, "Até", &mut request.before, &before_key, true);
        egui::ComboBox::from_id_salt("search-attachment")
            .selected_text(match request.has_attachment {
                Some(true) => "Com anexos",
                Some(false) => "Sem anexos",
                None => "Qualquer conteúdo",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut request.has_attachment, None, "Qualquer conteúdo");
                ui.selectable_value(&mut request.has_attachment, Some(true), "Com anexos");
                ui.selectable_value(&mut request.has_attachment, Some(false), "Sem anexos");
            });
        egui::ComboBox::from_id_salt("search-order")
            .selected_text(match request.order {
                SearchOrder::Newest => "Mais recentes",
                SearchOrder::Oldest => "Mais antigas",
                SearchOrder::Relevance => "Relevância",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut request.order, SearchOrder::Newest, "Mais recentes");
                ui.selectable_value(&mut request.order, SearchOrder::Oldest, "Mais antigas");
                ui.selectable_value(&mut request.order, SearchOrder::Relevance, "Relevância");
            });
    });
    let valid_date = |salt: &str, value: Option<i64>| {
        ui.ctx().data(|d| {
            d.get_temp::<(Option<i64>, String)>(egui::Id::unique(salt))
                .filter(|(saved, _)| *saved == value)
                .is_none_or(|(_, text)| text.trim().is_empty() || value.is_some())
        })
    };
    let dates_valid = valid_date(&after_key, request.after)
        && valid_date(&before_key, request.before)
        && request
            .after
            .zip(request.before)
            .is_none_or(|(after, before)| after <= before);
    if !dates_valid {
        ui.colored_label(
            theme::YELLOW,
            "Corrija as datas dos filtros. O início deve ser anterior ao fim.",
        );
    }
    submit |= ui
        .add_enabled_ui(dates_valid && !app.workspace_ui.search_loading, |ui| {
            components::primary_button(ui, "Buscar")
        })
        .inner
        .clicked();
    if submit && dates_valid && !app.workspace_ui.search_loading {
        request.offset = 0;
        app.begin_search(request.clone());
    }
    app.workspace_ui.extras.search_draft = Some(request);
    if app.workspace_ui.search_loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Buscando mensagens…");
        });
    }
    if let Some(error) = &app.workspace_ui.search_error {
        ui.colored_label(theme::color(theme::YELLOW), error);
    }
    let messages = app.workspace_ui.search_results.clone();
    ui.label(
        RichText::new(format!("{} resultados carregados", messages.len()))
            .small()
            .color(theme::color(theme::TEXT_MUTED)),
    );
    if messages.is_empty() && !app.workspace_ui.search_loading {
        ui.label("Nenhuma mensagem. Tente outras palavras ou amplie os filtros.");
    }
    result_rows(app, ui, &messages, "search-list");
    if app.workspace_ui.search_has_more
        && ui
            .add_enabled(
                !app.workspace_ui.search_loading,
                egui::Button::new("Carregar mais resultados"),
            )
            .clicked()
    {
        app.next_search_page();
    }
}

fn user_filter(
    ui: &mut Ui,
    label: &str,
    salt: &str,
    value: &mut Option<UserId>,
    users: &[(UserId, String)],
) {
    egui::ComboBox::from_id_salt(salt)
        .selected_text(
            value
                .and_then(|id| users.iter().find(|u| u.0 == id).map(|u| u.1.clone()))
                .unwrap_or_else(|| label.into()),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "Qualquer pessoa");
            for (id, name) in users {
                ui.selectable_value(value, Some(*id), name);
            }
        });
}

fn date_filter(ui: &mut Ui, label: &str, value: &mut Option<i64>, salt: &str, end: bool) {
    let key = egui::Id::unique(salt);
    let mut text = ui
        .ctx()
        .data(|d| d.get_temp::<(Option<i64>, String)>(key))
        .filter(|(saved, _)| *saved == *value)
        .map(|(_, text)| text)
        .unwrap_or_else(|| {
            value
                .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                .map(|d| {
                    d.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                        .to_string()
                })
                .unwrap_or_default()
        });
    ui.label(label);
    if ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .hint_text("AAAA-MM-DD")
                .desired_width(140.0),
        )
        .changed()
    {
        *value = chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .ok()
            .and_then(|d| {
                d.and_hms_opt(
                    if end { 23 } else { 0 },
                    if end { 59 } else { 0 },
                    if end { 59 } else { 0 },
                )
            })
            .and_then(|d| d.and_local_timezone(chrono::Local).single())
            .map(|d| d.timestamp());
    }
    if !text.is_empty() && value.is_none() {
        ui.colored_label(
            theme::color(theme::YELLOW),
            "Use uma data válida: AAAA-MM-DD",
        );
    }
    ui.ctx().data_mut(|d| d.insert_temp(key, (*value, text)));
}

fn result_rows(app: &mut LanvoxApp, ui: &mut Ui, messages: &[Message], salt: &str) {
    egui::ScrollArea::vertical()
        .id_salt(salt)
        .max_height((ui.available_height() - 48.0).max(100.0))
        .show(ui, |ui| {
            for message in messages {
                let channel = app
                    .view
                    .as_ref()
                    .and_then(|v| v.channel(message.channel))
                    .map(|c| {
                        let guild = app
                            .view
                            .as_ref()
                            .and_then(|v| v.guilds.iter().find(|g| g.id == c.guild));
                        match guild {
                            Some(g) => format!("{} · #{}", g.name, c.name),
                            None => c.name.clone(),
                        }
                    })
                    .unwrap_or_default();
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{channel} · {}", message.author_name)).strong());
                if let Some(date) = chrono::DateTime::from_timestamp(message.time, 0) {
                    ui.label(
                        RichText::new(
                            date.with_timezone(&chrono::Local)
                                .format("%d/%m/%Y %H:%M")
                                .to_string(),
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                }
                ui.label(message.text.chars().take(260).collect::<String>());
                if ui.small_button("Abrir no contexto").clicked() {
                    app.open_message(message);
                    if ui.ctx().content_rect().width() < 1000.0 {
                        app.workspace_ui.search_open = false;
                        app.workspace_ui.inbox_open = false;
                        app.workspace_ui.pinned_only = false;
                    }
                }
                ui.separator();
            }
        });
}

fn inbox_content(app: &mut LanvoxApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut app.workspace_ui.extras.inbox_mentions,
            false,
            "Não lidas",
        );
        ui.selectable_value(&mut app.workspace_ui.extras.inbox_mentions, true, "Menções");
    });
    let mentions_only = app.workspace_ui.extras.inbox_mentions;
    if app.workspace_ui.inbox_requested != Some(mentions_only)
        || (app.workspace_ui.inbox_dirty && !app.workspace_ui.inbox_loading)
    {
        app.load_inbox(mentions_only, None);
    }
    if ui.button("Atualizar").clicked() {
        app.load_inbox(mentions_only, None);
    }
    if app.workspace_ui.inbox_loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Carregando mensagens…");
        });
    } else if let Some(error) = &app.workspace_ui.inbox_error {
        ui.colored_label(theme::YELLOW, format!("Não foi possível carregar: {error}"));
    } else if app.client.as_ref().is_none_or(|client| !client.is_alive()) {
        ui.label("Conecte-se para consultar mensagens não lidas.");
    } else if app.workspace_ui.inbox_results.is_empty()
        && app.workspace_ui.inbox_requested.is_some()
    {
        ui.label("Tudo em dia. Não há mensagens nesta lista.");
    }
    let messages = app.workspace_ui.inbox_results.clone();
    result_rows(app, ui, &messages, "inbox-list");
    if app.workspace_ui.inbox_has_more
        && !app.workspace_ui.inbox_loading
        && ui.button("Carregar mais").clicked()
    {
        if let Some(before) = messages.last().map(|m| m.id) {
            app.load_inbox(mentions_only, Some(before));
        }
    }
}
