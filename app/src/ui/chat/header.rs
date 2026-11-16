use super::*;

pub fn header_panel(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let Some(ch) = app
        .selected
        .and_then(|id| app.view.as_ref()?.channel(id).cloned())
    else {
        return;
    };
    Panel::top("chat-header")
        .exact_size(theme::HEADER_H)
        .frame(
            Frame::new()
                .fill(theme::color(theme::CHAT))
                .inner_margin(Margin::symmetric(12, 0)),
        )
        .show(ui, |ui| {
            let wide = ui.available_width() >= 720.0;
            let narrow_members = ui.available_width() <= 650.0;
            let members_visible = if narrow_members {
                app.workspace_ui.members_overlay_open
            } else {
                app.show_members
            };
            let actions_width = if wide { 270.0 } else { 176.0 };
            let title_width = (ui.available_width() - actions_width - 12.0).max(40.0);
            ui.horizontal_centered(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(title_width, 32.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_width(title_width);
                        ui.set_max_width(title_width);
                        ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                        crate::ui::icons::paint(
                            ui.painter(),
                            r,
                            if ch.guild == 0 {
                                Icon::Users
                            } else {
                                Icon::Hash
                            },
                            theme::TEXT_MUTED,
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&ch.name).strong().size(16.0))
                                .truncate(),
                        )
                        .on_hover_text(if ch.guild == 0 {
                            format!("{}\n{}", ch.name, crate::branding::ADMIN_VISIBILITY_NOTICE)
                        } else {
                            format!("{}\n{}", ch.name, ch.topic)
                        });
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(actions_width, 32.0),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        ui.set_min_width(actions_width);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let more = widgets::icon_button(
                            ui,
                            Icon::More,
                            30.0,
                            theme::TEXT_MUTED,
                            "Mais opções da conversa",
                        );
                        egui::Popup::menu(&more).show(|ui| {
                            if !ch.topic.is_empty() {
                                ui.label(&ch.topic);
                                ui.separator();
                            }
                            if ch.guild == 0 {
                                ui.label(crate::branding::ADMIN_VISIBILITY_NOTICE);
                                ui.separator();
                            }
                            if ui.button("Mensagens fixadas").clicked() {
                                app.workspace_ui.pinned_only = true;
                                app.workspace_ui.search_open = false;
                                app.workspace_ui.inbox_open = false;
                                app.workspace(WorkspaceCommand::Pinned { channel: ch.id });
                                ui.close();
                            }
                            if ui.button("Configurações da conversa").clicked() {
                                app.open_selected_settings();
                                app.workspace_ui.editing_channel = None;
                                ui.close();
                            }
                        });
                        if widgets::icon_button(
                            ui,
                            Icon::Users,
                            30.0,
                            if members_visible {
                                theme::TEXT_BRIGHT
                            } else {
                                theme::TEXT_MUTED
                            },
                            if members_visible {
                                "Ocultar participantes"
                            } else {
                                "Mostrar participantes"
                            },
                        )
                        .clicked()
                        {
                            if narrow_members {
                                app.workspace_ui.members_overlay_open = !members_visible;
                            } else {
                                app.show_members = !members_visible;
                            }
                            if !members_visible {
                                app.workspace_ui.search_open = false;
                                app.workspace_ui.inbox_open = false;
                                app.workspace_ui.pinned_only = false;
                            }
                        }
                        if ch.guild == 0
                            && widgets::icon_button(
                                ui,
                                Icon::Phone,
                                30.0,
                                theme::TEXT_MUTED,
                                "Iniciar chamada",
                            )
                            .clicked()
                        {
                            if let Some(members) = app
                                .view
                                .as_ref()
                                .and_then(|v| {
                                    v.workspace
                                        .conversations
                                        .iter()
                                        .find(|c| c.channel == ch.id)
                                })
                                .map(|c| {
                                    c.members
                                        .iter()
                                        .copied()
                                        .filter(|id| *id != app.me())
                                        .collect()
                                })
                            {
                                app.start_call(members);
                            }
                        }
                        let submit = if wide {
                            let field = ui.add(
                                egui::TextEdit::singleline(&mut app.search_input)
                                    .desired_width(150.0)
                                    .hint_text("Buscar mensagens")
                                    .margin(Margin::symmetric(8, 5)),
                            );
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        } else {
                            ui.button("Buscar").clicked()
                        };
                        if submit {
                            app.workspace_ui.members_overlay_open = false;
                            if app.search_input.trim().is_empty() {
                                app.workspace_ui.search_request = SearchRequest {
                                    channel: Some(ch.id),
                                    ..Default::default()
                                };
                                app.workspace_ui.search_open = true;
                                app.workspace_ui.inbox_open = false;
                                app.workspace_ui.pinned_only = false;
                            } else {
                                app.begin_search(SearchRequest {
                                    channel: Some(ch.id),
                                    query: app.search_input.clone(),
                                    ..Default::default()
                                });
                            }
                        }
                    },
                );
            });
        });
}
