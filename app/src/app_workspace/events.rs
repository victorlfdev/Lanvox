use super::*;

impl LanvoxApp {
    pub fn on_workspace_event(&mut self, event: WorkspaceEvent) {
        let refresh_pins = match &event {
            WorkspaceEvent::MessageChanged { message, .. } => Some(message.channel),
            WorkspaceEvent::MessageDeleted { channel, .. } => Some(*channel),
            _ => None,
        };
        if let Some(channel) = refresh_pins
            .filter(|channel| self.workspace_ui.pinned_only && Some(*channel) == self.selected)
        {
            self.workspace(WorkspaceCommand::Pinned { channel });
        }
        if let WorkspaceEvent::History { channel, .. } = &event {
            if self.workspace_ui.recent_history.remove(channel) {
                self.workspace_ui.history_has_after.insert(*channel, false);
                if let Some(view) = &mut self.view {
                    view.messages.remove(channel);
                }
            }
        }
        if let Some(view) = &mut self.view {
            view.apply_workspace(&event);
        }
        match event {
            WorkspaceEvent::IdentityRotated { public_key, nonce } => {
                self.finish_identity_rotation(public_key, nonce)
            }
            WorkspaceEvent::Resync { snapshot } => {
                let me = self.me();
                let old_workspace = self
                    .view
                    .as_ref()
                    .map(|v| v.workspace.clone())
                    .unwrap_or_default();
                let mut view = ServerView::from_snapshot(me, snapshot);
                view.workspace = old_workspace;
                view.workspace
                    .conversations
                    .retain(|c| view.channels.iter().any(|ch| ch.id == c.channel));
                view.workspace
                    .memberships
                    .retain(|m| view.guilds.iter().any(|g| g.id == m.guild));
                let out_of_voice = view.my_voice().channel.is_none();
                let joining = self.joining_voice();
                if out_of_voice && !joining {
                    self.stop_media();
                }
                if self.selected.is_some_and(|id| view.channel(id).is_none()) {
                    self.selected = None;
                    self.home = true;
                }
                self.view = Some(view);
            }
            WorkspaceEvent::State(_) => {
                self.workspace_ui.custom_emoji_textures.clear();
                if let Some(channel) = self.workspace_ui.pending_thread {
                    if self
                        .view
                        .as_ref()
                        .is_some_and(|v| v.channel(channel).is_some())
                    {
                        self.workspace_ui.pending_thread = None;
                        self.select_channel(channel);
                    }
                }
                if let Some((name, members, direct)) =
                    self.workspace_ui.pending_conversation.clone()
                {
                    let found = self.view.as_ref().and_then(|view| {
                        view.workspace
                            .conversations
                            .iter()
                            .find(|c| {
                                c.direct == direct
                                    && (direct || c.name == name)
                                    && members.iter().all(|m| c.members.contains(m))
                            })
                            .map(|c| c.channel)
                    });
                    if let Some(channel) = found {
                        self.workspace_ui.pending_conversation = None;
                        self.home = true;
                        self.select_channel(channel);
                    }
                }
            }
            WorkspaceEvent::Ack { operation, .. } => {
                self.settings.pending.retain(|p| p.operation != operation);
                self.settings.save();
            }
            WorkspaceEvent::Pinned {
                channel,
                messages,
                details,
            } => {
                self.workspace_ui.pinned_messages.clear();
                self.workspace_ui.pinned_messages.insert(channel, messages);
                if let Some(view) = &mut self.view {
                    for detail in details {
                        view.workspace
                            .details
                            .retain(|old| old.message != detail.message);
                        view.workspace.details.push(detail);
                    }
                }
            }
            WorkspaceEvent::SearchResults {
                query,
                messages,
                details,
            } => {
                self.accept_search_page(query, messages, details, 0, false);
            }
            WorkspaceEvent::SearchPage {
                query,
                messages,
                details,
                offset,
                has_more,
            } => {
                self.accept_search_page(query, messages, details, offset, has_more);
            }
            WorkspaceEvent::InboxPage {
                mentions_only,
                before,
                messages,
                has_more,
            } => {
                if self.workspace_ui.inbox_requested == Some(mentions_only)
                    && self.workspace_ui.inbox_before == before
                {
                    if before.is_none() {
                        self.workspace_ui.inbox_results.clear();
                    }
                    for message in messages {
                        if !self
                            .workspace_ui
                            .inbox_results
                            .iter()
                            .any(|m| m.id == message.id)
                        {
                            self.workspace_ui.inbox_results.push(message);
                        }
                    }
                    self.workspace_ui.inbox_has_more = has_more;
                    self.workspace_ui.inbox_loading = false;
                }
            }
            WorkspaceEvent::Community(CommunityEvent::Context {
                channel,
                message,
                messages,
                details,
                has_before,
                has_after,
            }) => {
                self.workspace_ui
                    .history_has_after
                    .insert(channel, has_after);
                self.workspace_ui.recent_history.remove(&channel);
                if let Some(view) = &mut self.view {
                    view.messages.remove(&channel);
                    view.apply_workspace(&WorkspaceEvent::History {
                        channel,
                        messages,
                        details,
                        has_more: has_before,
                    });
                }
                self.workspace_ui.history_anchor = Some(message);
                self.workspace_ui.highlighted = Some((message, Instant::now()));
            }
            WorkspaceEvent::Community(CommunityEvent::AfterHistory {
                channel,
                messages,
                details,
                has_after,
            }) => {
                self.workspace_ui
                    .history_has_after
                    .insert(channel, has_after);
                if let Some(view) = &mut self.view {
                    let list = view.messages.entry(channel).or_default();
                    for message in messages {
                        if !list.iter().any(|m| m.id == message.id) {
                            list.push(message);
                        }
                    }
                    list.sort_by_key(|m| m.id);
                    if list.len() > 600 {
                        list.drain(..list.len() - 600);
                    }
                    for detail in details {
                        view.workspace
                            .details
                            .retain(|d| d.message != detail.message);
                        view.workspace.details.push(detail);
                    }
                    view.history_loading.remove(&channel);
                }
            }
            WorkspaceEvent::Community(CommunityEvent::Unread(summaries)) => {
                self.workspace_ui.inbox_dirty = true;
                if let Some(view) = &mut self.view {
                    for summary in summaries {
                        if summary.count > 0 {
                            view.unread.insert(summary.channel, summary.count);
                        } else {
                            view.unread.remove(&summary.channel);
                        }
                        if let Some(first) = summary.first {
                            self.workspace_ui
                                .first_unread
                                .insert(summary.channel, first);
                        }
                    }
                }
            }
            WorkspaceEvent::Community(CommunityEvent::Invites { invites, .. }) => {
                self.workspace_ui.extras.invites = invites;
            }
            WorkspaceEvent::Community(CommunityEvent::Moderation {
                bans,
                timeouts,
                audit,
                ..
            }) => {
                self.workspace_ui.extras.bans = bans;
                self.workspace_ui.extras.timeouts = timeouts;
                self.workspace_ui.extras.audit = audit;
            }
            WorkspaceEvent::Community(CommunityEvent::ThreadCreated { parent, channel }) => {
                if self
                    .workspace_ui
                    .forum_pending_draft
                    .as_ref()
                    .is_some_and(|(p, _)| *p == parent)
                {
                    if let Some((_, draft)) = self.workspace_ui.forum_pending_draft.take() {
                        self.compose.insert(channel, draft);
                        self.save_drafts();
                    }
                }
                self.home = false;
                if let Some(guild) = self
                    .view
                    .as_ref()
                    .and_then(|v| v.channel(parent))
                    .map(|c| c.guild)
                {
                    self.guild = Some(guild);
                }
                self.workspace_ui.pending_thread = Some(channel);
                if self
                    .view
                    .as_ref()
                    .is_some_and(|v| v.channel(channel).is_some())
                {
                    self.workspace_ui.pending_thread = None;
                    self.select_channel(channel);
                }
            }
            WorkspaceEvent::UploadProgress {
                operation,
                received,
            } => {
                if let Some(state) = self.workspace_ui.chunk_uploads.get_mut(&operation) {
                    state.acked = state.acked.max(received.min(state.total));
                    self.pump_chunks(&operation);
                }
            }
            WorkspaceEvent::UploadResumePoint {
                operation,
                received,
            } => {
                let mut valid = false;
                if let Some(state) = self.workspace_ui.chunk_uploads.get_mut(&operation) {
                    if received <= state.total
                        && (received == state.total
                            || received % ATTACHMENT_CHUNK_BYTES as u64 == 0)
                    {
                        state.sent = received;
                        state.acked = received;
                        state.finishing = false;
                        valid = true;
                    }
                }
                if valid {
                    self.pump_chunks(&operation);
                } else if self.workspace_ui.chunk_uploads.contains_key(&operation) {
                    self.workspace_ui.chunk_uploads.remove(&operation);
                    if let Some(channel) = self.workspace_ui.uploads.remove(&operation) {
                        self.workspace_ui.pending_batches.remove(&channel);
                        self.workspace_ui.send_after_upload.remove(&channel);
                        if let Some(file) = self.workspace_ui.upload_sources.remove(&operation) {
                            self.workspace_ui
                                .staged
                                .entry(channel)
                                .or_default()
                                .push(file);
                        }
                    }
                    self.workspace_ui.upload_names.remove(&operation);
                    self.notify("O host informou uma posição inválida para o envio.");
                }
            }
            WorkspaceEvent::AttachmentChunk { info, offset, data } => {
                use std::io::Write;
                let id = info.id;
                let Some(download) = self.workspace_ui.chunk_downloads.get_mut(&id) else {
                    return;
                };
                let in_order = offset == download.written
                    && !data.is_empty()
                    && download.written + data.len() as u64 <= download.info.size;
                if !in_order || download.file.write_all(&data).is_err() {
                    self.drop_download(id);
                    self.notify("Download interrompido.");
                    return;
                }
                download.written += data.len() as u64;
                download.last_progress = Instant::now();
                if download.written < download.info.size {
                    let next = download.written;
                    self.workspace(WorkspaceCommand::DownloadChunk {
                        attachment: id,
                        offset: next,
                    });
                } else if let Some(done) = self.workspace_ui.chunk_downloads.remove(&id) {
                    let _ = done.file.sync_all();
                    match done.purpose {
                        DownloadPurpose::Play => {
                            self.workspace_ui.media_files.insert(id, done.path);
                        }
                        DownloadPurpose::Save => {
                            self.notify(format!("Arquivo salvo em {}", done.path.display()));
                        }
                    }
                }
            }
            WorkspaceEvent::Uploaded { operation, info } => {
                self.workspace_ui.chunk_uploads.remove(&operation);
                self.workspace_ui.uploads.remove(&operation);
                self.workspace_ui.upload_names.remove(&operation);
                self.workspace_ui.upload_sources.remove(&operation);
                if self.workspace_ui.cancelled_uploads.remove(&operation) {
                    return;
                }
                let channel = info.channel;
                self.workspace_ui
                    .attachments
                    .entry(info.channel)
                    .or_default()
                    .push(info);
                if self.workspace_ui.send_after_upload.contains(&channel)
                    && !self.workspace_ui.uploads.values().any(|ch| *ch == channel)
                {
                    self.pump_uploads();
                }
            }
            WorkspaceEvent::Attachment { info, bytes } => {
                self.workspace_ui.preview_requests.remove(&info.id);
                if self.workspace_ui.download_requests.remove(&info.id) {
                    self.save_attachment(info, bytes);
                } else {
                    // Keep previews within a fixed total budget.
                    if self
                        .workspace_ui
                        .downloads
                        .values()
                        .map(|(_, b)| b.len())
                        .sum::<usize>()
                        + bytes.len()
                        > 24 * 1024 * 1024
                    {
                        self.workspace_ui.downloads.clear();
                        self.attachment_textures.clear();
                    }
                    self.workspace_ui.downloads.insert(info.id, (info, bytes));
                }
            }
            WorkspaceEvent::Invite { code, .. } => {
                self.workspace_ui.invite_created = code;
            }
            WorkspaceEvent::Notice(text) => self.notify(text),
            WorkspaceEvent::Error { operation, reason } => {
                if self.workspace_ui.forum_pending_draft.is_some() {
                    self.workspace_ui.forum_failed_draft =
                        self.workspace_ui.forum_pending_draft.take();
                }
                if self.workspace_ui.inbox_loading {
                    self.workspace_ui.inbox_error = Some(reason.clone());
                }
                self.workspace_ui.inbox_loading = false;
                if let Some(operation) = operation {
                    for p in &mut self.settings.pending {
                        if p.operation == operation {
                            p.error = Some(reason.clone());
                        }
                    }
                    if self.workspace_ui.chunk_uploads.remove(&operation).is_some() {
                        self.workspace(WorkspaceCommand::UploadAbort {
                            operation: operation.clone(),
                        });
                    }
                    if let Some(channel) = self.workspace_ui.uploads.remove(&operation) {
                        self.workspace_ui.send_after_upload.remove(&channel);
                        self.workspace_ui.pending_batches.remove(&channel);
                        if let Some(file) = self.workspace_ui.upload_sources.remove(&operation) {
                            self.workspace_ui
                                .staged
                                .entry(channel)
                                .or_default()
                                .push(file);
                        }
                    }
                    self.workspace_ui.upload_names.remove(&operation);
                    self.settings.save();
                }
                if self.workspace_ui.search_loading {
                    self.workspace_ui.search_loading = false;
                    self.workspace_ui.search_error = Some(reason.clone());
                }
                if let Some(view) = &mut self.view {
                    view.history_loading.clear();
                }
                self.notify(reason);
            }
            WorkspaceEvent::MessageChanged { ref message, .. }
                if self.selected == Some(message.channel) && self.chat_at_bottom =>
            {
                self.scroll_to_bottom = 2;
            }
            _ => {}
        }
    }
}
