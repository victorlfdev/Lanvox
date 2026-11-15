use super::*;

pub(super) fn voice_members(app: &mut LanvoxApp, ui: &mut egui::Ui, channel: ChannelId) {
    let Some(view) = &app.view else { return };
    let people = view.in_voice(channel);
    let mut open_stream = None;
    let mut menu_targets = Vec::new();
    for id in people {
        let state = view.voice.get(&id).cloned().unwrap_or_default();
        let name = view.user_name(id);
        let color = color32(view.user_color(id));
        let speaking = view.speaking.contains(&id)
            || (id == view.me && app.audio_shared.speaking.load(Ordering::Relaxed));
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(
                rect.shrink2(vec2(0.0, 1.0)).translate(vec2(28.0, 0.0)),
                4.0,
                theme::color(theme::HOVER),
            );
        }
        let av =
            Rect::from_center_size(pos2(rect.left() + 46.0, rect.center().y), vec2(24.0, 24.0));
        widgets::avatar_at(ui, av, &name, color, speaking, None);
        ui.painter().text(
            pos2(rect.left() + 66.0, rect.center().y),
            Align2::LEFT_CENTER,
            &name,
            FontId::proportional(14.0),
            if speaking {
                theme::color(theme::TEXT_BRIGHT)
            } else {
                theme::color(theme::TEXT_MUTED)
            },
        );
        let mut x = rect.right() - 6.0;
        let mut badge = |icon: Option<Icon>, live: bool| {
            if live {
                let w = 34.0;
                let r = Rect::from_min_size(pos2(x - w, rect.center().y - 8.0), vec2(w, 16.0));
                ui.painter().rect_filled(r, 4.0, theme::color(theme::RED));
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    "LIVE",
                    FontId::proportional(10.5),
                    Color32::WHITE,
                );
                x -= w + 4.0;
            } else if let Some(icon) = icon {
                let r =
                    Rect::from_min_size(pos2(x - 16.0, rect.center().y - 8.0), vec2(16.0, 16.0));
                icons::paint(ui.painter(), r, icon, theme::color(theme::TEXT_MUTED));
                x -= 20.0;
            }
        };
        if state.streaming {
            badge(None, true);
        }
        if state.camera {
            badge(Some(Icon::Camera), false);
        }
        if state.deafened {
            badge(Some(Icon::HeadphonesOff), false);
        } else if state.muted {
            badge(Some(Icon::MicOff), false);
        }
        if resp.clicked() && state.streaming {
            open_stream = Some((id, StreamKind::Screen));
        }
        let resp = if state.streaming {
            resp.on_hover_text("Clique para assistir à transmissão")
        } else {
            resp
        };
        menu_targets.push((resp, id, name));
    }
    for (resp, id, name) in menu_targets {
        crate::ui::call::person_audio_menu(app, &resp, id, &name);
    }
    if let Some(key) = open_stream {
        if app.view.as_ref().and_then(|v| v.my_voice().channel) == Some(channel) {
            app.selected = Some(channel);
            app.watch(key);
        } else {
            app.notify("Entre neste canal de voz para assistir.");
        }
    }
}

pub(super) fn voice_panel(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    channel: ChannelId,
    ctx: &egui::Context,
    compact: bool,
) {
    let (chan_name, server, ping) = app
        .view
        .as_ref()
        .map(|v| {
            (
                v.channel(channel)
                    .map(|c| c.name.clone())
                    .unwrap_or_default(),
                v.name.clone(),
                v.ping_ms,
            )
        })
        .unwrap_or_default();
    ui.horizontal(|ui| {
        let (r, sig) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        let quality_col = match ping {
            Some(p) if p > 250.0 => theme::color(theme::RED),
            Some(p) if p > 120.0 => theme::color(theme::YELLOW),
            _ => theme::color(theme::GREEN),
        };
        icons::paint(ui.painter(), r, Icon::Signal, quality_col);
        sig.on_hover_text(match ping {
            Some(p) => format!("Latência até o host: {p:.0} ms"),
            None => "Medindo latência…".into(),
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if !compact {
                ui.label(
                    RichText::new("Voz conectada")
                        .strong()
                        .size(14.0)
                        .color(quality_col),
                );
            }
            let place = format!("{chan_name} / {server}");
            if ui
                .add(
                    egui::Label::new(
                        RichText::new(place)
                            .size(12.0)
                            .color(theme::color(theme::TEXT_MUTED)),
                    )
                    .truncate()
                    .sense(Sense::click()),
                )
                .clicked()
            {
                app.selected = Some(channel);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(
                ui,
                Icon::HangUp,
                30.0,
                theme::color(theme::TEXT),
                "Desconectar",
            )
            .clicked()
            {
                app.leave_voice();
            }
        });
    });
    ui.add_space(if compact { 4.0 } else { 8.0 });
    ui.columns(2, |cols| {
        let cam_on = app.camera.is_some();
        if wide_button(
            &mut cols[0],
            if cam_on {
                Icon::Camera
            } else {
                Icon::CameraOff
            },
            if cam_on {
                "Desligar câmera"
            } else {
                "Ligar câmera"
            },
            cam_on,
        )
        .clicked()
        {
            app.toggle_camera(ctx);
        }
        let live = app.share.is_some();
        let label = if live {
            "Parar transmissão"
        } else {
            "Compartilhar sua tela"
        };
        if wide_button(&mut cols[1], Icon::Screen, label, live).clicked() {
            if live {
                app.stop_share();
            } else {
                app.open_share_picker();
            }
        }
    });
}

pub(super) fn wide_button(
    ui: &mut egui::Ui,
    icon: Icon,
    tip: &str,
    active: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let bg = if active {
        theme::color(theme::GREEN)
    } else if resp.hovered() {
        theme::color(theme::SELECTED)
    } else {
        theme::color(theme::INPUT)
    };
    ui.painter().rect_filled(rect, 6.0, bg);
    let ir = Rect::from_center_size(rect.center(), vec2(20.0, 20.0));
    icons::paint(ui.painter(), ir, icon, Color32::WHITE);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
    resp.on_hover_text(tip)
}

pub(super) fn user_panel(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let (name, color, me) = app
        .view
        .as_ref()
        .map(|v| (v.user_name(v.me), v.user_color(v.me), v.me))
        .unwrap_or((app.settings.name.clone(), app.settings.color, 0));
    let speaking = app.audio_shared.speaking.load(Ordering::Relaxed);
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let texture = app.avatar_texture(ui.ctx(), me);
        // We are connected by definition; only our chosen presence matters.
        let my_state = PresenceState::resolve(
            true,
            app.view
                .as_ref()
                .and_then(|v| v.workspace.profiles.iter().find(|p| p.user == me))
                .map(|p| p.presence),
        );
        if Avatar::new(&name, AvatarSize::Sm)
            .user(me)
            .color(color)
            .texture(texture.as_ref())
            .presence(my_state)
            .speaking(speaking)
            .backdrop(theme::color(theme::PANEL))
            .show(ui)
            .clicked()
        {
            app.open_profile(me);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(9.0);
            ui.add(
                egui::Label::new(
                    RichText::new(&name)
                        .strong()
                        .size(14.0)
                        .color(theme::color(theme::TEXT_BRIGHT)),
                )
                .truncate(),
            );
            ui.label(
                RichText::new(format!("#{me:04}"))
                    .size(12.0)
                    .color(theme::color(theme::TEXT_MUTED)),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if widgets::icon_button(
                ui,
                Icon::Gear,
                32.0,
                theme::color(theme::TEXT_MUTED),
                "Preferências",
            )
            .clicked()
            {
                app.modal = Modal::Settings(SettingsTab::Account);
            }
            let deaf = app.audio_shared.deafened.load(Ordering::Relaxed);
            let (icon, tip) = if deaf {
                (Icon::HeadphonesOff, "Ativar áudio")
            } else {
                (Icon::Headphones, "Desativar áudio")
            };
            if widgets::icon_button(ui, icon, 32.0, theme::color(theme::TEXT_MUTED), tip).clicked()
            {
                app.toggle_deafen();
            }
            let muted = app.audio_shared.muted.load(Ordering::Relaxed) || deaf;
            let (icon, tip) = if muted {
                (Icon::MicOff, "Ativar microfone")
            } else {
                (Icon::Mic, "Silenciar microfone")
            };
            if widgets::icon_button(ui, icon, 32.0, theme::color(theme::TEXT_MUTED), tip).clicked()
            {
                app.toggle_mute();
            }
        });
    });
}
