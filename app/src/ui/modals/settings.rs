use super::*;

pub(super) fn settings(app: &mut LanvoxApp, ctx: &egui::Context, tab: SettingsTab) {
    let mut new_tab = tab;
    let result = components::modal(
        ctx,
        "settings",
        "Preferências",
        components::ModalOptions::new(860.0)
            .subtitle(
                "Os ajustes são aplicados ao alterar. Fechar ou Esc preserva suas alterações.",
            )
            .secondary("Fechar"),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for (target, label) in [
                    (SettingsTab::Account, "Minha conta"),
                    (SettingsTab::Appearance, "Aparência"),
                    (SettingsTab::Voice, "Voz e áudio"),
                    (SettingsTab::Video, "Câmera"),
                    (SettingsTab::Streaming, "Compartilhamento de tela"),
                ] {
                    if ui.selectable_label(tab == target, label).clicked() {
                        new_tab = target;
                    }
                }
            });
            ui.separator();
            ui.push_id(format!("settings-{tab:?}"), |ui| match tab {
                SettingsTab::Account => account_tab(app, ui),
                SettingsTab::Appearance => appearance_tab(app, ui),
                SettingsTab::Voice => voice_tab(app, ui),
                SettingsTab::Video => video_tab(app, ui),
                SettingsTab::Streaming => streaming_tab(app, ui),
            });
        },
    );
    if result.cancelled() || new_tab != tab {
        ctx.data_mut(|d| d.remove::<usize>(recording_id()));
        crate::notifications::set_hotkeys(app.settings.mute_hotkey, app.settings.deafen_hotkey);
        app.audio_shared.mic_test.store(false, Ordering::Relaxed);
        if app
            .view
            .as_ref()
            .and_then(|v| v.my_voice().channel)
            .is_none()
        {
            app.audio = None;
        }
        app.camera_preview = None;
        app.save_preferences();
        app.modal = if result.cancelled() {
            Modal::None
        } else {
            Modal::Settings(new_tab)
        };
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
    let connected = app.demo || app.client.as_ref().is_some_and(|c| c.is_alive());
    ui.horizontal(|ui| {
        if app.connected() && ui.button("Editar perfil e presença").clicked() {
            app.open_profile(app.me());
        }
        ui.checkbox(&mut app.settings.notifications, "Notificações do sistema");
    });
    ui.checkbox(
        &mut app.settings.do_not_disturb,
        "Pausar notificações neste dispositivo",
    )
    .on_hover_text(
        "Não altera sua presença nos servidores. Para mudar sua presença, abra seu perfil.",
    );
    ui.checkbox(
        &mut app.settings.notification_preview,
        "Mostrar conteúdo nas notificações",
    );
    if ui
        .checkbox(
            &mut app.settings.auto_reconnect,
            "Reconectar automaticamente",
        )
        .changed()
        && !app.settings.auto_reconnect
        && app.reconnect_at.is_some()
    {
        app.cancel_connection();
    }
    startup_preferences(app, ui);
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
    if app
        .view
        .as_ref()
        .is_some_and(|v| v.workspace.host_owner == Some(v.me))
    {
        crate::ui::workspace::host_admin(app, ui);
    }
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
    let avatar_texture = app.avatar_texture(ui.ctx(), app.me());
    Frame::new()
        .fill(theme::color(theme::RAIL))
        .corner_radius(8)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                super::super::avatar::Avatar::new(
                    &app.settings.name,
                    super::super::avatar::AvatarSize::Xl,
                )
                .color(app.settings.color)
                .texture(avatar_texture.as_ref())
                .presence(super::super::avatar::PresenceState::resolve(
                    connected,
                    app.view
                        .as_ref()
                        .and_then(|v| v.workspace.profiles.iter().find(|p| p.user == v.me))
                        .map(|p| p.presence),
                ))
                .show(ui);
                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(&app.settings.name)
                            .size(20.0)
                            .strong()
                            .color(theme::color(theme::TEXT_BRIGHT)),
                    );
                    ui.label(
                        RichText::new(if connected {
                            app.view
                                .as_ref()
                                .and_then(|v| v.workspace.profiles.iter().find(|p| p.user == v.me))
                                .map(|p| crate::ui::workspace::presence_label(p.presence))
                                .unwrap_or("Disponível")
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
    components::avatar_color_picker(ui, &mut app.settings.color);
    if let Some(h) = &app.hosted {
        ui.add_space(16.0);
        widgets::section_label(ui, "Seu host");
        let ips = local_addresses();
        ui.label(format!(
            "Servidor {} em execução na porta {}.",
            h.name, h.port
        ));
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

fn startup_preferences(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    use crate::settings::{StartupConnection, StartupDestination};
    ui.add_space(12.0);
    ui.label(RichText::new("Inicialização").strong());
    #[cfg(windows)]
    {
        let mut enabled = app.windows_startup.as_ref().copied().unwrap_or(false);
        if ui
            .add_enabled(
                !app.demo,
                egui::Checkbox::new(&mut enabled, "Iniciar com o Windows"),
            )
            .changed()
        {
            app.windows_startup = crate::startup::set_enabled(enabled)
                .and_then(|()| crate::startup::enabled())
                .map_err(|e| e.to_string());
        }
        if let Err(error) = &app.windows_startup {
            ui.colored_label(
                theme::color(theme::YELLOW),
                format!("Não foi possível configurar a inicialização: {error}"),
            );
            if ui.small_button("Verificar novamente").clicked() && !app.demo {
                app.windows_startup = crate::startup::enabled().map_err(|e| e.to_string());
            }
        }
    }
    let before = app.settings.startup_connection.clone();
    let mut enabled = before != StartupConnection::Disabled;
    if ui
        .checkbox(&mut enabled, "Conectar automaticamente ao abrir")
        .changed()
    {
        app.settings.startup_connection = if enabled {
            StartupConnection::LastConnected
        } else {
            StartupConnection::Disabled
        };
        if !enabled && app.startup_destination.is_some() {
            app.cancel_connection();
        }
    }
    if enabled {
        let mut selected = match &app.settings.startup_connection {
            StartupConnection::Community(destination) => {
                format!("Comunidade salva · {}", destination.host)
            }
            _ => "Última comunidade conectada".into(),
        };
        let mut destinations = Vec::new();
        if let Some(view) = &app.view {
            for guild in &view.guilds {
                destinations.push((
                    StartupDestination {
                        host: app.current_host(),
                        guild: Some(guild.id),
                    },
                    guild.name.clone(),
                ));
            }
        }
        for host in &app.background_hosts {
            for guild in &host.view.guilds {
                destinations.push((
                    StartupDestination {
                        host: host.address.clone(),
                        guild: Some(guild.id),
                    },
                    guild.name.clone(),
                ));
            }
        }
        if let StartupConnection::Community(chosen) = &app.settings.startup_connection {
            if let Some((_, name)) = destinations
                .iter()
                .find(|(destination, _)| destination == chosen)
            {
                selected = format!("{name} · {}", chosen.host);
            }
        }
        egui::ComboBox::from_id_salt("startup-community")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut app.settings.startup_connection,
                    StartupConnection::LastConnected,
                    "Última comunidade conectada",
                );
                for (destination, name) in destinations {
                    let label = format!("{name} · {}", destination.host);
                    ui.selectable_value(
                        &mut app.settings.startup_connection,
                        StartupConnection::Community(destination),
                        label,
                    );
                }
            });
        ui.label(RichText::new("Conecte-se a uma comunidade para escolhê-la aqui. Voz, microfone e câmera permanecem desligados ao abrir.").small().color(theme::color(theme::TEXT_MUTED)));
    }
    if app.settings.startup_connection != before {
        app.save_preferences();
    }
    ui.add_space(12.0);
}

/// Signed updates from GitHub Releases, with an opt-out for automatic updates.
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
            "Atualizar automaticamente ao abrir e verificar a cada 5 minutos (contata o GitHub)",
        )
        .changed()
    {
        app.save_preferences();
    }
    let state = updater::state();
    match &state {
        State::Idle => {}
        State::Checking => {
            ui.label(muted("Procurando…"));
        }
        State::Installing => {
            ui.label(muted("Instalando atualização…"));
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
        State::Checking | State::Downloading(_) | State::Installing => {}
    });
    if matches!(state, State::Checking | State::Downloading(_)) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
    ui.label(muted(
        "Com a atualização automática ativada, o app baixa a versão nova, instala e reabre quando não houver conexão ou host ativo. O GitHub vê seu endereço IP.",
    ));
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
    ui.label(RichText::new("Ao compartilhar uma janela, o áudio usa o processo escolhido; uma tela inteira usa a saída do sistema.").small());
    if let Some(share) = &app.share {
        let stats = share.stats.lock().clone();
        ui.label(format!("{} · encode {:.1}ms", stats.codec, stats.encode_ms));
        ui.label(format!(
            "Bitrate-alvo {:.2} Mb/s · mínimo {:.0} Mb/s",
            stats.target_bps as f64 / 1_000_000.0,
            stats.minimum_bps as f64 / 1_000_000.0
        ));
        if stats.host_budget_exceeded {
            ui.label(
                RichText::new("A soma dos bitrates mínimos excede o orçamento de vídeo do host.")
                    .color(theme::color(theme::YELLOW)),
            );
        }
        ui.label(format!(
            "Eventos WGC {}/s · captura {:.1}fps · map ocupado {}/s · codificado {:.1}fps · aceitos na fila {:.1}fps · descartados {}",
            stats.capture_events,
            stats.capture_fps,
            stats.capture_busy,
            stats.encoded_fps,
            stats.fps,
            stats.dropped_frames
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
