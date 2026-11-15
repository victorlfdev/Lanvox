//! Local attention preferences and operating-system entry points.
use super::*;
use crate::settings::NotificationPreference;

impl LanvoxApp {
    pub fn notification_scope(&self) -> String {
        ui::quick_switcher::host_scope(self)
    }

    pub fn is_blocked(&self, user: UserId) -> bool {
        user != self.me()
            && self
                .settings
                .blocked_users
                .get(&self.notification_scope())
                .is_some_and(|users| users.contains(&user))
    }

    pub fn set_blocked(&mut self, user: UserId, blocked: bool) {
        if user == self.me() {
            return;
        }
        let scope = self.notification_scope();
        let users = self.settings.blocked_users.entry(scope).or_default();
        users.retain(|id| *id != user);
        if blocked {
            users.push(user);
        }
        self.settings.save();
        let name = self
            .view
            .as_ref()
            .map(|v| v.user_name(user))
            .unwrap_or_default();
        let (volume, muted) = self
            .settings
            .user_audio
            .get(&self.person_audio_key(user, &name))
            .copied()
            .unwrap_or((1.0, false));
        self.mixer
            .set_user(user, if blocked { 0.0 } else { volume }, blocked || muted);
    }

    pub fn apply_local_audio_preferences(&self) {
        if let Some(view) = &self.view {
            for user in view.users.values() {
                let (volume, muted) = self
                    .settings
                    .user_audio
                    .get(&self.person_audio_key(user.id, &user.name))
                    .copied()
                    .unwrap_or((1.0, false));
                self.mixer
                    .set_user(user.id, volume, muted || self.is_blocked(user.id));
                let (volume, muted) = self
                    .settings
                    .stream_audio
                    .get(&self.person_audio_key(user.id, &user.name))
                    .copied()
                    .unwrap_or((1.0, false));
                self.mixer
                    .set_stream_user(user.id, volume, muted || self.is_blocked(user.id));
            }
        }
    }

    pub fn person_audio_key(&self, user: UserId, _name: &str) -> String {
        format!("{}|user:{user}", self.notification_scope())
    }

    pub fn channel_notification_preference(&self, channel: ChannelId) -> NotificationPreference {
        let scope = self.notification_scope();
        self.settings
            .channel_notifications
            .get(&format!("{scope}|{channel}"))
            .copied()
            .unwrap_or_else(|| {
                self.view
                    .as_ref()
                    .and_then(|v| v.channel(channel))
                    .and_then(|c| {
                        self.settings
                            .guild_notifications
                            .get(&format!("{scope}|{}", c.guild))
                    })
                    .copied()
                    .unwrap_or_default()
            })
    }

    pub fn set_channel_notifications(
        &mut self,
        channel: ChannelId,
        preference: NotificationPreference,
    ) {
        self.settings.channel_notifications.insert(
            format!("{}|{channel}", self.notification_scope()),
            preference,
        );
        self.settings.save();
    }

    pub fn set_guild_notifications(&mut self, guild: GuildId, preference: NotificationPreference) {
        self.settings
            .guild_notifications
            .insert(format!("{}|{guild}", self.notification_scope()), preference);
        self.settings.save();
    }

    pub fn should_notify_message(&self, message: &Message) -> bool {
        if message.author == self.me()
            || self.is_blocked(message.author)
            || self.settings.do_not_disturb
        {
            return false;
        }
        let mentioned = self
            .view
            .as_ref()
            .and_then(|view| view.details(message.id))
            .is_some_and(|details| details.mentions.contains(&self.me()))
            || message.text.contains(&format!("<@{}>", self.me()));
        let scope = self.notification_scope();
        let guild_muted = self
            .view
            .as_ref()
            .and_then(|view| view.channel(message.channel))
            .and_then(|channel| {
                self.settings
                    .guild_notifications
                    .get(&format!("{scope}|{}", channel.guild))
            })
            .is_some_and(|preference| preference.muted_until > now_secs());
        !guild_muted
            && self
                .channel_notification_preference(message.channel)
                .allows(mentioned, now_secs())
    }

    pub fn stage_invite(&mut self, invite: String) {
        match crate::invites::parse(&invite) {
            Ok(_) => self.modal = Modal::JoinInvite { invite },
            Err(error) => self.notify(error),
        }
    }

    pub fn accept_invite(&mut self, ctx: &egui::Context, invite: String) {
        if let Err(error) = crate::invites::parse(&invite) {
            self.notify(error);
            return;
        }
        self.modal = Modal::None;
        if let Ok(link) = crate::invites::parse(&invite) {
            if let Some((channel, message)) = link.message {
                let host = crate::settings::host_address(&link.address);
                self.pending_message = Some((host, channel, message));
            }
        }
        self.join_input = invite.clone();
        // Message paths are presentation data, never part of the host address.
        if let Ok(link) = crate::invites::parse(&invite) {
            self.load_host_preferences(&link.address);
            if let Some(token) = link.token {
                self.settings.access_token = token;
            }
            if let Some(fingerprint) = link.fingerprint {
                self.settings.expected_host = fingerprint;
            }
            self.connect(ctx, link.address);
        }
    }

    pub fn open_notification(
        &mut self,
        ctx: &egui::Context,
        host: String,
        channel: ChannelId,
        message: MessageId,
    ) {
        let host = crate::settings::host_address(&host);
        let current = self
            .client
            .as_ref()
            .map(|c| crate::settings::host_address(&c.address));
        if current.as_deref() == Some(host.as_str()) {
            self.pending_message = None;
            self.open_message(&Message {
                id: message,
                channel,
                author: 0,
                author_name: String::new(),
                author_color: 0,
                time: 0,
                text: String::new(),
            });
        } else {
            self.pending_message = Some((host.clone(), channel, message));
            self.load_host_preferences(&host);
            self.connect(ctx, host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::NotificationLevel;
    #[test]
    fn quiet_preferences_expire_and_mentions_are_selective() {
        let mut preference = NotificationPreference {
            level: NotificationLevel::Mentions,
            muted_until: 100,
        };
        assert!(!preference.allows(true, 99));
        assert!(preference.allows(true, 100));
        assert!(!preference.allows(false, 100));
        preference.level = NotificationLevel::None;
        assert!(!preference.allows(true, 200));
    }
}
