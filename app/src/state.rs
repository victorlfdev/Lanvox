//! The client's copy of the server it is connected to, kept current by
//! applying what the server sends.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use crate::model::*;
use crate::protocol::ServerMsg;
#[path = "state_permissions.rs"]
mod permissions;

pub struct ServerView {
    pub me: UserId,
    pub name: String,
    pub guilds: Vec<Guild>,
    pub friends: Vec<FriendEntry>,
    pub calls: HashMap<ChannelId, CallInfo>,
    /// Private calls shown as voice channels (so the call view can treat them alike).
    pub call_channels: Vec<Channel>,
    pub channels: Vec<Channel>,
    pub users: BTreeMap<UserId, User>,
    pub messages: HashMap<ChannelId, Vec<Message>>,
    pub voice: HashMap<UserId, VoiceState>,
    pub stage_states: HashMap<ChannelId, (Vec<UserId>, Vec<UserId>)>,
    pub streams: Vec<StreamInfo>,
    pub speaking: HashSet<UserId>,
    pub typing: HashMap<(ChannelId, UserId), Instant>,
    pub unread: HashMap<ChannelId, u32>,
    pub ping_ms: Option<f32>,
    pub viewers: HashMap<StreamKind, u32>,
    pub workspace: WorkspaceSnapshot,
    pub history_more: HashMap<ChannelId, bool>,
    pub history_loading: HashSet<ChannelId>,
}

impl ServerView {
    pub fn from_snapshot(me: UserId, snap: ServerSnapshot) -> Self {
        let mut messages: HashMap<ChannelId, Vec<Message>> = HashMap::new();
        for m in snap.messages {
            messages.entry(m.channel).or_default().push(m);
        }
        let mut view = Self {
            me,
            name: snap.name,
            guilds: snap.guilds,
            friends: snap.friends,
            calls: snap.calls.into_iter().map(|c| (c.id, c)).collect(),
            call_channels: Vec::new(),
            channels: snap.channels,
            users: snap.users.into_iter().map(|u| (u.id, u)).collect(),
            messages,
            voice: snap.voice.into_iter().collect(),
            stage_states: HashMap::new(),
            streams: snap.streams,
            speaking: HashSet::new(),
            typing: HashMap::new(),
            unread: HashMap::new(),
            ping_ms: None,
            viewers: HashMap::new(),
            workspace: WorkspaceSnapshot::default(),
            history_more: HashMap::new(),
            history_loading: HashSet::new(),
        };
        view.sync_call_channels();
        view
    }

    /// Group conversation a call belongs to: every call member is in it.
    fn call_group(&self, call: &CallInfo) -> Option<&Conversation> {
        self.workspace
            .conversations
            .iter()
            .filter(|c| call.members.iter().all(|m| c.members.contains(m)))
            .min_by_key(|c| (c.members.len(), !c.direct))
    }

    /// Conversation (group or DM) a call lives in, if any.
    pub fn conversation_of_call(&self, call: ChannelId) -> Option<ChannelId> {
        self.calls
            .get(&call)
            .and_then(|c| self.call_group(c))
            .map(|c| c.channel)
    }

    /// The call currently attached to a conversation: the one I'm in, else the
    /// oldest one I'm a member of.
    pub fn call_of_conversation(&self, conversation: ChannelId) -> Option<&CallInfo> {
        let mine = self.my_voice().channel;
        let mut list: Vec<&CallInfo> = self
            .calls
            .values()
            .filter(|c| c.members.contains(&self.me))
            .filter(|c| self.call_group(c).map(|g| g.channel) == Some(conversation))
            .collect();
        list.sort_by_key(|c| (Some(c.id) != mine, c.id));
        list.into_iter().next()
    }

    /// Title of a private call: the group it was started from, else the other people in it.
    pub fn call_title(&self, call: &CallInfo) -> String {
        if let Some(group) = self.call_group(call) {
            if !group.name.trim().is_empty() {
                return group.name.clone();
            }
        }
        let others: Vec<String> = call
            .members
            .iter()
            .filter(|m| **m != self.me)
            .map(|m| self.user_name(*m))
            .collect();
        match others.len() {
            0 => "Chamar".into(),
            1 => format!("Chamada com {}", others[0]),
            _ => format!("Chamada de grupo: {}", others.join(", ")),
        }
    }

    fn sync_call_channels(&mut self) {
        let mut list: Vec<Channel> = self
            .calls
            .values()
            .map(|c| Channel {
                id: c.id,
                guild: 0,
                name: self.call_title(c),
                kind: ChannelKind::Voice,
                topic: String::new(),
            })
            .collect();
        list.sort_by_key(|c| c.id);
        self.call_channels = list;
    }

    pub fn guild_channels(&self, guild: GuildId) -> impl Iterator<Item = &Channel> {
        self.channels.iter().filter(move |c| c.guild == guild)
    }

    pub fn friend_status(&self, user: UserId) -> Option<FriendStatus> {
        self.friends
            .iter()
            .find(|f| f.user == user)
            .map(|f| f.status)
    }

    pub fn my_voice(&self) -> VoiceState {
        self.voice.get(&self.me).cloned().unwrap_or_default()
    }

    pub fn channel(&self, id: ChannelId) -> Option<&Channel> {
        self.channels
            .iter()
            .chain(self.call_channels.iter())
            .find(|c| c.id == id)
    }

    pub fn user_name(&self, id: UserId) -> String {
        self.users
            .get(&id)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "Alguém".into())
    }

    /// People who can be mentioned in `channel`: conversation members, or the guild's members.
    pub fn channel_members(&self, channel: ChannelId) -> Vec<(UserId, String)> {
        let ids: Vec<UserId> = if let Some(c) = self
            .workspace
            .conversations
            .iter()
            .find(|c| c.channel == channel)
        {
            c.members.clone()
        } else if let Some(ch) = self.channel(channel) {
            let guild: Vec<UserId> = self
                .workspace
                .memberships
                .iter()
                .filter(|m| m.guild == ch.guild)
                .map(|m| m.user)
                .collect();
            if guild.is_empty() {
                self.users.keys().copied().collect()
            } else {
                guild
            }
        } else {
            Vec::new()
        };
        ids.into_iter()
            .filter_map(|id| self.users.get(&id).map(|u| (id, u.name.clone())))
            .collect()
    }

    /// The preview to show for a reply: the server's, else the locally cached original.
    pub fn reply_preview(&self, details: &MessageDetails) -> Option<ReplyPreview> {
        let id = details.reply_to?;
        if let Some(p) = &details.reply_preview {
            return Some(p.clone());
        }
        let cached = self.messages.values().flatten().find(|m| m.id == id);
        cached.map(|m| ReplyPreview::from_original(Some(m)))
    }

    pub fn user_color(&self, id: UserId) -> u32 {
        self.users.get(&id).map(|u| u.color).unwrap_or(0x747f8d)
    }

    /// People in a voice channel, in a stable order.
    pub fn in_voice(&self, channel: ChannelId) -> Vec<UserId> {
        let mut v: Vec<UserId> = self
            .voice
            .iter()
            .filter(|(_, s)| s.channel == Some(channel))
            .map(|(id, _)| *id)
            .collect();
        v.sort();
        v
    }

    pub fn stream(&self, owner: UserId, kind: StreamKind) -> Option<&StreamInfo> {
        self.streams
            .iter()
            .find(|s| s.owner == owner && s.kind == kind)
    }

    pub fn typing_in(&self, channel: ChannelId) -> Vec<String> {
        self.typing
            .iter()
            .filter(|((c, u), t)| *c == channel && *u != self.me && t.elapsed().as_secs_f32() < 8.0)
            .map(|((_, u), _)| self.user_name(*u))
            .collect()
    }

    pub fn details(&self, id: MessageId) -> Option<&MessageDetails> {
        self.workspace.details.iter().find(|d| d.message == id)
    }

    pub fn insert_message(&mut self, message: Message) {
        let list = self.messages.entry(message.channel).or_default();
        if let Some(existing) = list.iter_mut().find(|m| m.id == message.id) {
            *existing = message;
        } else {
            list.push(message);
            list.sort_by_key(|m| m.id);
        }
        // The host owns history; the desktop keeps only a bounded working set.
        if list.len() > 600 {
            list.drain(..list.len() - 600);
        }
        let ids: HashSet<_> = self
            .messages
            .values()
            .flat_map(|list| list.iter().map(|m| m.id))
            .collect();
        self.workspace.details.retain(|d| ids.contains(&d.message));
    }

    pub fn apply_workspace(&mut self, event: &WorkspaceEvent) {
        match event {
            WorkspaceEvent::Read { channel, message } => {
                let previous = self
                    .workspace
                    .read
                    .iter()
                    .find(|(id, _)| id == channel)
                    .map(|(_, message)| *message)
                    .unwrap_or(0);
                self.workspace.read.retain(|(id, _)| id != channel);
                self.workspace
                    .read
                    .push((*channel, (*message).max(previous)));
            }
            WorkspaceEvent::State(state) => {
                self.workspace = state.clone();
                self.sync_call_channels();
                for conversation in &state.conversations {
                    if !self.channels.iter().any(|c| c.id == conversation.channel) {
                        self.channels.push(Channel {
                            id: conversation.channel,
                            guild: 0,
                            name: conversation.name.clone(),
                            kind: ChannelKind::Text,
                            topic: String::new(),
                        });
                    }
                }
                for (channel, cursor) in &state.read {
                    let unread = self
                        .messages
                        .get(channel)
                        .map(|messages| {
                            messages
                                .iter()
                                .filter(|m| m.id > *cursor && m.author != self.me)
                                .count() as u32
                        })
                        .unwrap_or(0);
                    if unread > 0 {
                        self.unread.insert(*channel, unread);
                    } else {
                        self.unread.remove(channel);
                    }
                }
                for summary in &state.community.unread {
                    if summary.count == 0 {
                        self.unread.remove(&summary.channel);
                    } else {
                        self.unread.insert(summary.channel, summary.count);
                    }
                }
            }
            WorkspaceEvent::Community(CommunityEvent::Unread(summaries)) => {
                for summary in summaries {
                    if summary.count == 0 {
                        self.unread.remove(&summary.channel);
                    } else {
                        self.unread.insert(summary.channel, summary.count);
                    }
                    self.workspace
                        .community
                        .unread
                        .retain(|s| s.channel != summary.channel);
                    self.workspace.community.unread.push(summary.clone());
                }
            }
            WorkspaceEvent::Community(CommunityEvent::MarkedUnread { channel, message }) => {
                self.workspace.read.retain(|(id, _)| id != channel);
                self.workspace.read.push((*channel, *message));
            }
            WorkspaceEvent::Community(CommunityEvent::PollUpdated(poll)) => {
                self.workspace
                    .community
                    .polls
                    .retain(|p| p.message != poll.message);
                self.workspace.community.polls.push(poll.clone());
            }
            WorkspaceEvent::Community(CommunityEvent::StageState {
                channel,
                speakers,
                requests,
            }) => {
                self.stage_states
                    .insert(*channel, (speakers.clone(), requests.clone()));
            }
            WorkspaceEvent::History {
                channel,
                messages,
                details,
                has_more,
            } => {
                let list = self.messages.entry(*channel).or_default();
                for message in messages {
                    if !list.iter().any(|m| m.id == message.id) {
                        list.push(message.clone());
                    }
                }
                list.sort_by_key(|m| m.id);
                // Keep the loaded historical page, evict newest cached rows when browsing back.
                if list.len() > 600 {
                    list.truncate(600);
                }
                for detail in details {
                    self.workspace
                        .details
                        .retain(|d| d.message != detail.message);
                    self.workspace.details.push(detail.clone());
                }
                self.history_more.insert(*channel, *has_more);
                self.history_loading.remove(channel);
            }
            WorkspaceEvent::MessageChanged { message, details } => {
                self.insert_message(message.clone());
                self.workspace.details.retain(|d| d.message != message.id);
                self.workspace.details.push(details.clone());
            }
            WorkspaceEvent::MessageDeleted { channel, message } => {
                if let Some(list) = self.messages.get_mut(channel) {
                    list.retain(|m| m.id != *message);
                }
                self.workspace.details.retain(|d| d.message != *message);
                // Replies to it now point at a removed message.
                for d in self
                    .workspace
                    .details
                    .iter_mut()
                    .filter(|d| d.reply_to == Some(*message))
                {
                    d.reply_preview = Some(ReplyPreview::from_original(None));
                }
            }
            WorkspaceEvent::CategoriesChanged {
                guild,
                categories,
                channel_settings,
            } => {
                self.workspace.categories.retain(|c| c.guild != *guild);
                self.workspace.categories.extend(categories.iter().cloned());
                let in_guild: HashSet<ChannelId> = self
                    .channels
                    .iter()
                    .filter(|c| c.guild == *guild)
                    .map(|c| c.id)
                    .collect();
                self.workspace
                    .channel_settings
                    .retain(|s| !in_guild.contains(&s.channel));
                self.workspace
                    .channel_settings
                    .extend(channel_settings.iter().cloned());
            }
            _ => {}
        }
    }

    /// Applies a category command to this copy only (the demo has no host). It mirrors what
    /// the host does, through the same pure functions.
    pub fn apply_category_command_locally(&mut self, guild: GuildId, command: &WorkspaceCommand) {
        use crate::categories as cat;
        let ws = &mut self.workspace;
        match command {
            WorkspaceCommand::CreateCategory { guild, name } => {
                let mut counter = 0;
                cat::create_category(&mut ws.categories, &mut counter, *guild, name.trim());
            }
            WorkspaceCommand::RenameCategory { category, name } => {
                if let Some(c) = ws.categories.iter_mut().find(|c| c.id == *category) {
                    c.name = name.trim().to_string();
                }
            }
            WorkspaceCommand::DeleteCategory { category } => {
                cat::delete_category(
                    &self.channels,
                    &mut ws.channel_settings,
                    &mut ws.categories,
                    *category,
                );
            }
            WorkspaceCommand::ReorderCategories { guild, order } => {
                let _ = cat::reorder_categories(&mut ws.categories, *guild, order);
            }
            WorkspaceCommand::MoveChannel {
                channel,
                category,
                index,
            } => {
                cat::move_channel(
                    &self.channels,
                    &mut ws.channel_settings,
                    &ws.categories,
                    *channel,
                    *category,
                    *index as usize,
                );
            }
            WorkspaceCommand::CreateChannelIn {
                name,
                kind,
                category,
                ..
            } => {
                let id = self.channels.iter().map(|c| c.id).max().unwrap_or(0) + 1;
                self.channels.push(Channel {
                    id,
                    guild,
                    name: name.clone(),
                    kind: *kind,
                    topic: String::new(),
                });
                cat::place_new_channel(
                    &self.channels,
                    &mut ws.channel_settings,
                    &ws.categories,
                    id,
                    *category,
                );
            }
            _ => {}
        }
    }

    /// Whether the person may manage channels (and so categories) in `guild`: the same rule the
    /// host enforces (community owner/administrator, or the host owner).
    pub fn can_manage_channels(&self, guild: GuildId) -> bool {
        self.guild_permissions(guild) & permission::MANAGE_CHANNELS != 0
    }

    /// Apply one update from the server. Returns a short notice to show, if any.
    pub fn apply(&mut self, msg: ServerMsg, viewing: Option<ChannelId>) -> Option<String> {
        match msg {
            ServerMsg::Welcome { .. }
            | ServerMsg::Audio { .. }
            | ServerMsg::Frame { .. }
            | ServerMsg::Workspace(_)
            | ServerMsg::AudioEncoded { .. }
            | ServerMsg::StreamAudio { .. }
            | ServerMsg::MediaSession { .. }
            | ServerMsg::Cursor { .. }
            | ServerMsg::EncodedFrame { .. } => {}
            ServerMsg::KeyframeWanted { .. } | ServerMsg::Challenge { .. } => {}
            ServerMsg::UserUpdated(u) => {
                if !u.online {
                    self.voice.remove(&u.id);
                    self.speaking.remove(&u.id);
                }
                self.users.insert(u.id, u);
                self.sync_call_channels();
            }
            ServerMsg::Message(m) => {
                self.typing.remove(&(m.channel, m.author));
                let duplicate = self
                    .messages
                    .get(&m.channel)
                    .is_some_and(|messages| messages.iter().any(|old| old.id == m.id));
                if !duplicate && Some(m.channel) != viewing && m.author != self.me {
                    *self.unread.entry(m.channel).or_default() += 1;
                }
                self.insert_message(m);
            }
            ServerMsg::ChannelCreated(c) => self.channels.push(c),
            ServerMsg::GuildCreated(g) => self.guilds.push(g),
            ServerMsg::Friends(list) => {
                let new_request = list
                    .iter()
                    .find(|f| {
                        f.status == FriendStatus::Incoming
                            && self.friend_status(f.user) != Some(FriendStatus::Incoming)
                    })
                    .map(|f| f.user);
                let new_friend = list
                    .iter()
                    .find(|f| {
                        f.status == FriendStatus::Friend
                            && self.friend_status(f.user) == Some(FriendStatus::Outgoing)
                    })
                    .map(|f| f.user);
                self.friends = list;
                if let Some(u) = new_request {
                    return Some(format!(
                        "{} enviou uma solicitação de amizade.",
                        self.user_name(u)
                    ));
                }
                if let Some(u) = new_friend {
                    return Some(format!(
                        "{} aceitou sua solicitação de amizade.",
                        self.user_name(u)
                    ));
                }
            }
            ServerMsg::CallUpdated(info) => {
                self.calls.insert(info.id, info);
                self.sync_call_channels();
            }
            ServerMsg::CallEnded { call } => {
                self.calls.remove(&call);
                self.sync_call_channels();
            }
            ServerMsg::Typing { user, channel } => {
                self.typing.insert((channel, user), Instant::now());
            }
            ServerMsg::Voice { user, state } => {
                if state.channel.is_none() {
                    self.speaking.remove(&user);
                }
                self.voice.insert(user, state);
            }
            ServerMsg::Speaking { user, speaking } => {
                if speaking {
                    self.speaking.insert(user);
                } else {
                    self.speaking.remove(&user);
                }
            }
            ServerMsg::StreamStarted(info) => {
                self.streams
                    .retain(|s| !(s.owner == info.owner && s.kind == info.kind));
                self.streams.push(info);
            }
            ServerMsg::StreamStopped { owner, kind } => {
                self.streams
                    .retain(|s| !(s.owner == owner && s.kind == kind));
            }
            ServerMsg::Viewers { kind, count } => {
                self.viewers.insert(kind, count);
            }
            ServerMsg::Pong(_) => {}
            ServerMsg::Error(e) => return Some(e),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> ServerView {
        ServerView::from_snapshot(
            1,
            ServerSnapshot {
                name: "test".into(),
                guilds: Vec::new(),
                friends: Vec::new(),
                calls: Vec::new(),
                channels: vec![Channel {
                    id: 7,
                    guild: 0,
                    name: "DM".into(),
                    kind: ChannelKind::Text,
                    topic: String::new(),
                }],
                users: Vec::new(),
                messages: Vec::new(),
                voice: Vec::new(),
                streams: Vec::new(),
            },
        )
    }
    fn message(id: u64) -> Message {
        Message {
            id,
            channel: 7,
            author: 2,
            author_name: "Ana".into(),
            author_color: 0,
            time: 0,
            text: format!("message {id}"),
        }
    }

    #[test]
    fn replayed_messages_do_not_duplicate_history_or_unread_counts() {
        let mut view = view();
        view.apply(ServerMsg::Message(message(9)), None);
        view.apply(ServerMsg::Message(message(9)), None);
        assert_eq!(view.messages[&7].len(), 1);
        assert_eq!(view.unread[&7], 1);
        let mut edited = message(9);
        edited.text = "edited".into();
        view.apply_workspace(&WorkspaceEvent::MessageChanged {
            message: edited,
            details: MessageDetails {
                message: 9,
                edited: Some(1),
                ..Default::default()
            },
        });
        assert_eq!(view.messages[&7][0].text, "edited");
        assert_eq!(view.unread[&7], 1);
    }

    #[test]
    fn paged_history_merges_in_order_and_caps_the_working_set() {
        let mut view = view();
        view.apply_workspace(&WorkspaceEvent::History {
            channel: 7,
            messages: (500..700).map(message).collect(),
            details: Vec::new(),
            has_more: true,
        });
        view.apply_workspace(&WorkspaceEvent::History {
            channel: 7,
            messages: (0..600).map(message).collect(),
            details: Vec::new(),
            has_more: false,
        });
        assert_eq!(view.messages[&7].len(), 600);
        assert_eq!(view.messages[&7][0].id, 0);
        assert!(view.messages[&7]
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id));
        assert!(!view.history_more[&7]);
        assert!(!view.history_loading.contains(&7));
    }

    #[test]
    fn persisted_read_cursor_and_deletion_are_applied_to_cached_messages() {
        let mut view = view();
        view.messages
            .insert(7, vec![message(1), message(2), message(3)]);
        view.apply_workspace(&WorkspaceEvent::State(WorkspaceSnapshot {
            read: vec![(7, 2)],
            ..Default::default()
        }));
        assert_eq!(view.unread[&7], 1);
        view.apply_workspace(&WorkspaceEvent::MessageDeleted {
            channel: 7,
            message: 2,
        });
        assert_eq!(
            view.messages[&7].iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }
}
