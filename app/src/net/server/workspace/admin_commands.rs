use super::*;

impl State {
    pub(super) fn admin_commands(
        &mut self,
        user: UserId,
        command: CommunityCommand,
    ) -> std::io::Result<()> {
        match command {
            CommunityCommand::StageStatus { channel } => {
                self.require_stage_access(user, channel)?;
                self.send_stage_state(user, channel);
            }
            CommunityCommand::StageRequest { channel, requested } => {
                self.require_stage_access(user, channel)?;
                require(
                    self.voice.get(&user).and_then(|v| v.channel) == Some(channel),
                    "Entre no palco primeiro",
                )?;
                if requested {
                    self.stage_requests.entry(channel).or_default().insert(user);
                } else if let Some(requests) = self.stage_requests.get_mut(&channel) {
                    requests.remove(&user);
                }
                self.broadcast_stage_state(channel);
            }
            CommunityCommand::StageSpeaker {
                channel,
                user: target,
                allowed,
            } => {
                self.require_stage_access(user, channel)?;
                require(
                    self.has_permission(user, channel, permission::MANAGE_MESSAGES),
                    "Você não pode moderar o palco",
                )?;
                require(
                    self.voice.get(&user).and_then(|v| v.channel) == Some(channel)
                        && self.voice.get(&target).and_then(|v| v.channel) == Some(channel),
                    "As pessoas precisam estar no palco",
                )?;
                require(
                    !allowed
                        || (self.has_permission(target, channel, permission::SPEAK)
                            && !self.is_timed_out(target, channel)),
                    "A pessoa não tem permissão para falar",
                )?;
                if allowed {
                    self.stage_speakers
                        .entry(channel)
                        .or_default()
                        .insert(target);
                } else if let Some(speakers) = self.stage_speakers.get_mut(&channel) {
                    speakers.remove(&target);
                }
                if let Some(requests) = self.stage_requests.get_mut(&channel) {
                    requests.remove(&target);
                }
                self.refresh_media_routes();
                self.broadcast_stage_state(channel);
            }
            CommunityCommand::UpsertRole { mut role } => {
                require(
                    self.guild_has(user, role.guild, permission::MANAGE_ROLES),
                    "Você não pode gerenciar cargos",
                )?;
                // Position zero is reserved for the community-wide base role.
                let is_base = role.position == 0;
                if is_base {
                    require(
                        self.workspace.host_owner == Some(user)
                            || self.role(user, role.guild) == Some(MemberRole::Owner),
                        "Somente o proprietário pode alterar as permissões básicas",
                    )?;
                    require(
                        role.permissions & !permission::MEMBER == 0,
                        "As permissões básicas só podem incluir ações de membro",
                    )?;
                    role.name = "Todos".into();
                    require(
                        !self
                            .workspace
                            .community
                            .roles
                            .iter()
                            .any(|r| r.guild == role.guild && r.position == 0 && r.id != role.id),
                        "Já existe uma configuração de permissões básicas",
                    )?;
                }
                role.name = clean(&role.name, 50);
                require(!role.name.is_empty(), "Nome de cargo vazio")?;
                role.permissions &= permission::ALL;
                require(
                    role.permissions & !self.guild_permissions(user, role.guild) == 0,
                    "Não é possível conceder permissões que você não possui",
                )?;
                require(
                    (is_base || role.position > 0)
                        && role.position < self.top_role(user, role.guild),
                    "Posição acima da sua hierarquia",
                )?;
                let old = self
                    .workspace
                    .community
                    .roles
                    .iter()
                    .find(|r| r.id == role.id);
                if role.id != 0 {
                    require(
                        old.is_some_and(|r| {
                            r.guild == role.guild
                                && (r.position == 0) == is_base
                                && r.position < self.top_role(user, role.guild)
                        }),
                        "Cargo protegido ou inexistente",
                    )?;
                } else {
                    require(
                        self.workspace.community.roles.len() < 1024,
                        "Limite de cargos atingido",
                    )?;
                    role.id = self
                        .workspace
                        .next_role
                        .max(
                            self.workspace
                                .community
                                .roles
                                .iter()
                                .map(|r| r.id)
                                .max()
                                .unwrap_or(0)
                                .checked_add(1)
                                .ok_or_else(|| std::io::Error::other("Limite de cargos"))?,
                        )
                        .max(1);
                    require(role.id < RoleId::MAX, "Limite de cargos atingido")?;
                }
                let previous = self.workspace.clone();
                self.workspace.next_role = self.workspace.next_role.max(role.id.saturating_add(1));
                self.workspace.community.roles.retain(|r| r.id != role.id);
                let guild = role.guild;
                self.audit(guild, user, None, "Cargo atualizado", &role.name);
                self.workspace.community.roles.push(role);
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            CommunityCommand::DeleteRole { guild, role } => {
                require(
                    self.guild_has(user, guild, permission::MANAGE_ROLES),
                    "Você não pode gerenciar cargos",
                )?;
                require(
                    self.workspace.community.roles.iter().any(|r| {
                        r.id == role
                            && r.guild == guild
                            && r.position > 0
                            && r.position < self.top_role(user, guild)
                    }),
                    "Cargo protegido ou inexistente",
                )?;
                let previous = self.workspace.clone();
                self.workspace.community.roles.retain(|r| r.id != role);
                for a in &mut self.workspace.community.assignments {
                    a.roles.retain(|r| *r != role);
                }
                self.workspace
                    .community
                    .overrides
                    .retain(|o| o.target != PermissionTarget::Role(role));
                self.audit(guild, user, None, "Cargo excluído", "");
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            CommunityCommand::AssignRoles {
                guild,
                user: target,
                mut roles,
            } => {
                require(
                    self.guild_has(user, guild, permission::MANAGE_ROLES)
                        && self.role(target, guild).is_some(),
                    "Você não pode atribuir cargos",
                )?;
                require(
                    target != user || self.administer(user, guild),
                    "Você não pode alterar seus próprios cargos",
                )?;
                require(
                    self.administer(user, guild)
                        || self.top_role(target, guild) < self.top_role(user, guild),
                    "Membro acima da sua hierarquia",
                )?;
                roles.sort_unstable();
                roles.dedup();
                require(roles.len() <= 32, "Limite de cargos por membro")?;
                require(
                    roles.iter().all(|id| {
                        self.workspace.community.roles.iter().any(|r| {
                            r.id == *id
                                && r.position > 0
                                && r.guild == guild
                                && r.position < self.top_role(user, guild)
                        })
                    }),
                    "Cargo inválido ou protegido",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .assignments
                    .retain(|a| a.guild != guild || a.user != target);
                self.workspace.community.assignments.push(RoleAssignment {
                    guild,
                    user: target,
                    roles,
                });
                self.audit(guild, user, Some(target), "Cargos atribuídos", "");
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            CommunityCommand::SetOverride { mut value } => {
                let guild = self.scope_guild(value.scope)?;
                require(
                    self.guild_has(user, guild, permission::MANAGE_ROLES),
                    "Você não pode alterar permissões",
                )?;
                match value.target {
                    PermissionTarget::Role(id) => require(
                        self.workspace.community.roles.iter().any(|r| {
                            r.guild == guild
                                && r.id == id
                                && r.position > 0
                                && r.position < self.top_role(user, guild)
                        }),
                        "Cargo inválido ou protegido",
                    )?,
                    PermissionTarget::Member(id) => {
                        require(self.role(id, guild).is_some(), "Membro não encontrado")?
                    }
                    PermissionTarget::Everyone => {}
                }
                value.allow &= permission::ALL;
                value.deny &= permission::ALL;
                require(
                    value.allow & value.deny == 0,
                    "Permissão simultaneamente permitida e negada",
                )?;
                require(
                    (value.allow | value.deny) & !self.guild_permissions(user, guild) == 0,
                    "Permissões acima das suas capacidades",
                )?;
                require(
                    self.workspace.community.overrides.len() < 8192
                        || self
                            .workspace
                            .community
                            .overrides
                            .iter()
                            .any(|o| o.scope == value.scope && o.target == value.target),
                    "Limite de regras atingido",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .overrides
                    .retain(|o| o.scope != value.scope || o.target != value.target);
                self.workspace.community.overrides.push(value);
                self.audit(guild, user, None, "Permissões alteradas", "");
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            CommunityCommand::RemoveOverride { scope, target } => {
                let guild = self.scope_guild(scope)?;
                require(
                    self.guild_has(user, guild, permission::MANAGE_ROLES),
                    "Você não pode alterar permissões",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .overrides
                    .retain(|o| o.scope != scope || o.target != target);
                self.audit(guild, user, None, "Regra de permissão removida", "");
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            CommunityCommand::UpdateProfileExtra {
                guild,
                bio,
                nickname,
            } => {
                require(
                    guild.is_none_or(|g| self.role(user, g).is_some()),
                    "Comunidade não autorizada",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .profile_extras
                    .retain(|p| p.user != user || p.guild != guild);
                self.workspace.community.profile_extras.push(ProfileExtra {
                    user,
                    guild,
                    bio: clean(&bio, 300),
                    nickname: clean(&nickname, 32),
                });
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::UpdateVisual {
                guild,
                icon,
                banner,
            } => {
                require(
                    self.guild_has(user, guild, permission::MANAGE_GUILD),
                    "Você não pode editar esta comunidade",
                )?;
                validate_visual(&icon, 256 * 1024, 1024)?;
                validate_visual(&banner, 512 * 1024, 2048)?;
                let total = self
                    .workspace
                    .community
                    .visuals
                    .iter()
                    .filter(|v| v.guild != guild)
                    .map(|v| v.icon.len() + v.banner.len())
                    .sum::<usize>();
                require(
                    total + icon.len() + banner.len() <= 8 * 1024 * 1024,
                    "Limite de imagens das comunidades atingido",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .visuals
                    .retain(|v| v.guild != guild);
                self.workspace.community.visuals.push(CommunityVisual {
                    guild,
                    icon,
                    banner,
                });
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::UpdateConversationIcon { channel, icon } => {
                require(
                    self.workspace
                        .conversations
                        .iter()
                        .any(|c| c.channel == channel && c.owner == user && !c.direct),
                    "Somente o responsável pelo grupo pode alterar a imagem",
                )?;
                validate_visual(&icon, 128 * 1024, 1024)?;
                let total = self
                    .workspace
                    .community
                    .conversation_visuals
                    .iter()
                    .filter(|v| v.channel != channel)
                    .map(|v| v.icon.len())
                    .sum::<usize>();
                require(
                    total + icon.len() <= 4 * 1024 * 1024,
                    "Limite de imagens de grupos atingido",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .conversation_visuals
                    .retain(|v| v.channel != channel);
                self.workspace
                    .community
                    .conversation_visuals
                    .push(ConversationVisual { channel, icon });
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::SetChannelPolicy { mut policy } => {
                let guild = self.scope_guild(PermissionScope::Channel(policy.channel))?;
                require(
                    self.guild_has(user, guild, permission::MANAGE_CHANNELS),
                    "Você não pode gerenciar canais",
                )?;
                policy.slowmode_secs = policy.slowmode_secs.min(21600);
                let kind = self
                    .channels
                    .iter()
                    .find(|c| c.id == policy.channel)
                    .unwrap()
                    .kind;
                require(
                    (policy.style == ChannelStyle::Stage && kind == ChannelKind::Voice)
                        || (policy.style != ChannelStyle::Stage && kind == ChannelKind::Text),
                    "Tipo incompatível com o canal",
                )?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .policies
                    .retain(|p| p.channel != policy.channel);
                self.workspace.community.policies.push(policy);
                self.audit(guild, user, None, "Política do canal alterada", "");
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::Timeout {
                guild,
                user: target,
                until,
                reason,
            } => {
                self.require_member_management(user, guild, target)?;
                require(until <= now_secs() + 28 * 86400, "Duração máxima: 28 dias")?;
                let previous = self.workspace.clone();
                self.workspace
                    .community
                    .timeouts
                    .retain(|t| t.guild != guild || t.user != target);
                if until > now_secs() {
                    self.workspace.community.timeouts.push(MemberTimeout {
                        guild,
                        user: target,
                        until,
                        reason: clean(&reason, 300),
                    });
                }
                self.audit(
                    guild,
                    user,
                    Some(target),
                    if until > now_secs() {
                        "Timeout aplicado"
                    } else {
                        "Timeout removido"
                    },
                    &reason,
                );
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_clients();
            }
            CommunityCommand::Unban {
                guild,
                user: target,
            } => {
                require(
                    self.guild_has(user, guild, permission::MODERATE),
                    "Você não pode remover banimentos",
                )?;
                let previous = self.workspace.clone();
                self.workspace.bans.retain(|b| *b != (guild, target));
                self.audit(guild, user, Some(target), "Banimento removido", "");
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::Moderation { guild } => {
                require(
                    self.guild_has(user, guild, permission::MODERATE),
                    "Moderação não autorizada",
                )?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::Moderation {
                        guild,
                        bans: self
                            .workspace
                            .bans
                            .iter()
                            .filter(|(g, _)| *g == guild)
                            .map(|(_, u)| BanInfo { guild, user: *u })
                            .collect(),
                        timeouts: self
                            .workspace
                            .community
                            .timeouts
                            .iter()
                            .filter(|t| t.guild == guild && t.until > now_secs())
                            .cloned()
                            .collect(),
                        audit: self
                            .workspace
                            .audit
                            .iter()
                            .rev()
                            .filter(|a| a.guild == guild)
                            .take(200)
                            .cloned()
                            .collect(),
                    })),
                );
            }
            CommunityCommand::Invites { guild } => {
                require(
                    self.guild_has(user, guild, permission::INVITE),
                    "Convites não autorizados",
                )?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::Invites {
                        guild,
                        invites: self
                            .workspace
                            .invites
                            .iter()
                            .filter(|i| {
                                i.guild == guild && i.expires > now_secs() && i.remaining > 0
                            })
                            .map(|i| InviteInfo {
                                guild,
                                creator: i.creator,
                                code: i.code.clone(),
                                expires: i.expires,
                                remaining: i.remaining,
                            })
                            .collect(),
                    })),
                );
            }
            _ => unreachable!("routed by communitycommand"),
        }
        Ok(())
    }
}
