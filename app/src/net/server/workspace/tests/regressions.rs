//! Regression coverage for the audit corrections.
use super::*;

#[test]
fn shared_attachment_limit_rejects_a_ninth_attachment() {
    let mut st = fixture();
    st.workspace_command(
        1,
        WorkspaceCommand::Upload {
            operation: "nine-file".into(),
            channel: 1,
            name: "test.txt".into(),
            media_type: "text/plain".into(),
            bytes: vec![1],
        },
    )
    .unwrap();
    let id = st.workspace.next_attachment - 1;
    let result = st.durable_send(1, None, 1, "nine files".into(), None, vec![id; 9]);
    assert!(result.unwrap_err().to_string().contains("oito anexos"));
}

#[test]
fn regression_persistence_cost_with_avatar_metadata() {
    for avatar_bytes in [0, 4 * 1024 * 1024] {
        let mut st = fixture();
        // Synthetic metadata at the global avatar budget, split across 16 profiles.
        // This isolates serialization/cloning; it is not a media/image benchmark.
        for user in 1..=16 {
            st.workspace.profiles.push(Profile {
                user,
                avatar: vec![127; avatar_bytes / 16],
                ..Default::default()
            });
        }
        let mut store = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
        store.import(&st.saved(false), &[]).unwrap();
        st.store = Some(Arc::new(Mutex::new(store)));
        let started = Instant::now();
        for i in 0..20 {
            st.durable_send(
                1,
                Some(format!("cost-{i}")),
                1,
                "hello".into(),
                None,
                vec![],
            )
            .unwrap();
        }
        eprintln!(
            "AUDIT_COST avatar_bytes={avatar_bytes} sends=20 elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }
}

#[test]
fn demoted_admin_loses_voice_and_receives_no_more_audio() {
    let mut st = fixture();
    let channel = st
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Voice)
        .unwrap()
        .id;
    st.workspace_command(
        1,
        WorkspaceCommand::SetRole {
            guild: 1,
            user: 2,
            role: MemberRole::Admin,
        },
    )
    .unwrap();
    community(
        &mut st,
        1,
        CommunityCommand::SetOverride {
            value: PermissionOverride {
                scope: PermissionScope::Channel(channel),
                target: PermissionTarget::Member(2),
                allow: 0,
                deny: permission::VIEW,
            },
        },
    )
    .unwrap();
    assert!(st.can_access(2, channel));
    for user in [2, 3] {
        st.voice.insert(
            user,
            VoiceState {
                channel: Some(channel),
                ..Default::default()
            },
        );
    }
    st.workspace_command(
        1,
        WorkspaceCommand::SetRole {
            guild: 1,
            user: 2,
            role: MemberRole::Member,
        },
    )
    .unwrap();
    assert!(!st.can_access(2, channel));
    assert_eq!(st.voice[&2].channel, None);
    let (out2, mut peer2) = pipe();
    let (out3, _peer3) = pipe();
    st.clients.insert(2, out2.clone());
    st.clients.insert(3, out3.clone());
    st.refresh_media_routes();
    st.media_router.dispatch(
        3,
        ClientMsg::Audio {
            seq: 1,
            pcm: vec![0; 480],
        },
    );
    peer2
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    assert!(read_packet::<_, ServerMsg>(&mut peer2).is_err());
    out2.close();
    out3.close();
}

#[test]
fn download_pieces_do_not_consume_control_action_budget() {
    let mut st = fixture();
    st.uploads.insert(
        1,
        (
            AttachmentInfo {
                id: 1,
                channel: 1,
                uploader: 1,
                name: "test.bin".into(),
                media_type: "application/octet-stream".into(),
                size: 1,
            },
            vec![1],
        ),
    );
    let (out, mut peer) = pipe();
    st.clients.insert(2, out.clone());
    let state = Arc::new(Mutex::new(st));
    for _ in 0..150 {
        handle_msg(
            &state,
            2,
            ClientMsg::Workspace(WorkspaceCommand::DownloadChunk {
                attachment: 1,
                offset: 0,
            }),
        );
        assert!(matches!(
            read_packet::<_, ServerMsg>(&mut peer).unwrap(),
            ServerMsg::Workspace(WorkspaceEvent::AttachmentChunk { .. })
        ));
    }
    handle_msg(
        &state,
        2,
        ClientMsg::Workspace(WorkspaceCommand::DownloadChunk {
            attachment: 1,
            offset: 0,
        }),
    );
    assert!(matches!(
        read_packet::<_, ServerMsg>(&mut peer).unwrap(),
        ServerMsg::Workspace(WorkspaceEvent::AttachmentChunk { .. })
    ));
    out.close();
}

#[test]
fn deleting_the_last_message_reference_removes_attachment_and_quota() {
    let mut st = fixture();
    let mut store = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
    store.import(&st.saved(false), &[]).unwrap();
    st.store = Some(Arc::new(Mutex::new(store)));
    st.workspace_command(
        1,
        WorkspaceCommand::Upload {
            operation: "audit-file".into(),
            channel: 1,
            name: "test.txt".into(),
            media_type: "text/plain".into(),
            bytes: b"private payload".to_vec(),
        },
    )
    .unwrap();
    let attachment = st.workspace.next_attachment - 1;
    st.durable_send(1, None, 1, "file".into(), None, vec![attachment])
        .unwrap();
    let message = st.messages.last().unwrap().id;
    let total = st.stored_attachment_bytes().unwrap();
    st.workspace_command(1, WorkspaceCommand::Delete { message })
        .unwrap();
    assert!(st.find_message(message).is_err());
    assert!(total > 0);
    assert_eq!(st.stored_attachment_bytes().unwrap(), 0);
    assert!(st.find_attachment(attachment).is_err());
    assert!(st
        .workspace_command(2, WorkspaceCommand::Download { attachment })
        .is_err());
}

#[test]
fn deletion_publishes_the_updated_unread_summary() {
    let mut st = fixture();
    st.durable_send(1, None, 1, "unread".into(), None, vec![])
        .unwrap();
    assert_eq!(st.unread_summary_for(2, 1).count, 1);
    let (out, mut peer) = pipe();
    st.clients.insert(2, out.clone());
    st.workspace_command(1, WorkspaceCommand::Delete { message: 1 })
        .unwrap();
    assert_eq!(st.unread_summary_for(2, 1).count, 0);
    assert!(matches!(
        read_packet::<_, ServerMsg>(&mut peer).unwrap(),
        ServerMsg::Workspace(WorkspaceEvent::MessageDeleted { .. })
    ));
    assert!(matches!(
        read_packet::<_, ServerMsg>(&mut peer).unwrap(),
        ServerMsg::Workspace(WorkspaceEvent::Community(CommunityEvent::Unread(_)))
    ));
    out.close();
}

#[test]
fn expired_persisted_uploads_do_not_block_admission() {
    let mut st = fixture();
    let mut store = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
    store.import(&st.saved(false), &[]).unwrap();
    for owner in 1..=3 {
        store
            .begin_resumable(
                &crate::storage::PartialUpload {
                    user: 2,
                    operation: format!("old-{owner}"),
                    info: AttachmentInfo {
                        id: owner,
                        channel: 1,
                        uploader: 2,
                        name: "test.bin".into(),
                        media_type: "application/octet-stream".into(),
                        size: 1,
                    },
                    digest: [0; 32],
                    created_at: now_secs() - 8 * 86400,
                },
                &st.saved(false),
            )
            .unwrap();
    }
    st.workspace.next_attachment = 4;
    let shared = Arc::new(Mutex::new(store));
    st.pending_uploads.attach(shared.clone(), None);
    st.store = Some(shared);
    let result = st.workspace_command(
        2,
        WorkspaceCommand::UploadBeginResumable {
            operation: "fresh".into(),
            channel: 1,
            name: "new.bin".into(),
            media_type: "application/octet-stream".into(),
            size: 1,
            digest: [0; 32],
        },
    );
    assert!(
        result.is_ok(),
        "expired persisted uploads must not consume admission slots"
    );
}

#[test]
fn query_replies_echo_the_generation_even_when_rate_limited() {
    let mut st = fixture();
    let (out, mut peer) = pipe();
    st.clients.insert(2, out.clone());
    st.rates.insert(2, (Instant::now(), 150));
    let state = Arc::new(Mutex::new(st));
    handle_msg(
        &state,
        2,
        ClientMsg::Workspace(WorkspaceCommand::Query {
            id: 42,
            command: Box::new(QueryCommand::History {
                channel: 1,
                before: None,
                limit: 10,
            }),
        }),
    );
    assert!(
        matches!(read_packet::<_,ServerMsg>(&mut peer).unwrap(),ServerMsg::Workspace(WorkspaceEvent::QueryReply {id:42,event}) if matches!(*event,QueryResult::Error(_)))
    );
    out.close();
}

#[test]
fn tagged_history_is_authorized_and_keeps_its_request_id() {
    let mut st = fixture();
    let (out, mut peer) = pipe();
    st.clients.insert(2, out.clone());
    st.handle_workspace(
        2,
        WorkspaceCommand::Query {
            id: 7,
            command: Box::new(QueryCommand::History {
                channel: 999,
                before: None,
                limit: 10,
            }),
        },
    );
    assert!(
        matches!(read_packet::<_,ServerMsg>(&mut peer).unwrap(),ServerMsg::Workspace(WorkspaceEvent::QueryReply {id:7,event}) if matches!(*event,QueryResult::Error(_)))
    );
    assert!(st.query.is_none());
    out.close();
}

#[test]
fn deleting_a_channel_collects_its_attachments_and_history_atomically() {
    let mut st = fixture();
    let mut db = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
    db.import(&st.saved(false), &[]).unwrap();
    st.store = Some(Arc::new(Mutex::new(db)));
    st.workspace_command(
        1,
        WorkspaceCommand::Upload {
            operation: "channel-file".into(),
            channel: 1,
            name: "a.txt".into(),
            media_type: "text/plain".into(),
            bytes: vec![1; 10],
        },
    )
    .unwrap();
    let id = st.workspace.next_attachment - 1;
    st.durable_send(1, None, 1, "file".into(), None, vec![id])
        .unwrap();
    st.workspace_command(1, WorkspaceCommand::DeleteChannel { channel: 1 })
        .unwrap();
    let db = st.store.as_ref().unwrap().lock();
    assert!(db.history(1, None, 100).unwrap().is_empty());
    assert!(db.attachment_info(id).unwrap().is_none());
    assert_eq!(db.attachment_bytes().unwrap(), 0);
}
