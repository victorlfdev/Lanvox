use super::*;

impl State {
    pub(super) fn workspace_snapshot(&self, user: UserId) -> WorkspaceSnapshot {
        let visible: HashSet<_> = self
            .channels
            .iter()
            .filter(|c| self.can_access(user, c.id))
            .map(|c| c.id)
            .collect();
        WorkspaceSnapshot {
            conversations: self
                .workspace
                .conversations
                .iter()
                .filter(|c| c.members.contains(&user))
                .cloned()
                .collect(),
            memberships: self
                .workspace
                .memberships
                .iter()
                .filter(|m| {
                    self.role(user, m.guild).is_some() || self.workspace.host_owner == Some(user)
                })
                .cloned()
                .collect(),
            channel_settings: self
                .workspace
                .channel_settings
                .iter()
                .filter(|s| visible.contains(&s.channel))
                .cloned()
                .collect(),
            categories: self
                .workspace
                .categories
                .iter()
                .filter(|c| {
                    self.role(user, c.guild).is_some() || self.workspace.host_owner == Some(user)
                })
                .cloned()
                .collect(),
            profiles: self.workspace.profiles.clone(),
            details: self.details_for(
                &self
                    .messages
                    .iter()
                    .filter(|m| visible.contains(&m.channel))
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            read: self
                .workspace
                .read
                .iter()
                .filter(|(u, _, _)| *u == user)
                .map(|(_, c, m)| (*c, *m))
                .collect(),
            host_owner: self.workspace.host_owner,
            community: Box::new(self.community_snapshot(user, &visible)),
            disabled_users: if self.workspace.host_owner == Some(user) {
                self.public_keys
                    .iter()
                    .filter(|(_, key)| self.is_revoked(key))
                    .map(|(id, _)| *id)
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    pub(in crate::net::server) fn send_workspace(&self, user: UserId) {
        self.send_to(
            user,
            &ServerMsg::Workspace(WorkspaceEvent::State(self.workspace_snapshot(user))),
        );
    }
    pub(super) fn refresh_clients(&mut self) {
        self.revoke_access();
        self.refresh_media_routes();
        for user in self.clients.keys() {
            self.send_to(
                *user,
                &ServerMsg::Workspace(WorkspaceEvent::Resync {
                    snapshot: self.snapshot(*user),
                }),
            );
            self.send_workspace(*user);
        }
    }

    /// Tells every member of `guild` the current categories and channel placement.
    pub(super) fn broadcast_categories(&self, guild: GuildId) {
        let categories: Vec<Category> = self
            .workspace
            .categories
            .iter()
            .filter(|c| c.guild == guild)
            .cloned()
            .collect();
        for user in self.clients.keys() {
            if self.role(*user, guild).is_none() && self.workspace.host_owner != Some(*user) {
                continue;
            }
            let channel_settings = self
                .workspace
                .channel_settings
                .iter()
                .filter(|s| {
                    self.channels
                        .iter()
                        .any(|c| c.id == s.channel && c.guild == guild)
                        && self.can_access(*user, s.channel)
                })
                .cloned()
                .collect();
            self.send_to(
                *user,
                &ServerMsg::Workspace(WorkspaceEvent::CategoriesChanged {
                    guild,
                    categories: categories.clone(),
                    channel_settings,
                }),
            );
        }
    }

    pub(super) fn unread_summary_for(&self, user: UserId, channel: ChannelId) -> UnreadSummary {
        let after = self
            .workspace
            .read
            .iter()
            .find(|(u, ch, _)| *u == user && *ch == channel)
            .map(|(_, _, m)| *m)
            .unwrap_or(0);
        if let Some(store) = &self.store {
            if let Ok(summary) = store.lock().unread_summary(user, channel, after) {
                return summary;
            }
        }
        let list: Vec<_> = self
            .messages
            .iter()
            .filter(|m| m.channel == channel && m.id > after && m.author != user)
            .collect();
        UnreadSummary {
            channel,
            count: list.len() as u32,
            mentions: list
                .iter()
                .filter(|m| {
                    self.workspace
                        .details
                        .iter()
                        .any(|d| d.message == m.id && d.mentions.contains(&user))
                })
                .count() as u32,
            first: list.iter().map(|m| m.id).min(),
            latest: list.iter().map(|m| m.id).max(),
        }
    }

    pub(super) fn unread_summaries(&self, user: UserId) -> Vec<UnreadSummary> {
        self.channels
            .iter()
            .filter(|c| self.can_access(user, c.id))
            .map(|c| self.unread_summary_for(user, c.id))
            .collect()
    }

    pub(super) fn send_unread(&self, user: UserId) {
        self.send_to(
            user,
            &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::Unread(
                self.unread_summaries(user),
            ))),
        );
    }

    pub(super) fn broadcast_unread(&self, channel: ChannelId) {
        for user in self
            .clients
            .keys()
            .filter(|u| self.can_access(**u, channel))
        {
            let summary = self.unread_summary_for(*user, channel);
            self.send_to(
                *user,
                &ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::Unread(vec![
                    summary,
                ]))),
            );
        }
    }

    pub(super) fn community_snapshot(
        &self,
        user: UserId,
        visible: &HashSet<ChannelId>,
    ) -> CommunitySnapshot {
        let member = |g| self.role(user, g).is_some() || self.workspace.host_owner == Some(user);
        let c = &self.workspace.community;
        CommunitySnapshot {
            roles: c
                .roles
                .iter()
                .filter(|r| member(r.guild))
                .cloned()
                .collect(),
            assignments: c
                .assignments
                .iter()
                .filter(|a| member(a.guild))
                .cloned()
                .collect(),
            overrides: c
                .overrides
                .iter()
                .filter(|o| self.scope_guild(o.scope).is_ok_and(member))
                .cloned()
                .collect(),
            profile_extras: c
                .profile_extras
                .iter()
                .filter(|p| p.guild.is_none_or(member))
                .cloned()
                .collect(),
            visuals: c
                .visuals
                .iter()
                .filter(|v| member(v.guild))
                .cloned()
                .collect(),
            conversation_visuals: c
                .conversation_visuals
                .iter()
                .filter(|v| visible.contains(&v.channel))
                .cloned()
                .collect(),
            policies: c
                .policies
                .iter()
                .filter(|p| visible.contains(&p.channel))
                .cloned()
                .collect(),
            threads: c
                .threads
                .iter()
                .filter(|t| visible.contains(&t.channel) && visible.contains(&t.parent))
                .cloned()
                .collect(),
            timeouts: c
                .timeouts
                .iter()
                .filter(|t| member(t.guild) && t.until > now_secs())
                .cloned()
                .collect(),
            unread: self.unread_summaries(user),
            polls: c
                .polls
                .iter()
                .filter(|p| visible.contains(&p.channel))
                .cloned()
                .collect(),
            emojis: c
                .emojis
                .iter()
                .filter(|e| member(e.guild))
                .cloned()
                .collect(),
            events: c
                .events
                .iter()
                .filter(|e| member(e.guild) && e.channel.is_none_or(|ch| visible.contains(&ch)))
                .cloned()
                .collect(),
        }
    }
}
