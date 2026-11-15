use super::*;

impl State {
    pub(super) fn message_commands(
        &mut self,
        user: UserId,
        command: WorkspaceCommand,
    ) -> std::io::Result<()> {
        match command {
            WorkspaceCommand::Pinned { channel } => {
                require(self.can_access(user, channel), "Conversa não autorizada")?;
                let messages = if let Some(store) = &self.store {
                    store.lock().pinned(channel)?
                } else {
                    self.messages
                        .iter()
                        .filter(|m| {
                            m.channel == channel
                                && self
                                    .workspace
                                    .details
                                    .iter()
                                    .any(|d| d.message == m.id && d.pinned)
                        })
                        .take(100)
                        .cloned()
                        .collect()
                };
                let details = self.details_for(&messages);
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Pinned {
                        channel,
                        messages,
                        details,
                    }),
                );
            }
            WorkspaceCommand::Refresh => {
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Resync {
                        snapshot: self.snapshot(user),
                    }),
                );
                self.send_workspace(user);
            }
            WorkspaceCommand::Send {
                operation,
                channel,
                text,
                reply_to,
                attachments,
            } => {
                return self.durable_send(
                    user,
                    Some(operation),
                    channel,
                    text,
                    reply_to,
                    attachments,
                )
            }
            WorkspaceCommand::History {
                channel,
                before,
                limit,
            } => {
                require(self.can_access(user, channel), "Conversa não autorizada")?;
                let limit = usize::from(limit).clamp(1, MAX_HISTORY_PAGE);
                let mut messages = self.page(channel, before, limit + 1)?;
                let has_more = messages.len() > limit;
                if has_more {
                    messages.remove(0);
                }
                let details = self.details_for(&messages);
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::History {
                        channel,
                        messages,
                        details,
                        has_more,
                    }),
                );
            }
            WorkspaceCommand::Edit { message, text } => {
                let mut message = self.find_message(message)?;
                require(
                    self.can_access(user, message.channel)
                        && message.author == user
                        && self.has_permission(user, message.channel, permission::SEND)
                        && !self.is_timed_out(user, message.channel),
                    "Somente o autor pode editar",
                )?;
                let text = clean(&text, MAX_MESSAGE_CHARS);
                require(!text.is_empty(), "Mensagem vazia")?;
                message.text = text;
                let previous = self.workspace.clone();
                let mentions = crate::mentions::mentioned(
                    &message.text,
                    &self.mention_candidates(message.channel),
                );
                let d = self.detail_mut(message.id);
                d.edited = Some(now_secs());
                d.mentions = mentions;
                let result = if let Some(store) = &self.store {
                    store.lock().update(&message, &self.saved(false))
                } else {
                    Ok(())
                };
                if let Err(e) = result {
                    self.workspace = previous;
                    return Err(e);
                }
                if let Some(m) = self.messages.iter_mut().find(|m| m.id == message.id) {
                    *m = message.clone();
                }
                if self.store.is_none() {
                    self.persist_workspace(previous)?;
                }
                let details = self.detail_mut(message.id).clone();
                let details = self.with_reply_preview(details);
                self.send_channel(
                    message.channel,
                    &ServerMsg::Workspace(WorkspaceEvent::MessageChanged { message, details }),
                );
            }
            WorkspaceCommand::Delete { message } => {
                let m = self.find_message(message)?;
                require(
                    self.can_access(user, m.channel)
                        && (m.author == user || self.can_moderate(user, m.channel)),
                    "Você não pode apagar esta mensagem",
                )?;
                let previous = self.workspace.clone();
                self.workspace.details.retain(|d| d.message != message);
                self.workspace
                    .community
                    .polls
                    .retain(|p| p.message != message);
                if let Some(store) = &self.store {
                    if let Err(e) = store.lock().delete(message, &self.saved(false)) {
                        self.workspace = previous;
                        return Err(e);
                    }
                }
                self.messages.retain(|m| m.id != message);
                if self.store.is_none() {
                    self.persist_workspace(previous)?;
                }
                self.send_channel(
                    m.channel,
                    &ServerMsg::Workspace(WorkspaceEvent::MessageDeleted {
                        channel: m.channel,
                        message,
                    }),
                );
            }
            WorkspaceCommand::React { message, emoji } => {
                let m = self.find_message(message)?;
                require(self.can_access(user, m.channel), "Conversa não autorizada")?;
                require(
                    self.has_permission(user, m.channel, permission::REACT)
                        && !self.is_timed_out(user, m.channel),
                    "Reações não permitidas",
                )?;
                let emoji = clean(&emoji, 64);
                if let Some(raw) = emoji.strip_prefix("custom:") {
                    let id = raw
                        .parse::<u64>()
                        .map_err(|_| std::io::Error::other("Emoji inválido"))?;
                    let guild = self
                        .channels
                        .iter()
                        .find(|c| c.id == m.channel)
                        .map(|c| c.guild)
                        .unwrap_or(0);
                    let available = self
                        .workspace
                        .community
                        .emojis
                        .iter()
                        .any(|e| e.id == id && e.guild == guild);
                    let removing = self.details_for(std::slice::from_ref(&m)).iter().any(|d| {
                        d.reactions
                            .iter()
                            .any(|r| r.emoji == emoji && r.users.contains(&user))
                    });
                    require(
                        available || removing,
                        "Emoji não disponível nesta comunidade",
                    )?;
                }
                require(!emoji.is_empty() && emoji.len() <= 128, "Reação inválida")?;
                let previous = self.workspace.clone();
                let d = self.detail_mut(message);
                if let Some(r) = d.reactions.iter_mut().find(|r| r.emoji == emoji) {
                    if r.users.contains(&user) {
                        r.users.retain(|u| *u != user);
                    } else {
                        r.users.push(user);
                    }
                } else {
                    require(d.reactions.len() < 20, "Limite de reações atingido")?;
                    d.reactions.push(Reaction {
                        emoji,
                        users: vec![user],
                    });
                }
                d.reactions.retain(|r| !r.users.is_empty());
                self.persist_workspace(previous)?;
                let details = self.detail_mut(message).clone();
                let details = self.with_reply_preview(details);
                self.send_channel(
                    m.channel,
                    &ServerMsg::Workspace(WorkspaceEvent::MessageChanged {
                        message: m,
                        details,
                    }),
                );
            }
            WorkspaceCommand::Pin { message, pinned } => {
                let m = self.find_message(message)?;
                require(
                    self.can_access(user, m.channel) && self.can_moderate(user, m.channel),
                    "Somente administradores podem fixar mensagens",
                )?;
                let previous = self.workspace.clone();
                self.detail_mut(message).pinned = pinned;
                self.persist_workspace(previous)?;
                let details = self.detail_mut(message).clone();
                let details = self.with_reply_preview(details);
                self.send_channel(
                    m.channel,
                    &ServerMsg::Workspace(WorkspaceEvent::MessageChanged {
                        message: m,
                        details,
                    }),
                );
            }
            WorkspaceCommand::Read { channel, message } => {
                require(self.can_access(user, channel), "Conversa não autorizada")?;
                require(
                    self.find_message(message)?.channel == channel,
                    "Cursor inválido",
                )?;
                let previous = self.workspace.clone();
                if let Some((_, _, cursor)) = self
                    .workspace
                    .read
                    .iter_mut()
                    .find(|(u, c, _)| *u == user && *c == channel)
                {
                    *cursor = (*cursor).max(message);
                } else {
                    self.workspace.read.push((user, channel, message));
                }
                self.persist_workspace(previous)?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Read { channel, message }),
                );
                self.send_unread(user);
            }
            WorkspaceCommand::Search { channel, query } => {
                let query = clean(&query, 200);
                require(
                    query.chars().count() >= 2,
                    "Digite ao menos dois caracteres",
                )?;
                if let Some(channel) = channel {
                    require(self.can_access(user, channel), "Conversa não autorizada")?;
                }
                let channels: Vec<_> = self
                    .channels
                    .iter()
                    .filter(|c| channel.is_none_or(|id| c.id == id) && self.can_access(user, c.id))
                    .map(|c| c.id)
                    .collect();
                let mut messages = Vec::new();
                for ch in channels {
                    if let Some(store) = &self.store {
                        messages.extend(store.lock().search(ch, &query, 50)?);
                    } else {
                        messages.extend(
                            self.messages
                                .iter()
                                .filter(|m| {
                                    m.channel == ch
                                        && m.text.to_lowercase().contains(&query.to_lowercase())
                                })
                                .take(50)
                                .cloned(),
                        );
                    }
                }
                messages.sort_by_key(|m| std::cmp::Reverse(m.id));
                messages.truncate(100);
                let details = self.details_for(&messages);
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::SearchResults {
                        query,
                        messages,
                        details,
                    }),
                );
            }
            WorkspaceCommand::Inbox {
                mentions_only,
                before,
                limit,
            } => {
                let channels: Vec<(ChannelId, MessageId)> = self
                    .channels
                    .iter()
                    .filter(|c| self.can_access(user, c.id))
                    .map(|c| {
                        (
                            c.id,
                            self.workspace
                                .read
                                .iter()
                                .find(|(u, ch, _)| *u == user && *ch == c.id)
                                .map(|(_, _, m)| *m)
                                .unwrap_or(0),
                        )
                    })
                    .collect();
                let limit = usize::from(limit).clamp(1, crate::storage::SEARCH_MAX_LIMIT);
                let (messages, has_more) = if let Some(store) = &self.store {
                    store
                        .lock()
                        .inbox(user, &channels, mentions_only, before, limit)?
                } else {
                    let mut messages: Vec<_> = self
                        .messages
                        .iter()
                        .filter(|m| {
                            m.author != user
                                && before.is_none_or(|id| m.id < id)
                                && channels
                                    .iter()
                                    .any(|(ch, cursor)| *ch == m.channel && m.id > *cursor)
                                && (!mentions_only
                                    || self
                                        .details_for(std::slice::from_ref(m))
                                        .iter()
                                        .any(|d| d.mentions.contains(&user)))
                        })
                        .cloned()
                        .collect();
                    messages.sort_by_key(|m| std::cmp::Reverse(m.id));
                    let has_more = messages.len() > limit;
                    messages.truncate(limit);
                    (messages, has_more)
                };
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::InboxPage {
                        mentions_only,
                        before,
                        messages,
                        has_more,
                    }),
                );
            }
            WorkspaceCommand::SearchAdvanced(mut request) => {
                request.query = clean(&request.query, 200);
                let filtered = request.author.is_some()
                    || request.after.is_some()
                    || request.before.is_some()
                    || request.mentions.is_some()
                    || request.has_attachment.is_some();
                require(
                    filtered
                        || (request.query.chars().count() >= 2
                            && crate::storage::fts_query(&request.query).is_some()),
                    "Digite ao menos dois caracteres",
                )?;
                require(
                    request
                        .after
                        .zip(request.before)
                        .is_none_or(|(a, b)| a <= b),
                    "Intervalo de datas inválido",
                )?;
                if let Some(channel) = request.channel {
                    require(self.can_access(user, channel), "Conversa não autorizada")?;
                }
                let channels: Vec<_> = self
                    .channels
                    .iter()
                    .filter(|c| {
                        request.channel.is_none_or(|id| c.id == id) && self.can_access(user, c.id)
                    })
                    .map(|c| c.id)
                    .collect();
                let limit = match request.limit as usize {
                    0 => crate::storage::SEARCH_DEFAULT_LIMIT,
                    n => n.min(crate::storage::SEARCH_MAX_LIMIT),
                };
                let (messages, has_more) = if let Some(store) = &self.store {
                    store.lock().search_advanced(&channels, &request)?
                } else {
                    self.search_in_memory(&channels, &request, limit)
                };
                let details = self.details_for(&messages);
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::SearchPage {
                        query: request.query.clone(),
                        messages,
                        details,
                        offset: request.offset,
                        has_more,
                    }),
                );
            }
            _ => unreachable!("routed by workspacecommand"),
        }
        Ok(())
    }
}
