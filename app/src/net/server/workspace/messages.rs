use super::*;

impl State {
    pub(in crate::net::server) fn send_channel(&self, channel: ChannelId, msg: &ServerMsg) {
        let packet = encode_packet(msg);
        for (user, out) in &self.clients {
            if self.can_access(*user, channel) {
                out.push_urgent(packet.clone());
            }
        }
    }

    pub(super) fn details_for(&self, messages: &[Message]) -> Vec<MessageDetails> {
        messages
            .iter()
            .filter_map(|m| {
                self.workspace
                    .details
                    .iter()
                    .find(|d| d.message == m.id)
                    .cloned()
                    .or_else(|| {
                        self.store
                            .as_ref()
                            .and_then(|s| s.lock().details(m.id).ok().flatten())
                    })
            })
            .map(|d| self.with_reply_preview(d))
            .collect()
    }

    /// Attach author and excerpt of the replied-to message (or the "removed" state).
    pub(super) fn with_reply_preview(&self, mut details: MessageDetails) -> MessageDetails {
        details.reply_preview = details
            .reply_to
            .map(|id| ReplyPreview::from_original(self.find_message(id).ok().as_ref()));
        details
    }

    /// Users who can be mentioned in `channel`, with their names.
    pub(super) fn mention_candidates(&self, channel: ChannelId) -> Vec<(UserId, String)> {
        self.users
            .values()
            .filter(|u| self.can_access(u.id, channel))
            .map(|u| (u.id, u.name.clone()))
            .collect()
    }

    /// Fallback when the host runs without a database: same filters over the cached messages.
    pub(super) fn search_in_memory(
        &self,
        channels: &[ChannelId],
        request: &SearchRequest,
        limit: usize,
    ) -> (Vec<Message>, bool) {
        let words: Vec<String> = request
            .query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase)
            .collect();
        let mut hits: Vec<Message> = self
            .messages
            .iter()
            .filter(|m| channels.contains(&m.channel))
            .filter(|m| request.author.is_none_or(|a| m.author == a))
            .filter(|m| request.after.is_none_or(|t| m.time >= t))
            .filter(|m| request.before.is_none_or(|t| m.time <= t))
            .filter(|m| {
                let text = m.text.to_lowercase();
                let tokens: Vec<&str> = text
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|t| !t.is_empty())
                    .collect();
                words
                    .iter()
                    .all(|w| tokens.iter().any(|t| t.starts_with(w.as_str())))
            })
            .filter(|m| {
                if request.mentions.is_none() && request.has_attachment.is_none() {
                    return true;
                }
                let d = self.details_for(std::slice::from_ref(m)).pop();
                request
                    .mentions
                    .is_none_or(|u| d.as_ref().is_some_and(|d| d.mentions.contains(&u)))
                    && request
                        .has_attachment
                        .is_none_or(|a| d.as_ref().is_some_and(|d| !d.attachments.is_empty()) == a)
            })
            .cloned()
            .collect();
        if request.order == SearchOrder::Oldest {
            hits.sort_by_key(|m| m.id);
        } else {
            hits.sort_by_key(|m| std::cmp::Reverse(m.id));
        }
        let page: Vec<_> = hits.into_iter().skip(request.offset as usize).collect();
        let more = page.len() > limit;
        (page.into_iter().take(limit).collect(), more)
    }

    pub(super) fn page(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: usize,
    ) -> std::io::Result<Vec<Message>> {
        if let Some(store) = &self.store {
            return store.lock().history(channel, before, limit);
        }
        let mut messages: Vec<_> = self
            .messages
            .iter()
            .filter(|m| m.channel == channel && before.is_none_or(|b| m.id < b))
            .cloned()
            .collect();
        messages.sort_by_key(|m| m.id);
        let skip = messages.len().saturating_sub(limit);
        Ok(messages.into_iter().skip(skip).collect())
    }

    pub(super) fn find_message(&self, id: MessageId) -> std::io::Result<Message> {
        if let Some(m) = self.messages.iter().find(|m| m.id == id) {
            return Ok(m.clone());
        }
        self.store
            .as_ref()
            .map(|s| s.lock().message(id))
            .transpose()?
            .flatten()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "Mensagem não encontrada")
            })
    }

    pub(super) fn detail_mut(&mut self, id: MessageId) -> &mut MessageDetails {
        if !self.workspace.details.iter().any(|d| d.message == id) {
            let details = self
                .store
                .as_ref()
                .and_then(|s| s.lock().details(id).ok().flatten())
                .unwrap_or(MessageDetails {
                    message: id,
                    ..Default::default()
                });
            self.workspace.details.push(details);
        }
        self.workspace
            .details
            .iter_mut()
            .find(|d| d.message == id)
            .unwrap()
    }

    pub(super) fn persist_workspace(&mut self, previous: WorkspaceData) -> std::io::Result<()> {
        if let Err(e) = self.save_to_disk() {
            self.workspace = previous;
            return Err(e);
        }
        Ok(())
    }

    pub(in crate::net::server) fn durable_send(
        &mut self,
        user: UserId,
        operation: Option<String>,
        channel: ChannelId,
        text: String,
        reply_to: Option<MessageId>,
        attachments: Vec<u64>,
    ) -> std::io::Result<()> {
        require(
            self.can_access(user, channel),
            "Você não tem acesso a esta conversa",
        )?;
        require(
            self.channels.iter().any(|c| c.id == channel),
            "Canal de texto inválido",
        )?;
        let text = text.trim().to_owned();
        require(
            text.chars().count() <= MAX_MESSAGE_CHARS,
            "Mensagem excede 2000 caracteres",
        )?;
        require(
            !text.is_empty() || !attachments.is_empty(),
            "Escreva uma mensagem ou anexe um arquivo",
        )?;
        if let Some(op) = &operation {
            require(
                !op.is_empty() && op.len() <= 128,
                "Identificador de envio inválido",
            )?;
            let existing = if let Some(store) = &self.store {
                store.lock().operation(user, op)?
            } else {
                self.operations.get(&(user, op.clone())).copied()
            };
            if let Some(message) = existing {
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Ack {
                        operation: op.clone(),
                        message,
                    }),
                );
                return Ok(());
            }
        }
        self.require_write(user, channel)?;
        require(
            infos_allowed(self, user, channel, !attachments.is_empty()),
            "Você não pode enviar anexos neste canal",
        )?;
        if let Some(reply) = reply_to {
            require(
                self.find_message(reply)?.channel == channel,
                "Resposta pertence a outra conversa",
            )?;
        }
        require(attachments.len() <= 8, "Limite de oito anexos por mensagem")?;
        let mut infos = Vec::new();
        for id in attachments {
            let info = self.find_attachment_info(id)?;
            require(
                info.channel == channel && info.uploader == user,
                "Anexo não autorizado",
            )?;
            infos.push(info);
        }
        let author = self
            .users
            .get(&user)
            .ok_or_else(|| std::io::Error::other("Usuário não encontrado"))?;
        require(
            self.next_message < i64::MAX as u64,
            "Limite de mensagens atingido",
        )?;
        let message = Message {
            id: self.next_message,
            channel,
            author: user,
            author_name: author.name.clone(),
            author_color: author.color,
            time: now_secs(),
            text,
        };
        let previous = self.workspace.clone();
        self.workspace
            .last_sends
            .retain(|(u, c, t)| (*u != user || *c != channel) && now_secs() - *t <= 86400);
        self.workspace.last_sends.push((user, channel, now_secs()));
        let mentions = crate::mentions::mentioned(&message.text, &self.mention_candidates(channel));
        let details = MessageDetails {
            message: message.id,
            reply_to,
            attachments: infos,
            mentions,
            ..Default::default()
        };
        self.workspace.details.push(details.clone());
        let details = self.with_reply_preview(details);
        self.next_message = self
            .next_message
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("Limite de mensagens atingido"))?;
        let result = if let Some(store) = &self.store {
            store
                .lock()
                .insert(&message, operation.as_deref(), &self.saved(false))
        } else {
            self.messages.push(message.clone());
            let result = self.save_to_disk();
            if result.is_err() {
                self.messages.pop();
            }
            result
        };
        if let Err(e) = result {
            self.next_message = message.id;
            self.workspace = previous;
            return Err(e);
        }
        if self.store.is_some() {
            self.messages.push(message.clone());
            self.trim_cache(channel);
        }
        if let Some(operation) = operation {
            if self.store.is_none() {
                self.operations
                    .insert((user, operation.clone()), message.id);
            }
            self.send_to(
                user,
                &ServerMsg::Workspace(WorkspaceEvent::Ack {
                    operation,
                    message: message.id,
                }),
            );
        }
        self.send_channel(channel, &ServerMsg::Message(message.clone()));
        self.send_channel(
            channel,
            &ServerMsg::Workspace(WorkspaceEvent::MessageChanged { message, details }),
        );
        self.broadcast_unread(channel);
        Ok(())
    }

    pub(super) fn trim_cache(&mut self, channel: ChannelId) {
        let count = self
            .messages
            .iter()
            .filter(|m| m.channel == channel)
            .count();
        let mut remove = count.saturating_sub(CACHE_PER_CHANNEL);
        self.messages.retain(|m| {
            if m.channel == channel && remove > 0 {
                remove -= 1;
                false
            } else {
                true
            }
        });
        if self.messages.len() > 4000 {
            self.messages.sort_by_key(|m| m.id);
            self.messages.drain(..self.messages.len() - 4000);
        }
        let cached: HashSet<_> = self.messages.iter().map(|m| m.id).collect();
        self.workspace
            .details
            .retain(|d| cached.contains(&d.message));
    }
}
