use super::*;

pub(super) fn handle_msg(state: &Arc<Mutex<State>>, me: UserId, msg: ClientMsg) {
    let mut st = state.lock();
    if !matches!(
        &msg,
        ClientMsg::Audio { .. }
            | ClientMsg::AudioEncoded { .. }
            | ClientMsg::StreamAudio { .. }
            | ClientMsg::Frame { .. }
            | ClientMsg::EncodedFrame { .. }
            | ClientMsg::Cursor(_)
            | ClientMsg::Ping(_)
    ) {
        let rate = st.rates.entry(me).or_insert((Instant::now(), 0));
        if rate.0.elapsed() > Duration::from_secs(10) {
            *rate = (Instant::now(), 0);
        }
        rate.1 += 1;
        if rate.1 > 150 {
            st.send_to(
                me,
                &ServerMsg::Error("Muitas ações. Aguarde alguns segundos.".into()),
            );
            return;
        }
    }
    match msg {
        ClientMsg::Hello { .. } | ClientMsg::Authenticate { .. } => {}
        ClientMsg::Ping(t) => st.send_to(me, &ServerMsg::Pong(t)),
        ClientMsg::Workspace(command) => st.handle_workspace(me, command),
        ClientMsg::SendMessage { channel, text } => {
            if let Err(e) = st.durable_send(me, None, channel, text, None, Vec::new()) {
                st.send_to(me, &ServerMsg::Error(e.to_string()));
            }
        }
        ClientMsg::CreateGuild { name } => {
            if let Err(e) = st.create_community(me, name) {
                st.send_to(me, &ServerMsg::Error(e.to_string()));
            }
        }
        ClientMsg::FriendRequest { name } => {
            let target = st
                .users
                .values()
                .find(|u| u.name.eq_ignore_ascii_case(name.trim()) && u.id != me)
                .map(|u| u.id);
            let Some(t) = target else {
                st.send_to(
                    me,
                    &ServerMsg::Error(format!("No one named {} is on this host yet.", name.trim())),
                );
                return;
            };
            match st.friends.get(&me).and_then(|m| m.get(&t)).copied() {
                Some(FriendStatus::Friend) => {
                    st.send_to(me, &ServerMsg::Error("You are already friends.".into()))
                }
                Some(FriendStatus::Outgoing) => {
                    st.send_to(me, &ServerMsg::Error("Request already sent.".into()))
                }
                Some(FriendStatus::Incoming) => {
                    st.set_relation(me, t, Some(FriendStatus::Friend));
                    st.save();
                }
                None => {
                    st.set_relation(me, t, Some(FriendStatus::Outgoing));
                    st.save();
                }
            }
        }
        ClientMsg::FriendRespond { user, accept } => {
            let current = st.friends.get(&me).and_then(|m| m.get(&user)).copied();
            match (current, accept) {
                (Some(FriendStatus::Incoming), true) => {
                    st.set_relation(me, user, Some(FriendStatus::Friend))
                }
                (Some(_), false) => st.set_relation(me, user, None),
                _ => return,
            }
            st.save();
        }
        ClientMsg::StartCall { members } => {
            if members.len() > 128 || st.calls.values().filter(|c| c.host == me).count() >= 4 {
                st.send_to(
                    me,
                    &ServerMsg::Error("Limite de chamadas pendentes atingido".into()),
                );
                return;
            }
            let mut list: Vec<UserId> = Vec::new();
            for m in members {
                if m != me && st.can_call(me, m) && !list.contains(&m) {
                    list.push(m);
                }
            }
            if list.is_empty() {
                st.send_to(
                    me,
                    &ServerMsg::Error("Pick at least one friend to call.".into()),
                );
                return;
            }
            let id = st.next_call;
            st.next_call += 1;
            let mut all = vec![me];
            all.extend(list);
            let info = CallInfo {
                id,
                host: me,
                members: all.clone(),
            };
            st.calls.insert(id, info.clone());
            st.call_deadlines
                .insert(id, Instant::now() + Duration::from_secs(45));
            for m in all {
                st.send_to(m, &ServerMsg::CallUpdated(info.clone()));
            }
        }
        ClientMsg::InviteToCall { call, user } => {
            let ok = st
                .calls
                .get(&call)
                .map(|c| c.members.contains(&me) && !c.members.contains(&user))
                .unwrap_or(false);
            if !ok || !st.can_call(me, user) {
                return;
            }
            let info = {
                let c = st.calls.get_mut(&call).unwrap();
                c.members.push(user);
                c.clone()
            };
            for m in info.members.clone() {
                st.send_to(m, &ServerMsg::CallUpdated(info.clone()));
            }
        }
        ClientMsg::DeclineCall { call } => {
            if !st.calls.get(&call).is_some_and(|c| c.members.contains(&me)) {
                return;
            }
            if st.voice.get(&me).and_then(|v| v.channel) == Some(call) {
                st.leave_voice(me);
            }
            let mut few = false;
            if let Some(c) = st.calls.get_mut(&call) {
                c.members.retain(|m| *m != me);
                few = c.members.len() < 2;
            }
            st.send_to(me, &ServerMsg::CallEnded { call });
            if few && !st.voice.values().any(|v| v.channel == Some(call)) {
                st.end_call(call);
            } else if let Some(info) = st.calls.get(&call).cloned() {
                for member in &info.members {
                    st.send_to(*member, &ServerMsg::CallUpdated(info.clone()));
                }
            }
        }
        ClientMsg::CreateChannel { guild, name, kind } => {
            if let Err(e) = st.create_channel(me, guild, name, kind, None) {
                st.send_to(me, &ServerMsg::Error(e.to_string()));
            }
        }
        ClientMsg::Typing { channel } => {
            if !st.can_access(me, channel) {
                return;
            }
            let packet = encode_packet(&ServerMsg::Typing { user: me, channel });
            for (id, out) in &st.clients {
                if *id != me && st.can_access(*id, channel) {
                    out.push_urgent(packet.clone());
                }
            }
        }
        ClientMsg::JoinVoice { channel } => {
            if st.voice.get(&me).and_then(|v| v.channel) == Some(channel) {
                return;
            }
            let is_channel = st
                .channels
                .iter()
                .any(|c| c.id == channel && c.kind == ChannelKind::Voice);
            let is_my_call = st
                .calls
                .get(&channel)
                .map(|c| c.members.contains(&me))
                .unwrap_or(false);
            if (!is_channel
                || !st.can_access(me, channel)
                || !st.has_permission(me, channel, permission::CONNECT)
                || st.is_timed_out(me, channel))
                && !is_my_call
            {
                return;
            }
            st.leave_voice(me);
            let v = st.voice.entry(me).or_default();
            v.channel = Some(channel);
            let state = v.clone();
            st.refresh_media_routes();
            st.broadcast(&ServerMsg::Voice { user: me, state });
            if st
                .voice
                .values()
                .filter(|v| v.channel == Some(channel))
                .count()
                >= 2
            {
                st.call_deadlines.remove(&channel);
            }
        }
        ClientMsg::LeaveVoice => st.leave_voice(me),
        ClientMsg::SetVoiceFlags { muted, deafened } => {
            let v = st.voice.entry(me).or_default();
            v.muted = muted || deafened;
            v.deafened = deafened;
            let state = v.clone();
            st.refresh_media_routes();
            st.broadcast(&ServerMsg::Voice { user: me, state });
        }
        ClientMsg::Speaking(speaking) => {
            let peers = st.voice_peers(me);
            let packet = encode_packet(&ServerMsg::Speaking { user: me, speaking });
            for p in peers {
                if let Some(out) = st.clients.get(&p) {
                    out.push_urgent(packet.clone());
                }
            }
        }
        ClientMsg::Audio { seq, pcm } => {
            if pcm.len() != 480 {
                return;
            }
            let muted = st.voice.get(&me).map(|v| v.muted).unwrap_or(true);
            if muted
                || st
                    .voice
                    .get(&me)
                    .and_then(|v| v.channel)
                    .is_some_and(|c| c < CALL_ID_BASE && !st.can_transmit_voice(me, c))
            {
                return;
            }
            let peers = st.voice_peers(me);
            let packet = encode_packet(&ServerMsg::Audio { from: me, seq, pcm });
            for p in peers {
                let deaf = st.voice.get(&p).map(|v| v.deafened).unwrap_or(false);
                if deaf {
                    continue;
                }
                if let Some(out) = st.clients.get(&p) {
                    out.push_urgent(packet.clone());
                }
            }
        }
        ClientMsg::AudioEncoded {
            seq,
            sent_micros,
            opus,
        } => {
            if opus.is_empty()
                || opus.len() > 1275
                || st.voice.get(&me).is_none_or(|v| v.muted)
                || st
                    .voice
                    .get(&me)
                    .and_then(|v| v.channel)
                    .is_some_and(|c| c < CALL_ID_BASE && !st.can_transmit_voice(me, c))
            {
                return;
            }
            let packet = encode_packet(&ServerMsg::AudioEncoded {
                from: me,
                seq,
                sent_micros,
                opus,
            });
            for peer in st.voice_peers(me) {
                if !st.voice.get(&peer).is_some_and(|v| v.deafened) {
                    if let Some(out) = st.clients.get(&peer) {
                        out.push_realtime(packet.clone());
                    }
                }
            }
        }
        ClientMsg::StreamAudio {
            seq,
            sent_micros,
            packet,
        } => {
            if packet.is_empty()
                || packet.len() > 1275
                || !st.streams.contains_key(&(me, StreamKind::Screen))
            {
                return;
            }
            let outgoing = encode_packet(&ServerMsg::StreamAudio {
                from: me,
                seq,
                sent_micros,
                packet,
            });
            if let Some(watchers) = st.watchers.get(&(me, StreamKind::Screen)) {
                for viewer in watchers {
                    if st.can_watch(*viewer, me, StreamKind::Screen)
                        && !st.voice.get(viewer).is_some_and(|v| v.deafened)
                    {
                        if let Some(out) = st.clients.get(viewer) {
                            out.push_realtime(outgoing.clone());
                        }
                    }
                }
            }
        }
        ClientMsg::StartStream(mut info) => {
            if st.voice.get(&me).and_then(|v| v.channel).is_some_and(|c| {
                c < CALL_ID_BASE
                    && (!st.has_permission(me, c, permission::STREAM) || st.is_timed_out(me, c))
            }) {
                return;
            }
            if picture_bytes(info.width, info.height).is_none()
                || info.fps == 0
                || info.fps > 60
                || info.label.len() > 512
            {
                st.send_to(me, &ServerMsg::Error("Invalid stream settings.".into()));
                return;
            }
            if st.voice.get(&me).and_then(|v| v.channel).is_none() {
                st.send_to(me, &ServerMsg::Error("Join a voice channel first.".into()));
                return;
            }
            info.owner = me;
            let kind = info.kind;
            st.streams.insert((me, kind), info.clone());
            st.refresh_media_routes();
            st.broadcast(&ServerMsg::StreamStarted(info));
            if let Some(v) = st.voice.get_mut(&me) {
                match kind {
                    StreamKind::Screen => v.streaming = true,
                    StreamKind::Camera => v.camera = true,
                }
                let state = v.clone();
                st.refresh_media_routes();
                st.broadcast(&ServerMsg::Voice { user: me, state });
            }
            st.viewers_changed((me, kind));
        }
        ClientMsg::StopStream(kind) => st.stop_stream(me, kind),
        // Pointer updates are forwarded by the media router and never reach the state actor.
        ClientMsg::Cursor(_) => {}
        ClientMsg::EncodedFrame { kind, frame } => {
            if !st.streams.contains_key(&(me, kind))
                || picture_bytes(frame.width, frame.height).is_none()
                || frame.data.is_empty()
                || frame.data.len() > 2 * 1024 * 1024
            {
                return;
            }
            let targets: Vec<_> = match kind {
                StreamKind::Screen => st
                    .watchers
                    .get(&(me, kind))
                    .map(|w| {
                        w.iter()
                            .copied()
                            .filter(|u| st.can_watch(*u, me, kind))
                            .collect()
                    })
                    .unwrap_or_default(),
                StreamKind::Camera => st.voice_peers(me),
            };
            let packet = encode_packet(&ServerMsg::EncodedFrame {
                owner: me,
                kind,
                frame,
            });
            let mut dropped = false;
            for viewer in targets {
                if let Some(out) = st.clients.get(&viewer) {
                    dropped |= out.push_bulk(packet.clone());
                }
            }
            if dropped {
                let key = (me, kind);
                if st
                    .last_keyframe_ask
                    .get(&key)
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
                {
                    st.last_keyframe_ask.insert(key, Instant::now());
                    st.send_to(me, &ServerMsg::KeyframeWanted { kind });
                }
            }
        }
        ClientMsg::Frame { kind, frame } => {
            if !st.streams.contains_key(&(me, kind)) || !frame.validate() {
                return;
            }
            let targets: Vec<UserId> = match kind {
                StreamKind::Screen => st
                    .watchers
                    .get(&(me, kind))
                    .map(|s| {
                        s.iter()
                            .copied()
                            .filter(|viewer| st.can_watch(*viewer, me, kind))
                            .collect()
                    })
                    .unwrap_or_default(),
                StreamKind::Camera => st.voice_peers(me),
            };
            if targets.is_empty() {
                return;
            }
            let packet = encode_packet(&ServerMsg::Frame {
                owner: me,
                kind,
                frame,
            });
            let mut lost = false;
            for t in targets {
                if let Some(out) = st.clients.get(&t) {
                    lost |= out.push_bulk(packet.clone());
                }
            }
            if lost {
                // A viewer missed pictures: ask for a full one, at most once a second.
                let key = (me, kind);
                let due = st
                    .last_keyframe_ask
                    .get(&key)
                    .map(|t| t.elapsed() > Duration::from_secs(1))
                    .unwrap_or(true);
                if due {
                    st.last_keyframe_ask.insert(key, Instant::now());
                    st.send_to(me, &ServerMsg::KeyframeWanted { kind });
                }
            }
        }
        ClientMsg::Watch { owner, kind } => {
            if !st.can_watch(me, owner, kind) {
                st.send_to(
                    me,
                    &ServerMsg::Error("Join the stream's voice channel before watching.".into()),
                );
                return;
            }
            st.watchers.entry((owner, kind)).or_default().insert(me);
            st.refresh_media_routes();
            st.send_to(owner, &ServerMsg::KeyframeWanted { kind });
            st.viewers_changed((owner, kind));
        }
        ClientMsg::Unwatch { owner, kind } => {
            if let Some(set) = st.watchers.get_mut(&(owner, kind)) {
                set.remove(&me);
            }
            st.refresh_media_routes();
            st.viewers_changed((owner, kind));
        }
        ClientMsg::RequestKeyframe { owner, kind } => {
            if st.can_watch(me, owner, kind) {
                st.send_to(owner, &ServerMsg::KeyframeWanted { kind });
            }
        }
    }
}
