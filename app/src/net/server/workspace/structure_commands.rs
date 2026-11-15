use super::*;

impl State {
    pub(super) fn structure_commands(
        &mut self,
        user: UserId,
        command: WorkspaceCommand,
    ) -> std::io::Result<()> {
        match command {
            WorkspaceCommand::UpdateChannel {
                channel,
                name,
                topic,
                category: _,
                position: _,
                allowed,
            } => {
                // Placement (category and position) is owned by MoveChannel; the legacy text
                // category and position fields of this command are ignored.
                let ch = self
                    .channels
                    .iter()
                    .find(|c| c.id == channel)
                    .ok_or_else(|| std::io::Error::other("Canal não encontrado"))?;
                require(
                    ch.guild != 0 && self.guild_has(user, ch.guild, permission::MANAGE_CHANNELS),
                    "Você não pode alterar o canal",
                )?;
                require(
                    allowed.len() <= MAX_MEMBERS
                        && allowed.iter().all(|m| self.role(*m, ch.guild).is_some()),
                    "Lista de membros inválida",
                )?;
                let current = self
                    .workspace
                    .channel_settings
                    .iter()
                    .find(|s| s.channel == channel)
                    .map(|s| s.allowed.as_slice())
                    .unwrap_or(&[]);
                require(
                    current == allowed.as_slice()
                        || self.guild_has(user, ch.guild, permission::MANAGE_ROLES),
                    "Você não pode alterar permissões",
                )?;
                let name = clean(&name, 40);
                require(!name.is_empty(), "Nome vazio")?;
                let previous = self.workspace.clone();
                let old = self.channels.clone();
                let ch = self.channels.iter_mut().find(|c| c.id == channel).unwrap();
                ch.name = name;
                ch.topic = clean(&topic, 256);
                match self
                    .workspace
                    .channel_settings
                    .iter_mut()
                    .find(|c| c.channel == channel)
                {
                    Some(settings) => settings.allowed = allowed,
                    None => self.workspace.channel_settings.push(ChannelSettings {
                        channel,
                        allowed,
                        ..Default::default()
                    }),
                }
                if let Err(e) = self.persist_workspace(previous) {
                    self.channels = old;
                    return Err(e);
                }
                self.revoke_access();
                self.refresh_clients();
            }
            WorkspaceCommand::CreateCategory { guild, name } => {
                self.require_category_admin(user, guild)?;
                let name = clean(&name, 50);
                require(!name.is_empty(), "Nome vazio")?;
                require(
                    self.workspace
                        .categories
                        .iter()
                        .filter(|c| c.guild == guild)
                        .count()
                        < categories::MAX_CATEGORIES,
                    "Limite de categorias atingido",
                )?;
                require(
                    self.workspace.next_category < CategoryId::MAX
                        && self
                            .workspace
                            .categories
                            .iter()
                            .all(|c| c.id < CategoryId::MAX),
                    "Limite de categorias atingido",
                )?;
                let previous = self.workspace.clone();
                let ws = &mut self.workspace;
                categories::create_category(
                    &mut ws.categories,
                    &mut ws.next_category,
                    guild,
                    &name,
                );
                self.persist_workspace(previous)?;
                self.broadcast_categories(guild);
            }
            WorkspaceCommand::RenameCategory { category, name } => {
                let guild = self.category_guild(user, category)?;
                let name = clean(&name, 50);
                require(!name.is_empty(), "Nome vazio")?;
                let previous = self.workspace.clone();
                if let Some(c) = self
                    .workspace
                    .categories
                    .iter_mut()
                    .find(|c| c.id == category)
                {
                    c.name = name;
                }
                self.persist_workspace(previous)?;
                self.broadcast_categories(guild);
            }
            WorkspaceCommand::DeleteCategory { category } => {
                let guild = self.category_guild(user, category)?;
                let previous = self.workspace.clone();
                let ws = &mut self.workspace;
                categories::delete_category(
                    &self.channels,
                    &mut ws.channel_settings,
                    &mut ws.categories,
                    category,
                );
                self.workspace
                    .community
                    .overrides
                    .retain(|o| o.scope != PermissionScope::Category(category));
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_media_routes();
                self.broadcast_categories(guild);
                self.refresh_clients();
            }
            WorkspaceCommand::ReorderCategories { guild, order } => {
                self.require_category_admin(user, guild)?;
                require(order.len() <= MAX_CHANNELS, "Lista inválida")?;
                let previous = self.workspace.clone();
                if let Err(reason) =
                    categories::reorder_categories(&mut self.workspace.categories, guild, &order)
                {
                    self.workspace = previous;
                    return Err(std::io::Error::other(reason));
                }
                self.persist_workspace(previous)?;
                self.broadcast_categories(guild);
            }
            WorkspaceCommand::MoveChannel {
                channel,
                category,
                index,
            } => {
                let guild = self
                    .channels
                    .iter()
                    .find(|c| c.id == channel)
                    .map(|c| c.guild)
                    .ok_or_else(|| std::io::Error::other("Canal não encontrado"))?;
                require(guild != 0, "Canal inválido")?;
                self.require_category_admin(user, guild)?;
                let previous = self.workspace.clone();
                let ws = &mut self.workspace;
                if !categories::move_channel(
                    &self.channels,
                    &mut ws.channel_settings,
                    &ws.categories,
                    channel,
                    category,
                    index as usize,
                ) {
                    self.workspace = previous;
                    return Err(std::io::Error::other("Categoria inválida"));
                }
                self.persist_workspace(previous)?;
                self.revoke_access();
                self.refresh_media_routes();
                self.broadcast_categories(guild);
                self.refresh_clients();
            }
            WorkspaceCommand::CreateChannelIn {
                guild,
                name,
                kind,
                category,
            } => self.create_channel(user, guild, name, kind, category)?,
            WorkspaceCommand::DeleteChannel { channel } => {
                let ch = self
                    .channels
                    .iter()
                    .find(|c| c.id == channel)
                    .ok_or_else(|| std::io::Error::other("Canal não encontrado"))?;
                require(
                    ch.guild != 0 && self.guild_has(user, ch.guild, permission::MANAGE_CHANNELS),
                    "Sem permissão para apagar canal",
                )?;
                let old = self.channels.clone();
                let previous = self.workspace.clone();
                let mut removed: HashSet<_> = self
                    .workspace
                    .community
                    .threads
                    .iter()
                    .filter(|t| t.parent == channel)
                    .map(|t| t.channel)
                    .collect();
                removed.insert(channel);
                self.channels.retain(|c| !removed.contains(&c.id));
                self.workspace
                    .community
                    .threads
                    .retain(|t| !removed.contains(&t.channel));
                self.workspace
                    .community
                    .polls
                    .retain(|p| !removed.contains(&p.channel));
                self.workspace
                    .community
                    .events
                    .retain(|e| e.channel.is_none_or(|c| !removed.contains(&c)));
                self.workspace
                    .community
                    .policies
                    .retain(|p| !removed.contains(&p.channel));
                self.workspace.community.overrides.retain(
                    |o| !matches!(o.scope,PermissionScope::Channel(c) if removed.contains(&c)),
                );
                self.workspace
                    .channel_settings
                    .retain(|c| !removed.contains(&c.channel));
                self.workspace.read.retain(|(_, c, _)| !removed.contains(c));
                if let Err(e) = self.save_to_disk() {
                    self.channels = old;
                    self.workspace = previous;
                    return Err(e);
                }
                self.revoke_access();
                self.refresh_clients();
            }
            WorkspaceCommand::UpdateGuild { guild, name } => {
                require(
                    self.guild_has(user, guild, permission::MANAGE_GUILD),
                    "Somente administradores podem alterar a comunidade",
                )?;
                let name = clean(&name, 40);
                require(!name.is_empty(), "Nome vazio")?;
                let old = self.guilds.clone();
                let g = self
                    .guilds
                    .iter_mut()
                    .find(|g| g.id == guild)
                    .ok_or_else(|| std::io::Error::other("Comunidade não encontrada"))?;
                g.name = name;
                if let Err(e) = self.save_to_disk() {
                    self.guilds = old;
                    return Err(e);
                }
                self.refresh_clients();
            }
            WorkspaceCommand::DeleteGuild { guild } => {
                require(
                    guild != 1,
                    "A comunidade inicial do host não pode ser apagada",
                )?;
                require(
                    self.workspace.host_owner == Some(user)
                        || self.role(user, guild) == Some(MemberRole::Owner),
                    "Somente o proprietário pode apagar a comunidade",
                )?;
                let old = (
                    self.guilds.clone(),
                    self.channels.clone(),
                    self.workspace.clone(),
                );
                self.guilds.retain(|g| g.id != guild);
                self.channels.retain(|c| c.guild != guild);
                self.workspace.memberships.retain(|m| m.guild != guild);
                self.workspace.categories.retain(|c| c.guild != guild);
                self.workspace.invites.retain(|i| i.guild != guild);
                self.workspace.community.roles.retain(|r| r.guild != guild);
                self.workspace.community.emojis.retain(|e| e.guild != guild);
                self.workspace.community.events.retain(|e| e.guild != guild);
                self.workspace
                    .community
                    .assignments
                    .retain(|r| r.guild != guild);
                self.workspace
                    .community
                    .profile_extras
                    .retain(|r| r.guild != Some(guild));
                self.workspace
                    .community
                    .visuals
                    .retain(|r| r.guild != guild);
                self.workspace
                    .community
                    .timeouts
                    .retain(|r| r.guild != guild);
                let channels: HashSet<_> = self.channels.iter().map(|c| c.id).collect();
                let categories: HashSet<_> =
                    self.workspace.categories.iter().map(|c| c.id).collect();
                self.workspace
                    .community
                    .policies
                    .retain(|p| channels.contains(&p.channel));
                self.workspace
                    .community
                    .threads
                    .retain(|p| channels.contains(&p.channel) && channels.contains(&p.parent));
                self.workspace
                    .community
                    .overrides
                    .retain(|o| match o.scope {
                        PermissionScope::Channel(c) => channels.contains(&c),
                        PermissionScope::Category(c) => categories.contains(&c),
                    });

                if let Err(e) = self.save_to_disk() {
                    self.guilds = old.0;
                    self.channels = old.1;
                    self.workspace = old.2;
                    return Err(e);
                }
                self.revoke_access();
                self.refresh_clients();
            }
            _ => unreachable!("routed by workspacecommand"),
        }
        Ok(())
    }

    pub(in crate::net::server) fn create_community(
        &mut self,
        user: UserId,
        name: String,
    ) -> std::io::Result<()> {
        require(
            self.guilds.len() < MAX_COMMUNITIES && self.channels.len() + 2 < MAX_CHANNELS,
            "Limite de comunidades/canais atingido",
        )?;
        let name = clean(&name, 40);
        require(!name.is_empty(), "Nome vazio")?;
        let old = (
            self.guilds.clone(),
            self.channels.clone(),
            self.workspace.clone(),
            self.next_guild,
            self.next_channel,
        );
        let guild = self.next_guild;
        let next_guild = guild
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("Limite de comunidades atingido"))?;
        self.next_guild = next_guild;
        self.guilds.push(Guild { id: guild, name });
        self.workspace.memberships.push(Membership {
            guild,
            user,
            role: MemberRole::Owner,
        });
        for (kind, name) in [
            (ChannelKind::Text, "geral"),
            (ChannelKind::Voice, "Conversa"),
        ] {
            require(
                self.next_channel < CALL_ID_BASE,
                "Limite de canais atingido",
            )?;
            self.channels.push(Channel {
                id: self.next_channel,
                guild,
                name: name.into(),
                kind,
                topic: String::new(),
            });
            self.next_channel += 1;
        }
        if let Err(e) = self.save_to_disk() {
            self.guilds = old.0;
            self.channels = old.1;
            self.workspace = old.2;
            self.next_guild = old.3;
            self.next_channel = old.4;
            return Err(e);
        }
        self.refresh_clients();
        Ok(())
    }

    pub(in crate::net::server) fn create_channel(
        &mut self,
        user: UserId,
        guild: GuildId,
        name: String,
        kind: ChannelKind,
        category: Option<CategoryId>,
    ) -> std::io::Result<()> {
        require(
            self.guild_has(user, guild, permission::MANAGE_CHANNELS),
            "Você não pode criar canais",
        )?;
        require(
            self.channels.len() < MAX_CHANNELS && self.next_channel < CALL_ID_BASE,
            "Limite de canais atingido",
        )?;
        require(
            category.is_none_or(|id| {
                self.workspace
                    .categories
                    .iter()
                    .any(|c| c.id == id && c.guild == guild)
            }),
            "Categoria inválida",
        )?;
        let name = clean(&name, 40);
        require(!name.is_empty(), "Nome vazio")?;
        let id = self.next_channel;
        self.next_channel += 1;
        let previous = self.workspace.clone();
        self.channels.push(Channel {
            id,
            guild,
            name,
            kind,
            topic: String::new(),
        });
        let ws = &mut self.workspace;
        categories::place_new_channel(
            &self.channels,
            &mut ws.channel_settings,
            &ws.categories,
            id,
            category,
        );
        if let Err(e) = self.save_to_disk() {
            self.channels.pop();
            self.next_channel = id;
            self.workspace = previous;
            return Err(e);
        }
        self.refresh_clients();
        self.broadcast_categories(guild);
        Ok(())
    }
}
