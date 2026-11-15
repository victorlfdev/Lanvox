use super::*;

impl State {
    pub(super) fn thread_commands(
        &mut self,
        user: UserId,
        command: CommunityCommand,
    ) -> std::io::Result<()> {
        match command {
            CommunityCommand::CreateThread {
                parent,
                message,
                name,
            } => {
                require(
                    self.can_access(user, parent)
                        && self.has_permission(user, parent, permission::SEND)
                        && !self.is_timed_out(user, parent),
                    "Você não pode criar uma discussão",
                )?;
                require(
                    !self
                        .workspace
                        .community
                        .threads
                        .iter()
                        .any(|t| t.channel == parent),
                    "Discussões aninhadas não são permitidas",
                )?;
                let parent_ch = self
                    .channels
                    .iter()
                    .find(|c| c.id == parent && c.guild != 0 && c.kind == ChannelKind::Text)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other("Canal de origem inválido"))?;
                if let Some(id) = message {
                    require(
                        self.find_message(id)?.channel == parent,
                        "Mensagem de outra conversa",
                    )?;
                }
                let name = clean(&name, 80);
                require(!name.is_empty(), "Nome da discussão vazio")?;
                require(
                    self.channels.len() < MAX_CHANNELS && self.next_channel < CALL_ID_BASE,
                    "Limite de canais atingido",
                )?;
                let previous = self.workspace.clone();
                let channel = self.next_channel;
                self.next_channel += 1;
                self.channels.push(Channel {
                    id: channel,
                    guild: parent_ch.guild,
                    name,
                    kind: ChannelKind::Text,
                    topic: format!("Discussão em {}", parent_ch.name),
                });
                self.workspace.community.threads.push(ThreadInfo {
                    channel,
                    parent,
                    message,
                    creator: user,
                    archived: false,
                    tags: Vec::new(),
                });
                if let Err(e) = self.persist_workspace(previous) {
                    self.channels.pop();
                    self.next_channel = channel;
                    return Err(e);
                }
                self.refresh_clients();
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(
                        CommunityEvent::ThreadCreated { parent, channel },
                    )),
                );
            }
            CommunityCommand::SetThreadArchived { channel, archived } => {
                require(self.can_access(user, channel), "Discussão não autorizada")?;
                let t = self
                    .workspace
                    .community
                    .threads
                    .iter()
                    .find(|t| t.channel == channel)
                    .ok_or_else(|| std::io::Error::other("Discussão não encontrada"))?;
                require(
                    t.creator == user || self.can_moderate(user, channel),
                    "Você não pode arquivar esta discussão",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .threads
                    .iter_mut()
                    .find(|t| t.channel == channel)
                    .unwrap()
                    .archived = archived;
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::SetThreadTags { channel, tags } => {
                require(self.can_access(user, channel), "Discussão não autorizada")?;
                let t = self
                    .workspace
                    .community
                    .threads
                    .iter()
                    .find(|t| t.channel == channel)
                    .ok_or_else(|| std::io::Error::other("Discussão não encontrada"))?;
                require(
                    t.creator == user || self.can_moderate(user, channel),
                    "Você não pode alterar esta discussão",
                )?;
                let mut tags: Vec<_> = tags
                    .into_iter()
                    .take(5)
                    .map(|s| clean(&s, 24))
                    .filter(|s| !s.is_empty())
                    .collect();
                tags.sort();
                tags.dedup();
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .threads
                    .iter_mut()
                    .find(|t| t.channel == channel)
                    .unwrap()
                    .tags = tags;
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::MessageContext {
                channel,
                message,
                limit,
            } => {
                require(
                    self.can_access(user, channel)
                        && self.find_message(message)?.channel == channel,
                    "Mensagem não autorizada",
                )?;
                let half = usize::from(limit.clamp(10, 100)) / 2;
                let mut before = self.page(channel, Some(message), half + 1)?;
                let has_before = before.len() > half;
                if has_before {
                    before.remove(0);
                }
                let mut after = if let Some(store) = &self.store {
                    store.lock().history_after(channel, message, half + 1)?
                } else {
                    let mut m: Vec<_> = self
                        .messages
                        .iter()
                        .filter(|m| m.channel == channel && m.id > message)
                        .cloned()
                        .collect();
                    m.sort_by_key(|m| m.id);
                    m.truncate(half + 1);
                    m
                };
                let has_after = after.len() > half;
                after.truncate(half);
                before.push(self.find_message(message)?);
                before.extend(after);
                let details = self.details_for(&before);
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::Context {
                        channel,
                        message,
                        messages: before,
                        details,
                        has_before,
                        has_after,
                    })),
                );
            }
            CommunityCommand::Unread => self.send_unread(user),
            CommunityCommand::MarkUnread { channel, message } => {
                require(
                    self.can_access(user, channel)
                        && self.find_message(message)?.channel == channel,
                    "Mensagem não autorizada",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .read
                    .retain(|(u, c, _)| *u != user || *c != channel);
                self.workspace
                    .read
                    .push((user, channel, message.saturating_sub(1)));
                self.persist_workspace(previous)?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(
                        CommunityEvent::MarkedUnread {
                            channel,
                            message: message.saturating_sub(1),
                        },
                    )),
                );
                self.send_unread(user);
            }

            CommunityCommand::HistoryAfter {
                channel,
                after,
                limit,
            } => {
                require(self.can_access(user, channel), "Conversa não autorizada")?;
                let limit = usize::from(limit.clamp(1, 100));
                let mut messages = if let Some(store) = &self.store {
                    store.lock().history_after(channel, after, limit + 1)?
                } else {
                    let mut messages: Vec<_> = self
                        .messages
                        .iter()
                        .filter(|m| m.channel == channel && m.id > after)
                        .cloned()
                        .collect();
                    messages.sort_by_key(|m| m.id);
                    messages.truncate(limit + 1);
                    messages
                };
                let has_after = messages.len() > limit;
                messages.truncate(limit);
                let details = self.details_for(&messages);
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(
                        CommunityEvent::AfterHistory {
                            channel,
                            messages,
                            details,
                            has_after,
                        },
                    )),
                );
            }
            CommunityCommand::Forward {
                operation,
                message,
                target,
                note,
            } => {
                require(
                    self.can_access(user, target),
                    "Conversa de destino não autorizada",
                )?;
                require(
                    !operation.is_empty() && operation.len() <= 128,
                    "Identificador de envio inválido",
                )?;
                let existing = if let Some(store) = &self.store {
                    store.lock().operation(user, &operation)?
                } else {
                    self.operations.get(&(user, operation.clone())).copied()
                };
                if let Some(message) = existing {
                    self.send_to(
                        user,
                        &ServerMsg::Workspace(WorkspaceEvent::Ack { operation, message }),
                    );
                    return Ok(());
                }
                let original = self.find_message(message)?;
                require(
                    self.can_access(user, original.channel),
                    "Mensagem não autorizada",
                )?;
                self.require_write(user, target)?;
                let prefix = clean(&note, 300);
                let text = format!(
                    "{}Encaminhada:\n{}",
                    if prefix.is_empty() {
                        String::new()
                    } else {
                        format!("{prefix}\n\n")
                    },
                    original.text
                );
                require(
                    text.chars().count() <= MAX_MESSAGE_CHARS,
                    "Mensagem encaminhada excede o limite",
                )?;
                let details = self.details_for(std::slice::from_ref(&original));
                let mut attachments = Vec::new();
                if let Some(d) = details.first() {
                    require(
                        d.attachments.is_empty()
                            || self.has_permission(user, target, permission::ATTACH),
                        "Anexos não permitidos no destino",
                    )?;
                    for attachment in &d.attachments {
                        let info = self.find_attachment_info(attachment.id)?;
                        require(info.channel == original.channel, "Anexo não autorizado")?;
                        let copy_operation = format!("forward:{operation}:{}", attachment.id);
                        let existing = if let Some(store) = &self.store {
                            store.lock().uploaded(user, &copy_operation)?
                        } else {
                            self.workspace
                                .upload_operations
                                .iter()
                                .find(|(u, op, _)| *u == user && op == &copy_operation)
                                .and_then(|(_, _, id)| {
                                    self.uploads.get(id).map(|(info, _)| info.clone())
                                })
                        };
                        if let Some(info) = existing {
                            require(
                                info.channel == target && info.uploader == user,
                                "Copia de anexo invalida",
                            )?;
                            attachments.push(info.id);
                            continue;
                        }
                        let id = self.workspace.next_attachment.max(1);
                        require(id < i64::MAX as u64, "Limite de anexos atingido")?;
                        let copy = AttachmentInfo {
                            id,
                            channel: target,
                            uploader: user,
                            name: info.name,
                            media_type: info.media_type,
                            size: info.size,
                        };
                        let total = self.stored_attachment_bytes()?;
                        require(
                            total + copy.size <= ATTACHMENT_STORE_LIMIT,
                            "Armazenamento de anexos cheio",
                        )?;
                        let previous = self.workspace.clone();
                        self.workspace.next_attachment = id + 1;
                        if let Some(store) = &self.store {
                            if let Err(e) = store.lock().copy_attachment(
                                attachment.id,
                                &copy,
                                &copy_operation,
                                &self.saved(false),
                            ) {
                                self.workspace = previous;
                                return Err(e);
                            }
                        } else {
                            let bytes = self.find_attachment(attachment.id)?.1;
                            self.uploads.insert(id, (copy, bytes));
                            self.workspace
                                .upload_operations
                                .push((user, copy_operation, id));
                            if let Err(e) = self.persist_workspace(previous) {
                                self.uploads.remove(&id);
                                return Err(e);
                            }
                        }
                        attachments.push(id);
                    }
                }
                self.durable_send(user, Some(operation), target, text, None, attachments)?;
            }
            _ => unreachable!("routed by communitycommand"),
        }
        Ok(())
    }
}
