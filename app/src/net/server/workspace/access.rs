use super::*;

impl State {
    /// An owner configured with a key that was later rotated must follow the
    /// committed rotation; the old key is revoked and would lock the owner out.
    pub(in crate::net::server) fn follow_owner_rotation(&mut self) {
        for _ in 0..=self.workspace.rotations.len() {
            let Some(key) = self.options.owner_key else {
                return;
            };
            match self.workspace.rotations.iter().find(|r| r.old == key) {
                Some(receipt) if receipt.new != key => self.options.owner_key = Some(receipt.new),
                _ => return,
            }
        }
    }

    pub(in crate::net::server) fn is_revoked(&self, key: &[u8; 32]) -> bool {
        self.options.revoked_keys.contains(key) || self.workspace.revoked_keys.contains(key)
    }

    pub(in crate::net::server) fn initialize_memberships(&mut self) {
        if self.workspace.initialized {
            return;
        }
        // Preserve access for identities already present in the legacy JSON.
        for guild in &self.guilds {
            for user in self.users.keys() {
                self.workspace.memberships.push(Membership {
                    guild: guild.id,
                    user: *user,
                    role: MemberRole::Member,
                });
            }
        }
        self.workspace.host_owner = if let Some(key) = self.options.owner_key {
            self.public_keys
                .iter()
                .find(|(_, k)| **k == key)
                .map(|(id, _)| *id)
        } else if self.options.allow_registration {
            self.users.keys().min().copied()
        } else {
            None
        };
        if let Some(owner) = self.workspace.host_owner {
            for m in &mut self.workspace.memberships {
                if m.user == owner {
                    m.role = MemberRole::Owner;
                }
            }
        }
        self.workspace.initialized = true;
    }

    pub(in crate::net::server) fn admit_user(&mut self, user: UserId) {
        let owns = self
            .options
            .owner_key
            .is_some_and(|key| self.public_keys.get(&user) == Some(&key));
        if owns || (self.options.allow_registration && self.workspace.host_owner.is_none()) {
            self.workspace.host_owner = Some(user);
        }
        if (owns || !self.users.contains_key(&user))
            && !self
                .workspace
                .memberships
                .iter()
                .any(|m| m.user == user && m.guild == 1)
            && !self.workspace.bans.contains(&(1, user))
        {
            self.workspace.memberships.push(Membership {
                guild: 1,
                user,
                role: if self.workspace.host_owner == Some(user) {
                    MemberRole::Owner
                } else {
                    MemberRole::Member
                },
            });
        }
    }

    pub(super) fn role(&self, user: UserId, guild: GuildId) -> Option<MemberRole> {
        self.workspace
            .memberships
            .iter()
            .find(|m| m.user == user && m.guild == guild)
            .map(|m| m.role)
    }

    pub(in crate::net::server) fn administer(&self, user: UserId, guild: GuildId) -> bool {
        self.workspace.host_owner == Some(user)
            || matches!(
                self.role(user, guild),
                Some(MemberRole::Owner | MemberRole::Admin)
            )
    }

    pub(in crate::net::server) fn can_see_voice(
        &self,
        viewer: UserId,
        user: UserId,
        state: &VoiceState,
    ) -> bool {
        viewer == user
            || state.channel.is_none_or(|channel| {
                if channel < CALL_ID_BASE {
                    self.can_access(viewer, channel)
                } else {
                    self.calls
                        .get(&channel)
                        .is_some_and(|c| c.members.contains(&viewer))
                }
            })
    }

    pub(in crate::net::server) fn can_access(&self, user: UserId, channel: ChannelId) -> bool {
        if let Some(c) = self
            .workspace
            .conversations
            .iter()
            .find(|c| c.channel == channel)
        {
            return c.members.contains(&user);
        }
        let Some(ch) = self.channels.iter().find(|c| c.id == channel) else {
            return false;
        };
        if !self.workspace.initialized {
            return true;
        }
        if self.workspace.bans.contains(&(ch.guild, user)) {
            return false;
        }
        self.effective_permissions(user, channel) & permission::VIEW != 0
    }

    pub(super) fn can_moderate(&self, user: UserId, channel: ChannelId) -> bool {
        if let Some(c) = self
            .workspace
            .conversations
            .iter()
            .find(|c| c.channel == channel)
        {
            return c.owner == user;
        }
        self.channels
            .iter()
            .find(|c| c.id == channel)
            .is_some_and(|_| self.has_permission(user, channel, permission::MANAGE_MESSAGES))
    }

    pub(super) fn revoke_access(&mut self) {
        let revoke: Vec<_> = self
            .voice
            .iter()
            .filter(|(u, v)| {
                v.channel.is_some_and(|c| {
                    c < CALL_ID_BASE
                        && (!self.can_access(**u, c)
                            || !self.has_permission(**u, c, permission::CONNECT)
                            || self.is_timed_out(**u, c))
                })
            })
            .map(|(u, _)| *u)
            .collect();
        for user in revoke {
            self.leave_voice(user);
        }
        let stop: Vec<_> = self
            .streams
            .keys()
            .copied()
            .filter(|(u, _)| {
                self.voice.get(u).and_then(|v| v.channel).is_some_and(|c| {
                    c < CALL_ID_BASE && !self.has_permission(*u, c, permission::STREAM)
                })
            })
            .collect();
        for (user, kind) in stop {
            self.stop_stream(user, kind);
        }
    }

    pub(super) fn require_category_admin(
        &self,
        user: UserId,
        guild: GuildId,
    ) -> std::io::Result<()> {
        require(
            guild != 0 && self.guilds.iter().any(|g| g.id == guild),
            "Comunidade não encontrada",
        )?;
        // Same permission as editing or creating channels.
        require(
            self.guild_has(user, guild, permission::MANAGE_CHANNELS),
            "Você não pode gerenciar categorias",
        )
    }

    pub(super) fn category_guild(
        &self,
        user: UserId,
        category: CategoryId,
    ) -> std::io::Result<GuildId> {
        let guild = self
            .workspace
            .categories
            .iter()
            .find(|c| c.id == category)
            .map(|c| c.guild)
            .ok_or_else(|| std::io::Error::other("Categoria não encontrada"))?;
        self.require_category_admin(user, guild)?;
        Ok(guild)
    }

    pub(super) fn guild_permissions(&self, user: UserId, guild: GuildId) -> u64 {
        if self.administer(user, guild) {
            return permission::ALL;
        }
        if self.role(user, guild).is_none() || self.workspace.bans.contains(&(guild, user)) {
            return 0;
        }
        let assigned = self
            .workspace
            .community
            .assignments
            .iter()
            .find(|a| a.guild == guild && a.user == user);
        let base = self
            .workspace
            .community
            .roles
            .iter()
            .find(|r| r.guild == guild && r.position == 0)
            .map(|r| r.permissions)
            .unwrap_or(permission::MEMBER);
        self.workspace
            .community
            .roles
            .iter()
            .filter(|r| r.guild == guild && assigned.is_some_and(|a| a.roles.contains(&r.id)))
            .fold(base, |p, r| p | r.permissions)
            & permission::ALL
    }

    pub(super) fn guild_has(&self, user: UserId, guild: GuildId, permission: u64) -> bool {
        self.guild_permissions(user, guild) & permission == permission
    }

    pub(super) fn top_role(&self, user: UserId, guild: GuildId) -> i32 {
        if self.administer(user, guild) {
            return i32::MAX;
        }
        let assigned = self
            .workspace
            .community
            .assignments
            .iter()
            .find(|a| a.guild == guild && a.user == user);
        self.workspace
            .community
            .roles
            .iter()
            .filter(|r| r.guild == guild && assigned.is_some_and(|a| a.roles.contains(&r.id)))
            .map(|r| r.position)
            .max()
            .unwrap_or(0)
    }

    pub(super) fn scope_guild(&self, scope: PermissionScope) -> std::io::Result<GuildId> {
        match scope {
            PermissionScope::Channel(id) => self
                .channels
                .iter()
                .find(|c| c.id == id && c.guild != 0)
                .map(|c| c.guild),
            PermissionScope::Category(id) => self
                .workspace
                .categories
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.guild),
        }
        .ok_or_else(|| std::io::Error::other("Destino não encontrado"))
    }

    /// Everyone -> combined roles -> individual member, category before channel.
    /// Explicit channel overrides refine category inheritance; owner/admin bypass them.
    pub(in crate::net::server) fn effective_permissions(
        &self,
        user: UserId,
        channel: ChannelId,
    ) -> u64 {
        if let Some(c) = self
            .workspace
            .conversations
            .iter()
            .find(|c| c.channel == channel)
        {
            return if c.members.contains(&user) {
                permission::MEMBER
            } else {
                0
            };
        }
        let Some(ch) = self.channels.iter().find(|c| c.id == channel) else {
            return 0;
        };
        if !self.workspace.initialized {
            return permission::MEMBER;
        }
        if self.workspace.bans.contains(&(ch.guild, user)) {
            return 0;
        }
        if self.administer(user, ch.guild) {
            return permission::ALL;
        }
        if self.role(user, ch.guild).is_none() {
            return 0;
        }
        let inherited = self
            .workspace
            .community
            .threads
            .iter()
            .find(|t| t.channel == channel)
            .map(|t| t.parent)
            .unwrap_or(channel);
        if inherited != channel
            && (self
                .workspace
                .community
                .threads
                .iter()
                .any(|t| t.channel == inherited)
                || self.effective_permissions(user, inherited) & permission::VIEW == 0)
        {
            return 0;
        }
        let settings = self
            .workspace
            .channel_settings
            .iter()
            .find(|c| c.channel == inherited);
        if settings.is_some_and(|s| !s.allowed.is_empty() && !s.allowed.contains(&user)) {
            return 0;
        }
        let mut result = self.guild_permissions(user, ch.guild);
        let roles = self
            .workspace
            .community
            .assignments
            .iter()
            .find(|a| a.guild == ch.guild && a.user == user);
        let mut scopes = Vec::new();
        if let Some(id) = settings.and_then(|s| s.category_id) {
            scopes.push(PermissionScope::Category(id));
        }
        scopes.push(PermissionScope::Channel(inherited));
        if inherited != channel {
            scopes.push(PermissionScope::Channel(channel));
        }
        for scope in scopes {
            let values: Vec<_> = self
                .workspace
                .community
                .overrides
                .iter()
                .filter(|o| o.scope == scope)
                .collect();
            for tier in 0..3 {
                let mut allow = 0;
                let mut deny = 0;
                for o in &values {
                    let matches = match (tier, o.target) {
                        (0, PermissionTarget::Everyone) => true,
                        (1, PermissionTarget::Role(id)) => {
                            roles.is_some_and(|r| r.roles.contains(&id))
                        }
                        (2, PermissionTarget::Member(id)) => id == user,
                        _ => false,
                    };
                    if matches {
                        allow |= o.allow;
                        deny |= o.deny;
                    }
                }
                result = (result & !deny) | allow;
            }
        }
        result & permission::ALL
    }

    pub(in crate::net::server) fn has_permission(
        &self,
        user: UserId,
        channel: ChannelId,
        permission: u64,
    ) -> bool {
        let value = self.effective_permissions(user, channel);
        value & permission::VIEW != 0 && value & permission == permission
    }

    pub(in crate::net::server) fn can_transmit_voice(
        &self,
        user: UserId,
        channel: ChannelId,
    ) -> bool {
        self.has_permission(user, channel, permission::SPEAK)
            && !self.is_timed_out(user, channel)
            && (!self
                .workspace
                .community
                .policies
                .iter()
                .any(|p| p.channel == channel && p.style == ChannelStyle::Stage)
                || self.has_permission(user, channel, permission::MANAGE_MESSAGES)
                || self
                    .stage_speakers
                    .get(&channel)
                    .is_some_and(|s| s.contains(&user)))
    }

    pub(in crate::net::server) fn require_stage_access(
        &self,
        user: UserId,
        channel: ChannelId,
    ) -> std::io::Result<()> {
        require(
            self.can_access(user, channel)
                && self
                    .workspace
                    .community
                    .policies
                    .iter()
                    .any(|p| p.channel == channel && p.style == ChannelStyle::Stage),
            "Palco indisponível",
        )
    }

    pub(in crate::net::server) fn send_stage_state(&self, user: UserId, channel: ChannelId) {
        if self.require_stage_access(user, channel).is_err() {
            return;
        }
        let mut speakers: Vec<_> = self
            .stage_speakers
            .get(&channel)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        let mut requests: Vec<_> = self
            .stage_requests
            .get(&channel)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        speakers.sort_unstable();
        requests.sort_unstable();
        self.send_to(
            user,
            &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::StageState {
                channel,
                speakers,
                requests,
            })),
        );
    }

    pub(in crate::net::server) fn broadcast_stage_state(&self, channel: ChannelId) {
        for user in self.clients.keys() {
            self.send_stage_state(*user, channel);
        }
    }

    pub(in crate::net::server) fn is_timed_out(&self, user: UserId, channel: ChannelId) -> bool {
        self.channels
            .iter()
            .find(|c| c.id == channel)
            .is_some_and(|ch| {
                self.workspace
                    .community
                    .timeouts
                    .iter()
                    .any(|t| t.guild == ch.guild && t.user == user && t.until > now_secs())
            })
    }

    pub(super) fn require_write(&self, user: UserId, channel: ChannelId) -> std::io::Result<()> {
        require(
            self.can_access(user, channel) && self.has_permission(user, channel, permission::SEND),
            "Você não pode enviar mensagens neste canal",
        )?;
        require(
            !self.is_timed_out(user, channel),
            "Você está temporariamente impedido de participar",
        )?;
        let admin = self
            .channels
            .iter()
            .find(|c| c.id == channel)
            .is_some_and(|c| self.administer(user, c.guild));
        require(
            !self
                .workspace
                .community
                .threads
                .iter()
                .any(|t| t.channel == channel && t.archived),
            "Esta discussão está arquivada",
        )?;
        if let Some(p) = self
            .workspace
            .community
            .policies
            .iter()
            .find(|p| p.channel == channel)
        {
            require(
                p.style != ChannelStyle::Forum,
                "Crie uma publicação para conversar neste fórum",
            )?;
            require(
                p.style != ChannelStyle::Announcement
                    || self.has_permission(user, channel, permission::MANAGE_MESSAGES),
                "Somente moderadores podem publicar anúncios",
            )?;
            if !admin && p.slowmode_secs > 0 {
                require(
                    !self.workspace.last_sends.iter().any(|(u, c, t)| {
                        *u == user && *c == channel && now_secs() - *t < i64::from(p.slowmode_secs)
                    }),
                    "Modo lento ativo. Aguarde antes de enviar outra mensagem",
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn require_member_management(
        &self,
        actor: UserId,
        guild: GuildId,
        target: UserId,
    ) -> std::io::Result<()> {
        require(
            self.guild_has(actor, guild, permission::MODERATE),
            "Você não pode moderar membros",
        )?;
        require(
            actor != target
                && self.role(target, guild).is_some()
                && self.workspace.host_owner != Some(target)
                && self.role(target, guild) != Some(MemberRole::Owner),
            "Membro protegido ou inválido",
        )?;
        require(
            self.role(actor, guild) == Some(MemberRole::Owner)
                || self.workspace.host_owner == Some(actor)
                || (!self.administer(target, guild)
                    && self.top_role(actor, guild) > self.top_role(target, guild)),
            "O membro tem cargo igual ou superior",
        )
    }

    pub(super) fn audit(
        &mut self,
        guild: GuildId,
        actor: UserId,
        target: Option<UserId>,
        action: &str,
        reason: &str,
    ) {
        let id = self
            .workspace
            .audit
            .last()
            .map_or(1, |a| a.id.saturating_add(1));
        self.workspace.audit.push(AuditEntry {
            id,
            guild,
            actor,
            target,
            action: clean(action, 100),
            reason: clean(reason, 300),
            time: now_secs(),
        });
        if self.workspace.audit.len() > 1000 {
            self.workspace
                .audit
                .drain(..self.workspace.audit.len() - 1000);
        }
    }
}
