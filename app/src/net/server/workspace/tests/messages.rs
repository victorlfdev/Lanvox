use super::*;

#[test]
fn thread_inherits_parent_privacy_archiving_and_tags_are_authorized() {
    let mut st = fixture();
    community(
        &mut st,
        2,
        CommunityCommand::CreateThread {
            parent: 1,
            message: None,
            name: "Topic".into(),
        },
    )
    .unwrap();
    let channel = st.workspace.community.threads.last().unwrap().channel;
    st.durable_send(2, None, channel, "discussion".into(), None, vec![])
        .unwrap();
    assert!(community(
        &mut st,
        3,
        CommunityCommand::SetThreadArchived {
            channel,
            archived: true
        }
    )
    .is_err());
    community(
        &mut st,
        2,
        CommunityCommand::SetThreadTags {
            channel,
            tags: vec!["Games".into(), "Games".into(), "".into()],
        },
    )
    .unwrap();
    assert_eq!(st.workspace.community.threads[0].tags, vec!["Games"]);
    community(
        &mut st,
        2,
        CommunityCommand::SetThreadArchived {
            channel,
            archived: true,
        },
    )
    .unwrap();
    assert!(st
        .durable_send(2, None, channel, "blocked".into(), None, vec![])
        .is_err());
    community(
        &mut st,
        1,
        CommunityCommand::SetOverride {
            value: PermissionOverride {
                scope: PermissionScope::Channel(1),
                target: PermissionTarget::Member(2),
                allow: 0,
                deny: permission::VIEW,
            },
        },
    )
    .unwrap();
    assert!(!st.can_access(2, channel));
    assert!(!st
        .workspace_snapshot(2)
        .community
        .threads
        .iter()
        .any(|t| t.channel == channel));
    assert!(community(
        &mut st,
        2,
        CommunityCommand::CreateThread {
            parent: channel,
            message: None,
            name: "Nested".into()
        }
    )
    .is_err());
}

#[test]
fn durable_unread_counts_and_context_do_not_depend_on_the_cache() {
    let mut st = fixture();
    let mut store = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
    store.import(&st.saved(false), &[]).unwrap();
    st.store = Some(Arc::new(Mutex::new(store)));
    for i in 0..260 {
        st.durable_send(
            2,
            Some(format!("message-{i}")),
            1,
            if i % 10 == 0 { "hello <@1>" } else { "hello" }.into(),
            None,
            vec![],
        )
        .unwrap();
    }
    assert_eq!(st.messages.len(), CACHE_PER_CHANNEL);
    let unread = st.unread_summary_for(1, 1);
    assert_eq!(unread.count, 260);
    assert_eq!(unread.mentions, 26);
    assert_eq!(unread.first, Some(1));
    st.workspace_command(
        1,
        WorkspaceCommand::Read {
            channel: 1,
            message: 200,
        },
    )
    .unwrap();
    assert_eq!(st.unread_summary_for(1, 1).count, 60);
    community(
        &mut st,
        1,
        CommunityCommand::MarkUnread {
            channel: 1,
            message: 10,
        },
    )
    .unwrap();
    assert_eq!(st.unread_summary_for(1, 1).count, 251);
    let (out, mut peer) = pipe();
    st.clients.insert(1, out);
    community(
        &mut st,
        1,
        CommunityCommand::MessageContext {
            channel: 1,
            message: 130,
            limit: 10,
        },
    )
    .unwrap();
    let event = crate::protocol::read_packet::<_, ServerMsg>(&mut peer).unwrap();
    match event {
        ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::Context {
            messages,
            has_before,
            has_after,
            ..
        })) => {
            assert_eq!(messages.first().unwrap().id, 125);
            assert_eq!(messages.last().unwrap().id, 135);
            assert!(has_before && has_after);
        }
        _ => panic!("wrong event"),
    }
}

#[test]
fn forwarding_clones_authorized_attachments_without_exposing_the_original() {
    let mut st = fixture();
    st.workspace_command(
        1,
        WorkspaceCommand::OpenConversation {
            name: "".into(),
            members: vec![2],
            direct: true,
        },
    )
    .unwrap();
    let source = st.workspace.conversations[0].channel;
    st.workspace_command(
        1,
        WorkspaceCommand::Upload {
            operation: "file".into(),
            channel: source,
            name: "private.txt".into(),
            media_type: "text/plain".into(),
            bytes: b"data".to_vec(),
        },
    )
    .unwrap();
    let attachment = st.workspace.next_attachment - 1;
    st.durable_send(1, None, source, "original".into(), None, vec![attachment])
        .unwrap();
    let message = st.messages.last().unwrap().id;
    assert!(community(
        &mut st,
        3,
        CommunityCommand::Forward {
            operation: "forward-stable".into(),
            message,
            target: 1,
            note: String::new()
        }
    )
    .is_err());
    community(
        &mut st,
        2,
        CommunityCommand::Forward {
            operation: "forward-stable".into(),
            message,
            target: 1,
            note: "For everyone".into(),
        },
    )
    .unwrap();
    let forwarded = st.messages.last().unwrap();
    assert_eq!(forwarded.author, 2);
    assert_eq!(forwarded.channel, 1);
    assert!(forwarded.text.contains("Encaminhada:\noriginal"));
    let details = st.details_for(std::slice::from_ref(forwarded));
    let copy = &details[0].attachments[0];
    assert_ne!(copy.id, attachment);
    assert_eq!(copy.channel, 1);
    assert_eq!(copy.uploader, 2);
    st.workspace_command(
        3,
        WorkspaceCommand::Download {
            attachment: copy.id,
        },
    )
    .unwrap();
    assert!(st
        .workspace_command(3, WorkspaceCommand::Download { attachment })
        .is_err());
}

#[test]
fn history_after_is_authorized_and_pages_without_repeating_the_anchor() {
    let mut st = fixture();
    for i in 0..12 {
        st.durable_send(1, None, 1, format!("Message {i}"), None, vec![])
            .unwrap();
    }
    let (out, mut peer) = pipe();
    st.clients.insert(2, out);
    community(
        &mut st,
        2,
        CommunityCommand::HistoryAfter {
            channel: 1,
            after: 3,
            limit: 5,
        },
    )
    .unwrap();
    match crate::protocol::read_packet::<_, ServerMsg>(&mut peer).unwrap() {
        ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::AfterHistory {
            messages,
            has_after,
            ..
        })) => {
            assert_eq!(messages.first().unwrap().id, 4);
            assert_eq!(messages.last().unwrap().id, 8);
            assert!(has_after);
        }
        _ => panic!("wrong event"),
    }
    community(
        &mut st,
        1,
        CommunityCommand::SetOverride {
            value: PermissionOverride {
                scope: PermissionScope::Channel(1),
                target: PermissionTarget::Member(2),
                allow: 0,
                deny: permission::VIEW,
            },
        },
    )
    .unwrap();
    assert!(community(
        &mut st,
        2,
        CommunityCommand::HistoryAfter {
            channel: 1,
            after: 3,
            limit: 5
        }
    )
    .is_err());
}
