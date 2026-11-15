use super::*;

impl State {
    pub(super) fn poll_commands(
        &mut self,
        user: UserId,
        command: CommunityCommand,
    ) -> std::io::Result<()> {
        match command {
            CommunityCommand::CreatePoll {
                operation,
                channel,
                question,
                options,
                multiple,
            } => {
                require(self.can_access(user, channel), "Conversa não autorizada")?;
                require(
                    !operation.is_empty() && operation.len() <= 128,
                    "Identificador de enquete inválido",
                )?;
                let existing = if let Some(store) = &self.store {
                    store.lock().operation(user, &operation)?
                } else {
                    self.operations.get(&(user, operation.clone())).copied()
                };
                if let Some(message) = existing {
                    require(
                        self.workspace
                            .community
                            .polls
                            .iter()
                            .any(|p| p.message == message && p.channel == channel),
                        "Operação pertence a outro envio",
                    )?;
                    self.send_to(
                        user,
                        &ServerMsg::Workspace(WorkspaceEvent::Ack { operation, message }),
                    );
                    return Ok(());
                }
                self.require_write(user, channel)?;
                let question = clean(&question, 200);
                let options: Vec<_> = options.into_iter().map(|s| clean(&s, 100)).collect();
                require(
                    !question.is_empty()
                        && (2..=10).contains(&options.len())
                        && options.iter().all(|s| !s.is_empty()),
                    "Enquete requer pergunta e entre 2 e 10 opções",
                )?;
                let unique: HashSet<_> = options.iter().map(|s| s.to_lowercase()).collect();
                require(
                    unique.len() == options.len(),
                    "As opções devem ser diferentes",
                )?;
                require(
                    self.workspace.community.polls.len() < 2048,
                    "Limite de enquetes atingido",
                )?;
                let message = self.next_message;
                let poll = Poll {
                    message,
                    channel,
                    creator: user,
                    question: question.clone(),
                    options,
                    votes: Vec::new(),
                    multiple,
                    closed: false,
                };
                let previous = self.workspace.clone();
                self.workspace.community.polls.push(poll.clone());
                if let Err(error) = self.durable_send(
                    user,
                    Some(operation),
                    channel,
                    format!("Enquete: {question}"),
                    None,
                    Vec::new(),
                ) {
                    self.workspace = previous;
                    return Err(error);
                }
                self.send_channel(
                    channel,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::PollUpdated(
                        poll,
                    ))),
                );
            }
            CommunityCommand::VotePoll {
                message,
                mut choices,
            } => {
                let poll = self
                    .workspace
                    .community
                    .polls
                    .iter()
                    .find(|p| p.message == message)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other("Enquete não encontrada"))?;
                require(
                    self.can_access(user, poll.channel)
                        && !self.is_timed_out(user, poll.channel)
                        && self.has_permission(user, poll.channel, permission::REACT),
                    "Votação não autorizada",
                )?;
                require(!poll.closed, "Esta enquete foi encerrada")?;
                choices.sort_unstable();
                choices.dedup();
                require(
                    choices.len() <= poll.options.len()
                        && choices.iter().all(|i| usize::from(*i) < poll.options.len())
                        && (poll.multiple || choices.len() <= 1),
                    "Seleção inválida",
                )?;
                require(
                    poll.votes.len() < MAX_MEMBERS || poll.votes.iter().any(|v| v.user == user),
                    "Limite de participantes atingido",
                )?;
                let previous = self.workspace.clone();
                let p = self
                    .workspace
                    .community
                    .polls
                    .iter_mut()
                    .find(|p| p.message == message)
                    .unwrap();
                p.votes.retain(|v| v.user != user);
                if !choices.is_empty() {
                    p.votes.push(PollVote { user, choices });
                }
                let changed = p.clone();
                self.persist_workspace(previous)?;
                self.send_channel(
                    poll.channel,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::PollUpdated(
                        changed,
                    ))),
                );
            }
            CommunityCommand::ClosePoll { message } => {
                let poll = self
                    .workspace
                    .community
                    .polls
                    .iter()
                    .find(|p| p.message == message)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other("Enquete não encontrada"))?;
                require(
                    self.can_access(user, poll.channel)
                        && (poll.creator == user || self.can_moderate(user, poll.channel)),
                    "Somente o criador ou moderador pode encerrar",
                )?;
                let previous = self.workspace.clone();
                let p = self
                    .workspace
                    .community
                    .polls
                    .iter_mut()
                    .find(|p| p.message == message)
                    .unwrap();
                p.closed = true;
                let changed = p.clone();
                self.persist_workspace(previous)?;
                self.send_channel(
                    poll.channel,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::PollUpdated(
                        changed,
                    ))),
                );
            }
            _ => unreachable!("routed by communitycommand"),
        }
        Ok(())
    }
}
