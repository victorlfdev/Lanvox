use super::*;

impl State {
    pub(super) fn event_commands(
        &mut self,
        user: UserId,
        command: CommunityCommand,
    ) -> std::io::Result<()> {
        match command {
            CommunityCommand::UpsertEvent { mut event } => {
                require(
                    self.guild_has(user, event.guild, permission::MANAGE_GUILD),
                    "Você não pode agendar eventos",
                )?;
                event.name = clean(&event.name, 100);
                event.description = clean(&event.description, 1000);
                require(!event.name.is_empty(), "Nome do evento vazio")?;
                require(
                    event.starts_at >= now_secs() - 86400
                        && event.starts_at <= now_secs() + 5 * 365 * 86400,
                    "Horário do evento fora do intervalo permitido",
                )?;
                require(
                    event.ends_at.is_none_or(|end| {
                        end > event.starts_at && end <= event.starts_at + 30 * 86400
                    }),
                    "Fim do evento inválido",
                )?;
                require(
                    event.channel.is_none_or(|id| {
                        self.channels
                            .iter()
                            .any(|c| c.id == id && c.guild == event.guild)
                            && self.can_access(user, id)
                    }),
                    "Canal do evento inválido",
                )?;
                if event.id != 0 {
                    let previous = self
                        .workspace
                        .community
                        .events
                        .iter()
                        .find(|e| e.id == event.id && e.guild == event.guild)
                        .ok_or_else(|| std::io::Error::other("Evento não encontrado"))?;
                    event.creator = previous.creator;
                    event.interested = previous.interested.clone();
                } else {
                    require(
                        self.workspace
                            .community
                            .events
                            .iter()
                            .filter(|e| e.guild == event.guild)
                            .count()
                            < 100,
                        "Limite de eventos da comunidade",
                    )
                    .and_then(|_| {
                        require(
                            self.workspace.community.events.len() < 1024,
                            "Limite de eventos do host",
                        )
                    })?;
                    event.id = self
                        .workspace
                        .next_event
                        .max(
                            self.workspace
                                .community
                                .events
                                .iter()
                                .map(|e| e.id)
                                .max()
                                .unwrap_or(0)
                                .checked_add(1)
                                .ok_or_else(|| {
                                    std::io::Error::other("Limite de eventos atingido")
                                })?,
                        )
                        .max(1);
                    require(event.id < i64::MAX as u64, "Limite de eventos atingido")?;
                    event.creator = user;
                    event.interested = Vec::new();
                }
                let guild = event.guild;
                let previous = self.workspace.clone();
                self.workspace.next_event =
                    self.workspace.next_event.max(event.id.saturating_add(1));
                self.workspace.community.events.retain(|e| e.id != event.id);
                self.workspace.community.events.push(event);
                self.audit(guild, user, None, "Evento atualizado", "");
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::DeleteEvent { guild, id } => {
                require(
                    self.guild_has(user, guild, permission::MANAGE_GUILD),
                    "Você não pode remover eventos",
                )?;
                require(
                    self.workspace
                        .community
                        .events
                        .iter()
                        .any(|e| e.guild == guild && e.id == id),
                    "Evento não encontrado",
                )?;
                let previous = self.workspace.clone();
                self.workspace.community.events.retain(|e| e.id != id);
                self.audit(guild, user, None, "Evento removido", "");
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::RespondEvent { event, interested } => {
                let e = self
                    .workspace
                    .community
                    .events
                    .iter()
                    .find(|e| e.id == event)
                    .ok_or_else(|| std::io::Error::other("Evento não encontrado"))?;
                require(
                    self.role(user, e.guild).is_some()
                        && e.channel.is_none_or(|c| self.can_access(user, c))
                        && !e.cancelled,
                    "Evento não autorizado ou cancelado",
                )?;
                require(
                    !interested || e.interested.len() < MAX_MEMBERS || e.interested.contains(&user),
                    "Limite de interessados",
                )?;
                let previous = self.workspace.clone();
                let e = self
                    .workspace
                    .community
                    .events
                    .iter_mut()
                    .find(|e| e.id == event)
                    .unwrap();
                e.interested.retain(|u| *u != user);
                if interested {
                    e.interested.push(user);
                }
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            _ => unreachable!("routed by communitycommand"),
        }
        Ok(())
    }
}
