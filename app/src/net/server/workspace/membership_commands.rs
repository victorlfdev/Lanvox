use super::*;

impl State {
    pub(super) fn membership_commands(
        &mut self,
        user: UserId,
        command: WorkspaceCommand,
    ) -> std::io::Result<()> {
        match command {
            WorkspaceCommand::CreateInvite {
                guild,
                max_uses,
                expires_hours,
            } => {
                require(
                    self.guild_has(user, guild, permission::INVITE)
                        && self.guilds.iter().any(|g| g.id == guild),
                    "Sem permissão para criar convites",
                )?;
                require(
                    self.workspace.invites.len() < 4096,
                    "Limite de convites atingido",
                )?;
                let previous = self.workspace.clone();
                let invite = Invite {
                    guild,
                    creator: user,
                    code: random_token(),
                    expires: now_secs() + i64::from(expires_hours.clamp(1, 168)) * 3600,
                    remaining: max_uses.clamp(1, 100),
                };
                self.workspace.invites.push(invite.clone());
                self.persist_workspace(previous)?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Invite {
                        guild,
                        code: invite.code,
                        expires: invite.expires,
                    }),
                );
            }
            WorkspaceCommand::RedeemInvite { code } => {
                let invite = self
                    .workspace
                    .invites
                    .iter()
                    .find(|i| i.code == code && i.expires > now_secs() && i.remaining > 0)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other("Convite inválido ou expirado"))?;
                require(
                    !self.workspace.bans.contains(&(invite.guild, user)),
                    "Você foi banido desta comunidade",
                )?;
                if self.role(user, invite.guild).is_some() {
                    return Ok(());
                }
                let previous = self.workspace.clone();
                self.workspace.memberships.push(Membership {
                    guild: invite.guild,
                    user,
                    role: MemberRole::Member,
                });
                self.workspace
                    .invites
                    .iter_mut()
                    .find(|i| i.code == code)
                    .unwrap()
                    .remaining -= 1;
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            WorkspaceCommand::RevokeInvite { code } => {
                let invite = self
                    .workspace
                    .invites
                    .iter()
                    .find(|i| i.code == code)
                    .ok_or_else(|| std::io::Error::other("Convite não encontrado"))?;
                require(
                    self.guild_has(user, invite.guild, permission::INVITE),
                    "Sem permissão para revogar",
                )?;
                let previous = self.workspace.clone();
                self.workspace.invites.retain(|i| i.code != code);
                self.persist_workspace(previous)?;
            }
            WorkspaceCommand::SetRole {
                guild,
                user: target,
                role,
            } => {
                require(
                    self.workspace.host_owner == Some(user)
                        || self.role(user, guild) == Some(MemberRole::Owner),
                    "Somente o proprietário pode alterar cargos",
                )?;
                require(
                    target != user,
                    "Use outro administrador para transferir sua função",
                )?;
                let previous = self.workspace.clone();
                let member = self
                    .workspace
                    .memberships
                    .iter_mut()
                    .find(|m| m.guild == guild && m.user == target)
                    .ok_or_else(|| std::io::Error::other("Membro não encontrado"))?;
                member.role = role;
                if role == MemberRole::Owner {
                    for m in &mut self.workspace.memberships {
                        if m.guild == guild && m.user != target && m.role == MemberRole::Owner {
                            m.role = MemberRole::Admin;
                        }
                    }
                }
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            WorkspaceCommand::RemoveMember {
                guild,
                user: target,
                ban,
            } => {
                require(
                    self.administer(user, guild) && target != user,
                    "Você não pode remover este membro",
                )?;
                require(
                    self.workspace.host_owner != Some(target)
                        && self.role(target, guild) != Some(MemberRole::Owner),
                    "O proprietário não pode ser removido",
                )?;
                require(
                    self.role(target, guild) != Some(MemberRole::Admin)
                        || self.workspace.host_owner == Some(user)
                        || self.role(user, guild) == Some(MemberRole::Owner),
                    "Somente o proprietário pode remover administradores",
                )?;
                let previous = self.workspace.clone();
                self.require_member_management(user, guild, target)?;
                self.workspace
                    .community
                    .assignments
                    .retain(|a| a.guild != guild || a.user != target);
                self.workspace
                    .memberships
                    .retain(|m| m.guild != guild || m.user != target);
                self.audit(
                    guild,
                    user,
                    Some(target),
                    if ban {
                        "Membro banido"
                    } else {
                        "Membro removido"
                    },
                    "",
                );
                if ban && !self.workspace.bans.contains(&(guild, target)) {
                    self.workspace.bans.push((guild, target));
                }
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            WorkspaceCommand::LeaveGuild { guild } => {
                require(
                    self.role(user, guild) != Some(MemberRole::Owner),
                    "Transfira a propriedade antes de sair",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .memberships
                    .retain(|m| m.guild != guild || m.user != user);
                self.workspace
                    .community
                    .assignments
                    .retain(|a| a.guild != guild || a.user != user);
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            _ => unreachable!("routed by workspacecommand"),
        }
        Ok(())
    }
}
