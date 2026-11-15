use super::*;

pub(super) fn handle_client(
    stream: TcpStream,
    state: Arc<Mutex<State>>,
    host_key: Arc<crate::auth::transport::HostKey>,
    media: Arc<crate::net::datagram::ServerMedia>,
    connection_stop: Arc<AtomicBool>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Ok(stream) =
        crate::auth::transport::server_handshake_with_stop(stream, &host_key, connection_stop)
    else {
        return;
    };
    let Ok(mut write_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::with_capacity(256 * 1024, stream);

    let Ok(nonce) = crate::auth::challenge() else {
        return;
    };
    if write_packet(
        &mut write_half,
        &ServerMsg::Challenge {
            version: crate::auth::PROTOCOL_VERSION,
            nonce,
        },
    )
    .is_err()
    {
        return;
    }
    let Ok(hello) = read_packet_limit::<_, ClientMsg>(&mut reader, 8192) else {
        return;
    };
    let (hello, token) = match hello {
        ClientMsg::Authenticate { hello, token } => (*hello, token),
        message => (message, String::new()),
    };
    let ClientMsg::Hello {
        name,
        color,
        public_key,
        signature,
    } = hello
    else {
        return;
    };
    {
        let st = state.lock();
        let known = st.public_keys.values().any(|key| *key == public_key);
        if st.is_revoked(&public_key) {
            let _ = write_packet(
                &mut write_half,
                &ServerMsg::Error("Identidade revogada pelo proprietário do host".into()),
            );
            return;
        }
        let allowed = known
            || st.options.allow_registration
            || st.options.owner_key == Some(public_key)
            || st.options.allowlist.contains(&public_key)
            || st
                .options
                .registration_token
                .as_ref()
                .is_some_and(|expected| token_matches(expected, &token));
        if !allowed {
            let _ = write_packet(
                &mut write_half,
                &ServerMsg::Error(
                    "Entrada restrita. Peça o convite de acesso ao proprietário do host.".into(),
                ),
            );
            return;
        }
    }
    if !crate::auth::verify(&public_key, &nonce, &name, color, &signature) {
        let _ = write_packet(
            &mut write_half,
            &ServerMsg::Error("Identity verification failed.".into()),
        );
        return;
    }
    let name: String = name.trim().chars().take(32).collect();
    let name = if name.is_empty() {
        "Guest".to_string()
    } else {
        name
    };
    let _ = reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(30)));
    let Ok(outbox) = Outbox::start_secure(write_half) else {
        return;
    };

    let me: UserId = {
        let mut st = state.lock();
        let id = match st.identify(&name, public_key) {
            Ok(id) => id,
            Err(reason) => {
                // Write the rejection before closing, rather than losing a queued error.
                let _ = write_packet(reader.get_mut(), &ServerMsg::Error(reason.into()));
                outbox.close();
                return;
            }
        };
        let user = User {
            id,
            name: name.clone(),
            color,
            online: true,
        };
        let previous_workspace = st.workspace.clone();
        st.admit_user(id);
        let previous = st.users.insert(id, user.clone());
        st.voice.insert(id, VoiceState::default());
        st.clients.insert(id, outbox.clone());
        if let Err(error) = st.save_to_disk() {
            eprintln!("Could not persist the authenticated identity: {error}");
            st.workspace = previous_workspace;
            st.clients.remove(&id);
            st.voice.remove(&id);
            if let Some(previous) = previous {
                st.users.insert(id, previous);
            } else {
                st.users.remove(&id);
                st.public_keys.remove(&id);
            }
            let _ = write_packet(
                reader.get_mut(),
                &ServerMsg::Error(
                    "The host could not save your identity. Check its data directory.".into(),
                ),
            );
            outbox.close();
            return;
        }
        st.refresh_media_routes();
        let snapshot = st.snapshot(id);
        outbox.push_urgent(encode_packet(&ServerMsg::Welcome { you: id, snapshot }));
        st.broadcast(&ServerMsg::UserUpdated(user));
        st.send_workspace(id);
        if let Ok((session, sender)) = media.register(id) {
            outbox.attach_media(sender);
            outbox.push_urgent(encode_packet(&session));
        }
        id
    };

    let (media_router, uploads) = {
        let st = state.lock();
        (st.media_router.clone(), st.pending_uploads.clone())
    };
    while let Ok(msg) = read_packet::<_, ClientMsg>(&mut reader) {
        if outbox.is_closed() {
            break;
        }
        // Upload pieces are stored right here, never under the shared state lock: a big
        // upload must not delay everyone else's pings, messages and calls.
        let msg = match msg {
            ClientMsg::Workspace(WorkspaceCommand::UploadChunk {
                operation,
                offset,
                data,
            }) => {
                let event = match uploads.chunk(me, &operation, offset, &data) {
                    Ok(received) => WorkspaceEvent::UploadProgress {
                        operation,
                        received,
                    },
                    Err(e) => WorkspaceEvent::Error {
                        operation: Some(operation),
                        reason: e.to_string(),
                    },
                };
                outbox.push_urgent(encode_packet(&ServerMsg::Workspace(event)));
                continue;
            }
            other => other,
        };
        if let Some(msg) = media_router.dispatch(me, msg) {
            handle_msg(&state, me, msg);
        }
    }

    // Gone: tidy up.
    media.unregister(me);
    let mut st = state.lock();
    st.leave_voice(me);
    st.voice.remove(&me);
    st.clients.remove(&me);
    st.drop_pending_uploads(me);
    st.refresh_media_routes();
    if let Some(u) = st.users.get_mut(&me) {
        u.online = false;
        let u = u.clone();
        st.broadcast(&ServerMsg::UserUpdated(u));
    }
    st.save();
    outbox.close();
}
