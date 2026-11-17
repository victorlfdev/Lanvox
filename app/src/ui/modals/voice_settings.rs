use super::*;

pub(super) fn voice_tab(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    if ui.button("Atualizar dispositivos").clicked() {
        app.device_cache = None;
        app.restart_audio();
    }
    if app.device_cache.is_none() {
        app.device_cache = Some((
            crate::media::audio::input_device_names(),
            crate::media::audio::output_device_names(),
            Vec::new(),
        ));
    }
    let (inputs, outputs) = app
        .device_cache
        .as_ref()
        .map(|(i, o, _)| (i.clone(), o.clone()))
        .unwrap_or_default();
    let mut restart = false;
    ui.columns(2, |cols| {
        widgets::section_label(&mut cols[0], "Dispositivo de entrada");
        restart |= device_combo(
            &mut cols[0],
            "input-dev",
            &mut app.settings.input_device,
            &inputs,
        );
        widgets::section_label(&mut cols[1], "Dispositivo de saída");
        restart |= device_combo(
            &mut cols[1],
            "output-dev",
            &mut app.settings.output_device,
            &outputs,
        );
        widgets::section_label(&mut cols[0], "Volume de entrada");
        let mut gain = app.settings.input_gain * 100.0;
        if cols[0]
            .add(egui::Slider::new(&mut gain, 0.0..=200.0).suffix("%"))
            .changed()
        {
            app.settings.input_gain = gain / 100.0;
            app.audio_shared.set_input_gain(app.settings.input_gain);
        }
        widgets::section_label(&mut cols[1], "Volume de saída");
        let mut vol = app.settings.output_volume * 100.0;
        if cols[1]
            .add(egui::Slider::new(&mut vol, 0.0..=200.0).suffix("%"))
            .changed()
        {
            app.settings.output_volume = vol / 100.0;
            app.mixer.set_output_volume(app.settings.output_volume);
        }
    });
    if restart {
        app.restart_audio();
    }

    ui.add_space(12.0);
    widgets::section_label(ui, "Teste de microfone");
    ui.horizontal_wrapped(|ui| {
        let testing = app.audio_shared.mic_test.load(Ordering::Relaxed);
        if components::primary_button(
            ui,
            if testing {
                "Parar teste"
            } else {
                "Testar microfone"
            },
        )
        .clicked()
        {
            app.audio_shared.mic_test.store(!testing, Ordering::Relaxed);
            if !testing && app.audio.is_none() {
                app.start_audio();
            }
        }
        ui.label(
            RichText::new("Fale para ouvir o retorno do microfone.")
                .size(12.0)
                .color(theme::color(theme::TEXT_FAINT)),
        );
    });

    ui.add_space(12.0);
    widgets::section_label(ui, "Modo de entrada");
    let mut mode = app.settings.input_mode;
    ui.radio_value(
        &mut mode,
        InputMode::SmartVoice,
        "Voz inteligente (automática)",
    );
    ui.radio_value(
        &mut mode,
        InputMode::VoiceActivity,
        "Atividade de voz (limiar manual)",
    );
    ui.radio_value(&mut mode, InputMode::PushToTalk, "Pressione para falar");
    if mode != app.settings.input_mode {
        app.settings.input_mode = mode;
        app.audio_shared.set_mode(mode);
    }
    if mode == InputMode::PushToTalk {
        ui.add_space(6.0);
        widgets::section_label(ui, "Tecla para falar");
        egui::ComboBox::from_id_salt("ptt-key")
            .selected_text(app.settings.ptt_key.label())
            .show_ui(ui, |ui| {
                for k in PttKey::ALL {
                    if ui
                        .selectable_value(&mut app.settings.ptt_key, k, k.label())
                        .clicked()
                    {
                        app.audio_shared.ptt_vk.store(k.vk(), Ordering::Relaxed);
                    }
                }
            });
        ui.label(
            RichText::new("No Windows, funciona também com o aplicativo em segundo plano.")
                .size(12.0)
                .color(theme::color(theme::TEXT_FAINT)),
        );
    } else if mode == InputMode::SmartVoice {
        ui.add_space(6.0);
        level_meter(app, ui);
        let mut margin = app.settings.smart_margin_db;
        if ui
            .add(
                egui::Slider::new(&mut margin, 6.0..=35.0)
                    .suffix(" dB")
                    .text("Rigor contra som de fundo"),
            )
            .changed()
        {
            app.settings.smart_margin_db = margin;
            app.audio_shared.set_smart_margin_db(margin);
        }
        ui.label(
            RichText::new(
                "Aumente se a TV ou outras vozes ao fundo ainda passam; diminua se sua voz estiver sendo cortada.",
            )
            .size(12.0)
            .color(theme::color(theme::TEXT_FAINT)),
        );
        ui.label(
            RichText::new(
                "Ajusta-se sozinha ao ruído do ambiente e usa o RNNoise para reconhecer sua fala. Não há limiar para regular.",
            )
            .size(12.0)
            .color(theme::color(theme::TEXT_FAINT)),
        );
    } else {
        ui.add_space(6.0);
        widgets::section_label(ui, "Sensibilidade do microfone");
        level_meter(app, ui);
        let mut th = app.settings.threshold_db;
        if ui
            .add(
                egui::Slider::new(&mut th, -100.0..=0.0)
                    .suffix(" dB")
                    .text("Limiar"),
            )
            .changed()
        {
            app.settings.threshold_db = th;
            app.audio_shared.set_threshold_db(th);
        }
    }
    sounds_and_shortcuts(app, ui);
    ui.collapsing("Processamento e diagnóstico avançados", |ui| {
        voice_processing(app, ui)
    });
    if app.audio.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    }
}

fn voice_processing(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let mut changed = ui
        .checkbox(&mut app.settings.echo_cancellation, "Cancelamento de eco")
        .changed();
    changed |= ui
        .checkbox(&mut app.settings.noise_suppression, "Supressão de ruído")
        .changed();
    if app.settings.noise_suppression {
        changed |= ui
            .checkbox(
                &mut app.settings.rnnoise,
                "Usar RNNoise (rede neural, mais forte)",
            )
            .changed();
    }
    if changed {
        app.audio_shared.processor.set_rnnoise(app.settings.rnnoise);
        app.audio_shared.processor.configure(
            app.settings.echo_cancellation,
            app.settings.noise_suppression,
        );
    }
    ui.collapsing("Ajuste avançado de eco", |ui| {
        if ui
            .add(
                egui::Slider::new(&mut app.settings.echo_delay_ms, 0..=500)
                    .suffix(" ms")
                    .text("Atraso do dispositivo"),
            )
            .changed()
        {
            app.audio_shared
                .processor
                .set_delay_ms(app.settings.echo_delay_ms);
        }
    });
    if let Some(client) = &app.client {
        let stats = client.media_stats();
        ui.label(if stats.active {
            "Mídia por UDP cifrado: ativa"
        } else {
            "Mídia por UDP: aguardando rota"
        });
        ui.label(format!(
            "Enviado {:.1} MiB · recebido {:.1} MiB · descartes {} · fila {} KiB",
            stats.sent_bytes as f64 / 1048576.0,
            stats.received_bytes as f64 / 1048576.0,
            stats.dropped,
            stats.queued_bytes / 1024
        ));
        let people: Vec<_> = app
            .view
            .as_ref()
            .map(|v| v.users.values().map(|u| (u.id, u.name.clone())).collect())
            .unwrap_or_default();
        for (id, name) in people {
            if let Some(stats) = app.mixer.voice_stats(id) {
                ui.label(format!(
                    "{name}: jitter {:.1}ms · buffer {}ms · perdas {} · atrasados {}",
                    stats.jitter_ms, stats.target_ms, stats.lost, stats.late
                ));
            }
        }
    }
}

/// Which shortcut is waiting for a key press: 0 = mute, 1 = deafen.
#[allow(deprecated)]
pub(super) fn recording_id() -> egui::Id {
    egui::Id::unique("hotkey-recording")
}

pub(super) fn hotkey_row(app: &mut LanvoxApp, ui: &mut egui::Ui, slot: usize, title: &str) {
    use crate::settings::Hotkey;
    let recording: Option<usize> = ui.data(|d| d.get_temp(recording_id()));
    let active = recording == Some(slot);
    let current = if slot == 0 {
        app.settings.mute_hotkey
    } else {
        app.settings.deafen_hotkey
    };
    let registered = {
        let (m, d) = crate::notifications::hotkey_status();
        if slot == 0 {
            m
        } else {
            d
        }
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(title);
        let text = if active { "Pressione as teclas…".to_string() } else { current.label() };
        if ui.add_sized([170.0, 26.0], egui::Button::new(text).selected(active)).clicked() {
            if active {
                ui.data_mut(|d| d.remove::<usize>(recording_id()));
                crate::notifications::set_hotkeys(app.settings.mute_hotkey, app.settings.deafen_hotkey);
            } else {
                ui.data_mut(|d| d.insert_temp(recording_id(), slot));
                // Release global shortcuts so the combination reaches this window.
                crate::notifications::set_hotkeys(Hotkey::none(), Hotkey::none());
            }
        }
        if ui.button("Remover").clicked() {
            if slot == 0 { app.settings.mute_hotkey = Hotkey::none() } else { app.settings.deafen_hotkey = Hotkey::none() }
            ui.data_mut(|d| d.remove::<usize>(recording_id()));
            crate::notifications::set_hotkeys(app.settings.mute_hotkey, app.settings.deafen_hotkey);
        }
        if !active && current.is_set() && cfg!(windows) && !registered {
            ui.label(
                RichText::new("Indisponível em segundo plano (combinação em uso). Funciona com o app em foco.")
                    .size(12.0)
                    .color(theme::color(theme::TEXT_FAINT)),
            );
        }
    });
    if active {
        let captured = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        if let Some((key, modifiers)) = captured {
            let cancel = key == egui::Key::Escape;
            if let Some(vk) = Hotkey::vk_for_key(key) {
                let is_function = (0x70..=0x87).contains(&vk);
                if !cancel && (modifiers.ctrl || modifiers.alt || is_function) {
                    let hotkey = Hotkey::new(modifiers.ctrl, modifiers.shift, modifiers.alt, vk);
                    if slot == 0 {
                        app.settings.mute_hotkey = hotkey
                    } else {
                        app.settings.deafen_hotkey = hotkey
                    }
                    ui.data_mut(|d| d.remove::<usize>(recording_id()));
                    crate::notifications::set_hotkeys(
                        app.settings.mute_hotkey,
                        app.settings.deafen_hotkey,
                    );
                }
            }
            if cancel {
                ui.data_mut(|d| d.remove::<usize>(recording_id()));
                crate::notifications::set_hotkeys(
                    app.settings.mute_hotkey,
                    app.settings.deafen_hotkey,
                );
            }
        }
        ui.label(
            RichText::new("Use Ctrl ou Alt com uma tecla (ou uma tecla F). Esc cancela.")
                .size(12.0)
                .color(theme::color(theme::TEXT_FAINT)),
        );
        ui.ctx().request_repaint();
    }
}

pub(super) fn sounds_and_shortcuts(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    ui.add_space(12.0);
    widgets::section_label(ui, "Sons de notificação");
    ui.checkbox(
        &mut app.settings.sounds,
        "Tocar sons (entrada/saída na chamada, chamadas privadas, mensagens, microfone)",
    );
    ui.add_enabled_ui(app.settings.sounds, |ui| {
        let mut volume = app.settings.sounds_volume * 100.0;
        if ui
            .add(
                egui::Slider::new(&mut volume, 0.0..=100.0)
                    .suffix("%")
                    .text("Volume dos sons"),
            )
            .changed()
        {
            app.settings.sounds_volume = volume / 100.0;
        }
        if ui.button("Testar som").clicked() {
            crate::sfx::play(crate::sfx::Sfx::Join, &app.settings);
        }
    });

    ui.add_space(12.0);
    widgets::section_label(ui, "Atalhos de teclado");
    hotkey_row(app, ui, 0, "Ativar/desativar microfone");
    hotkey_row(app, ui, 1, "Ativar/desativar áudio (ensurdecer)");
    ui.label(
        RichText::new("No Windows, os atalhos funcionam também com o aplicativo em segundo plano, como dentro de um jogo.")
            .size(12.0)
            .color(theme::color(theme::TEXT_FAINT)),
    );
}

pub(super) fn level_meter(app: &LanvoxApp, ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width().min(420.0), 10.0), Sense::hover());
    let to_x = |db: f32| rect.left() + rect.width() * ((db + 100.0) / 100.0).clamp(0.0, 1.0);
    let th_x = to_x(app.settings.threshold_db);
    ui.painter().rect_filled(
        Rect::from_min_max(rect.min, pos2(th_x, rect.bottom())),
        5.0,
        theme::color(theme::YELLOW).gamma_multiply(0.6),
    );
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(th_x, rect.top()), rect.max),
        5.0,
        theme::color(theme::GREEN).gamma_multiply(0.6),
    );
    if app.audio.is_some() {
        let lvl = to_x(app.audio_shared.level_db());
        ui.painter().rect_filled(
            Rect::from_min_max(rect.min, pos2(lvl, rect.bottom())),
            5.0,
            Color32::WHITE.gamma_multiply(0.85),
        );
    }
    ui.painter().line_segment(
        [
            pos2(th_x, rect.top() - 3.0),
            pos2(th_x, rect.bottom() + 3.0),
        ],
        Stroke::new(2.0, Color32::WHITE),
    );
    if app.audio.is_none() {
        ui.label(
            RichText::new("Inicie o teste ou entre em uma chamada para ver o nível.")
                .size(12.0)
                .color(theme::color(theme::TEXT_FAINT)),
        );
    }
}

pub(super) fn device_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<String>,
    names: &[String],
) -> bool {
    let mut changed = false;
    let shown = value.clone().unwrap_or_else(|| "Padrão do sistema".into());
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width() - 8.0)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(value.is_none(), "Padrão do sistema")
                .clicked()
            {
                *value = None;
                changed = true;
            }
            for n in names {
                if ui
                    .selectable_label(value.as_deref() == Some(n.as_str()), n)
                    .clicked()
                {
                    *value = Some(n.clone());
                    changed = true;
                }
            }
        });
    changed
}

/// Cisco's license requires the on/off switch, this attribution next to it and its
/// license text where licensing information is shown.
fn h264_codec_section(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    use crate::media::openh264_runtime::{self as codec, Status};
    widgets::section_label(ui, "Codec de vídeo H.264");
    if ui
        .checkbox(
            &mut app.settings.h264_codec,
            "Usar o OpenH264 da Cisco (baixado da Cisco neste computador)",
        )
        .changed()
    {
        app.save_preferences();
        codec::configure(app.settings.h264_codec);
    }
    ui.label(RichText::new(codec::ATTRIBUTION).strong());
    let status = codec::status();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(status.label()).color(theme::color(theme::TEXT_MUTED)));
        if app.settings.h264_codec
            && matches!(status, Status::Missing | Status::Failed(_))
            && ui.button("Baixar agora").clicked()
        {
            codec::download();
        }
    });
    if status == Status::Downloading {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
    ui.label(
        RichText::new(
            "Sem o codec, sua câmera e sua tela são enviadas como JPEG, e vídeos H.264 \
             de outras pessoas não aparecem. Mudanças valem para a próxima transmissão.",
        )
        .small()
        .color(theme::color(theme::TEXT_MUTED)),
    );
    egui::CollapsingHeader::new("Licença do OpenH264")
        .id_salt("openh264-license")
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(codec::LICENSE).small());
                });
        });
    ui.add_space(14.0);
}

fn link_preview_section(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    widgets::section_label(ui, "Prévias de links de vídeo");
    if ui
        .checkbox(
            &mut app.settings.remote_link_previews,
            "Carregar miniaturas de links de vídeo (YouTube e links diretos)",
        )
        .changed()
    {
        app.save_preferences();
    }
    ui.label(
        RichText::new(
            "Desativado por padrão: carregar uma miniatura contata o site do link e revela o seu endereço IP a ele.",
        )
        .color(theme::color(theme::TEXT_MUTED)),
    );
    ui.add_space(8.0);
}

pub(super) fn video_tab(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let needs = app
        .device_cache
        .as_ref()
        .map(|(_, _, c)| c.is_empty())
        .unwrap_or(true);
    if needs {
        let cams = crate::media::camera::camera_names();
        match &mut app.device_cache {
            Some((_, _, c)) => *c = cams,
            None => app.device_cache = Some((Vec::new(), Vec::new(), cams)),
        }
    }
    let cams = app
        .device_cache
        .as_ref()
        .map(|(_, _, c)| c.clone())
        .unwrap_or_default();
    widgets::section_label(ui, "Câmera");
    if cams.is_empty() {
        ui.label(
            RichText::new("Nenhuma câmera encontrada.").color(theme::color(theme::TEXT_MUTED)),
        );
        ui.collapsing("Vídeo e privacidade avançados", |ui| {
            link_preview_section(app, ui);
            h264_codec_section(app, ui);
        });
        return;
    }
    let current = cams
        .get(app.settings.camera_index as usize)
        .cloned()
        .unwrap_or_else(|| cams[0].clone());
    let before = app.settings.camera_index;
    egui::ComboBox::from_id_salt("camera")
        .width(320.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (i, n) in cams.iter().enumerate() {
                ui.selectable_value(&mut app.settings.camera_index, i as u32, n);
            }
        });
    if app.settings.camera_index != before {
        app.restart_camera(&ctx);
    }
    ui.add_space(12.0);
    let previous_quality = app.settings.camera;
    components::form_row(ui, "Qualidade da câmera", |ui| {
        egui::ComboBox::from_id_salt("camera-resolution")
            .selected_text(app.settings.camera.resolution.label())
            .show_ui(ui, |ui| {
                for resolution in crate::media::camera::CameraResolution::ALL {
                    ui.selectable_value(
                        &mut app.settings.camera.resolution,
                        resolution,
                        resolution.label(),
                    );
                }
            });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut app.settings.camera.fps, 15, "15 fps");
            ui.selectable_value(&mut app.settings.camera.fps, 30, "30 fps");
        });
        ui.checkbox(&mut app.settings.camera.mirror, "Espelhar minha prévia");
    });
    if app.settings.camera != previous_quality {
        app.settings.camera = app.settings.camera.normalized();
        app.save_preferences();
        app.restart_camera(&ctx);
    }
    if let Some(camera) = app.camera.as_ref().or(app.camera_preview.as_ref()) {
        let stats = camera.stats.lock();
        ui.label(
            RichText::new(format!(
                "{} × {} · {:.1} fps · {} · adaptação {}",
                stats.width, stats.height, stats.fps, stats.encoder, stats.adaptation
            ))
            .small()
            .color(theme::color(theme::TEXT_MUTED)),
        );
        if stats.target_bps > 0 {
            ui.label(format!(
                "Bitrate-alvo {:.2} Mb/s · mínimo 1 Mb/s",
                stats.target_bps as f64 / 1_000_000.0
            ));
        }
        if stats.host_budget_exceeded {
            ui.label(
                RichText::new("A soma dos bitrates mínimos excede o orçamento de vídeo do host.")
                    .color(theme::color(theme::YELLOW)),
            );
        }
    }
    ui.add_space(10.0);
    let key = (app.me(), StreamKind::Camera);
    let width = ui.available_width().min(400.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, width * 9.0 / 16.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 8.0, theme::color(theme::RAIL));
    if app.camera_preview.is_some() || app.camera.is_some() {
        if let Some((tex, size)) = app.texture_for(&ctx, key) {
            let s = (rect.width() / size.x).min(rect.height() / size.y);
            ui.painter().image(
                tex,
                Rect::from_center_size(rect.center(), size * s),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    } else {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Prévia desligada",
            FontId::proportional(14.0),
            theme::color(theme::TEXT_MUTED),
        );
    }
    ui.add_space(8.0);
    if app.camera.is_none() {
        let on = app.camera_preview.is_some();
        if components::primary_button(
            ui,
            if on {
                "Parar prévia"
            } else {
                "Testar câmera"
            },
        )
        .clicked()
        {
            if on {
                app.camera_preview = None;
                app.pictures.remove(key);
            } else {
                app.camera_preview = Some(crate::media::camera::CameraSession::start_configured(
                    app.settings.camera_index,
                    None,
                    app.pictures.clone(),
                    app.me(),
                    ctx.clone(),
                    app.settings.camera,
                ));
            }
        }
    }
    ui.collapsing("Vídeo e privacidade avançados", |ui| {
        link_preview_section(app, ui);
        h264_codec_section(app, ui);
    });
}
