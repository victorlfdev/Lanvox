use super::*;

pub fn channels(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    // Server name header.
    Panel::top("server-header")
        .exact_size(theme::HEADER_H)
        .frame(
            Frame::new()
                .fill(theme::color(theme::SIDEBAR))
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(
                    ui,
                    Icon::Gear,
                    28.0,
                    theme::color(theme::TEXT_MUTED),
                    "Gerenciar",
                )
                .clicked()
                {
                    app.workspace_ui.managing = true;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let name = if app.home {
                        "Encontre ou comece uma conversa".to_string()
                    } else {
                        app.view
                            .as_ref()
                            .and_then(|v| {
                                v.guilds
                                    .iter()
                                    .find(|g| Some(g.id) == app.guild)
                                    .map(|g| g.name.clone())
                            })
                            .unwrap_or_default()
                    };
                    let addr = app
                        .client
                        .as_ref()
                        .map(|c| c.address.clone())
                        .unwrap_or_default();
                    let hosting = if app.hosted.is_some() {
                        " (hospedado neste computador)"
                    } else {
                        ""
                    };
                    if app.home {
                        if ui
                            .add(
                                egui::Button::new("Buscar conversas · Ctrl+K")
                                    .fill(theme::color(theme::PANEL)),
                            )
                            .clicked()
                        {
                            app.quick.toggle();
                        }
                        return;
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new(name)
                                .strong()
                                .size(16.0)
                                .color(theme::color(theme::TEXT_BRIGHT)),
                        )
                        .truncate(),
                    )
                    .on_hover_text(format!("Conectado a {addr}{hosting}"));
                });
            });
            let r = ui.max_rect();
            ui.painter().line_segment(
                [
                    pos2(r.left() - 16.0, r.bottom()),
                    pos2(r.right() + 16.0, r.bottom()),
                ],
                Stroke::new(1.0, theme::color(theme::DIVIDER)),
            );
        });

    // User panel (always) and voice panel (in a call) at the bottom. Short
    // windows get tighter panels so the channel list keeps its height.
    let compact = ctx.content_rect().height() < 640.0;
    Panel::bottom("user-panel")
        .exact_size(if compact { 46.0 } else { theme::USER_PANEL_H })
        .frame(
            Frame::new()
                .fill(theme::color(theme::PANEL))
                .inner_margin(Margin::symmetric(8, 0)),
        )
        .show(ui, |ui| user_panel(app, ui));

    let in_voice = app.view.as_ref().and_then(|v| v.my_voice().channel);
    if let Some(ch) = in_voice {
        Panel::bottom("voice-panel")
            .frame(
                Frame::new()
                    .fill(theme::color(theme::PANEL))
                    .inner_margin(Margin {
                        left: 10,
                        right: 8,
                        top: if compact { 4 } else { 8 },
                        bottom: if compact { 4 } else { 8 },
                    })
                    .stroke(Stroke::new(1.0, theme::color(theme::SIDEBAR))),
            )
            .show(ui, |ui| voice_panel(app, ui, ch, &ctx, compact));
    }

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(theme::color(theme::SIDEBAR))
                .inner_margin(Margin::symmetric(8, 0)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.add_space(14.0);
                    if app.home {
                        home_list(app, ui);
                    } else {
                        if let Some(guild) = app.guild {
                            crate::ui::community_ui::banner(app, ui, guild);
                            event_section(app, ui, guild);
                        }
                        channel_list(app, ui);
                        ui.add_space(12.0);
                    }
                });
        });
}

/// Upcoming community events are available from the channel list to every member.
fn event_section(app: &mut LanvoxApp, ui: &mut egui::Ui, guild: GuildId) {
    let now = now_secs();
    let mut events = app
        .view
        .as_ref()
        .map(|view| {
            view.workspace
                .community
                .events
                .iter()
                .filter(|event| {
                    event.guild == guild
                        && !event.cancelled
                        && event.ends_at.is_none_or(|end| end > now)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    events.sort_by_key(|event| event.starts_at);
    let title = if events.is_empty() {
        "Eventos".to_owned()
    } else {
        format!("Eventos — {}", events.len())
    };
    ui.push_id(("community-events", guild), |ui| {
        ui.collapsing(title, |ui| {
            if events.is_empty() {
                ui.label(
                    RichText::new("Nenhum evento agendado.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            for event in events {
                ui.separator();
                ui.add(egui::Label::new(RichText::new(&event.name).strong()).wrap());
                let date = chrono::DateTime::from_timestamp(event.starts_at, 0)
                    .map(|date| {
                        date.with_timezone(&chrono::Local)
                            .format("%d/%m %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|| "Data indisponível".into());
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        date,
                        if event.starts_at <= now {
                            "Em andamento"
                        } else {
                            "Agendado"
                        }
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                ui.horizontal_wrapped(|ui| {
                    let interested = event.interested.contains(&app.me());
                    if ui.selectable_label(interested, "Tenho interesse").clicked() {
                        app.workspace(WorkspaceCommand::Community(
                            CommunityCommand::RespondEvent {
                                event: event.id,
                                interested: !interested,
                            },
                        ));
                    }
                    if let Some(channel) = event.channel {
                        if ui.small_button("Abrir canal").clicked() {
                            app.select_channel(channel);
                        }
                    }
                });
            }
        });
    });
    ui.add_space(6.0);
}

/// The channel list: uncategorised channels on top, then each category (text and voice mixed)
/// with a collapse arrow. People who can manage channels also get drag and drop and a context
/// menu on category headers.
pub(super) fn channel_list(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let Some(guild) = app.guild else { return };
    let Some(view) = &app.view else { return };
    let manage = view.can_manage_channels(guild);
    let channels: Vec<Channel> = view
        .channels
        .iter()
        .filter(|c| c.guild == guild)
        .cloned()
        .collect();
    let sections = crate::categories::layout(
        guild,
        &channels,
        &view.workspace.channel_settings,
        &view.workspace.categories,
    );
    let scope = format!("{}|{guild}", crate::ui::quick_switcher::host_scope(app));
    let collapsed: Vec<CategoryId> = app
        .settings
        .collapsed_categories
        .get(&scope)
        .cloned()
        .unwrap_or_default();

    ui.horizontal(|ui| {
        ui.add_space(4.0);
        widgets::section_label(ui, "Canais");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if manage
                && widgets::icon_button(
                    ui,
                    Icon::Plus,
                    18.0,
                    theme::color(theme::TEXT_MUTED),
                    "Criar canal",
                )
                .clicked()
            {
                app.modal = Modal::CreateChannel {
                    kind: ChannelKind::Text,
                    name: String::new(),
                    category: None,
                };
            }
        });
    });
    ui.add_space(2.0);

    let dragging = app.workspace_ui.sidebar_drag;
    let mut rows: Vec<Row> = Vec::new();
    // Vertical extent of each category block, in display order (for dragging categories).
    let mut blocks: Vec<(CategoryId, f32, f32)> = Vec::new();
    let mut toggle: Option<CategoryId> = None;

    for section in &sections {
        let Some(category) = &section.category else {
            // Uncategorised channels are the top group; the strip keeps it a drop target
            // even when it is empty.
            let (strip, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), TOP_STRIP_H), Sense::hover());
            rows.push(Row {
                kind: RowKind::Header(None),
                top: strip.top(),
                bottom: strip.bottom(),
            });
            for id in &section.channels {
                if let Some(ch) = channels.iter().find(|c| c.id == *id) {
                    channel_row(app, ui, ch, None, manage, &mut rows);
                }
            }
            continue;
        };
        let is_collapsed = collapsed.contains(&category.id);
        ui.add_space(8.0);
        let block_top = ui.cursor().top();
        let header = category_header(
            app,
            ui,
            category,
            is_collapsed,
            section.channels.len(),
            manage,
        );
        if header.toggle {
            toggle = Some(category.id);
        }
        let header_row = rows.len();
        rows.push(Row {
            kind: RowKind::Header(Some(category.id)),
            top: header.rect.top(),
            bottom: header.rect.bottom(),
        });
        for id in &section.channels {
            let selected = app.selected == Some(*id);
            if is_collapsed && !selected {
                continue;
            }
            let Some(ch) = channels.iter().find(|c| c.id == *id) else {
                continue;
            };
            // A collapsed category still shows the open channel, but as part of its header
            // for drag purposes (its siblings are hidden, so indexes would lie).
            let mut scratch = Vec::new();
            let target = if is_collapsed {
                &mut scratch
            } else {
                &mut rows
            };
            channel_row(app, ui, ch, Some(category.id), manage, target);
            if is_collapsed {
                if let Some(last) = scratch.last() {
                    rows[header_row].bottom = last.bottom;
                }
            }
        }
        blocks.push((category.id, block_top, ui.min_rect().bottom()));
    }

    if manage {
        ui.add_space(10.0);
        if ui
            .add(
                egui::Button::new(
                    RichText::new("+ Nova categoria")
                        .size(12.0)
                        .color(theme::color(theme::TEXT_MUTED)),
                )
                .frame(false),
            )
            .clicked()
        {
            app.modal = Modal::CreateCategory {
                name: String::new(),
            };
        }
    }

    if let Some(id) = toggle {
        let list = app
            .settings
            .collapsed_categories
            .entry(scope.clone())
            .or_default();
        if let Some(i) = list.iter().position(|c| *c == id) {
            list.remove(i);
        } else {
            list.push(id);
        }
        // Forget categories that no longer exist.
        let known: Vec<CategoryId> = app
            .view
            .as_ref()
            .map(|v| v.workspace.categories.iter().map(|c| c.id).collect())
            .unwrap_or_default();
        list.retain(|c| known.contains(c));
        app.settings.save();
    }

    if manage {
        drag_and_drop(app, ui, guild, &rows, &blocks, &collapsed, dragging);
    } else {
        app.workspace_ui.sidebar_drag = None;
    }
}

pub(super) fn category_header(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    category: &Category,
    collapsed: bool,
    count: usize,
    manage: bool,
) -> HeaderResult {
    let sense = if manage {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_ROW_H), sense);
    let dragged = app.workspace_ui.sidebar_drag == Some(SidebarDrag::Category(category.id));
    if resp.hovered() && !dragged {
        ui.painter()
            .rect_filled(rect, 4.0, theme::color(theme::HOVER));
    }
    // Disclosure arrow, drawn so it does not depend on font glyphs.
    let c = pos2(rect.left() + 10.0, rect.center().y);
    let arrow = if collapsed {
        vec![
            c + vec2(-2.5, -4.0),
            c + vec2(3.0, 0.0),
            c + vec2(-2.5, 4.0),
        ]
    } else {
        vec![
            c + vec2(-4.0, -2.5),
            c + vec2(4.0, -2.5),
            c + vec2(0.0, 3.0),
        ]
    };
    let color = if resp.hovered() || dragged {
        theme::color(theme::TEXT_BRIGHT)
    } else {
        theme::color(theme::TEXT_MUTED)
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(arrow, color, Stroke::NONE));
    ui.painter().text(
        pos2(rect.left() + 24.0, rect.center().y),
        Align2::LEFT_CENTER,
        category.name.to_uppercase(),
        FontId::proportional(12.0),
        color,
    );
    if collapsed && count > 0 {
        ui.painter().text(
            pos2(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(11.0),
            theme::color(theme::TEXT_FAINT),
        );
    }
    let mut toggle = resp.clicked();
    let mut plus_clicked = false;
    if manage && resp.hovered() && app.workspace_ui.sidebar_drag.is_none() {
        let plus =
            Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(18.0, 18.0));
        let plus_resp = ui.interact(plus, resp.id.with("plus"), Sense::click());
        icons::paint(
            ui.painter(),
            plus.shrink(3.0),
            Icon::Plus,
            if plus_resp.hovered() {
                theme::color(theme::TEXT_BRIGHT)
            } else {
                theme::color(theme::TEXT_MUTED)
            },
        );
        plus_resp
            .clone()
            .on_hover_text("Criar canal nesta categoria");
        if plus_resp.clicked() {
            plus_clicked = true;
            toggle = false;
        }
    }
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!(
                "Categoria {}, {}",
                category.name,
                if collapsed { "recolhida" } else { "expandida" }
            ),
        )
    });
    if manage {
        if resp.drag_started() {
            app.workspace_ui.sidebar_drag = Some(SidebarDrag::Category(category.id));
        }
        let id = category.id;
        let mut action: Option<u8> = None;
        resp.context_menu(|ui| {
            if ui.button("Criar canal aqui").clicked() {
                action = Some(0);
                ui.close();
            }
            if ui.button("Renomear categoria").clicked() {
                action = Some(1);
                ui.close();
            }
            if ui.button("Excluir categoria").clicked() {
                action = Some(2);
                ui.close();
            }
        });
        if plus_clicked {
            action = Some(0);
        }
        match action {
            Some(0) => {
                app.modal = Modal::CreateChannel {
                    kind: ChannelKind::Text,
                    name: String::new(),
                    category: Some(id),
                }
            }
            Some(1) => {
                app.modal = Modal::RenameCategory {
                    category: id,
                    name: category.name.clone(),
                }
            }
            Some(2) => app.modal = Modal::DeleteCategory { category: id },
            _ => {}
        }
    }
    HeaderResult { rect, toggle }
}
