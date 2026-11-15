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
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            let full = ui.max_rect();
            ui.painter().line_segment(
                [
                    egui::pos2(full.left() - 16.0, full.bottom()),
                    egui::pos2(full.right() + 16.0, full.bottom()),
                ],
                egui::Stroke::new(1.0, theme::color(theme::DIVIDER)),
            );
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ch.guild != 0 {
                    let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                    crate::ui::icons::paint(ui.painter(), r, Icon::Hash, theme::TEXT_FAINT);
                } else if let Some(person) = app
                    .view
                    .as_ref()
                    .and_then(|v| {
                        v.workspace
                            .conversations
                            .iter()
                            .find(|c| c.channel == ch.id)
                            .filter(|c| c.direct)
                            .and_then(|c| c.members.iter().find(|id| **id != v.me))
                            .and_then(|id| v.users.get(id))
                    })
                    .cloned()
                {
                    let texture = app.avatar_texture(ui.ctx(), person.id);
                    let presence = PresenceState::of_user(app, person.id);
                    if Avatar::new(&person.name, AvatarSize::Sm)
                        .user(person.id)
                        .color(person.color)
                        .texture(texture.as_ref())
                        .presence(presence)
                        .show(ui)
                        .clicked()
                    {
                        app.open_profile(person.id);
                    }
                }
                ui.label(
                    RichText::new(ch.name.clone())
                        .strong()
                        .size(16.0)
                        .color(theme::TEXT_BRIGHT),
                );
                if ch.guild == 0 && ui.available_width() > 560.0 {
                    ui.label(
                        RichText::new("· visível ao administrador do host")
                            .size(12.0)
                            .color(theme::TEXT_MUTED),
                    )
                    .on_hover_text(crate::branding::ADMIN_VISIBILITY_NOTICE);
                }
                let topic_width = (ui.available_width() - 310.0).clamp(0.0, 180.0);
                if !ch.topic.is_empty() && topic_width >= 40.0 {
                    let (r, _) = ui.allocate_exact_size(vec2(1.0, 24.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(r, 0.0, theme::color(theme::SELECTED));
                    ui.add_sized(
                        [topic_width, 24.0],
                        egui::Label::new(
                            RichText::new(&ch.topic).size(14.0).color(theme::TEXT_MUTED),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&ch.topic);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let members_col = if app.show_members {
                        theme::TEXT_BRIGHT
                    } else {
                        theme::TEXT_MUTED
                    };
                    if widgets::icon_button(
                        ui,
                        Icon::Users,
                        30.0,
                        members_col,
                        "Mostrar participantes",
                    )
                    .clicked()
                    {
                        app.show_members = !app.show_members;
                    }
                    let search = ui
                        .scope(|ui| {
                            ui.visuals_mut().extreme_bg_color = theme::color(theme::PANEL);
                            ui.add(
                                egui::TextEdit::singleline(&mut app.search_input)
                                    .desired_width(160.0)
                                    .margin(Margin::symmetric(8, 5))
                                    .hint_text(format!("Buscar {}", ch.name)),
                            )
                        })
                        .inner;
                    if search.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !app.search_input.trim().is_empty()
                    {
                        app.begin_search(SearchRequest {
                            channel: Some(ch.id),
                            query: app.search_input.clone(),
                            ..Default::default()
                        });
                    }
                    ui.add_space(4.0);
                    let pin_col = if app.workspace_ui.pinned_only {
                        theme::TEXT_BRIGHT
                    } else {
                        theme::TEXT_MUTED
                    };
                    if widgets::icon_button(ui, Icon::Pin, 30.0, pin_col, "Mensagens fixadas")
                        .clicked()
                    {
                        app.workspace_ui.pinned_only = !app.workspace_ui.pinned_only;
                        if app.workspace_ui.pinned_only {
                            app.workspace(WorkspaceCommand::Pinned { channel: ch.id });
                        }
                    }
                    if widgets::icon_button(ui, Icon::Gear, 30.0, theme::TEXT_MUTED, "Gerenciar")
                        .clicked()
                    {
                        app.workspace_ui.managing = true;
                        app.workspace_ui.editing_channel = None;
                    }
                    if ch.guild == 0
                        && widgets::icon_button(ui, Icon::Phone, 30.0, theme::TEXT_MUTED, "Chamar")
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
                });
            });
        });
}
