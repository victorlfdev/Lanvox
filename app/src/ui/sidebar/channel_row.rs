use super::*;

/// One channel (text or voice) in a group. Registers its extent in `rows`.
pub(super) fn channel_row(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    ch: &Channel,
    category: Option<CategoryId>,
    manage: bool,
    rows: &mut Vec<Row>,
) {
    let kind = ch.kind;
    let selected = app.selected == Some(ch.id);
    let unread = app
        .view
        .as_ref()
        .and_then(|v| v.unread.get(&ch.id))
        .copied()
        .unwrap_or(0);
    let sense = if manage {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), sense);
    let dragged = app.workspace_ui.sidebar_drag == Some(SidebarDrag::Channel(ch.id));
    let bg = if selected {
        Some(theme::color(theme::SELECTED))
    } else if resp.hovered() && !dragged {
        Some(theme::color(theme::HOVER))
    } else {
        None
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, 4.0, bg);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4,
            Stroke::new(2.0, theme::TEXT_BRIGHT),
            egui::StrokeKind::Inside,
        );
    }
    if resp.gained_focus() {
        resp.scroll_to_me(Some(Align::Center));
    }
    let text_col = if dragged {
        theme::color(theme::TEXT_FAINT)
    } else if selected || unread > 0 || resp.hovered() {
        theme::color(theme::TEXT_BRIGHT)
    } else {
        theme::color(theme::TEXT_MUTED)
    };
    let icon = if kind == ChannelKind::Text {
        Icon::Hash
    } else {
        Icon::Speaker
    };
    icons::paint(
        ui.painter(),
        Rect::from_min_size(
            pos2(rect.left() + 8.0, rect.center().y - 10.0),
            vec2(20.0, 20.0),
        ),
        icon,
        theme::color(theme::TEXT_FAINT),
    );
    let name_clip = Rect::from_min_max(
        pos2(rect.left() + 36.0, rect.top()),
        pos2(rect.right() - 5.0, rect.bottom()),
    );
    ui.painter().with_clip_rect(name_clip).text(
        pos2(rect.left() + 36.0, rect.center().y),
        Align2::LEFT_CENTER,
        &ch.name,
        FontId::proportional(15.0),
        text_col,
    );
    if unread > 0 && !selected {
        let pill = Rect::from_min_size(
            pos2(rect.left() - 8.0, rect.center().y - 4.0),
            vec2(4.0, 8.0),
        );
        ui.painter().rect_filled(pill, 2.0, Color32::WHITE);
    }
    resp.clone().on_hover_text(&ch.name);
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!(
                "Canal de {} {}{}{}",
                if kind == ChannelKind::Text {
                    "texto"
                } else {
                    "voz"
                },
                ch.name,
                if selected { ", selecionado" } else { "" },
                if unread > 0 {
                    format!(", {unread} não lidas")
                } else {
                    String::new()
                }
            ),
        )
    });
    resp.context_menu(|ui| {
        if ui.button("Marcar como lido").clicked() {
            app.mark_read(ch.id);
            ui.close();
        }
        ui.menu_button("Notificações", |ui| {
            use crate::settings::{NotificationLevel, NotificationPreference};
            let current = app.channel_notification_preference(ch.id);
            for (level, label) in [
                (NotificationLevel::All, "Todas"),
                (NotificationLevel::Mentions, "Somente menções"),
                (NotificationLevel::None, "Nenhuma"),
            ] {
                if ui.selectable_label(current.level == level, label).clicked() {
                    app.set_channel_notifications(
                        ch.id,
                        NotificationPreference {
                            level,
                            muted_until: current.muted_until,
                        },
                    );
                    ui.close();
                }
            }
            if ui.button("Silenciar por 1 hora").clicked() {
                app.set_channel_notifications(
                    ch.id,
                    NotificationPreference {
                        level: current.level,
                        muted_until: now_secs() + 3600,
                    },
                );
                ui.close();
            }
            if ui.button("Reativar").clicked() {
                app.set_channel_notifications(
                    ch.id,
                    NotificationPreference {
                        level: current.level,
                        muted_until: 0,
                    },
                );
                ui.close();
            }
        });
        if manage {
            ui.menu_button("Mover canal", |ui| {
                let categories = app
                    .view
                    .as_ref()
                    .map(|v| {
                        v.workspace
                            .categories
                            .iter()
                            .filter(|c| c.guild == ch.guild)
                            .map(|c| (Some(c.id), c.name.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for (target, name) in
                    std::iter::once((None, "Sem categoria".to_owned())).chain(categories)
                {
                    if ui.button(name).clicked() {
                        app.category_command(WorkspaceCommand::MoveChannel {
                            channel: ch.id,
                            category: target,
                            index: u32::MAX,
                        });
                        ui.close();
                    }
                }
            });
            let mut siblings = app
                .view
                .as_ref()
                .map(|v| {
                    v.channels
                        .iter()
                        .filter(|c| c.guild == ch.guild)
                        .filter_map(|c| {
                            let settings = v
                                .workspace
                                .channel_settings
                                .iter()
                                .find(|s| s.channel == c.id);
                            if settings.and_then(|s| s.category_id) == category {
                                Some((c.id, settings.map(|s| s.position).unwrap_or(0)))
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            siblings.sort_by_key(|(id, position)| (*position, *id));
            let index = siblings
                .iter()
                .position(|(id, _)| *id == ch.id)
                .unwrap_or(0);
            if ui
                .add_enabled(index > 0, egui::Button::new("Subir canal"))
                .clicked()
            {
                app.category_command(WorkspaceCommand::MoveChannel {
                    channel: ch.id,
                    category,
                    index: index.saturating_sub(1) as u32,
                });
                ui.close();
            }
            if ui
                .add_enabled(
                    index + 1 < siblings.len(),
                    egui::Button::new("Descer canal"),
                )
                .clicked()
            {
                app.category_command(WorkspaceCommand::MoveChannel {
                    channel: ch.id,
                    category,
                    index: (index + 1) as u32,
                });
                ui.close();
            }
        }
        if manage && ui.button("Editar canal").clicked() {
            app.open_guild_settings(Some(ch.guild), 1);
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::unique(("admin-channel-selection", ch.guild)),
                    ch.id,
                )
            });
            ui.close();
        }
    });
    if manage && resp.drag_started() {
        app.workspace_ui.sidebar_drag = Some(SidebarDrag::Channel(ch.id));
    }
    if resp.clicked() {
        app.select_channel(ch.id);
    }
    if kind == ChannelKind::Voice {
        voice_members(app, ui, ch.id);
    }
    // The row owns the voice participants under it, so a drop below them is "after" it.
    rows.push(Row {
        kind: RowKind::Channel {
            channel: ch.id,
            category,
        },
        top: rect.top(),
        bottom: ui.min_rect().bottom(),
    });
}

/// Runs a sidebar drag: shows the destination indicator and, on release, sends the move.
pub(super) fn drag_and_drop(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    guild: GuildId,
    rows: &[Row],
    blocks: &[(CategoryId, f32, f32)],
    collapsed: &[CategoryId],
    dragging: Option<SidebarDrag>,
) {
    let ctx = ui.ctx().clone();
    // A drag may have started on this very frame.
    let Some(drag) = app.workspace_ui.sidebar_drag.or(dragging) else {
        return;
    };
    let pointer = ctx.pointer_latest_pos();
    let released = !ctx.input(|i| i.pointer.any_down());
    let cancelled = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let inside = pointer.is_some_and(|p| ui.clip_rect().contains(p));
    if cancelled || (released && !inside) || pointer.is_none() {
        app.workspace_ui.sidebar_drag = None;
        return;
    }
    let y = pointer.map(|p| p.y).unwrap_or_default();
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    ctx.request_repaint();
    let left = ui.min_rect().left();
    let right = ui.max_rect().right();
    let line = |y: f32| {
        ui.painter().rect_filled(
            Rect::from_min_max(pos2(left, y - 1.5), pos2(right, y + 1.5)),
            1.5,
            theme::color(theme::ACCENT),
        );
    };

    let mut command = None;
    match drag {
        SidebarDrag::Channel(id) => {
            let Some(drop) = crate::categories::locate_channel_drop(rows, id, collapsed, y) else {
                app.workspace_ui.sidebar_drag = None;
                return;
            };
            if let Some(line_y) = crate::categories::channel_indicator_y(rows, id, drop) {
                line(line_y);
            }
            if drop.category.is_some_and(|c| collapsed.contains(&c)) {
                if let Some(h) = rows
                    .iter()
                    .find(|r| r.kind == RowKind::Header(drop.category))
                {
                    ui.painter().rect_stroke(
                        Rect::from_min_max(pos2(left, h.top), pos2(right, h.bottom)),
                        4.0,
                        Stroke::new(1.5, theme::color(theme::ACCENT)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            if released {
                command = Some(WorkspaceCommand::MoveChannel {
                    channel: id,
                    category: drop.category,
                    index: drop.index.min(u32::MAX as usize) as u32,
                });
            }
        }
        SidebarDrag::Category(id) => {
            let extents: Vec<(f32, f32)> = blocks.iter().map(|(_, t, b)| (*t, *b)).collect();
            let at = crate::categories::locate_category_drop(&extents, y);
            let line_y = match extents.get(at) {
                Some((top, _)) => top - 4.0,
                None => extents.last().map(|(_, b)| b + 2.0).unwrap_or(y),
            };
            line(line_y);
            if released {
                let order: Vec<CategoryId> = blocks.iter().map(|(c, _, _)| *c).collect();
                if let Some(from) = order.iter().position(|c| *c == id) {
                    let new = crate::categories::reordered(&order, from, at);
                    if new != order {
                        command = Some(WorkspaceCommand::ReorderCategories { guild, order: new });
                    }
                }
            }
        }
    }

    // A label that follows the pointer.
    let name = match drag {
        SidebarDrag::Channel(id) => app
            .view
            .as_ref()
            .and_then(|v| v.channel(id))
            .map(|c| c.name.clone()),
        SidebarDrag::Category(id) => app
            .view
            .as_ref()
            .and_then(|v| v.workspace.categories.iter().find(|c| c.id == id))
            .map(|c| c.name.to_uppercase()),
    }
    .unwrap_or_default();
    if let Some(p) = pointer {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("sidebar-drag-ghost"),
        ));
        let galley = painter.layout_no_wrap(
            name,
            FontId::proportional(14.0),
            theme::color(theme::TEXT_BRIGHT),
        );
        let rect = Rect::from_min_size(p + vec2(12.0, 6.0), galley.size() + vec2(16.0, 8.0));
        painter.rect_filled(rect, 4.0, theme::color(theme::RAIL));
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, theme::color(theme::ACCENT)),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            rect.min + vec2(8.0, 4.0),
            galley,
            theme::color(theme::TEXT_BRIGHT),
        );
    }

    if released {
        app.workspace_ui.sidebar_drag = None;
        if let Some(command) = command {
            app.category_command(command);
        }
    }
}
