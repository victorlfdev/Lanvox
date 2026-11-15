use super::*;

impl State {
    pub(super) fn conversation_commands(
        &mut self,
        user: UserId,
        command: WorkspaceCommand,
    ) -> std::io::Result<()> {
        match command {
            WorkspaceCommand::OpenConversation {
                name,
                members,
                direct,
            } => {
                require(
                    self.workspace.conversations.len() < 4096,
                    "Limite de conversas atingido",
                )?;
                let mut members = members;
                members.push(user);
                members.sort_unstable();
                members.dedup();
                require(
                    (2..=MAX_MEMBERS).contains(&members.len()) && (!direct || members.len() == 2),
                    "Selecione os membros da conversa",
                )?;
                require(
                    members
                        .iter()
                        .all(|m| *m == user || self.are_friends(user, *m)),
                    "Adicione os participantes como amigos primeiro",
                )?;
                if direct {
                    if let Some(c) = self
                        .workspace
                        .conversations
                        .iter()
                        .find(|c| c.direct && c.members == members)
                    {
                        self.send_to(
                            user,
                            &ServerMsg::ChannelCreated(
                                self.channels
                                    .iter()
                                    .find(|ch| ch.id == c.channel)
                                    .unwrap()
                                    .clone(),
                            ),
                        );
                        self.send_workspace(user);
                        return Ok(());
                    }
                }
                require(
                    self.next_channel < CALL_ID_BASE,
                    "Limite de canais atingido",
                )?;
                let channel = self.next_channel;
                let name = clean(&name, 80);
                let name = if name.is_empty() {
                    members
                        .iter()
                        .filter(|m| **m != user)
                        .filter_map(|m| self.users.get(m).map(|u| u.name.clone()))
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    name
                };
                let previous = self.workspace.clone();
                self.workspace.conversations.push(Conversation {
                    channel,
                    name: name.clone(),
                    owner: user,
                    members: members.clone(),
                    direct,
                });
                self.channels.push(Channel {
                    id: channel,
                    guild: 0,
                    name,
                    kind: ChannelKind::Text,
                    topic: String::new(),
                });
                self.next_channel += 1;
                if let Err(e) = self.save_to_disk() {
                    self.workspace = previous;
                    self.channels.pop();
                    self.next_channel = channel;
                    return Err(e);
                }
                let ch = self.channels.last().unwrap().clone();
                for member in members {
                    self.send_to(member, &ServerMsg::ChannelCreated(ch.clone()));
                    self.send_workspace(member);
                }
            }
            WorkspaceCommand::UpdateConversation {
                channel,
                name,
                members,
            } => {
                let current = self
                    .workspace
                    .conversations
                    .iter()
                    .find(|c| c.channel == channel)
                    .ok_or_else(|| std::io::Error::other("Conversa não encontrada"))?;
                require(
                    current.owner == user && !current.direct,
                    "Somente o criador pode alterar um grupo",
                )?;
                let mut members = members;
                members.push(user);
                members.sort_unstable();
                members.dedup();
                require(
                    members.len() <= MAX_MEMBERS
                        && members.iter().all(|m| {
                            *m == user || current.members.contains(m) || self.are_friends(user, *m)
                        }),
                    "Participantes inválidos",
                )?;
                let previous = self.workspace.clone();
                let old_channels = self.channels.clone();
                let c = self
                    .workspace
                    .conversations
                    .iter_mut()
                    .find(|c| c.channel == channel)
                    .unwrap();
                c.name = clean(&name, 80);
                c.members = members;
                self.channels
                    .iter_mut()
                    .find(|c| c.id == channel)
                    .unwrap()
                    .name = c.name.clone();
                if let Err(e) = self.persist_workspace(previous) {
                    self.channels = old_channels;
                    return Err(e);
                }
                self.refresh_clients();
            }
            WorkspaceCommand::LeaveConversation { channel } => {
                let previous = self.workspace.clone();
                let c = self
                    .workspace
                    .conversations
                    .iter_mut()
                    .find(|c| c.channel == channel && c.members.contains(&user))
                    .ok_or_else(|| std::io::Error::other("Conversa não encontrada"))?;
                c.members.retain(|m| *m != user);
                if c.owner == user {
                    c.owner = c.members.first().copied().unwrap_or(0);
                }
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            _ => unreachable!("routed by workspacecommand"),
        }
        Ok(())
    }
}
