use super::*;

impl LanvoxApp {
    pub fn current_host(&self) -> String {
        self.reconnect_address
            .clone()
            .unwrap_or_else(|| self.settings.last_address.clone())
    }

    pub fn workspace(&self, command: WorkspaceCommand) {
        self.send(ClientMsg::Workspace(command));
    }
    pub fn workspace_query(&mut self, command: WorkspaceCommand) {
        let scope = match &command {
            WorkspaceCommand::SearchAdvanced(_) => "search".to_owned(),
            WorkspaceCommand::Inbox { .. } => "inbox".to_owned(),
            WorkspaceCommand::Pinned { channel } => format!("pins:{channel}"),
            WorkspaceCommand::History { channel, .. }
            | WorkspaceCommand::Community(
                CommunityCommand::HistoryAfter { channel, .. }
                | CommunityCommand::MessageContext { channel, .. },
            ) => format!("history:{channel}"),
            _ => {
                self.workspace(command);
                return;
            }
        };
        let Some(command) = QueryCommand::from_command(command) else {
            return;
        };
        let id = rand::random::<u64>();
        self.workspace_ui.queries.insert(scope, id);
        self.workspace(WorkspaceCommand::Query {
            id,
            command: Box::new(command),
        });
    }

    pub fn select_channel(&mut self, channel: ChannelId) {
        self.selected = Some(channel);
        let scope = format!("{}|{}|{channel}", self.local_identity, self.current_host());
        let position = self
            .workspace_ui
            .reading_positions
            .get(&channel)
            .copied()
            .or_else(|| self.settings.read_positions.get(&scope).copied())
            .or_else(|| {
                self.view
                    .as_ref()
                    .and_then(|v| {
                        v.workspace
                            .community
                            .unread
                            .iter()
                            .find(|s| s.channel == channel && s.count > 0)
                    })
                    .and_then(|s| s.first)
            });
        self.scroll_to_bottom = if position.is_some() { 0 } else { 3 };
        self.chat_at_bottom = position.is_none();
        self.workspace_ui.history_anchor = position;
        self.workspace_ui.reply_to = None;
        self.workspace_ui.pinned_only = false;
        if let Some(view) = &self.view {
            let cursor = view
                .workspace
                .read
                .iter()
                .find(|(ch, _)| *ch == channel)
                .map(|(_, id)| *id)
                .unwrap_or(0);
            if let Some(first) = view
                .messages
                .get(&channel)
                .and_then(|list| list.iter().find(|m| m.id > cursor && m.author != view.me))
            {
                self.workspace_ui.first_unread.insert(channel, first.id);
            }
        }
        if let Some(message) = position {
            let cached = self
                .view
                .as_ref()
                .and_then(|v| v.messages.get(&channel))
                .is_some_and(|list| list.iter().any(|m| m.id == message));
            if !cached {
                self.workspace_query(WorkspaceCommand::Community(
                    CommunityCommand::MessageContext {
                        channel,
                        message,
                        limit: 100,
                    },
                ));
                return;
            }
        }
        if let Some(view) = &self.view {
            if view.messages.get(&channel).is_none_or(Vec::is_empty) {
                self.load_history(channel, false);
            }
        }
    }

    pub fn mark_read(&mut self, channel: ChannelId) {
        let last = self
            .view
            .as_ref()
            .filter(|v| !v.history_after.contains(&channel))
            .and_then(|v| v.messages.get(&channel))
            .and_then(|m| m.last())
            .map(|m| m.id);
        if let Some(last) = last {
            self.mark_read_until(channel, last);
        }
    }
    pub fn mark_all_read(&mut self, channel: ChannelId) {
        let last = self.view.as_ref().and_then(|v| {
            v.workspace
                .community
                .unread
                .iter()
                .find(|s| s.channel == channel)
                .and_then(|s| s.latest)
                .or_else(|| {
                    v.messages
                        .get(&channel)
                        .and_then(|m| m.last())
                        .map(|m| m.id)
                })
        });
        if let Some(last) = last {
            self.mark_read_until(channel, last);
        }
    }
    fn mark_read_until(&mut self, channel: ChannelId, message: MessageId) {
        if self.view.as_ref().is_some_and(|v| {
            v.workspace
                .read
                .iter()
                .any(|(ch, id)| *ch == channel && *id >= message)
        }) {
            return;
        }
        self.workspace(WorkspaceCommand::Read { channel, message });
        if let Some(view) = &mut self.view {
            view.workspace.read.retain(|(ch, _)| *ch != channel);
            view.workspace.read.push((channel, message));
        }
    }

    pub fn mark_unread(&mut self, channel: ChannelId, message: MessageId) {
        self.workspace_ui.first_unread.insert(channel, message);
        self.chat_at_bottom = false;
        self.workspace_ui.reading_positions.insert(channel, message);
        self.workspace(WorkspaceCommand::Community(CommunityCommand::MarkUnread {
            channel,
            message,
        }));
    }

    pub fn load_history(&mut self, channel: ChannelId, older: bool) {
        let Some(view) = &mut self.view else { return };
        if !view.history_loading.insert(channel) {
            return;
        }
        let before = if older {
            view.messages
                .get(&channel)
                .and_then(|list| list.first())
                .map(|m| m.id)
        } else {
            None
        };
        if older {
            self.workspace_ui.history_anchor = before;
        } else {
            self.workspace_ui.recent_history.insert(channel);
        }
        self.workspace_query(WorkspaceCommand::History {
            channel,
            before,
            limit: 100,
        });
    }

    pub fn load_history_after(&mut self, channel: ChannelId) {
        let Some(view) = &mut self.view else {
            return;
        };
        if !view.history_loading.insert(channel) {
            return;
        }
        let after = view
            .messages
            .get(&channel)
            .and_then(|list| list.last())
            .map(|m| m.id)
            .unwrap_or(0);
        self.workspace_query(WorkspaceCommand::Community(
            CommunityCommand::HistoryAfter {
                channel,
                after,
                limit: 100,
            },
        ));
    }

    pub fn open_conversation(&mut self, name: String, members: Vec<UserId>, direct: bool) {
        if let Some(conversation) = self.view.as_ref().and_then(|view| {
            view.workspace
                .conversations
                .iter()
                .find(|c| direct && c.direct && members.iter().all(|m| c.members.contains(m)))
                .cloned()
        }) {
            self.home = true;
            self.select_channel(conversation.channel);
            return;
        }
        self.workspace_ui.pending_conversation = Some((name.clone(), members.clone(), direct));
        self.workspace(WorkspaceCommand::OpenConversation {
            name,
            members,
            direct,
        });
    }

    pub fn open_profile(&mut self, user: UserId) {
        self.workspace_ui.profile = Some(user);
        let profile = self
            .view
            .as_ref()
            .and_then(|v| v.workspace.profiles.iter().find(|p| p.user == user))
            .cloned()
            .unwrap_or_else(|| Profile {
                user,
                ..Default::default()
            });
        self.workspace_ui.profile_status = profile.status;
        self.workspace_ui.profile_presence = profile.presence;
        self.workspace_ui.profile_avatar = profile.avatar;
    }

    pub fn send_pending(&self, pending: &crate::settings::PendingMessage) {
        self.workspace(WorkspaceCommand::Send {
            operation: pending.operation.clone(),
            channel: pending.channel,
            text: pending.text.clone(),
            reply_to: pending.reply_to,
            attachments: pending.attachments.clone(),
        });
    }

    pub fn replay_pending(&mut self) {
        let host = self.current_host();
        let identity = self.local_identity.clone();
        let pending: Vec<_> = self
            .settings
            .pending
            .iter_mut()
            .filter(|p| p.host == host && p.identity == identity && p.error.is_none())
            .map(|p| {
                p.attempted_at = now_secs();
                p.clone()
            })
            .collect();
        for message in pending {
            self.send_pending(&message);
        }
    }

    pub fn retry_pending(&mut self, operation: &str) {
        let pending = self
            .settings
            .pending
            .iter_mut()
            .find(|p| p.operation == operation)
            .map(|p| {
                p.error = None;
                p.attempted_at = now_secs();
                p.clone()
            });
        if let Some(pending) = pending {
            self.settings.save();
            self.send_pending(&pending);
        }
    }

    pub fn tick_pending(&mut self) {
        if !self.client.as_ref().is_some_and(|c| c.is_alive()) {
            self.workspace_ui.chunk_uploads.clear();
            let interrupted_downloads: Vec<u64> =
                self.workspace_ui.chunk_downloads.keys().copied().collect();
            for id in interrupted_downloads {
                self.drop_download(id);
            }
            let interrupted = std::mem::take(&mut self.workspace_ui.uploads);
            for (operation, channel) in interrupted {
                self.workspace_ui.upload_names.remove(&operation);
                if let Some(file) = self.workspace_ui.upload_sources.remove(&operation) {
                    self.workspace_ui
                        .staged
                        .entry(channel)
                        .or_default()
                        .insert(0, file);
                }
            }
        } else {
            self.pump_uploads();
            let retries: Vec<_> = self
                .workspace_ui
                .chunk_downloads
                .iter_mut()
                .filter_map(|(id, download)| {
                    if download.retry_at.is_some_and(|at| Instant::now() >= at) {
                        download.retry_at = None;
                        Some((*id, download.written))
                    } else {
                        None
                    }
                })
                .collect();
            for (attachment, offset) in retries {
                self.workspace(WorkspaceCommand::DownloadChunk { attachment, offset });
            }
            // A download whose pieces stopped arriving (host error, lost reply) is abandoned
            // instead of leaving a spinner on screen forever.
            let stalled: Vec<u64> = self
                .workspace_ui
                .chunk_downloads
                .iter()
                .filter(|(_, d)| d.last_progress.elapsed() > Duration::from_secs(30))
                .map(|(id, _)| *id)
                .collect();
            for id in stalled {
                self.drop_download(id);
                self.notify("O download parou de responder e foi cancelado.");
            }
        }
        let mut changed = false;
        for pending in &mut self.settings.pending {
            if pending.error.is_none() && now_secs().saturating_sub(pending.attempted_at) > 20 {
                pending.error = Some("Sem confirmação. Você pode reenviar com segurança.".into());
                changed = true;
            }
        }
        if changed {
            self.settings.save();
        }
        let mut expired = Vec::new();
        for call in self.ringing() {
            let at = self
                .workspace_ui
                .call_seen
                .entry(call.id)
                .or_insert_with(Instant::now);
            if at.elapsed() >= Duration::from_secs(60) {
                expired.push(call.id);
            }
        }
        for call in expired {
            self.decline_call(call);
        }
    }

    pub fn open_message(&mut self, message: &Message) {
        self.home = self
            .view
            .as_ref()
            .and_then(|v| v.channel(message.channel))
            .is_some_and(|c| c.guild == 0);
        self.selected = Some(message.channel);
        if let Some(guild) = self
            .view
            .as_ref()
            .and_then(|v| v.channel(message.channel))
            .map(|c| c.guild)
            .filter(|g| *g != 0)
        {
            self.guild = Some(guild);
        }
        self.scroll_to_bottom = 0;
        self.workspace_ui.history_anchor = Some(message.id);
        self.workspace_ui.recent_history.insert(message.channel);
        self.workspace_ui.highlighted = Some((message.id, Instant::now()));
        self.workspace_query(WorkspaceCommand::Community(
            CommunityCommand::MessageContext {
                channel: message.channel,
                message: message.id,
                limit: 100,
            },
        ));
    }
}
