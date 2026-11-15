use super::*;

#[test]
fn screen_watch_requires_the_same_channel_and_is_revoked_on_switch() {
    let mut state = fixture();
    for (id, channel) in [(1, Some(3)), (2, Some(3)), (3, None)] {
        state.voice.insert(
            id,
            VoiceState {
                channel,
                ..VoiceState::default()
            },
        );
    }
    state.streams.insert(
        (1, StreamKind::Screen),
        StreamInfo {
            owner: 1,
            kind: StreamKind::Screen,
            label: "screen".into(),
            width: 64,
            height: 64,
            fps: 30,
        },
    );
    let state = Arc::new(Mutex::new(state));
    handle_msg(
        &state,
        3,
        ClientMsg::Watch {
            owner: 1,
            kind: StreamKind::Screen,
        },
    );
    assert!(!state.lock().watchers.contains_key(&(1, StreamKind::Screen)));
    handle_msg(
        &state,
        2,
        ClientMsg::Watch {
            owner: 1,
            kind: StreamKind::Screen,
        },
    );
    assert!(state.lock().watchers[&(1, StreamKind::Screen)].contains(&2));
    handle_msg(&state, 2, ClientMsg::JoinVoice { channel: 4 });
    assert!(!state.lock().watchers[&(1, StreamKind::Screen)].contains(&2));
    assert!(!state.lock().can_watch(2, 1, StreamKind::Screen));
    handle_msg(
        &state,
        2,
        ClientMsg::Watch {
            owner: 1,
            kind: StreamKind::Screen,
        },
    );
    assert!(!state.lock().watchers[&(1, StreamKind::Screen)].contains(&2));
}

#[test]
fn declining_an_active_private_call_revokes_voice_and_screen_access() {
    let mut state = fixture();
    let call = CALL_ID_BASE;
    state.calls.insert(
        call,
        CallInfo {
            id: call,
            host: 1,
            members: vec![1, 2],
        },
    );
    for id in [1, 2] {
        state.voice.insert(
            id,
            VoiceState {
                channel: Some(call),
                ..VoiceState::default()
            },
        );
    }
    state.streams.insert(
        (1, StreamKind::Screen),
        StreamInfo {
            owner: 1,
            kind: StreamKind::Screen,
            label: "screen".into(),
            width: 64,
            height: 64,
            fps: 30,
        },
    );
    state
        .watchers
        .insert((1, StreamKind::Screen), HashSet::from([2]));
    let state = Arc::new(Mutex::new(state));
    handle_msg(&state, 2, ClientMsg::DeclineCall { call });
    let state = state.lock();
    assert_eq!(state.voice[&2].channel, None);
    assert!(!state.calls[&call].members.contains(&2));
    assert!(!state.can_watch(2, 1, StreamKind::Screen));
    assert!(!state.watchers[&(1, StreamKind::Screen)].contains(&2));
}

#[test]
fn chat_voice_and_screen_share_are_relayed() {
    let server = host();
    let (mut aw, mut ar, a) = client(server.port, "Ana");
    let (mut bw, mut br, _b) = client(server.port, "Ben");

    // Text reaches everyone.
    write_packet(
        &mut aw,
        &ClientMsg::SendMessage {
            channel: 1,
            text: "hi".into(),
        },
    )
    .unwrap();
    let m = wait_for(&mut br, |m| matches!(m, ServerMsg::Message(_)));
    let ServerMsg::Message(m) = m else {
        unreachable!()
    };
    assert_eq!(m.text, "hi");

    // Both join the Lounge; Ana's voice reaches Ben.
    write_packet(&mut aw, &ClientMsg::JoinVoice { channel: 3 }).unwrap();
    write_packet(&mut bw, &ClientMsg::JoinVoice { channel: 3 }).unwrap();
    wait_for(
        &mut ar,
        |m| matches!(m, ServerMsg::Voice { state, .. } if state.channel == Some(3)),
    );
    wait_for(
        &mut br,
        |m| matches!(m, ServerMsg::Voice { user, state } if *user != a && state.channel == Some(3)),
    );
    std::thread::sleep(Duration::from_millis(100));
    write_packet(
        &mut aw,
        &ClientMsg::Audio {
            seq: 1,
            pcm: vec![7; 480],
        },
    )
    .unwrap();
    let au = wait_for(&mut br, |m| matches!(m, ServerMsg::Audio { .. }));
    assert!(matches!(au, ServerMsg::Audio { from, .. } if from == a));

    // Ana goes live; Ben watches; the owner is asked for a full picture; frames reach Ben.
    let info = StreamInfo {
        owner: 0,
        kind: StreamKind::Screen,
        label: "Screen 1".into(),
        width: 64,
        height: 64,
        fps: 30,
    };
    write_packet(&mut aw, &ClientMsg::StartStream(info)).unwrap();
    wait_for(&mut br, |m| matches!(m, ServerMsg::StreamStarted(_)));
    let (mut outsider, mut outsider_reader, _) = client(server.port, "Outsider");
    write_packet(
        &mut outsider,
        &ClientMsg::Watch {
            owner: a,
            kind: StreamKind::Screen,
        },
    )
    .unwrap();
    assert!(matches!(
        wait_for(&mut outsider_reader, |m| matches!(m, ServerMsg::Error(_))),
        ServerMsg::Error(_)
    ));
    write_packet(
        &mut bw,
        &ClientMsg::Watch {
            owner: a,
            kind: StreamKind::Screen,
        },
    )
    .unwrap();
    wait_for(&mut ar, |m| {
        matches!(
            m,
            ServerMsg::KeyframeWanted {
                kind: StreamKind::Screen
            }
        )
    });
    let mut enc = crate::media::codec::TileEncoder::new();
    let pic = vec![200u8; 64 * 64 * 4];
    let frame = enc.encode(&pic, 64, 64, 80, true).unwrap();
    write_packet(
        &mut aw,
        &ClientMsg::Frame {
            kind: StreamKind::Screen,
            frame,
        },
    )
    .unwrap();
    let f = wait_for(&mut br, |m| matches!(m, ServerMsg::Frame { .. }));
    let ServerMsg::Frame { owner, frame, .. } = f else {
        unreachable!()
    };
    assert_eq!(owner, a);
    assert!(frame.keyframe);

    // An unchanged picture sends nothing; one changed tile sends one tile.
    assert!(enc.encode(&pic, 64, 64, 80, false).is_none());
    let mut pic2 = vec![200u8; 128 * 64 * 4];
    pic2[0] = 0;
    let mut enc2 = crate::media::codec::TileEncoder::new();
    enc2.encode(&vec![200u8; 128 * 64 * 4], 128, 64, 80, true)
        .unwrap();
    let delta = enc2.encode(&pic2, 128, 64, 80, false).unwrap();
    assert_eq!(delta.tiles.len(), 1);
    server.stop();
}

#[test]
fn friends_servers_and_group_calls() {
    let server = host();
    let (mut aw, mut ar, a) = client(server.port, "Ana");
    let (mut bw, mut br, b) = client(server.port, "Ben");
    let (mut cw, mut cr, c) = client(server.port, "Cleo");

    // Ana asks Ben and Cleo; both accept.
    for name in ["Ben", "Cleo"] {
        write_packet(&mut aw, &ClientMsg::FriendRequest { name: name.into() }).unwrap();
    }
    wait_for(
        &mut br,
        |m| matches!(m, ServerMsg::Friends(l) if l.iter().any(|f| f.user == a && f.status == FriendStatus::Incoming)),
    );
    write_packet(
        &mut bw,
        &ClientMsg::FriendRespond {
            user: a,
            accept: true,
        },
    )
    .unwrap();
    wait_for(
        &mut cr,
        |m| matches!(m, ServerMsg::Friends(l) if l.iter().any(|f| f.user == a && f.status == FriendStatus::Incoming)),
    );
    write_packet(
        &mut cw,
        &ClientMsg::FriendRespond {
            user: a,
            accept: true,
        },
    )
    .unwrap();
    wait_for(
        &mut ar,
        |m| matches!(m, ServerMsg::Friends(l) if l.iter().filter(|f| f.status == FriendStatus::Friend).count() == 2),
    );

    // Unknown names are refused.
    write_packet(
        &mut aw,
        &ClientMsg::FriendRequest {
            name: "Nobody".into(),
        },
    )
    .unwrap();
    wait_for(&mut ar, |m| matches!(m, ServerMsg::Error(_)));

    // A new server appears for everyone with its own channels.
    write_packet(
        &mut aw,
        &ClientMsg::CreateGuild {
            name: "Club".into(),
        },
    )
    .unwrap();
    // Communities are private: only the creator receives the new community and its channels.
    let ServerMsg::Workspace(WorkspaceEvent::Resync { snapshot }) = wait_for(
        &mut ar,
        |m| matches!(m, ServerMsg::Workspace(WorkspaceEvent::Resync { snapshot }) if snapshot.guilds.iter().any(|g| g.name == "Club")),
    ) else {
        unreachable!()
    };
    let g = snapshot.guilds.iter().find(|g| g.name == "Club").unwrap();
    assert!(snapshot.channels.iter().any(|c| c.guild == g.id));

    // Group call: Ana calls Ben and Cleo, everyone joins, voice flows, and it ends when empty.
    write_packet(
        &mut aw,
        &ClientMsg::StartCall {
            members: vec![b, c],
        },
    )
    .unwrap();
    let ServerMsg::CallUpdated(call) =
        wait_for(&mut ar, |m| matches!(m, ServerMsg::CallUpdated(_)))
    else {
        unreachable!()
    };
    assert!(call.id >= CALL_ID_BASE && call.members.len() == 3 && call.host == a);
    wait_for(&mut br, |m| matches!(m, ServerMsg::CallUpdated(_)));
    wait_for(&mut cr, |m| matches!(m, ServerMsg::CallUpdated(_)));
    for w in [&mut aw, &mut bw, &mut cw] {
        write_packet(w, &ClientMsg::JoinVoice { channel: call.id }).unwrap();
    }
    wait_for(
        &mut ar,
        |m| matches!(m, ServerMsg::Voice { user, state } if *user == c && state.channel == Some(call.id)),
    );
    std::thread::sleep(Duration::from_millis(100));
    write_packet(
        &mut bw,
        &ClientMsg::Audio {
            seq: 1,
            pcm: vec![5; 480],
        },
    )
    .unwrap();
    wait_for(
        &mut ar,
        |m| matches!(m, ServerMsg::Audio { from, .. } if *from == b),
    );
    wait_for(
        &mut cr,
        |m| matches!(m, ServerMsg::Audio { from, .. } if *from == b),
    );

    // Someone who is not in the call cannot join it.
    let (mut dw, mut dr, _d) = client(server.port, "Dan");
    write_packet(&mut dw, &ClientMsg::JoinVoice { channel: call.id }).unwrap();
    write_packet(&mut dw, &ClientMsg::Ping(1)).unwrap();
    wait_for(&mut dr, |m| matches!(m, ServerMsg::Pong(1)));

    for w in [&mut aw, &mut bw, &mut cw] {
        write_packet(w, &ClientMsg::LeaveVoice).unwrap();
    }
    wait_for(
        &mut ar,
        |m| matches!(m, ServerMsg::CallEnded { call: id } if *id == call.id),
    );
    server.stop();
}
