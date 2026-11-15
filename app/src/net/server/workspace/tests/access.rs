use super::*;

#[test]
fn host_access_revocation_overrides_owner_allowlist_and_registration_policy() {
    let mut st = fixture();
    let key = st.public_keys[&2];
    st.options.allowlist.push(key);
    st.options.registration_token = Some("invite".into());
    assert!(st
        .workspace_command(
            2,
            WorkspaceCommand::SetHostAccess {
                user: 1,
                allowed: false
            }
        )
        .is_err());
    st.workspace_command(
        1,
        WorkspaceCommand::SetHostAccess {
            user: 2,
            allowed: false,
        },
    )
    .unwrap();
    assert!(st.identify("Pessoa 2", key).is_err());
    st.workspace_command(
        1,
        WorkspaceCommand::SetHostAccess {
            user: 2,
            allowed: true,
        },
    )
    .unwrap();
    assert_eq!(st.identify("Pessoa 2", key).unwrap(), 2);
    st.options.revoked_keys.push(key);
    st.options.owner_key = Some(key);
    assert!(st.identify("Pessoa 2", key).is_err());
    assert!(st
        .workspace_command(
            1,
            WorkspaceCommand::SetHostAccess {
                user: 2,
                allowed: true
            }
        )
        .is_err());
}

#[test]
fn private_conversations_filter_snapshots_history_search_and_attachments() {
    let mut st = fixture();
    st.workspace_command(
        1,
        WorkspaceCommand::OpenConversation {
            name: String::new(),
            members: vec![2],
            direct: true,
        },
    )
    .unwrap();
    let channel = st.workspace.conversations[0].channel;
    assert!(st.can_access(1, channel) && st.can_access(2, channel));
    assert!(!st.can_access(3, channel));
    assert!(!st.snapshot(3).channels.iter().any(|c| c.id == channel));
    assert!(st.workspace_snapshot(3).conversations.is_empty());
    st.durable_send(
        1,
        Some("first".into()),
        channel,
        "segredo".into(),
        None,
        Vec::new(),
    )
    .unwrap();
    for command in [
        WorkspaceCommand::History {
            channel,
            before: None,
            limit: 100,
        },
        WorkspaceCommand::Search {
            channel: Some(channel),
            query: "segredo".into(),
        },
        WorkspaceCommand::Send {
            operation: "intruder".into(),
            channel,
            text: "não".into(),
            reply_to: None,
            attachments: Vec::new(),
        },
    ] {
        assert!(st.workspace_command(3, command).is_err());
    }
    st.workspace_command(
        1,
        WorkspaceCommand::Upload {
            operation: "file".into(),
            channel,
            name: "../segredo.txt".into(),
            media_type: "text/plain".into(),
            bytes: b"privado".to_vec(),
        },
    )
    .unwrap();
    let id = st.workspace.next_attachment - 1;
    assert!(st
        .workspace_command(3, WorkspaceCommand::Download { attachment: id })
        .is_err());
    assert!(st
        .workspace_command(
            2,
            WorkspaceCommand::Send {
                operation: "stolen".into(),
                channel,
                text: String::new(),
                reply_to: None,
                attachments: vec![id]
            }
        )
        .is_err());
    assert_eq!(st.messages.len(), 1);
}

#[test]
fn permissions_revocation_roles_and_membership_are_enforced() {
    let mut st = fixture();
    assert!(st
        .create_channel(2, 1, "não permitido".into(), ChannelKind::Text, None)
        .is_err());
    st.create_community(1, "Clube".into()).unwrap();
    let guild = st.guilds.last().unwrap().id;
    let channel = st.channels.last().unwrap().id;
    assert!(!st.can_access(2, channel));
    st.workspace_command(
        1,
        WorkspaceCommand::CreateInvite {
            guild,
            max_uses: 1,
            expires_hours: 1,
        },
    )
    .unwrap();
    let code = st.workspace.invites.last().unwrap().code.clone();
    st.workspace_command(2, WorkspaceCommand::RedeemInvite { code: code.clone() })
        .unwrap();
    assert!(st.can_access(2, channel));
    assert!(st
        .workspace_command(3, WorkspaceCommand::RedeemInvite { code })
        .is_err());
    st.voice.insert(
        2,
        VoiceState {
            channel: Some(channel),
            ..Default::default()
        },
    );
    st.workspace_command(
        1,
        WorkspaceCommand::RemoveMember {
            guild,
            user: 2,
            ban: true,
        },
    )
    .unwrap();
    assert!(!st.can_access(2, channel));
    assert_eq!(st.voice[&2].channel, None);
    assert!(st
        .workspace_command(
            2,
            WorkspaceCommand::SetRole {
                guild,
                user: 3,
                role: MemberRole::Admin
            }
        )
        .is_err());
}

#[test]
fn idempotency_is_durable_and_failed_commit_does_not_publish() {
    let dir = std::env::temp_dir().join(format!("lanvox-transaction-{}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("host.sqlite3");
    let mut st = fixture();
    let mut store = crate::storage::Store::open(&path).unwrap();
    store.import(&st.saved(false), &[]).unwrap();
    st.store = Some(Arc::new(Mutex::new(store)));
    st.durable_send(
        1,
        Some("stable-operation".into()),
        1,
        "confirmada".into(),
        None,
        Vec::new(),
    )
    .unwrap();
    st.store = None;
    st.messages.clear();
    st.operations.clear();
    st.store = Some(Arc::new(Mutex::new(
        crate::storage::Store::open(&path).unwrap(),
    )));
    st.durable_send(
        1,
        Some("stable-operation".into()),
        1,
        "confirmada".into(),
        None,
        Vec::new(),
    )
    .unwrap();
    assert_eq!(st.page(1, None, 100).unwrap().len(), 1);
    assert_eq!(st.next_message, 2);
    let before = st.next_message;
    assert!(st
        .durable_send(
            1,
            Some("stable-operation".into()),
            999,
            "inacessível".into(),
            None,
            Vec::new()
        )
        .is_err());
    assert_eq!(st.next_message, before);
    drop(st);
    std::fs::remove_file(path).unwrap();
    for suffix in ["sqlite3-wal", "sqlite3-shm"] {
        let p = dir.join("host.sqlite3").with_extension(suffix);
        if p.exists() {
            std::fs::remove_file(p).unwrap();
        }
    }
    std::fs::remove_dir(dir).unwrap();
}

pub(super) fn custom_role(
    st: &mut State,
    target: UserId,
    position: i32,
    permissions: u64,
) -> RoleId {
    community(
        st,
        1,
        CommunityCommand::UpsertRole {
            role: CommunityRole {
                id: 0,
                guild: 1,
                name: format!("Role {position}"),
                position,
                permissions,
                color: 0x0e9488,
            },
        },
    )
    .unwrap();
    let id = st.workspace.community.roles.last().unwrap().id;
    community(
        st,
        1,
        CommunityCommand::AssignRoles {
            guild: 1,
            user: target,
            roles: vec![id],
        },
    )
    .unwrap();
    id
}

#[test]
fn custom_roles_inherit_categories_and_revocation_blocks_all_read_paths() {
    let mut st = fixture();
    let role = custom_role(&mut st, 2, 10, permission::MEMBER);
    st.workspace_command(
        1,
        WorkspaceCommand::CreateCategory {
            guild: 1,
            name: "Private".into(),
        },
    )
    .unwrap();
    let category = st.workspace.categories.last().unwrap().id;
    st.workspace_command(
        1,
        WorkspaceCommand::MoveChannel {
            channel: 1,
            category: Some(category),
            index: 0,
        },
    )
    .unwrap();
    community(
        &mut st,
        1,
        CommunityCommand::SetOverride {
            value: PermissionOverride {
                scope: PermissionScope::Category(category),
                target: PermissionTarget::Everyone,
                allow: 0,
                deny: permission::VIEW,
            },
        },
    )
    .unwrap();
    community(
        &mut st,
        1,
        CommunityCommand::SetOverride {
            value: PermissionOverride {
                scope: PermissionScope::Category(category),
                target: PermissionTarget::Role(role),
                allow: permission::VIEW,
                deny: 0,
            },
        },
    )
    .unwrap();
    assert!(st.can_access(1, 1) && st.can_access(2, 1));
    assert!(!st.can_access(3, 1));
    st.durable_send(
        2,
        Some("private".into()),
        1,
        "private data".into(),
        None,
        vec![],
    )
    .unwrap();
    st.workspace_command(
        2,
        WorkspaceCommand::Upload {
            operation: "upload".into(),
            channel: 1,
            name: "a.txt".into(),
            media_type: "text/plain".into(),
            bytes: vec![1],
        },
    )
    .unwrap();
    let file = st.workspace.next_attachment - 1;
    community(
        &mut st,
        1,
        CommunityCommand::AssignRoles {
            guild: 1,
            user: 2,
            roles: vec![],
        },
    )
    .unwrap();
    for cmd in [
        WorkspaceCommand::History {
            channel: 1,
            before: None,
            limit: 20,
        },
        WorkspaceCommand::SearchAdvanced(SearchRequest {
            channel: Some(1),
            query: "private".into(),
            ..Default::default()
        }),
        WorkspaceCommand::Download { attachment: file },
        WorkspaceCommand::Community(CommunityCommand::MessageContext {
            channel: 1,
            message: 1,
            limit: 20,
        }),
    ] {
        assert!(st.workspace_command(2, cmd).is_err());
    }
    assert!(!st
        .workspace_snapshot(2)
        .channel_settings
        .iter()
        .any(|c| c.channel == 1));
}

#[test]
fn delegated_roles_cannot_escalate_hierarchy_or_legacy_admin() {
    let mut st = fixture();
    custom_role(
        &mut st,
        2,
        10,
        permission::MANAGE_ROLES | permission::MODERATE | permission::MANAGE_MESSAGES,
    );
    let high = custom_role(&mut st, 3, 20, permission::MEMBER);
    for cmd in [
        CommunityCommand::UpsertRole {
            role: CommunityRole {
                id: 0,
                guild: 1,
                name: "Above".into(),
                position: 11,
                permissions: 0,
                color: 0,
            },
        },
        CommunityCommand::UpsertRole {
            role: CommunityRole {
                id: 0,
                guild: 1,
                name: "Escalation".into(),
                position: 9,
                permissions: permission::MANAGE_GUILD,
                color: 0,
            },
        },
        CommunityCommand::DeleteRole {
            guild: 1,
            role: high,
        },
        CommunityCommand::AssignRoles {
            guild: 1,
            user: 2,
            roles: vec![high],
        },
        CommunityCommand::Timeout {
            guild: 1,
            user: 3,
            until: now_secs() + 60,
            reason: "test".into(),
        },
    ] {
        assert!(community(&mut st, 2, cmd).is_err());
    }
    assert!(st
        .workspace_command(
            2,
            WorkspaceCommand::SetRole {
                guild: 1,
                user: 3,
                role: MemberRole::Admin
            }
        )
        .is_err());
    assert!(st
        .workspace_command(
            2,
            WorkspaceCommand::UpdateGuild {
                guild: 1,
                name: "Hijacked".into()
            }
        )
        .is_err());
    st.durable_send(3, None, 1, "message".into(), None, vec![])
        .unwrap();
    st.workspace_command(2, WorkspaceCommand::Delete { message: 1 })
        .unwrap();
    assert!(st.find_message(1).is_err());
}

#[test]
fn slowmode_and_timeout_preserve_idempotency_and_prevent_content_changes() {
    let mut st = fixture();
    community(
        &mut st,
        1,
        CommunityCommand::SetChannelPolicy {
            policy: ChannelPolicy {
                channel: 1,
                slowmode_secs: 60,
                style: ChannelStyle::Standard,
            },
        },
    )
    .unwrap();
    st.durable_send(2, Some("same".into()), 1, "hello".into(), None, vec![])
        .unwrap();
    st.durable_send(2, Some("same".into()), 1, "hello".into(), None, vec![])
        .unwrap();
    assert!(st
        .durable_send(2, Some("new".into()), 1, "too soon".into(), None, vec![])
        .is_err());
    assert_eq!(st.messages.len(), 1);
    st.voice.insert(
        2,
        VoiceState {
            channel: Some(3),
            ..Default::default()
        },
    );
    community(
        &mut st,
        1,
        CommunityCommand::Timeout {
            guild: 1,
            user: 2,
            until: now_secs() + 60,
            reason: "Cooldown".into(),
        },
    )
    .unwrap();
    assert!(st.voice[&2].channel.is_none());
    assert!(!st.can_transmit_voice(2, 3));
    for cmd in [
        WorkspaceCommand::Edit {
            message: 1,
            text: "edit".into(),
        },
        WorkspaceCommand::React {
            message: 1,
            emoji: "x".into(),
        },
        WorkspaceCommand::Upload {
            operation: "blocked".into(),
            channel: 1,
            name: "a".into(),
            media_type: "text/plain".into(),
            bytes: vec![1],
        },
    ] {
        assert!(st.workspace_command(2, cmd).is_err());
    }
    community(
        &mut st,
        1,
        CommunityCommand::Timeout {
            guild: 1,
            user: 2,
            until: 0,
            reason: "Restored".into(),
        },
    )
    .unwrap();
    assert!(!st.is_timed_out(2, 1));
    assert!(st.can_transmit_voice(2, 3));
}

#[test]
fn failed_role_persistence_rolls_back_the_entire_change() {
    let mut st = fixture();
    st.save_path = Some(
        std::env::temp_dir()
            .join(format!("missing-{}", rand::random::<u64>()))
            .join("host.json"),
    );
    assert!(community(
        &mut st,
        1,
        CommunityCommand::UpsertRole {
            role: CommunityRole {
                id: 0,
                guild: 1,
                name: "Role".into(),
                position: 10,
                permissions: permission::MODERATE,
                color: 0
            }
        }
    )
    .is_err());
    assert!(st.workspace.community.roles.is_empty());
    assert!(st.workspace.audit.is_empty());
}

#[test]
fn stages_keep_audience_microphones_out_of_the_voice_routes() {
    let mut st = fixture();
    community(
        &mut st,
        1,
        CommunityCommand::SetChannelPolicy {
            policy: ChannelPolicy {
                channel: 3,
                slowmode_secs: 0,
                style: ChannelStyle::Stage,
            },
        },
    )
    .unwrap();
    assert!(st.can_transmit_voice(1, 3));
    assert!(!st.can_transmit_voice(2, 3));
    custom_role(&mut st, 2, 10, permission::MANAGE_MESSAGES);
    assert!(st.can_transmit_voice(2, 3));
}

#[test]
fn stage_requests_require_presence_and_only_moderators_can_grant_speech() {
    let mut st = fixture();
    community(
        &mut st,
        1,
        CommunityCommand::SetChannelPolicy {
            policy: ChannelPolicy {
                channel: 3,
                slowmode_secs: 0,
                style: ChannelStyle::Stage,
            },
        },
    )
    .unwrap();
    assert!(community(
        &mut st,
        2,
        CommunityCommand::StageRequest {
            channel: 3,
            requested: true
        }
    )
    .is_err());
    for user in [1, 2, 3] {
        st.voice.insert(
            user,
            VoiceState {
                channel: Some(3),
                ..Default::default()
            },
        );
    }
    community(
        &mut st,
        2,
        CommunityCommand::StageRequest {
            channel: 3,
            requested: true,
        },
    )
    .unwrap();
    assert!(st.stage_requests[&3].contains(&2));
    assert!(community(
        &mut st,
        3,
        CommunityCommand::StageSpeaker {
            channel: 3,
            user: 2,
            allowed: true
        }
    )
    .is_err());
    community(
        &mut st,
        1,
        CommunityCommand::StageSpeaker {
            channel: 3,
            user: 2,
            allowed: true,
        },
    )
    .unwrap();
    assert!(st.can_transmit_voice(2, 3));
    st.leave_voice_inner(2);
    assert!(!st.stage_speakers[&3].contains(&2));
}

#[test]
fn base_role_only_owner_can_change_and_cannot_be_assigned() {
    let mut st = fixture();
    let role = CommunityRole {
        guild: 1,
        position: 0,
        permissions: permission::VIEW,
        ..Default::default()
    };
    assert!(community(
        &mut st,
        2,
        CommunityCommand::UpsertRole { role: role.clone() }
    )
    .is_err());
    community(&mut st, 1, CommunityCommand::UpsertRole { role }).unwrap();
    let id = st
        .workspace
        .community
        .roles
        .iter()
        .find(|r| r.position == 0)
        .unwrap()
        .id;
    assert!(!st.has_permission(2, 1, permission::SEND));
    assert!(community(
        &mut st,
        1,
        CommunityCommand::AssignRoles {
            guild: 1,
            user: 2,
            roles: vec![id]
        }
    )
    .is_err());
    assert!(community(
        &mut st,
        1,
        CommunityCommand::DeleteRole { guild: 1, role: id }
    )
    .is_err());
}
