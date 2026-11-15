use super::*;

impl LanvoxApp {
    pub(super) fn on_server_msg(&mut self, msg: crate::protocol::ServerMsg) {
        use crate::protocol::ServerMsg;
        if let ServerMsg::Workspace(event) = msg {
            self.on_workspace_event(event);
            return;
        }
        let me = self.me();
        let selected_kind = self
            .selected
            .and_then(|id| self.view.as_ref()?.channel(id).map(|c| c.kind));
        let viewing = if self.chat_at_bottom
            && self.notification_focused
            && crate::ui::call::chat_is_visible(selected_kind, self.call_chat_open)
        {
            self.selected
        } else {
            None
        };
        // Stream bookkeeping before the view changes.
        if let ServerMsg::StreamStopped { owner, kind } = &msg {
            let key = (*owner, *kind);
            self.watching.remove(&key);
            self.unwatched.remove(&key);
            if self.focused == Some(key) {
                self.focused = None;
                self.fullscreen = false;
            }
            if self.popout == Some(key) {
                self.popout = None;
            }
            if *owner != me {
                self.pictures.remove(key);
                self.textures.remove(&key);
            }
        }
        if let ServerMsg::Message(m) = &msg {
            let should_notify = self.should_notify_message(m);
            if m.author != me
                && !self.notification_focused
                && self.settings.notifications
                && !self.settings.do_not_disturb
                && should_notify
            {
                let body = if self.settings.notification_preview {
                    crate::mentions::display(&m.text, |id| {
                        self.view.as_ref().map(|v| v.user_name(id))
                    })
                    .chars()
                    .take(220)
                    .collect::<String>()
                } else {
                    "Você recebeu uma nova mensagem.".into()
                };
                let host = self
                    .client
                    .as_ref()
                    .map(|c| c.address.as_str())
                    .unwrap_or_default();
                crate::notifications::notify_message(
                    &format!("Lanvox · {}", m.author_name),
                    &body,
                    host,
                    m.channel,
                    m.id,
                );
            }
            if m.author != me && Some(m.channel) != viewing && should_notify {
                crate::sfx::play(crate::sfx::Sfx::Message, &self.settings);
            }
            if Some(m.channel) == viewing && self.chat_at_bottom {
                self.scroll_to_bottom = 3;
            }
        }
        // A call I just started: join it right away.
        let mut start_call = None;
        if let ServerMsg::CallUpdated(info) = &msg {
            if info.host != me
                && self.workspace_ui.sound_call != Some(info.id)
                && self.settings.notifications
                && !self.settings.do_not_disturb
            {
                self.workspace_ui.sound_call = Some(info.id);
                crate::notifications::notify(
                    "Chamada no Lanvox",
                    "Um amigo está chamando. Abra o Lanvox para atender.",
                );
            }
            let known = self
                .view
                .as_ref()
                .map(|v| v.calls.contains_key(&info.id))
                .unwrap_or(true);
            if !known && info.host == me {
                start_call = Some(info.id);
            }
        }
        let Some(view) = &mut self.view else { return };
        let notice = view.apply(msg, viewing);
        let mine = view.my_voice().channel;
        if let Some(notice) = notice {
            self.notify(notice);
        }
        // A join we just sent is not yet reflected in the view. Until the server
        // confirms *that* channel, neither the old state (still in the previous
        // channel) nor the transient "left" broadcast the server sends before the
        // new join may tear the audio down.
        let out_of_call = Self::resolve_out_of_call(&mut self.voice_join, mine, Instant::now());
        // Our own call ended on the server side.
        if self.audio.is_some()
            && out_of_call
            && !self.audio_shared.mic_test.load(Ordering::Relaxed)
        {
            self.stop_media();
        }
        if let Some(id) = start_call {
            self.join_call(id);
        }
    }

    // ----- text -----

    pub fn send_message(&mut self, channel: ChannelId) {
        let text = self
            .compose
            .get(&channel)
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_owned();
        // Bind `@name` to identities before sending (`<@id>`), longest name first.
        let text = match &self.view {
            Some(v) if !self.demo => crate::mentions::encode(&text, &v.channel_members(channel)),
            _ => text,
        };
        let attachments: Vec<u64> = self
            .workspace_ui
            .attachments
            .get(&channel)
            .map(|files| files.iter().map(|f| f.id).collect())
            .unwrap_or_default();
        if text.is_empty() && attachments.is_empty() {
            return;
        }
        if self.demo {
            if let Some(v) = &mut self.view {
                let me = v.me;
                let (name, color) = (v.user_name(me), v.user_color(me));
                let list = v.messages.entry(channel).or_default();
                let id = list.last().map(|m| m.id + 1).unwrap_or(1);
                list.push(Message {
                    id,
                    channel,
                    author: me,
                    author_name: name,
                    author_color: color,
                    time: now_secs(),
                    text,
                });
            }
        } else {
            let pending = crate::settings::PendingMessage {
                operation: format!("{:032x}", rand::random::<u128>()),
                host: self.current_host(),
                identity: self.local_identity.clone(),
                channel,
                text,
                reply_to: self.workspace_ui.reply_to.take(),
                attachments,
                error: None,
                attempted_at: now_secs(),
            };
            self.settings.pending.push(pending.clone());
            if let Err(error) = self.settings.try_save() {
                self.settings.pending.pop();
                self.notify(format!(
                    "Não foi possível preservar seu envio: {error}. O texto continua no rascunho."
                ));
                return;
            }
            self.send_pending(&pending);
        }
        self.compose.insert(channel, String::new());
        self.workspace_ui.attachments.remove(&channel);
        self.save_drafts();
        self.scroll_to_bottom = 3;
    }

    pub fn typed(&mut self, channel: ChannelId) {
        if self.last_typing_sent.elapsed() > Duration::from_secs(4) {
            self.last_typing_sent = Instant::now();
            self.send(ClientMsg::Typing { channel });
        }
    }

    // ----- voice -----

    pub fn create_channel(
        &mut self,
        name: String,
        kind: ChannelKind,
        category: Option<CategoryId>,
    ) {
        let guild = self.guild.unwrap_or(1);
        self.category_command(WorkspaceCommand::CreateChannelIn {
            guild,
            name,
            kind,
            category,
        });
    }

    /// Category and channel-placement commands. The host applies and broadcasts them; the demo
    /// (no host) applies them to its local copy with the same pure logic.
    pub fn category_command(&mut self, command: WorkspaceCommand) {
        if self.demo {
            let guild = self.guild.unwrap_or(1);
            if let Some(view) = &mut self.view {
                view.apply_category_command_locally(guild, &command);
            }
        } else {
            self.workspace(command);
        }
    }
}
