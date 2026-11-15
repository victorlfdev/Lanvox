use super::*;

pub(super) fn settings(app: &mut LanvoxApp, ctx: &egui::Context, tab: SettingsTab) {
    let mut close = false;
    let mut new_tab = tab;
    let resp = egui::Modal::new(egui::Id::unique("settings"))
        .frame(
            Frame::new()
                .fill(theme::color(theme::CHAT))
                .corner_radius(8),
        )
        .show(ctx, |ui| {
            let size = vec2(
                (ctx.content_rect().width() - 80.0).clamp(560.0, 900.0),
                (ctx.content_rect().height() - 80.0).clamp(380.0, 620.0),
            );
            ui.set_min_size(size);
            ui.set_max_size(size);
            ui.horizontal_top(|ui| {
                // Navigation.
                Frame::new()
                    .fill(theme::color(theme::SIDEBAR))
                    .inner_margin(Margin::same(14))
                    .corner_radius(egui::CornerRadius {
                        nw: 8,
                        sw: 8,
                        ne: 0,
                        se: 0,
                    })
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(180.0);
                            ui.set_max_width(180.0);
                            ui.set_min_height(size.y - 28.0);
                            widgets::section_label(ui, "Preferências");
                            ui.add_space(8.0);
                            for (target, label) in [
                                (SettingsTab::Account, "Minha conta"),
                                (SettingsTab::Appearance, "Aparência"),
                                (SettingsTab::Voice, "Voz e áudio"),
                                (SettingsTab::Video, "Câmera"),
                                (SettingsTab::Streaming, "Compartilhamento de tela"),
                            ] {
                                if ui
                                    .add_sized(
                                        [180.0, 40.0],
                                        egui::Button::new(label).selected(tab == target).wrap(),
                                    )
                                    .clicked()
                                {
                                    new_tab = target;
                                }
                            }
                        });
                    });
                ui.vertical(|ui| {
                    // Never wider than the modal minus the nav column, so
                    // wide content wraps/scrolls instead of pushing the modal
                    // past the window.
                    let content_w = (size.x - 230.0).max(280.0);
                    ui.set_width(content_w);
                    ui.set_max_width(content_w);
                    ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let title = match tab {
                            SettingsTab::Account => "Minha conta",
                            SettingsTab::Appearance => "Aparência",
                            SettingsTab::Voice => "Voz e áudio",
                            SettingsTab::Video => "Câmera",
                            SettingsTab::Streaming => "Compartilhamento de tela",
                        };
                        ui.label(
                            RichText::new(title)
                                .size(20.0)
                                .strong()
                                .color(theme::color(theme::TEXT_BRIGHT)),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(12.0);
                            if widgets::icon_button(
                                ui,
                                Icon::Close,
                                28.0,
                                theme::color(theme::TEXT_MUTED),
                                "Fechar (Esc)",
                            )
                            .clicked()
                            {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .max_height(size.y - 74.0)
                        .max_width(content_w)
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            Frame::new()
                                .inner_margin(Margin {
                                    left: 8,
                                    right: 20,
                                    top: 0,
                                    bottom: 12,
                                })
                                .show(ui, |ui| {
                                    ui.set_max_width((content_w - 28.0).max(240.0));
                                    match tab {
                                        SettingsTab::Account => account_tab(app, ui),
                                        SettingsTab::Appearance => appearance_tab(app, ui),
                                        SettingsTab::Voice => voice_tab(app, ui),
                                        SettingsTab::Video => video_tab(app, ui),
                                        SettingsTab::Streaming => streaming_tab(app, ui),
                                    }
                                });
                        });
                });
            });
        });
    if resp.should_close() {
        close = true;
    }
    if close {
        app.modal = Modal::None;
        app.audio_shared.mic_test.store(false, Ordering::Relaxed);
        app.camera_preview = None;
        if app
            .view
            .as_ref()
            .and_then(|v| v.my_voice().channel)
            .is_none()
        {
            app.audio = None;
        }
        app.settings.save();
    } else if new_tab != tab {
        if new_tab != SettingsTab::Video {
            app.camera_preview = None;
        }
        app.modal = Modal::Settings(new_tab);
    }
}

pub(super) fn appearance_tab(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    ui.label("Ajuste o espaço da interface ao seu monitor e ao seu jeito de conversar.");
    ui.add_space(16.0);
    components::form_row(ui, "Tema", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut app.settings.theme, 0, "Escuro");
            ui.selectable_value(&mut app.settings.theme, 1, "Ônix");
            ui.selectable_value(&mut app.settings.theme, 2, "Cinza");
        });
    });
    components::form_row(ui, "Densidade", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut app.settings.ui_density, 0, "Compacta");
            ui.selectable_value(&mut app.settings.ui_density, 1, "Padrão");
            ui.selectable_value(&mut app.settings.ui_density, 2, "Espaçosa");
        });
    });
    components::form_row(ui, "Escala da interface", |ui| {
        ui.add(
            egui::Slider::new(&mut app.settings.ui_scale, 0.8..=1.4)
                .step_by(0.05)
                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
        );
    });
    components::form_row(ui, "Tamanho do texto no chat", |ui| {
        if ui
            .add(
                egui::Slider::new(&mut app.settings.chat_text_size, 12.0..=24.0)
                    .step_by(1.0)
                    .suffix(" px"),
            )
            .changed()
        {
            app.workspace_ui.message_heights.clear();
        }
    });
    components::form_row(ui, "Espaço entre grupos de mensagens", |ui| {
        if ui
            .add(
                egui::Slider::new(&mut app.settings.chat_group_spacing, 0.0..=32.0)
                    .step_by(1.0)
                    .suffix(" px"),
            )
            .changed()
        {
            app.workspace_ui.message_heights.clear();
        }
    });
    components::form_row(ui, "Lista de canais", |ui| {
        ui.add(egui::Slider::new(&mut app.settings.channel_width, 200.0..=360.0).suffix(" px"));
    });
    components::form_row(ui, "Lista de membros", |ui| {
        ui.add(egui::Slider::new(&mut app.settings.member_width, 190.0..=320.0).suffix(" px"));
    });
    ui.label(
        RichText::new("As laterais também podem ser redimensionadas arrastando a divisória.")
            .small()
            .color(theme::color(theme::TEXT_MUTED)),
    );
    if ui.button("Restaurar aparência padrão").clicked() {
        app.settings.ui_scale = 1.0;
        app.settings.ui_density = 1;
        app.settings.chat_text_size = 15.0;
        app.settings.chat_group_spacing = 14.0;
        app.workspace_ui.message_heights.clear();
        app.settings.theme = 0;
        app.settings.channel_width = theme::SIDEBAR_W;
        app.settings.member_width = theme::MEMBERS_W;
    }
}

pub(super) fn account_tab(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if app.connected() && ui.button("Editar perfil e presença").clicked() {
            app.open_profile(app.me());
        }
        ui.checkbox(&mut app.settings.notifications, "Notificações do sistema");
    });
    ui.checkbox(&mut app.settings.do_not_disturb, "Não perturbar");
    ui.checkbox(
        &mut app.settings.notification_preview,
        "Mostrar conteúdo nas notificações",
    );
    ui.checkbox(
        &mut app.settings.auto_reconnect,
        "Reconectar automaticamente",
    );
    if let Ok(fingerprint) = crate::notifications::identity_fingerprint() {
        ui.collapsing("Identidade base deste dispositivo", |ui| {
            ui.label("Guarde uma cópia privada para recuperar sua conta em outra instalação.");
            ui.label(egui::RichText::new(&fingerprint).monospace());
            if ui.button("Copiar impressão digital").clicked() {
                ui.ctx().copy_text(fingerprint.clone());
            }
        });
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                app.file_pending.is_none(),
                egui::Button::new("Exportar identidade base…"),
            )
            .clicked()
        {
            let (tx, rx) = crossbeam_channel::bounded(1);
            app.file_pending = Some(rx);
            std::thread::spawn(move || {
                let result = if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("lanvox-identity.key")
                    .save_file()
                {
                    match crate::notifications::export_identity(&path) {
                        Ok(()) => crate::app::FileTask::Saved(
                            "Identidade exportada. Mantenha esse arquivo privado.".into(),
                        ),
                        Err(e) => {
                            crate::app::FileTask::Error(format!("Não foi possível exportar: {e}"))
                        }
                    }
                } else {
                    crate::app::FileTask::Cancelled
                };
                let _ = tx.send(result);
            });
        }
        if ui
            .add_enabled(
                !app.connected()
                    && !app.connecting
                    && app.hosted.is_none()
                    && app.file_pending.is_none(),
                egui::Button::new("Importar identidade base…"),
            )
            .clicked()
        {
            let (tx, rx) = crossbeam_channel::bounded(1);
            app.file_pending = Some(rx);
            std::thread::spawn(move || {
                let result = if let Some(path) = rfd::FileDialog::new().pick_file() {
                    match crate::notifications::import_identity(&path) {
                        Ok(()) => crate::app::FileTask::IdentityImported,
                        Err(e) => {
                            crate::app::FileTask::Error(format!("Não foi possível importar: {e}"))
                        }
                    }
                } else {
                    crate::app::FileTask::Cancelled
                };
                let _ = tx.send(result);
            });
        }
    });
    host_identity_controls(app, ui);
    if let Some(client) = &app.client {
        ui.separator();
        ui.label(format!("Host autenticado: {}", client.address));
        ui.add(egui::Label::new(RichText::new(&client.host_fingerprint).monospace()).wrap());
        if ui.button("Copiar identidade do host").clicked() {
            ui.ctx().copy_text(client.host_fingerprint.clone());
        }
    }
    if let Some(host) = &app.hosted {
        let name = host.name.clone();
        let port = host.port;
        let fingerprint = host.fingerprint.clone();
        let token = host.registration_token.clone();
        ui.separator();
        ui.label(format!("{name} · {}:{port}", app.settings.host_bind));
        if let Some(token) = token {
            if ui.button("Copiar convite privado do host").clicked() {
                ui.ctx().copy_text(format!(
                    "lanvox://{}:{port}?token={token}&fingerprint={fingerprint}",
                    app.settings.host_bind
                ));
            }
            ui.label(
                RichText::new(
                    "O convite contém acesso ao host. Compartilhe somente com seus amigos.",
                )
                .small(),
            );
        }
        if ui.button("Parar o servidor local").clicked() {
            app.stop_host();
        }
    }
    ui.add_space(12.0);
    Frame::new()
        .fill(theme::color(theme::RAIL))
        .corner_radius(8)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::avatar(
                    ui,
                    72.0,
                    &app.settings.name,
                    color32(app.settings.color),
                    false,
                    Some(true),
                );
                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(&app.settings.name)
                            .size(20.0)
                            .strong()
                            .color(theme::color(theme::TEXT_BRIGHT)),
                    );
                    ui.label(
                        RichText::new(if app.connected() {
                            "Disponível"
                        } else {
                            "Sem conexão"
                        })
                        .color(theme::color(theme::TEXT_MUTED)),
                    );
                });
            });
        });
    ui.add_space(12.0);
    widgets::section_label(ui, "Nome de exibição");
    ui.add(
        egui::TextEdit::singleline(&mut app.settings.name)
            .char_limit(32)
            .margin(Margin::symmetric(10, 8)),
    );
    if app.connected() {
        ui.label(
            RichText::new("O novo nome será usado na próxima conexão.")
                .size(12.0)
                .color(theme::color(theme::TEXT_FAINT)),
        );
    }
    ui.add_space(8.0);
    widgets::section_label(ui, "Cor do avatar");
    ui.horizontal(|ui| {
        for c in AVATAR_COLORS {
            let (rect, r) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
            ui.painter().circle_filled(rect.center(), 13.0, color32(c));
            if app.settings.color == c {
                ui.painter()
                    .circle_stroke(rect.center(), 15.0, Stroke::new(2.0, Color32::WHITE));
            }
            if r.clicked() {
                app.settings.color = c;
            }
        }
    });
    if let Some(h) = &app.hosted {
        ui.add_space(16.0);
        widgets::section_label(ui, "Seu host");
        let ips = local_addresses();
        ui.label(format!("\"{}\" is running on port {}.", h.name, h.port));
        if !ips.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Endereços disponíveis: {}",
                    ips.iter()
                        .map(|ip| format!("{ip}:{}", h.port))
                        .collect::<Vec<_>>()
                        .join("  ou  ")
                ))
                .color(theme::color(theme::TEXT_MUTED)),
            );
        }
    }
    ui.add_space(16.0);
    update_section(app, ui);
}

/// Signed updates from GitHub Releases. Nothing is contacted until the person asks or
/// turns on the startup check.
fn update_section(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    use crate::updater::{self, State};
    let muted = |text: &str| RichText::new(text.to_string()).color(theme::color(theme::TEXT_MUTED));
    widgets::section_label(ui, "Atualizações");
    ui.label(muted(&format!(
        "Versão instalada: {} (protocolo {}).",
        env!("CARGO_PKG_VERSION"),
        crate::auth::PROTOCOL_VERSION
    )));
    if !updater::configured() {
        ui.label(muted(
            "Esta versão não tem chave de atualização: baixe novas versões manualmente em github.com.",
        ));
        return;
    }
    if ui
        .checkbox(
            &mut app.settings.check_updates_on_start,
            "Procurar atualizações ao abrir (contata o GitHub, que vê o seu endereço IP)",
        )
        .changed()
    {
        app.settings.save();
    }
    let state = updater::state();
    match &state {
        State::Idle => {}
        State::Checking => {
            ui.label(muted("Procurando…"));
        }
        State::UpToDate => {
            ui.label(muted("Você está na versão mais recente."));
        }
        State::Failed(error) => {
            ui.label(
                RichText::new(format!("Falha: {error}")).color(theme::color(theme::TEXT_MUTED)),
            );
        }
        State::Available(manifest) | State::Downloading(manifest) | State::Ready(manifest, _) => {
            ui.label(RichText::new(format!("Versão {} disponível.", manifest.version)).strong());
            if !manifest.notes.is_empty() {
                ui.label(muted(&manifest.notes));
            }
            if manifest.protocol != crate::auth::PROTOCOL_VERSION {
                ui.label(muted(
                    "Esta versão muda o protocolo: atualize o host e todos os clientes juntos, ou eles não conseguirão se conectar.",
                ));
            }
        }
    }
    ui.horizontal_wrapped(|ui| match &state {
        State::Idle | State::UpToDate | State::Failed(_) => {
            if ui.button("Procurar atualizações").clicked() {
                updater::check();
            }
        }
        State::Available(_) => {
            if ui.button("Baixar e verificar").clicked() {
                updater::download();
            }
        }
        State::Ready(..) => {
            let warning = if app.hosted.is_some() {
                "Instalar e reiniciar (encerra o seu host)"
            } else {
                "Instalar e reiniciar"
            };
            if ui.button(warning).clicked() {
                match updater::launch_installer() {
                    Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(error) => app.notify(error),
                }
            }
        }
        State::Checking | State::Downloading(_) => {}
    });
    if matches!(state, State::Checking | State::Downloading(_)) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
    ui.label(muted(
        "Cada atualização é assinada com a chave do projeto; arquivos sem assinatura válida, de versão antiga ou que não batem com o hash assinado são recusados.",
    ));
}

pub(super) fn host_identity_controls(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let host = app
        .client
        .as_ref()
        .map(|client| client.address.clone())
        .unwrap_or_else(|| crate::settings::host_address(&app.settings.last_address));
    if host.is_empty() {
        return;
    }
    let connected = app.client.as_ref().is_some_and(|client| client.is_alive());
    let pending = app
        .settings
        .identity_migrations
        .iter()
        .any(|receipt| receipt.host == host);
    ui.separator();
    ui.label(RichText::new(format!("Identidade para {host}")).strong());
    if connected {
        ui.add(egui::Label::new(RichText::new(&app.local_identity).monospace()).wrap());
        if pending {
            ui.label(RichText::new("Há uma rotação pendente. Você pode retomá-la ou reconectar para recuperar a confirmação.").color(theme::color(theme::YELLOW)));
        }
        ui.checkbox(
            &mut app.workspace_ui.rotation_confirm,
            "Confirmo trocar a chave somente neste host; precisarei exportar uma nova cópia",
        );
        if ui
            .add_enabled(
                app.workspace_ui.rotation_confirm,
                egui::Button::new(if pending {
                    "Retomar rotação"
                } else {
                    "Rotacionar identidade neste host"
                }),
            )
            .clicked()
        {
            app.begin_identity_rotation();
        }
    }
    ui.horizontal_wrapped(|ui|{
        if ui.add_enabled(!pending&&app.file_pending.is_none(),egui::Button::new("Exportar identidade deste host…")).clicked(){
            let (tx,rx)=crossbeam_channel::bounded(1);app.file_pending=Some(rx);let host=host.clone();
            std::thread::spawn(move||{
                let result=if let Some(path)=rfd::FileDialog::new().set_file_name("lanvox-host-identity.key").save_file(){
                    match crate::auth::local::export_host_identity(&host,&path){Ok(())=>crate::app::FileTask::Saved("Identidade do host exportada. Mantenha o arquivo privado.".into()),Err(error)=>crate::app::FileTask::Error(format!("Não foi possível exportar: {error}"))}
                }else{crate::app::FileTask::Cancelled};let _=tx.send(result);
            });
        }
        if ui.add_enabled(!connected&&!app.connected()&&!app.connecting&&app.hosted.is_none()&&app.file_pending.is_none(),egui::Button::new("Importar identidade deste host…")).clicked(){
            let (tx,rx)=crossbeam_channel::bounded(1);app.file_pending=Some(rx);let host=host.clone();
            std::thread::spawn(move||{
                let result=if let Some(path)=rfd::FileDialog::new().pick_file(){
                    match crate::auth::local::import_host_identity(&host,&path){Ok(())=>crate::app::FileTask::Saved("Identidade importada para este host. Os outros hosts foram preservados.".into()),Err(error)=>crate::app::FileTask::Error(format!("Não foi possível importar: {error}"))}
                }else{crate::app::FileTask::Cancelled};let _=tx.send(result);
            });
        }
    });
    ui.label(RichText::new("Rotação e cópia por host preservam sua identidade nas outras comunidades hospedadas em máquinas diferentes.").small().color(theme::color(theme::TEXT_MUTED)));
}

/// This computer's network address, found by asking the system which source it would use.
pub(super) fn local_addresses() -> Vec<String> {
    crate::notifications::network_addresses()
        .into_iter()
        .filter(|(_, ip)| ip.is_ipv4())
        .map(|(_, ip)| ip.to_string())
        .collect()
}

pub(super) fn streaming_tab(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    ui.checkbox(
        &mut app.settings.stream.motion_video,
        "Otimizar para movimento (H.264)",
    );
    ui.checkbox(
        &mut app.settings.stream.share_audio,
        "Compartilhar áudio do aplicativo/sistema (Windows)",
    );
    ui.checkbox(
        &mut app.settings.stream.adaptive,
        "Adaptar automaticamente resolução e FPS à rede",
    );
    ui.add(
        egui::Slider::new(&mut app.settings.stream.max_bitrate_kbps, 500..=20000)
            .suffix(" kbit/s")
            .text("Orçamento de vídeo"),
    );
    ui.label(RichText::new("Ao compartilhar uma janela, o áudio usa o processo escolhido; uma tela inteira usa a saída do sistema.").small());
    if let Some(share) = &app.share {
        let stats = share.stats.lock().clone();
        ui.label(format!("{} · encode {:.1}ms", stats.codec, stats.encode_ms));
        ui.label(format!(
            "Captura {:.1}fps · codificado {:.1}fps · enviado {:.1}fps · descartados {}",
            stats.capture_fps, stats.encoded_fps, stats.fps, stats.dropped_frames
        ));
        ui.label(if stats.audio_active {
            "Áudio compartilhado ativo"
        } else {
            "Sem áudio compartilhado ativo"
        });
        if let Some(error) = stats.audio_error {
            ui.label(RichText::new(error).color(theme::color(theme::YELLOW)));
        }
    }
    widgets::section_label(ui, "Resolução padrão");
    ui.horizontal(|ui| {
        for r in Resolution::ALL {
            if pill(ui, r.label(), app.settings.stream.resolution == r).clicked() {
                app.settings.stream.resolution = r;
            }
        }
    });
    ui.add_space(8.0);
    widgets::section_label(ui, "Taxa de quadros padrão");
    ui.horizontal(|ui| {
        for f in FRAME_RATES {
            if pill(ui, &format!("{f} fps"), app.settings.stream.fps == f).clicked() {
                app.settings.stream.fps = f;
            }
        }
    });
    ui.add_space(8.0);
    widgets::section_label(ui, "Qualidade máxima de imagem");
    let mut q = app.settings.stream.quality as f32;
    if ui
        .add(egui::Slider::new(
            &mut q,
            crate::media::codec::MIN_QUALITY as f32..=100.0,
        ))
        .changed()
    {
        app.settings.stream.quality = q as u8;
    }
    ui.label(
        RichText::new(
            "A qualidade é ajustada ao orçamento de vídeo. Use movimento para jogos ou texto nítido para conteúdo estático.",
        )
        .size(12.0)
        .color(theme::color(theme::TEXT_FAINT)),
    );
}
