use super::*;

#[test]
fn authentication_fails_if_identity_ownership_cannot_be_saved() {
    let missing = std::env::temp_dir().join(format!("lanvox-missing-{}", rand::random::<u64>()));
    let server = start_listener(
        "Test".into(),
        test_listener("127.0.0.1"),
        Some(missing.join("hosted-server.json")),
    )
    .unwrap();
    let (identity, _) = crate::auth::Identity::generate().unwrap();
    let (_writer, _reader, reply) = login(server.port, "Ana", &identity);
    assert!(
        matches!(reply, ServerMsg::Error(message) if message.contains("could not save your identity"))
    );
    assert!(!missing.exists());
    server.stop();
}

#[test]
fn local_owner_migration_preserves_history_and_never_rebinds_a_protected_profile() {
    let dir = std::env::temp_dir().join(format!("lanvox-migration-{}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("hosted-server.json");
    let mut state = fixture();
    state.save_path = Some(path.clone());
    state.users.insert(
        7,
        User {
            id: 7,
            name: "Ana".into(),
            color: 42,
            online: false,
        },
    );
    state.messages.push(Message {
        id: 1,
        channel: 1,
        author: 7,
        author_name: "Ana".into(),
        author_color: 42,
        time: 0,
        text: "Keep this history".into(),
    });
    state
        .friends
        .insert(7, HashMap::from([(8, FriendStatus::Friend)]));
    state.save_to_disk().unwrap();
    let original = std::fs::read(&path).unwrap();
    let (identity, _) = crate::auth::Identity::generate().unwrap();
    let key = identity.public_key();
    let server = start_listener_with_owner(
        "Test".into(),
        test_listener("127.0.0.1"),
        Some(path.clone()),
        Some(("Ana", key)),
    )
    .unwrap();
    server.stop();
    drop(server);
    let migrated: Saved = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(migrated.public_keys[&7], key);
    assert_eq!(migrated.users[0].id, 7);
    assert_eq!(migrated.messages[0].text, "Keep this history");
    assert_eq!(migrated.friends, vec![(7, 8, FriendStatus::Friend)]);
    let backup = path.with_extension("pre-identity-v2.json");
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    let (different, _) = crate::auth::Identity::generate().unwrap();
    let server = start_listener_with_owner(
        "Test".into(),
        test_listener("127.0.0.1"),
        Some(path.clone()),
        Some(("Ana", different.public_key())),
    )
    .unwrap();
    server.stop();
    let unchanged: Saved = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(unchanged.public_keys[&7], key);
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(backup).unwrap();
    drop(server);
    std::thread::sleep(Duration::from_millis(250));
    for suffix in [
        "sqlite3",
        "sqlite3-wal",
        "sqlite3-shm",
        "pre-sqlite.json",
        "host-key",
    ] {
        let file = dir.join("hosted-server.json").with_extension(suffix);
        for _ in 0..40 {
            if !file.exists() || std::fs::remove_file(&file).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn failed_migration_backup_leaves_the_original_host_data_untouched() {
    let dir = std::env::temp_dir().join(format!(
        "lanvox-migration-failure-{}",
        rand::random::<u64>()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("hosted-server.json");
    let mut state = fixture();
    state.save_path = Some(path.clone());
    state.users.insert(
        7,
        User {
            id: 7,
            name: "Ana".into(),
            color: 0,
            online: false,
        },
    );
    state.save_to_disk().unwrap();
    let original = std::fs::read(&path).unwrap();
    let backup = path.with_extension("pre-identity-v2.json");
    std::fs::create_dir(&backup).unwrap();
    assert!(start_listener_with_owner(
        "Test".into(),
        test_listener("127.0.0.1"),
        Some(path.clone()),
        Some(("Ana", [1; 32]))
    )
    .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_dir(backup).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn identity_ownership_is_persisted_and_cannot_be_claimed_by_name() {
    let mut state = fixture();
    let key = [1; 32];
    let id = state.identify("Ana", key).unwrap();
    state.users.insert(
        id,
        User {
            id,
            name: "Ana".into(),
            color: 0,
            online: false,
        },
    );
    assert!(state.identify("Ana", [2; 32]).is_err());
    assert!(state.identify("aNa", [2; 32]).is_err());
    assert_eq!(state.identify("Ana", key).unwrap(), id);
    let path = std::env::temp_dir().join(format!("lanvox-state-{}.json", rand::random::<u64>()));
    state.save_path = Some(path.clone());
    state.save();
    let saved: Saved = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved.public_keys[&id], key);
    state.public_keys = saved.public_keys;
    assert_eq!(state.identify("Renamed", key).unwrap(), id);
    state.public_keys.clear();
    assert!(
        state.identify("Ana", key).is_err(),
        "legacy names cannot be claimed without ownership evidence"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn corrupted_host_state_is_reported_instead_of_resetting_identities() {
    let path = std::env::temp_dir().join(format!("lanvox-corrupt-{}.json", rand::random::<u64>()));
    std::fs::write(&path, b"corrupt").unwrap();
    let listener = test_listener("127.0.0.1");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    assert!(start("Test".into(), port, Some(path.clone())).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn authenticated_reconnection_rejects_name_theft_and_duplicate_sessions() {
    let server = host();
    let (identity, _) = crate::auth::Identity::generate().unwrap();
    let (aw, _ar, reply) = login(server.port, "Ana", &identity);
    let ServerMsg::Welcome { you: a, .. } = reply else {
        panic!("no welcome")
    };
    let (_bw, mut br, _) = client(server.port, "Observer");
    let (_dw, _dr, duplicate) = login(server.port, "Ana", &identity);
    assert!(matches!(duplicate, ServerMsg::Error(_)));
    let (impostor, _) = crate::auth::Identity::generate().unwrap();
    let (_iw, _ir, rejected) = login(server.port, "aNa", &impostor);
    assert!(matches!(rejected, ServerMsg::Error(_)));
    aw.shutdown(std::net::Shutdown::Both).unwrap();
    wait_for(
        &mut br,
        |m| matches!(m, ServerMsg::UserUpdated(u) if u.id == a && !u.online),
    );
    let (_rw, _rr, reconnected) = login(server.port, "Ana", &identity);
    assert!(matches!(reconnected, ServerMsg::Welcome { you, .. } if you == a));
    server.stop();
}

#[test]
fn desktop_invites_migrate_to_protected_storage_and_invalid_files_are_preserved() {
    let dir = std::env::temp_dir().join(format!("lanvox-token-test-{}", rand::random::<u64>()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("state.json");
    let secret = path.with_extension("host-token");
    let token = "a1".repeat(32);
    std::fs::write(&secret, token.as_bytes()).unwrap();
    assert_eq!(desktop_token(Some(&path)).unwrap(), token);
    assert_eq!(
        crate::secret_store::read(&secret).unwrap(),
        token.as_bytes()
    );
    #[cfg(windows)]
    assert_ne!(std::fs::read(&secret).unwrap(), token.as_bytes());
    let invalid = "x".repeat(64);
    std::fs::write(&secret, invalid.as_bytes()).unwrap();
    assert!(desktop_token(Some(&path)).is_err());
    assert_eq!(std::fs::read(&secret).unwrap(), invalid.as_bytes());
    std::fs::remove_file(secret).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn enrollment_token_admits_members_without_granting_host_ownership() {
    let (owner, _) = crate::auth::Identity::generate().unwrap();
    let (member, _) = crate::auth::Identity::generate().unwrap();
    let host = start_configured(
        "Private".into(),
        None,
        ServerOptions {
            port: 0,
            owner_key: Some(owner.public_key()),
            registration_token: Some("only-in-test".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let (_denied_writer, _denied_reader, denied) = login(host.port, "Member", &member);
    assert!(matches!(denied, ServerMsg::Error(_)));
    let socket = TcpStream::connect(("127.0.0.1", host.port)).unwrap();
    let (mut stream, _) = crate::auth::transport::client_handshake(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let ServerMsg::Challenge { nonce, .. } = read_packet::<_, ServerMsg>(&mut reader).unwrap()
    else {
        panic!("challenge")
    };
    write_packet(
        &mut stream,
        &ClientMsg::Authenticate {
            hello: Box::new(member.hello("Member", 0, &nonce)),
            token: "only-in-test".into(),
        },
    )
    .unwrap();
    let ServerMsg::Welcome { you: member_id, .. } =
        read_packet::<_, ServerMsg>(&mut reader).unwrap()
    else {
        panic!("welcome")
    };
    let ServerMsg::Workspace(WorkspaceEvent::State(workspace)) = wait_for(&mut reader, |m| {
        matches!(m, ServerMsg::Workspace(WorkspaceEvent::State(_)))
    }) else {
        unreachable!()
    };
    assert_eq!(workspace.host_owner, None);
    assert!(workspace
        .memberships
        .iter()
        .any(|m| m.user == member_id && m.role == MemberRole::Member));
    let (mut owner_writer, mut owner_reader, welcome) = login(host.port, "Owner", &owner);
    let ServerMsg::Welcome { you: owner_id, .. } = welcome else {
        panic!("owner welcome")
    };
    let ServerMsg::Workspace(WorkspaceEvent::State(workspace)) = wait_for(&mut owner_reader, |m| {
        matches!(m, ServerMsg::Workspace(WorkspaceEvent::State(_)))
    }) else {
        unreachable!()
    };
    assert_eq!(workspace.host_owner, Some(owner_id));
    write_packet(
        &mut stream,
        &ClientMsg::CreateChannel {
            guild: 1,
            name: "unauthorized".into(),
            kind: ChannelKind::Text,
        },
    )
    .unwrap();
    assert!(matches!(
        wait_for(&mut reader, |m| matches!(m, ServerMsg::Error(_))),
        ServerMsg::Error(_)
    ));
    write_packet(
        &mut owner_writer,
        &ClientMsg::Workspace(WorkspaceCommand::SetHostAccess {
            user: member_id,
            allowed: false,
        }),
    )
    .unwrap();
    wait_for(
        &mut owner_reader,
        |m| matches!(m,ServerMsg::Workspace(WorkspaceEvent::State(snapshot)) if snapshot.disabled_users.contains(&member_id)),
    );
    let (_writer, _reader, rejected) = login(host.port, "Member", &member);
    assert!(matches!(rejected,ServerMsg::Error(reason) if reason.contains("revogada")));
    host.stop();
}
