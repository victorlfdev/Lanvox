use super::*;

impl State {
    pub(super) fn drop_pending_uploads(&mut self, user: UserId) {
        self.pending_uploads.drop_user(user);
    }

    pub(super) fn identify(&mut self, name: &str, key: [u8; 32]) -> Result<UserId, &'static str> {
        if self.is_revoked(&key) {
            return Err("Identidade revogada pelo proprietário do host");
        }
        let existing = self
            .public_keys
            .iter()
            .find(|(_, k)| **k == key)
            .map(|(id, _)| *id);
        if self
            .users
            .values()
            .any(|u| u.name.eq_ignore_ascii_case(name) && Some(u.id) != existing)
        {
            if self
                .users
                .values()
                .any(|u| u.name.eq_ignore_ascii_case(name) && !self.public_keys.contains_key(&u.id))
            {
                return Err("This profile was created before identity keys. Its owner must start the updated desktop host locally with this name once to migrate it, or you can choose a different name.");
            }
            return Err("This name is reserved by another identity. Choose a different name.");
        }
        if let Some(id) = existing {
            if self.clients.contains_key(&id) {
                return Err("This identity is already connected.");
            }
            return Ok(id);
        }
        if self.users.len() >= 2048 {
            return Err("Limite de identidades do host atingido");
        }
        let id = self.next_user;
        self.next_user = self.next_user.checked_add(1).ok_or("User limit reached")?;
        self.public_keys.insert(id, key);
        Ok(id)
    }

    pub(super) fn same_voice(&self, a: UserId, b: UserId) -> bool {
        let channel = self.voice.get(&a).and_then(|v| v.channel);
        channel.is_some_and(|ch| {
            self.voice.get(&b).and_then(|v| v.channel) == Some(ch)
                && (if ch < CALL_ID_BASE {
                    [a, b].into_iter().all(|user| {
                        self.can_access(user, ch)
                            && self.has_permission(user, ch, permission::CONNECT)
                            && !self.is_timed_out(user, ch)
                    })
                } else {
                    self.calls
                        .get(&ch)
                        .is_some_and(|c| c.members.contains(&a) && c.members.contains(&b))
                })
        })
    }

    pub(super) fn can_watch(&self, viewer: UserId, owner: UserId, kind: StreamKind) -> bool {
        viewer != owner
            && self.streams.contains_key(&(owner, kind))
            && self.same_voice(viewer, owner)
    }

    pub(super) fn friend_list(&self, user: UserId) -> Vec<FriendEntry> {
        let mut v: Vec<FriendEntry> = self
            .friends
            .get(&user)
            .map(|m| {
                m.iter()
                    .map(|(u, s)| FriendEntry {
                        user: *u,
                        status: *s,
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|f| f.user);
        v
    }

    pub(super) fn send_friends(&self, user: UserId) {
        self.send_to(user, &ServerMsg::Friends(self.friend_list(user)));
    }

    pub(super) fn set_relation(&mut self, a: UserId, b: UserId, a_to_b: Option<FriendStatus>) {
        let flip = |s: FriendStatus| match s {
            FriendStatus::Friend => FriendStatus::Friend,
            FriendStatus::Incoming => FriendStatus::Outgoing,
            FriendStatus::Outgoing => FriendStatus::Incoming,
        };
        match a_to_b {
            Some(s) => {
                self.friends.entry(a).or_default().insert(b, s);
                self.friends.entry(b).or_default().insert(a, flip(s));
            }
            None => {
                if let Some(m) = self.friends.get_mut(&a) {
                    m.remove(&b);
                }
                if let Some(m) = self.friends.get_mut(&b) {
                    m.remove(&a);
                }
            }
        }
        self.send_friends(a);
        self.send_friends(b);
    }

    pub(super) fn are_friends(&self, a: UserId, b: UserId) -> bool {
        self.friends.get(&a).and_then(|m| m.get(&b)) == Some(&FriendStatus::Friend)
    }

    pub(super) fn can_call(&self, a: UserId, b: UserId) -> bool {
        self.are_friends(a, b)
            || self
                .workspace
                .conversations
                .iter()
                .any(|c| c.members.contains(&a) && c.members.contains(&b))
    }

    pub(super) fn calls_of(&self, user: UserId) -> Vec<CallInfo> {
        self.calls
            .values()
            .filter(|c| c.members.contains(&user))
            .cloned()
            .collect()
    }

    pub(super) fn snapshot(&self, user: UserId) -> ServerSnapshot {
        let mut messages = Vec::new();
        for ch in self
            .channels
            .iter()
            .filter(|ch| self.can_access(user, ch.id))
        {
            let of_channel: Vec<&Message> = self
                .messages
                .iter()
                .filter(|m| m.channel == ch.id)
                .collect();
            let skip = of_channel.len().saturating_sub(SNAPSHOT_PER_CHANNEL);
            messages.extend(of_channel.into_iter().skip(skip).cloned());
        }
        messages.sort_by_key(|m| m.id);
        if messages.len() > 400 {
            messages.drain(..messages.len() - 400);
        }
        let mut users: Vec<User> = self
            .users
            .values()
            .cloned()
            .map(|mut u| {
                if u.id != user
                    && self
                        .workspace
                        .profiles
                        .iter()
                        .any(|p| p.user == u.id && p.presence == Presence::Invisible)
                {
                    u.online = false;
                }
                u
            })
            .collect();
        users.sort_by_key(|u| u.id);
        ServerSnapshot {
            name: self.name.clone(),
            guilds: self
                .guilds
                .iter()
                .filter(|g| {
                    self.administer(user, g.id)
                        || self
                            .workspace
                            .memberships
                            .iter()
                            .any(|m| m.guild == g.id && m.user == user)
                })
                .cloned()
                .collect(),
            friends: self.friend_list(user),
            calls: self.calls_of(user),
            channels: self
                .channels
                .iter()
                .filter(|ch| self.can_access(user, ch.id))
                .cloned()
                .collect(),
            users,
            messages,
            voice: self
                .voice
                .iter()
                .filter(|(id, v)| {
                    **id == user
                        || v.channel.is_none_or(|c| {
                            if c < CALL_ID_BASE {
                                self.can_access(user, c)
                            } else {
                                self.calls
                                    .get(&c)
                                    .is_some_and(|call| call.members.contains(&user))
                            }
                        })
                })
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
            streams: self
                .streams
                .values()
                .filter(|i| {
                    self.voice
                        .get(&i.owner)
                        .is_some_and(|v| self.can_see_voice(user, i.owner, v))
                })
                .cloned()
                .collect(),
        }
    }

    pub(super) fn broadcast(&self, msg: &ServerMsg) {
        for (viewer, out) in &self.clients {
            let visible = match msg {
                ServerMsg::Voice { user, state } => self.can_see_voice(*viewer, *user, state),
                ServerMsg::StreamStarted(info) => self
                    .voice
                    .get(&info.owner)
                    .is_some_and(|v| self.can_see_voice(*viewer, info.owner, v)),
                ServerMsg::StreamStopped { owner, .. } => self
                    .voice
                    .get(owner)
                    .is_some_and(|v| self.can_see_voice(*viewer, *owner, v)),
                _ => true,
            };
            if !visible {
                continue;
            }
            if let ServerMsg::UserUpdated(user) = msg {
                let mut user = user.clone();
                if *viewer != user.id
                    && self
                        .workspace
                        .profiles
                        .iter()
                        .any(|p| p.user == user.id && p.presence == Presence::Invisible)
                {
                    user.online = false;
                }
                out.push_urgent(encode_packet(&ServerMsg::UserUpdated(user)));
            } else {
                out.push_urgent(encode_packet(msg));
            }
        }
    }

    pub(super) fn send_to(&self, user: UserId, msg: &ServerMsg) {
        if let Some(out) = self.clients.get(&user) {
            let wrapped = match (self.query, msg) {
                (Some((recipient, id)), ServerMsg::Workspace(event)) if recipient == user => {
                    QueryResult::from_event(event.clone()).map(|event| {
                        ServerMsg::Workspace(WorkspaceEvent::QueryReply {
                            id,
                            event: Box::new(event),
                        })
                    })
                }
                _ => None,
            };
            let msg = wrapped.as_ref().unwrap_or(msg);
            if matches!(
                msg,
                ServerMsg::Workspace(WorkspaceEvent::State(_) | WorkspaceEvent::Resync { .. })
            ) {
                out.push_snapshot(encode_packet(msg));
            } else {
                out.push_urgent(encode_packet(msg));
            }
        }
    }

    /// For file pieces: queued behind anything urgent for that client.
    pub(super) fn send_file_to(&self, user: UserId, msg: &ServerMsg) {
        if let Some(out) = self.clients.get(&user) {
            out.push_file(encode_packet(msg));
        }
    }

    pub(super) fn voice_peers(&self, user: UserId) -> Vec<UserId> {
        let Some(ch) = self.voice.get(&user).and_then(|v| v.channel) else {
            return Vec::new();
        };
        self.voice
            .iter()
            .filter(|(id, v)| **id != user && v.channel == Some(ch) && self.same_voice(user, **id))
            .map(|(id, _)| *id)
            .collect()
    }

    pub(super) fn save(&self) {
        if let Err(error) = self.save_to_disk() {
            eprintln!("Could not save the Lanvox host: {error}");
            self.broadcast(&ServerMsg::Error(
                "The host could not save its data. Changes are currently only in memory.".into(),
            ));
        }
    }

    pub(super) fn saved(&self, include_messages: bool) -> Saved {
        Saved {
            guilds: self.guilds.clone(),
            next_guild: self.next_guild,
            friends: self
                .friends
                .iter()
                .flat_map(|(a, m)| m.iter().map(move |(b, s)| (*a, *b, *s)))
                .collect(),
            channels: self.channels.clone(),
            users: self.users.values().cloned().collect(),
            messages: if include_messages {
                self.messages.clone()
            } else {
                Vec::new()
            },
            next_channel: self.next_channel,
            next_user: self.next_user,
            next_message: self.next_message,
            public_keys: self.public_keys.clone(),
            workspace: self.workspace.clone(),
        }
    }

    pub(super) fn save_to_disk(&self) -> std::io::Result<()> {
        use std::io::Write;
        if let Some(store) = &self.store {
            return store.lock().save_state(&self.saved(false));
        }
        let Some(path) = &self.save_path else {
            return Ok(());
        };
        let saved = self.saved(true);
        let json = serde_json::to_vec(&saved)?;
        let tmp = path.with_extension("tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    }

    pub(super) fn stop_stream(&mut self, owner: UserId, kind: StreamKind) {
        self.last_keyframe_ask.remove(&(owner, kind));
        if self.streams.remove(&(owner, kind)).is_some() {
            self.watchers.remove(&(owner, kind));
            self.refresh_media_routes();
            self.broadcast(&ServerMsg::StreamStopped { owner, kind });
        }
        if let Some(v) = self.voice.get_mut(&owner) {
            match kind {
                StreamKind::Screen => v.streaming = false,
                StreamKind::Camera => v.camera = false,
            }
            let state = v.clone();
            self.broadcast(&ServerMsg::Voice { user: owner, state });
        }
    }

    pub(super) fn leave_voice(&mut self, user: UserId) {
        let previous = self.voice.get(&user).and_then(|v| v.channel);
        self.leave_voice_inner(user);
        // A private call ends when its last person leaves.
        if let Some(ch) = previous.filter(|c| *c >= CALL_ID_BASE) {
            if !self.voice.values().any(|v| v.channel == Some(ch)) {
                self.end_call(ch);
            }
        }
    }

    pub(super) fn end_call(&mut self, call: ChannelId) {
        self.call_deadlines.remove(&call);
        if let Some(info) = self.calls.remove(&call) {
            for m in info.members {
                self.send_to(m, &ServerMsg::CallEnded { call });
            }
        }
    }

    pub(super) fn leave_voice_inner(&mut self, user: UserId) {
        let stage_channel = self.voice.get(&user).and_then(|v| v.channel);
        if let Some(channel) = stage_channel {
            if let Some(speakers) = self.stage_speakers.get_mut(&channel) {
                speakers.remove(&user);
            }
            if let Some(requests) = self.stage_requests.get_mut(&channel) {
                requests.remove(&user);
            }
        }
        self.stop_stream(user, StreamKind::Screen);
        self.stop_stream(user, StreamKind::Camera);
        let mut changed = Vec::new();
        for (key, set) in &mut self.watchers {
            if set.remove(&user) {
                changed.push(*key);
            }
        }
        for key in changed {
            self.viewers_changed(key);
        }
        if let Some(v) = self.voice.get_mut(&user) {
            v.channel = None;
            v.camera = false;
            v.streaming = false;
            let state = v.clone();
            self.refresh_media_routes();
            self.broadcast(&ServerMsg::Voice { user, state });
        }
        if let Some(channel) = stage_channel {
            self.broadcast_stage_state(channel);
        }
    }

    pub(super) fn viewers_changed(&self, key: StreamKey) {
        let count = self.watchers.get(&key).map(|s| s.len()).unwrap_or(0) as u32;
        self.send_to(key.0, &ServerMsg::Viewers { kind: key.1, count });
    }
}
