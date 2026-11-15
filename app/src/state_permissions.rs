//! Capability hints for controls. The host remains the authority for every operation.
use super::*;

impl ServerView {
    pub fn guild_permissions(&self, guild: GuildId) -> u64 {
        self.guild_permissions_for(guild, self.me)
    }

    pub fn guild_permissions_for(&self, guild: GuildId, user: UserId) -> u64 {
        if self.workspace.host_owner == Some(user)
            || self.workspace.memberships.iter().any(|m| {
                m.guild == guild
                    && m.user == user
                    && matches!(m.role, MemberRole::Owner | MemberRole::Admin)
            })
        {
            return permission::ALL;
        }
        if !self
            .workspace
            .memberships
            .iter()
            .any(|m| m.guild == guild && m.user == user)
        {
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
            .filter(|role| {
                role.guild == guild && assigned.is_some_and(|a| a.roles.contains(&role.id))
            })
            .fold(base, |value, role| value | role.permissions)
            & permission::ALL
    }

    pub fn channel_permissions(&self, channel: ChannelId) -> u64 {
        self.channel_permissions_for(channel, self.me)
    }

    pub fn channel_permissions_for(&self, channel: ChannelId, user: UserId) -> u64 {
        if let Some(conversation) = self
            .workspace
            .conversations
            .iter()
            .find(|c| c.channel == channel)
        {
            return if conversation.members.contains(&user) {
                permission::MEMBER
            } else {
                0
            };
        }
        if let Some(call) = self.calls.get(&channel) {
            return if call.members.contains(&user) {
                permission::MEMBER
            } else {
                0
            };
        }
        let Some(ch) = self.channel(channel) else {
            return 0;
        };
        if self.workspace.host_owner == Some(user)
            || self.workspace.memberships.iter().any(|m| {
                m.guild == ch.guild
                    && m.user == user
                    && matches!(m.role, MemberRole::Owner | MemberRole::Admin)
            })
        {
            return permission::ALL;
        }
        if !self
            .workspace
            .memberships
            .iter()
            .any(|m| m.guild == ch.guild && m.user == user)
        {
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
                || self.channel_permissions_for(inherited, user) & permission::VIEW == 0)
        {
            return 0;
        }
        let settings = self
            .workspace
            .channel_settings
            .iter()
            .find(|s| s.channel == inherited);
        if settings.is_some_and(|s| !s.allowed.is_empty() && !s.allowed.contains(&user)) {
            return 0;
        }
        let mut value = self.guild_permissions_for(ch.guild, user);
        let assigned = self
            .workspace
            .community
            .assignments
            .iter()
            .find(|a| a.guild == ch.guild && a.user == user);
        let mut scopes = Vec::new();
        if let Some(category) = settings.and_then(|s| s.category_id) {
            scopes.push(PermissionScope::Category(category));
        }
        scopes.push(PermissionScope::Channel(inherited));
        if inherited != channel {
            scopes.push(PermissionScope::Channel(channel));
        }
        for scope in scopes {
            for tier in 0..3 {
                let mut allow = 0;
                let mut deny = 0;
                for entry in self
                    .workspace
                    .community
                    .overrides
                    .iter()
                    .filter(|entry| entry.scope == scope)
                {
                    let applies = match (tier, entry.target) {
                        (0, PermissionTarget::Everyone) => true,
                        (1, PermissionTarget::Role(role)) => {
                            assigned.is_some_and(|a| a.roles.contains(&role))
                        }
                        (2, PermissionTarget::Member(target)) => target == user,
                        _ => false,
                    };
                    if applies {
                        allow |= entry.allow;
                        deny |= entry.deny;
                    }
                }
                value = (value & !deny) | allow;
            }
        }
        value & permission::ALL
    }

    pub fn has_permission(&self, channel: ChannelId, capability: u64) -> bool {
        let value = self.channel_permissions(channel);
        value & permission::VIEW != 0 && value & capability == capability
    }

    pub fn can_send(&self, channel: ChannelId) -> bool {
        let guild = self.channel(channel).map(|c| c.guild);
        self.has_permission(channel, permission::SEND)
            && !self
                .workspace
                .community
                .timeouts
                .iter()
                .any(|t| Some(t.guild) == guild && t.user == self.me && t.until > now_secs())
            && !self
                .workspace
                .community
                .threads
                .iter()
                .any(|t| t.channel == channel && t.archived)
            && !self.workspace.community.policies.iter().any(|p| {
                p.channel == channel
                    && (p.style == ChannelStyle::Forum
                        || (p.style == ChannelStyle::Announcement
                            && !self.has_permission(channel, permission::MANAGE_MESSAGES)))
            })
    }
}
