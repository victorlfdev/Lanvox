//! Independent host sessions. Only the selected host owns live microphone/video subscriptions.
use super::*;
use crate::protocol::ServerMsg;

pub struct BackgroundHost {
    pub address: String,
    pub identity: String,
    pub client: Option<Client>,
    pub view: ServerView,
    pub selected: Option<ChannelId>,
    pub guild: Option<GuildId>,
    pub home: bool,
    pub compose: HashMap<ChannelId, String>,
    pub workspace: WorkspaceUi,
    pub pictures: PictureStore,
    pub decoder: Decoder,
    pub mixer: Arc<Mixer>,
    pub deferred: std::collections::VecDeque<WorkspaceEvent>,
    pub deferred_bytes: usize,
    pub reconnect_at: Option<Instant>,
    pub reconnect_attempt: u8,
    pub reconnect_pending: Option<crossbeam_channel::Receiver<Result<Client, String>>>,
    pub reconnect_error: Option<String>,
}

pub struct HostSessionInfo {
    pub address: String,
    pub name: String,
    pub online: bool,
    pub active: bool,
    pub unread: u32,
    pub reconnecting: bool,
    pub connection_error: Option<String>,
}

impl LanvoxApp {
    pub fn host_sessions(&self) -> Vec<HostSessionInfo> {
        let mut sessions = Vec::new();
        if let Some(view) = &self.view {
            sessions.push(HostSessionInfo {
                address: self.current_host(),
                name: view.name.clone(),
                online: self.client.as_ref().is_some_and(|c| c.is_alive()),
                active: true,
                unread: view.unread.values().copied().fold(0, u32::saturating_add),
                reconnecting: self.connecting || self.reconnect_at.is_some(),
                connection_error: self.join_error.clone(),
            });
        }
        sessions.extend(self.background_hosts.iter().map(|host| {
            HostSessionInfo {
                address: host.address.clone(),
                name: host.view.name.clone(),
                online: host.client.as_ref().is_some_and(|c| c.is_alive()),
                active: false,
                unread: host
                    .view
                    .unread
                    .values()
                    .copied()
                    .fold(0, u32::saturating_add),
                reconnecting: self.settings.auto_reconnect
                    && (host.reconnect_pending.is_some() || host.reconnect_at.is_some()),
                connection_error: host.reconnect_error.clone(),
            }
        }));
        sessions
    }

    pub(super) fn park_current_host(&mut self, ctx: &egui::Context) {
        if self.view.is_none() {
            return;
        }
        let address = self.current_host();
        self.save_drafts();
        self.leave_voice();
        let Some(mut view) = self.view.take() else {
            return;
        };
        view.voice.remove(&view.me);
        let pictures = std::mem::take(&mut self.pictures);
        let decoder = std::mem::replace(
            &mut self.decoder,
            Decoder::start(self.pictures.clone(), ctx.clone()),
        );
        let mixer = std::mem::replace(&mut self.mixer, Arc::new(Mixer::new()));
        self.mixer.set_output_volume(self.settings.output_volume);
        self.background_hosts.retain(|host| host.address != address);
        self.background_hosts.push(BackgroundHost {
            address,
            identity: self.local_identity.clone(),
            client: self.client.take(),
            view,
            selected: self.selected.take(),
            guild: self.guild.take(),
            home: self.home,
            compose: std::mem::take(&mut self.compose),
            workspace: std::mem::take(&mut self.workspace_ui),
            pictures,
            decoder,
            mixer,
            deferred: Default::default(),
            deferred_bytes: 0,
            reconnect_at: None,
            reconnect_attempt: 0,
            reconnect_pending: None,
            reconnect_error: None,
        });
        *self.audio_shared.sender.lock() = None;
        self.attachment_textures.clear();
        self.avatar_textures.clear();
        self.videos = Default::default();
        self.pictures.clear();
        self.textures.clear();
        self.cursor_textures.clear();
        self.search_input.clear();
        self.dismissed_calls.clear();
        self.home = false;
        self.quick = Default::default();
    }

    /// Restores the exact conversation/drafts/UI state. Returns true for an already live connection.
    pub(super) fn activate_host(&mut self, address: &str, ctx: &egui::Context) -> bool {
        let Some(index) = self
            .background_hosts
            .iter()
            .position(|host| host.address == address)
        else {
            return false;
        };
        let mut target = self.background_hosts.remove(index);
        self.park_current_host(ctx);
        if target.client.as_ref().is_some_and(|c| !c.is_alive()) {
            target.client = None;
        }
        self.local_identity = target.identity;
        self.reconnect_address = Some(target.address.clone());
        self.settings.last_address = target.address;
        self.selected = target.selected;
        self.guild = target.guild;
        self.home = target.home;
        self.compose = target.compose;
        self.workspace_ui = target.workspace;
        self.view = Some(target.view);
        self.client = target.client;
        self.pictures = target.pictures;
        self.decoder = target.decoder;
        self.mixer = target.mixer;
        self.mixer.set_output_volume(self.settings.output_volume);
        *self.audio_shared.sender.lock() = self.client.as_ref().map(|client| client.sender());
        self.connecting = false;
        self.reconnect_at = None;
        self.connection_status.clear();
        self.settings.save();
        for event in target.deferred {
            self.on_workspace_event(event);
        }
        self.client.is_some()
    }

    pub fn switch_host(&mut self, ctx: &egui::Context, address: String) {
        if self.file_pending.is_some() {
            self.notify("Conclua o seletor de arquivos antes de trocar de host.");
            return;
        }
        let address = crate::settings::host_address(&address);
        if self.current_host() == address && self.client.as_ref().is_some_and(|c| c.is_alive()) {
            return;
        }
        if self.connection_pending.is_some() {
            self.cancel_connection();
        }
        if !self.activate_host(&address, ctx) {
            self.load_host_preferences(&address);
            self.connect(ctx, address);
        }
    }

    pub fn close_host_session(&mut self, address: &str) {
        let address = crate::settings::host_address(address);
        if self.current_host() == address && self.view.is_some() {
            self.disconnect();
        }
        self.background_hosts.retain(|host| host.address != address);
    }

    pub fn poll_background_hosts(&mut self, ctx: &egui::Context) {
        for host in &mut self.background_hosts {
            let completed = host
                .reconnect_pending
                .as_ref()
                .and_then(|rx| match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        Some(Err("A conexão em segundo plano foi encerrada.".into()))
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => None,
                });
            if let Some(result) = completed {
                host.reconnect_pending = None;
                match result {
                    Ok(client)
                        if crate::settings::key_fingerprint(client.public_key) == host.identity =>
                    {
                        host.reconnect_attempt = 0;
                        host.reconnect_at = None;
                        host.reconnect_error = None;
                        client.send(crate::protocol::ClientMsg::Workspace(
                            WorkspaceCommand::Refresh,
                        ));
                        host.client = Some(client);
                    }
                    Ok(_) => {
                        host.reconnect_at = None;
                        host.reconnect_error = Some(
                            "A identidade local mudou; selecione o host para verificar a conexão."
                                .into(),
                        );
                    }
                    Err(error) => {
                        host.reconnect_error = Some(error);
                        host.reconnect_attempt = host.reconnect_attempt.saturating_add(1);
                        let seconds = (1u64 << host.reconnect_attempt.min(5)).min(30);
                        host.reconnect_at = Some(Instant::now() + Duration::from_secs(seconds));
                    }
                }
            }
            if self.settings.auto_reconnect
                && host.client.is_none()
                && host.reconnect_pending.is_none()
                && host.reconnect_error.as_deref()
                    != Some("A identidade local mudou; selecione o host para verificar a conexão.")
                && host.reconnect_at.is_none_or(|at| Instant::now() >= at)
            {
                let address = host.address.clone();
                let name = self.settings.name.clone();
                let color = self.settings.color;
                let credentials = self
                    .settings
                    .hosts
                    .get(&address)
                    .cloned()
                    .unwrap_or_default();
                let options = crate::net::client::ConnectOptions {
                    invite: credentials.access_token,
                    expected_fingerprint: (!credentials.expected_fingerprint.trim().is_empty())
                        .then_some(credentials.expected_fingerprint),
                };
                let mixer = host.mixer.clone();
                let decoder = host.decoder.clone();
                let repaint = ctx.clone();
                let (tx, rx) = crossbeam_channel::bounded(1);
                match std::thread::Builder::new()
                    .name("reconnect-background-host".into())
                    .spawn(move || {
                        let result = Client::connect_with_options(
                            &address,
                            &name,
                            color,
                            repaint.clone(),
                            mixer,
                            decoder,
                            options,
                        )
                        .map_err(|e| format!("Não foi possível reconectar: {e}"));
                        let _ = tx.send(result);
                        repaint.request_repaint();
                    }) {
                    Ok(_) => {
                        host.reconnect_pending = Some(rx);
                        host.reconnect_at = None;
                    }
                    Err(error) => {
                        host.reconnect_error =
                            Some(format!("Não foi possível iniciar a reconexão: {error}"));
                        host.reconnect_attempt = host.reconnect_attempt.saturating_add(1);
                        host.reconnect_at = Some(
                            Instant::now()
                                + Duration::from_secs(
                                    (1u64 << host.reconnect_attempt.min(5)).min(30),
                                ),
                        );
                    }
                }
            }
            let events: Vec<_> = host
                .client
                .as_ref()
                .map(|client| client.events.try_iter().take(256).collect())
                .unwrap_or_default();
            for event in events {
                match event {
                    NetEvent::Server(ServerMsg::Workspace(WorkspaceEvent::Resync { snapshot })) => {
                        host.view = ServerView::from_snapshot(host.view.me, snapshot);
                    }
                    NetEvent::Server(ServerMsg::Workspace(event)) => {
                        host.view.apply_workspace(&event);
                        // State broadcasts are applied immediately. Replies with UI/operation
                        // effects belong to this host and are replayed when it is selected.
                        if !matches!(
                            &event,
                            WorkspaceEvent::State(_)
                                | WorkspaceEvent::Read { .. }
                                | WorkspaceEvent::MessageChanged { .. }
                                | WorkspaceEvent::MessageDeleted { .. }
                                | WorkspaceEvent::CategoriesChanged { .. }
                                | WorkspaceEvent::Community(
                                    CommunityEvent::Unread(_)
                                        | CommunityEvent::MarkedUnread { .. }
                                        | CommunityEvent::PollUpdated(_)
                                )
                        ) {
                            let bytes = match &event {
                                WorkspaceEvent::Attachment { bytes, .. } => bytes.len(),
                                _ => 0,
                            };
                            if host.deferred.len() >= 32
                                || host.deferred_bytes.saturating_add(bytes) > 24 * 1024 * 1024
                            {
                                host.deferred.clear();
                                host.deferred_bytes = 0;
                                host.deferred.push_back(WorkspaceEvent::Error { operation: None, reason: "Algumas consultas deste host expiraram em segundo plano. Repita a busca ou o download.".into() });
                            } else {
                                host.deferred_bytes += bytes;
                                host.deferred.push_back(event);
                            }
                        }
                    }
                    NetEvent::Server(message) => {
                        if let ServerMsg::Message(message) = &message {
                            let scope = format!("{}|{}", host.identity, host.address);
                            let blocked = self
                                .settings
                                .blocked_users
                                .get(&scope)
                                .is_some_and(|ids| ids.contains(&message.author));
                            let guild_preference = host
                                .view
                                .channel(message.channel)
                                .and_then(|channel| {
                                    self.settings
                                        .guild_notifications
                                        .get(&format!("{scope}|{}", channel.guild))
                                })
                                .copied()
                                .unwrap_or_default();
                            let preference = self
                                .settings
                                .channel_notifications
                                .get(&format!("{scope}|{}", message.channel))
                                .copied()
                                .unwrap_or(guild_preference);
                            let mentioned = message.text.contains(&format!("<@{}>", host.view.me));
                            let duplicate =
                                host.view
                                    .messages
                                    .get(&message.channel)
                                    .is_some_and(|messages| {
                                        messages.iter().any(|m| m.id == message.id)
                                    });
                            if !duplicate
                                && !blocked
                                && message.author != host.view.me
                                && self.settings.notifications
                                && !self.settings.do_not_disturb
                                && guild_preference.muted_until <= now_secs()
                                && preference.allows(mentioned, now_secs())
                            {
                                let body = if self.settings.notification_preview {
                                    message.text.chars().take(220).collect::<String>()
                                } else {
                                    "Você recebeu uma nova mensagem.".into()
                                };
                                crate::notifications::notify_message(
                                    &format!("{} · {}", host.view.name, message.author_name),
                                    &body,
                                    &host.address,
                                    message.channel,
                                    message.id,
                                );
                            }
                        }
                        host.view.apply(message, None);
                    }
                    NetEvent::Welcome { you, snapshot } => {
                        host.view = ServerView::from_snapshot(you, snapshot);
                        if let Some(client) = &host.client {
                            client.send(crate::protocol::ClientMsg::Workspace(
                                WorkspaceCommand::Refresh,
                            ));
                        }
                    }
                    NetEvent::Latency(ms) => host.view.ping_ms = Some(ms),
                    NetEvent::Closed(_) => {
                        host.client = None;
                        host.view.voice.clear();
                        host.view.streams.clear();
                        host.view.speaking.clear();
                        host.view.ping_ms = None;
                        host.reconnect_at = Some(Instant::now() + Duration::from_secs(2));
                        host.reconnect_error = None;
                    }
                }
            }
        }
    }
}
