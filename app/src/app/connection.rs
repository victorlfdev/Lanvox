use super::*;

pub struct ConnectionFailure {
    pub message: String,
    pub retryable: bool,
}

impl ConnectionFailure {
    fn from_io(address: &str, error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        let retryable = matches!(
            error.kind(),
            ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::NotConnected
                | ErrorKind::TimedOut
                | ErrorKind::WouldBlock
                | ErrorKind::Interrupted
                | ErrorKind::AddrNotAvailable
                | ErrorKind::NetworkUnreachable
                | ErrorKind::HostUnreachable
                | ErrorKind::NotFound
        );
        Self {
            message: format!("Não foi possível entrar em {address}: {error}"),
            retryable,
        }
    }
}

impl LanvoxApp {
    pub fn connect(&mut self, ctx: &egui::Context, address: String) -> bool {
        self.connect_using(ctx, address, None)
    }

    pub fn connect_from_form(&mut self, ctx: &egui::Context, address: String) -> bool {
        self.cancel_connection();
        let supplied = crate::settings::HostCredentials {
            access_token: self.settings.access_token.clone(),
            expected_fingerprint: self.settings.expected_host.clone(),
        };
        self.connect_using(ctx, address, Some(supplied))
    }

    fn connect_using(
        &mut self,
        ctx: &egui::Context,
        address: String,
        mut supplied: Option<crate::settings::HostCredentials>,
    ) -> bool {
        if self.connection_pending.is_some() {
            return false;
        }
        if self.file_pending.is_some() {
            self.join_error =
                Some("Conclua ou cancele o seletor de arquivo antes de trocar de conexão.".into());
            return false;
        }
        let address = if crate::branding::is_link(&address) {
            let link = match crate::invites::parse(&address) {
                Ok(link) => link,
                Err(error) => {
                    self.join_error = Some(error);
                    return false;
                }
            };
            let mut credentials = self.settings.credentials_for(&link.address);
            if let Some(token) = link.token {
                credentials.access_token = token;
            }
            if let Some(fingerprint) = link.fingerprint {
                credentials.expected_fingerprint = fingerprint;
            }
            supplied = Some(credentials);
            if let Some((channel, message)) = link.message {
                self.pending_message = Some((
                    crate::settings::host_address(&link.address),
                    channel,
                    message,
                ));
            }
            link.address
        } else {
            address
        };
        let address = crate::settings::host_address(&address);
        if self
            .startup_destination
            .as_ref()
            .is_some_and(|destination| destination.host != address)
        {
            self.startup_destination = None;
        }
        if self.client.as_ref().is_some_and(|client| {
            client.is_alive() && crate::settings::host_address(&client.address) == address
        }) {
            return true;
        }
        if self.activate_host(&address, ctx) {
            return true;
        }
        if self.view.is_some()
            && self.current_host() != address
            && self.background_hosts.len() >= 15
        {
            self.join_error = Some("Feche uma sessão antes de abrir mais de 16 hosts.".into());
            return false;
        }
        if self.view.is_some() && self.current_host() != address {
            self.park_current_host(ctx);
        }
        if let Err(error) = self.stage_connection_migration(&address) {
            self.join_error = Some(format!(
                "Não foi possível preservar a recuperação da identidade: {error}"
            ));
            return false;
        }
        let credentials = supplied.unwrap_or_else(|| self.settings.credentials_for(&address));
        self.settings.access_token = credentials.access_token;
        self.settings.expected_host = credentials.expected_fingerprint;
        self.connecting = true;
        self.join_error = None;
        self.reconnect_at = None;
        self.connection_status = "Conectando e autenticando…".into();
        self.reconnect_address = Some(address.clone());
        let name = self.settings.name.clone();
        let color = self.settings.color;
        let local_host = self
            .hosted
            .as_ref()
            .filter(|host| address == format!("{}:{}", self.settings.host_bind, host.port));
        let options = crate::net::client::ConnectOptions {
            invite: local_host
                .and_then(|host| host.registration_token.clone())
                .unwrap_or_else(|| self.settings.access_token.clone()),
            expected_fingerprint: local_host.map(|host| host.fingerprint.clone()).or_else(|| {
                if self.settings.expected_host.trim().is_empty() {
                    None
                } else {
                    Some(self.settings.expected_host.trim().to_owned())
                }
            }),
        };
        let mixer = self.mixer.clone();
        let decoder = self.decoder.clone();
        let ctx = ctx.clone();
        let (tx, rx) = bounded(1);
        let spawned = std::thread::Builder::new()
            .name("connect-host".into())
            .spawn(move || {
                let result = Client::connect_with_options(
                    &address,
                    &name,
                    color,
                    ctx.clone(),
                    mixer,
                    decoder,
                    options,
                )
                .map_err(|e| ConnectionFailure::from_io(&address, e));
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        if let Err(error) = spawned {
            self.connecting = false;
            self.join_error = Some(format!("Não foi possível iniciar a conexão: {error}"));
            return false;
        }
        self.connection_pending = Some(rx);
        true
    }

    pub fn cancel_connection(&mut self) {
        self.connection_pending = None;
        self.connecting = false;
        self.reconnect_at = None;
        self.reconnect_address = None;
        self.connection_status.clear();
        self.startup_destination = None;
        if self.new_host_pending {
            self.hosted = None;
            self.new_host_pending = false;
        }
    }

    pub(super) fn poll_connection(&mut self, ctx: &egui::Context) {
        let result =
            self.connection_pending
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        Some(Err(ConnectionFailure {
                            message: "A tarefa de conexão foi encerrada. Tente novamente.".into(),
                            retryable: false,
                        }))
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => None,
                });
        if let Some(result) = result {
            self.connection_pending = None;
            match result {
                Ok(client) => {
                    self.local_identity = crate::settings::key_fingerprint(client.public_key);
                    self.reconnect_address = Some(client.address.clone());
                    self.settings.last_address = client.address.clone();
                    self.settings.hosts.insert(
                        self.settings.last_address.clone(),
                        crate::settings::HostCredentials {
                            access_token: self.settings.access_token.clone(),
                            expected_fingerprint: client.host_fingerprint.clone(),
                        },
                    );
                    let favorite = crate::settings::host_address(&client.address);
                    if !favorite.is_empty() && !self.settings.favorites.contains(&favorite) {
                        self.settings.favorites.push(favorite);
                    }
                    self.save_preferences();
                    *self.audio_shared.sender.lock() = Some(client.sender());
                    self.client = Some(client);
                    self.host_error = None;
                    self.local_host_busy = false;
                    self.new_host_pending = false;
                    self.connection_status = "Sincronizando conversas…".into();
                }
                Err(error) => {
                    self.connecting = false;
                    self.join_error = Some(error.message.clone());
                    if self.new_host_pending {
                        self.hosted = None;
                        self.new_host_pending = false;
                        self.host_error = Some(error.message);
                    }
                    if error.retryable
                        && (self.view.is_some() || self.startup_destination.is_some())
                        && self.settings.auto_reconnect
                    {
                        self.schedule_reconnect();
                    } else {
                        self.startup_destination = None;
                        self.connection_status = "Falha na conexão".into();
                    }
                }
            }
        }
        if self.file_pending.is_none() && self.reconnect_at.is_some_and(|at| Instant::now() >= at) {
            if let Some(address) = self.reconnect_address.clone() {
                self.connect(ctx, address);
            }
        }
        if self.connecting || self.reconnect_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    pub(super) fn schedule_reconnect(&mut self) {
        self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
        let seconds = (1u64 << self.reconnect_attempt.min(5)).min(30);
        self.reconnect_at = Some(Instant::now() + Duration::from_secs(seconds));
        self.connection_status = format!(
            "Conexão interrompida. Tentativa {} em {seconds}s…",
            self.reconnect_attempt
        );
    }

    pub(super) fn connection_lost(&mut self, reason: String) {
        self.preserve_voice_note();
        self.stop_media();
        *self.audio_shared.sender.lock() = None;
        self.client = None;
        self.connecting = false;
        if let Some(view) = &mut self.view {
            view.voice.clear();
            view.streams.clear();
            view.speaking.clear();
            view.ping_ms = None;
        }
        self.notify(format!("Conexão interrompida: {reason}"));
        if self.settings.auto_reconnect && self.reconnect_address.is_some() {
            self.schedule_reconnect();
        } else {
            self.connection_status =
                "Desconectado. Suas conversas e rascunhos foram preservados.".into();
        }
    }

    pub fn disconnect(&mut self) {
        self.remember_destination();
        self.preserve_voice_note();
        self.save_drafts();
        self.cancel_connection();
        self.leave_voice();
        *self.audio_shared.sender.lock() = None;
        self.client = None;
        self.view = None;
        self.selected = None;
        self.guild = None;
        self.home = false;
        self.dismissed_calls.clear();
        self.group_pick.clear();
        self.connecting = false;
        self.compose.clear();
        self.workspace_ui = WorkspaceUi::default();
        self.file_pending = None;
        self.attachment_textures.clear();
        self.videos = Default::default();
        self.avatar_textures.clear();
        self.search_input.clear();
        self.pictures.clear();
        self.textures.clear();
        self.cursor_textures.clear();
        self.watching.clear();
        self.unwatched.clear();
        self.focused = None;
    }

    pub fn poll_network(&mut self) {
        let Some(client) = &self.client else { return };
        let events: Vec<NetEvent> = client.events.try_iter().collect();
        for ev in events {
            match ev {
                NetEvent::Welcome { you, snapshot } => {
                    self.connecting = false;
                    self.reconnect_attempt = 0;
                    self.connection_status.clear();
                    let restore_channel =
                        self.startup_destination.is_some() || self.selected.is_none();
                    let view = ServerView::from_snapshot(you, snapshot);
                    if let Some(destination) = self.startup_destination.take() {
                        self.guild = destination
                            .guild
                            .filter(|id| view.guilds.iter().any(|g| g.id == *id));
                    }
                    if self.guild.is_none() {
                        self.guild = view.guilds.first().map(|g| g.id);
                    }
                    if self.selected.is_none() && !self.home {
                        self.selected = view
                            .channels
                            .iter()
                            .find(|c| c.kind == ChannelKind::Text && Some(c.guild) == self.guild)
                            .map(|c| c.id);
                    }
                    self.view = Some(view);
                    self.recover_identity_namespace(you);
                    if restore_channel && !self.home {
                        if let Some(guild) = self.guild {
                            self.selected =
                                crate::ui::quick_switcher::channel_for_guild(self, guild);
                        }
                    }
                    self.settings.last_destination = Some(crate::settings::StartupDestination {
                        host: self.current_host(),
                        guild: self.guild,
                    });
                    self.save_preferences();
                    self.restore_drafts();
                    self.scroll_to_bottom = 3;
                    self.replay_pending();
                    self.workspace(WorkspaceCommand::Refresh);
                }
                NetEvent::Latency(ms) => {
                    if let Some(v) = &mut self.view {
                        v.ping_ms = Some(ms);
                    }
                }
                NetEvent::Server(msg) => self.on_server_msg(msg),
                NetEvent::Closed(reason) => {
                    self.connection_lost(reason);
                    return;
                }
            }
        }
    }

    pub fn host(&mut self, ctx: &egui::Context) {
        if self.file_pending.is_some() {
            self.notify("Conclua ou cancele o seletor de arquivo antes de iniciar outro host.");
            return;
        }
        self.host_error = None;
        self.join_error = None;
        self.local_host_busy = false;
        if let Some(host) = &self.hosted {
            self.connect(ctx, format!("{}:{}", self.settings.host_bind, host.port));
            return;
        }
        let name = self.host_input.trim().to_string();
        let name = if name.is_empty() {
            "Minha comunidade".to_string()
        } else {
            name
        };
        self.settings.host_name = name.clone();
        self.save_preferences();
        // The owner authenticates with the key this device uses for this exact host address,
        // which is the rotated key after a rotation (not the stale base identity).
        let owner_address = crate::settings::host_address(&format!(
            "{}:{}",
            self.settings.host_bind,
            crate::protocol::DEFAULT_PORT
        ));
        let identity = match crate::auth::local::host_identity(&owner_address) {
            Ok(identity) => identity,
            Err(error) => {
                self.host_error = Some(format!("Não foi possível abrir sua identidade: {error}"));
                return;
            }
        };
        match server::start_desktop_configured(
            name,
            crate::settings::server_save_path(),
            &self.settings.name,
            identity.public_key(),
            server::ServerOptions {
                bind: self.settings.host_bind.clone(),
                ..Default::default()
            },
        ) {
            Ok(h) => {
                let port = h.port;
                self.hosted = Some(h);
                self.new_host_pending = true;
                if !self.connect(ctx, format!("{}:{port}", self.settings.host_bind)) {
                    // A failed creation must not leave a hidden listener occupying the port.
                    self.hosted = None;
                    self.host_error = self.join_error.take();
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                self.local_host_busy = true;
                self.host_error = Some("Outro servidor já usa esta porta. Entre no host existente ou encerre-o antes de criar outro.".into());
            }
            Err(error) => {
                self.host_error = Some(format!("Não foi possível iniciar o host: {error}"))
            }
        }
    }

    /// DNS, sockets and authentication never block egui's frame thread.
    pub fn load_host_preferences(&mut self, address: &str) {
        let preferences = self
            .settings
            .hosts
            .get(&crate::settings::host_address(address))
            .cloned()
            .unwrap_or_default();
        self.settings.access_token = preferences.access_token;
        self.settings.expected_host = preferences.expected_fingerprint;
    }

    pub fn stop_host(&mut self) {
        self.disconnect();
        self.hosted = None;
        self.notify("Servidor local encerrado.");
    }

    pub fn save_drafts(&mut self) {
        let host = self
            .reconnect_address
            .as_deref()
            .unwrap_or(&self.settings.last_address);
        for (channel, text) in &self.compose {
            let key = format!("{}|{host}|{channel}", self.local_identity);
            if text.is_empty() {
                self.settings.drafts.remove(&key);
            } else {
                self.settings.drafts.insert(key, text.clone());
            }
        }
        self.save_preferences();
        self.draft_saved_at = Instant::now();
    }

    pub fn restore_drafts(&mut self) {
        let host = self
            .reconnect_address
            .as_deref()
            .unwrap_or(&self.settings.last_address);
        let prefix = format!("{}|{host}|", self.local_identity);
        for (key, text) in &self.settings.drafts {
            if let Some(channel) = key
                .strip_prefix(&prefix)
                .and_then(|channel| channel.parse().ok())
            {
                self.compose.entry(channel).or_insert_with(|| text.clone());
            }
        }
        self.restore_attachment_drafts();
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn startup_retries_network_failures_without_an_existing_session_and_can_be_cancelled() {
        let (mut app, ctx) = crate::app::ui_regressions::fixture();
        app.view = None;
        app.startup_destination = Some(crate::settings::StartupDestination {
            host: "offline:47800".into(),
            guild: Some(7),
        });
        app.reconnect_address = Some("offline:47800".into());
        let (tx, rx) = bounded(1);
        app.connection_pending = Some(rx);
        tx.send(Err(ConnectionFailure::from_io(
            "offline",
            std::io::ErrorKind::ConnectionRefused.into(),
        )))
        .ok();
        app.poll_connection(&ctx);
        assert!(app.reconnect_at.is_some());
        app.cancel_connection();
        assert!(app.reconnect_at.is_none());
        assert!(app.startup_destination.is_none());
        assert!(app.reconnect_address.is_none());
    }

    #[test]
    fn startup_stops_on_authentication_errors_or_when_reconnection_is_disabled() {
        for (kind, reconnect) in [
            (std::io::ErrorKind::PermissionDenied, true),
            (std::io::ErrorKind::InvalidData, true),
            (std::io::ErrorKind::TimedOut, false),
        ] {
            let (mut app, ctx) = crate::app::ui_regressions::fixture();
            app.view = None;
            app.settings.auto_reconnect = reconnect;
            app.startup_destination = Some(crate::settings::StartupDestination {
                host: "host:47800".into(),
                guild: None,
            });
            let (tx, rx) = bounded(1);
            app.connection_pending = Some(rx);
            tx.send(Err(ConnectionFailure::from_io("host", kind.into())))
                .ok();
            app.poll_connection(&ctx);
            assert!(app.reconnect_at.is_none());
            assert!(app.startup_destination.is_none());
            assert!(app.join_error.is_some());
        }
    }
}
