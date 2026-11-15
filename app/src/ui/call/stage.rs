use super::*;

/// The big view of one stream, with its overlay controls.
pub fn stream_stage(app: &mut LanvoxApp, ui: &mut egui::Ui, key: StreamKey, fullscreen: bool) {
    let ctx = ui.ctx().clone();
    let rect = ui.max_rect();
    let me = app.me();
    let own = key.0 == me;
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        if fullscreen { 0.0 } else { 8.0 },
        theme::color(theme::TILE_BG),
    );
    match app.texture_for(&ctx, key) {
        Some((tex, size)) => {
            let fit = contain(rect, size);
            painter.image(
                tex,
                fit,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if key.1 == StreamKind::Screen {
                app.paint_cursor(&ctx, &painter, fit, key.0);
            }
        }
        None => {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                crate::media::openh264_runtime::waiting_text(),
                FontId::proportional(16.0),
                theme::TEXT_MUTED,
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    // Right click on the picture adjusts the stream's volume (added before the buttons so
    // they stay on top).
    if !own && key.1 == StreamKind::Screen {
        let owner_name = app
            .view
            .as_ref()
            .map(|v| v.user_name(key.0))
            .unwrap_or_default();
        app.stream_audio(key.0, &owner_name);
        let resp = ui.interact(
            rect,
            ui.scope_id().with(("stage-audio", key)),
            Sense::click(),
        );
        stream_audio_menu(app, &resp, key.0, &owner_name);
    }

    // Keep the controls available to keyboard and assistive technology in fullscreen.
    // Top bar overlay.
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 44.0));
    painter.rect_filled(
        bar,
        egui::CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        },
        Color32::from_black_alpha(120),
    );
    let name = app
        .view
        .as_ref()
        .map(|v| v.user_name(key.0))
        .unwrap_or_default();
    let label = app
        .view
        .as_ref()
        .and_then(|v| v.stream(key.0, key.1))
        .map(|s| s.label.clone())
        .unwrap_or_default();
    let live = Rect::from_min_size(bar.left_top() + vec2(12.0, 13.0), vec2(36.0, 18.0));
    painter.rect_filled(live, 4.0, theme::RED);
    painter.text(
        live.center(),
        Align2::CENTER_CENTER,
        "LIVE",
        FontId::proportional(11.0),
        Color32::WHITE,
    );
    let title = if own {
        format!("Sua transmissão — {label}")
    } else {
        format!("{name} — {label}")
    };
    painter.text(
        pos2(live.right() + 10.0, bar.center().y - 7.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(14.0),
        Color32::WHITE,
    );
    let stats = if own {
        let (s, preset, source) = app
            .share
            .as_ref()
            .map(|s| {
                (
                    s.stats.lock().clone(),
                    format!("{} {} fps", s.settings.resolution.label(), s.settings.fps),
                    s.source_name.clone(),
                )
            })
            .unwrap_or_default();
        let viewers = app
            .view
            .as_ref()
            .and_then(|v| v.viewers.get(&StreamKind::Screen))
            .copied()
            .unwrap_or(0);
        format!(
            "{source} · {preset} preset · {}x{} · {:.0} fps · {:.1} Mb/s · quality {} · {} watching",
            s.width,
            s.height,
            s.fps,
            s.kbps / 1000.0,
            s.quality,
            viewers
        )
    } else {
        app.stream_stats(key)
            .map(|(w, h, fps, kbps)| format!("{w}x{h} · {fps:.0} fps · {:.1} Mb/s", kbps / 1000.0))
            .unwrap_or_default()
    };
    painter.text(
        pos2(live.right() + 10.0, bar.center().y + 9.0),
        Align2::LEFT_CENTER,
        stats,
        FontId::proportional(11.5),
        theme::TEXT_MUTED,
    );

    // Buttons on the right.
    let mut x = bar.right() - 8.0;
    let mut button = |ui: &mut egui::Ui, icon: Icon, tip: &str| -> bool {
        let r = Rect::from_min_size(pos2(x - 32.0, bar.top() + 6.0), vec2(32.0, 32.0));
        x -= 36.0;
        let resp = ui
            .interact(r, ui.scope_id().with(("stage-btn", tip)), Sense::click())
            .on_hover_text(tip);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
        if resp.hovered() || resp.has_focus() {
            ui.painter()
                .rect_filled(r, 4.0, Color32::from_white_alpha(30));
        }
        if resp.has_focus() {
            ui.painter().rect_stroke(
                r.shrink(1.0),
                4,
                Stroke::new(2.0, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        icons::paint(ui.painter(), r.shrink(7.0), icon, Color32::WHITE);
        resp.clicked()
    };
    if button(
        ui,
        Icon::Close,
        if own {
            "Parar transmissão"
        } else {
            "Parar de assistir"
        },
    ) {
        if own {
            app.stop_share();
        } else {
            app.stop_watching(key);
        }
        return;
    }
    if button(
        ui,
        Icon::Expand,
        if fullscreen {
            "Sair da tela cheia"
        } else {
            "Tela cheia"
        },
    ) {
        app.fullscreen = !fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(app.fullscreen));
    }
    if !fullscreen && button(ui, Icon::PopOut, "Abrir em outra janela") {
        app.popout = Some(key);
        app.focused = None;
    }
    if fullscreen && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.fullscreen = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
}

/// A separate window showing one stream.
pub fn popout(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some(key) = app.popout else { return };
    let title = app
        .view
        .as_ref()
        .map(|v| {
            if key.0 == v.me {
                "Sua transmissão".to_string()
            } else {
                format!("Transmissão de {}", v.user_name(key.0))
            }
        })
        .unwrap_or_else(|| "Transmissão".into());
    let mut close = false;
    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of("stream-popout"),
        egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([960.0, 540.0])
            .with_min_inner_size(POPOUT_MIN),
        |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                close = true;
            }
            egui::CentralPanel::default()
                .frame(Frame::new().fill(theme::color(theme::CALL_BG)))
                .show(ui, |ui| {
                    let rect = ui.max_rect();
                    let ctx = ui.ctx().clone();
                    match app.texture_for(&ctx, key) {
                        Some((tex, size)) => {
                            let fit = contain(rect, size);
                            ui.painter().image(
                                tex,
                                fit,
                                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                            if key.1 == StreamKind::Screen {
                                let painter = ui.painter().clone();
                                app.paint_cursor(&ctx, &painter, fit, key.0);
                            }
                        }
                        None => {
                            ui.painter().text(
                                rect.center(),
                                Align2::CENTER_CENTER,
                                crate::media::openh264_runtime::waiting_text(),
                                FontId::proportional(16.0),
                                theme::TEXT_MUTED,
                            );
                        }
                    }
                    if key.0 != app.me() && key.1 == StreamKind::Screen {
                        let owner_name = app
                            .view
                            .as_ref()
                            .map(|v| v.user_name(key.0))
                            .unwrap_or_default();
                        app.stream_audio(key.0, &owner_name);
                        let resp =
                            ui.interact(rect, ui.scope_id().with("popout-audio"), Sense::click());
                        stream_audio_menu(app, &resp, key.0, &owner_name);
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(33));
                });
        },
    );
    if close {
        app.popout = None;
        app.focused = Some(key);
    }
}

pub(super) fn controls(app: &mut LanvoxApp, ui: &mut egui::Ui, chat_channel: Option<ChannelId>) {
    let ctx = ui.ctx().clone();
    let n = if chat_channel.is_some() { 6.0 } else { 5.0 };
    let gap = if ui.available_width() < 520.0 {
        6.0
    } else {
        12.0
    };
    let size = ((ui.available_width() - (n - 1.0) * gap) / (n * 1.2)).clamp(30.0, 48.0);
    let total = n * size * 1.2 + (n - 1.0) * gap;
    ui.horizontal_centered(|ui| {
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        ui.spacing_mut().item_spacing.x = gap;
        let cam = app.camera.is_some();
        if widgets::round_button(
            ui,
            if cam { Icon::Camera } else { Icon::CameraOff },
            size,
            if cam {
                theme::GREEN
            } else {
                theme::color(theme::INPUT)
            },
            if cam {
                "Desligar câmera"
            } else {
                "Ligar câmera"
            },
        )
        .clicked()
        {
            app.toggle_camera(&ctx);
        }
        let live = app.share.is_some();
        if widgets::round_button(
            ui,
            Icon::Screen,
            size,
            if live {
                theme::GREEN
            } else {
                theme::color(theme::INPUT)
            },
            if live {
                "Parar transmissão"
            } else {
                "Compartilhar sua tela"
            },
        )
        .clicked()
        {
            if live {
                app.stop_share();
            } else {
                app.open_share_picker();
            }
        }
        let deaf = app.audio_shared.deafened.load(Ordering::Relaxed);
        let muted = app.audio_shared.muted.load(Ordering::Relaxed) || deaf;
        if widgets::round_button(
            ui,
            if muted { Icon::MicOff } else { Icon::Mic },
            size,
            if muted {
                theme::RED
            } else {
                theme::color(theme::INPUT)
            },
            if muted {
                "Ativar microfone"
            } else {
                "Silenciar microfone"
            },
        )
        .clicked()
        {
            app.toggle_mute();
        }
        if widgets::round_button(
            ui,
            if deaf {
                Icon::HeadphonesOff
            } else {
                Icon::Headphones
            },
            size,
            if deaf {
                theme::RED
            } else {
                theme::color(theme::INPUT)
            },
            if deaf {
                "Ativar áudio"
            } else {
                "Desativar áudio"
            },
        )
        .clicked()
        {
            app.toggle_deafen();
        }
        if let Some(channel) = chat_channel {
            let open = app.call_chat_open;
            let unread = app
                .view
                .as_ref()
                .and_then(|v| v.unread.get(&channel))
                .copied()
                .unwrap_or(0);
            let resp = widgets::round_button(
                ui,
                Icon::Hash,
                size,
                if open {
                    theme::ACCENT
                } else {
                    theme::color(theme::INPUT)
                },
                if open { "Fechar chat" } else { "Abrir chat" },
            );
            if let (false, Some(text)) = (open, unread_badge(unread)) {
                let c = resp.rect.right_top() + vec2(-4.0, 4.0);
                ui.painter().circle_filled(c, 9.0, theme::RED);
                ui.painter().text(
                    c,
                    Align2::CENTER_CENTER,
                    text,
                    FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
            if resp.clicked() {
                app.call_chat_open = !open;
                if app.call_chat_open {
                    app.scroll_to_bottom = 3;
                    app.chat_at_bottom = true;
                    app.mark_read(channel);
                }
            }
        }
        if widgets::round_button(ui, Icon::HangUp, size, theme::RED, "Desconectar").clicked() {
            app.leave_voice();
        }
    });
}
