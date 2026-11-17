use super::*;

pub(super) fn desktop_token(path: Option<&std::path::Path>) -> std::io::Result<String> {
    let Some(save_path) = path else {
        return Ok(workspace::random_token());
    };
    let path = save_path.with_extension("host-token");
    match crate::secret_store::read(&path) {
        Ok(bytes) => {
            let token = std::str::from_utf8(&bytes)
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Convite local inválido; restaure o backup",
                    )
                })?
                .trim();
            if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Convite local inválido; restaure o backup",
                ));
            }
            crate::secret_store::migrate_validated(&path, &bytes)?;
            Ok(token.to_owned())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = workspace::random_token();
            match crate::secret_store::write_new(&path, token.as_bytes()) {
                Ok(()) => Ok(token),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    desktop_token(Some(save_path))
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
pub(super) fn start_listener(
    name: String,
    listener: TcpListener,
    save_path: Option<PathBuf>,
) -> std::io::Result<ServerHandle> {
    start_listener_with_owner(name, listener, save_path, None)
}

#[cfg(test)]
pub(super) fn start_listener_with_owner(
    name: String,
    listener: TcpListener,
    save_path: Option<PathBuf>,
    local_owner: Option<(&str, [u8; 32])>,
) -> std::io::Result<ServerHandle> {
    let mut options = ServerOptions {
        allow_registration: true,
        ..ServerOptions::default()
    };
    if let Some((_, key)) = local_owner {
        options.owner_key = Some(key);
        options.allow_registration = false;
        options.registration_token = Some(workspace::random_token());
    }
    start_listener_configured(name, listener, save_path, local_owner, options)
}

pub(super) fn start_listener_configured(
    name: String,
    listener: TcpListener,
    save_path: Option<PathBuf>,
    local_owner: Option<(&str, [u8; 32])>,
    options: ServerOptions,
) -> std::io::Result<ServerHandle> {
    let port = listener.local_addr()?.port();
    // A host bound to loopback must not open sockets on other interfaces,
    // otherwise the OS firewall prompts for every new executable.
    let loopback_only = listener.local_addr()?.ip().is_loopback();

    let mut store = if let Some(path) = save_path
        .as_ref()
        .filter(|p| p.with_extension("sqlite3").exists())
    {
        Some(crate::storage::Store::open(
            &path.with_extension("sqlite3"),
        )?)
    } else {
        None
    };
    let current = store
        .as_ref()
        .map(|s| s.load_state::<Saved>())
        .transpose()?
        .flatten();
    let mut saved: Saved = if let Some(saved) = current {
        saved
    } else {
        match save_path.as_ref().map(std::fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => Saved::default(),
        }
    };
    let mut migrated = false;
    if let Some((owner_name, key)) = local_owner {
        if !saved
            .public_keys
            .values()
            .any(|registered| *registered == key)
        {
            if let Some(user) = saved.users.iter().find(|u| {
                u.name.eq_ignore_ascii_case(owner_name.trim())
                    && !saved.public_keys.contains_key(&u.id)
            }) {
                if let Some(path) = &save_path {
                    use std::io::Write;
                    let backup = path.with_extension("pre-identity-v2.json");
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&backup)
                    {
                        Ok(mut file) => {
                            file.write_all(&std::fs::read(path)?)?;
                            file.sync_all()?;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            // A directory or a partial backup must not count as a recoverable copy.
                            let bytes = std::fs::read(&backup)?;
                            let _: Saved = serde_json::from_slice(&bytes).map_err(|e| {
                                std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                            })?;
                        }
                        Err(error) => return Err(error),
                    }
                }
                saved.public_keys.insert(user.id, key);
                migrated = true;
            }
        }
    }
    let mut channels = saved.channels;
    if channels.is_empty() && !saved.workspace.initialized {
        channels = default_channels();
    }
    let next_channel = saved
        .next_channel
        .max(channels.iter().map(|c| c.id).max().unwrap_or(0) + 1);
    // Legacy JSON (or a fresh host) may still carry free-text categories.
    let mut workspace = saved.workspace;
    workspace.adopt_legacy_categories(&channels);
    let guilds = if saved.guilds.is_empty() {
        vec![Guild {
            id: 1,
            name: name.clone(),
        }]
    } else {
        saved.guilds
    };
    let next_guild = saved
        .next_guild
        .max(guilds.iter().map(|g| g.id).max().unwrap_or(0) + 1);
    let mut friends: HashMap<UserId, HashMap<UserId, FriendStatus>> = HashMap::new();
    for (a, b, s) in saved.friends {
        friends.entry(a).or_default().insert(b, s);
    }
    let mut users = HashMap::new();
    for mut u in saved.users {
        u.online = false;
        users.insert(u.id, u);
    }
    let next_user = saved
        .next_user
        .max(users.keys().max().copied().unwrap_or(0) + 1);
    let next_message = saved
        .next_message
        .max(saved.messages.iter().map(|m| m.id).max().unwrap_or(0) + 1);

    let state = Arc::new(Mutex::new(State {
        name: name.clone(),
        channels,
        users,
        messages: saved.messages,
        voice: HashMap::new(),
        stage_speakers: HashMap::new(),
        stage_requests: HashMap::new(),
        streams: HashMap::new(),
        guilds,
        next_guild,
        friends,
        calls: HashMap::new(),
        next_call: CALL_ID_BASE,
        watchers: HashMap::new(),
        clients: HashMap::new(),
        next_channel,
        next_user,
        next_message,
        last_keyframe_ask: HashMap::new(),
        save_path,
        public_keys: saved.public_keys,
        workspace,
        store: None,
        options: options.clone(),
        uploads: HashMap::new(),
        pending_uploads: Arc::default(),
        operations: HashMap::new(),
        query: None,
        rates: HashMap::new(),
        download_rates: HashMap::new(),
        call_deadlines: HashMap::new(),
        media_router: Arc::new(media_router::MediaRouter::default()),
    }));
    if migrated {
        state.lock().save_to_disk()?;
    }
    if store.is_none() {
        if let Some(path) = state
            .lock()
            .save_path
            .as_ref()
            .filter(|p| p.parent().is_some_and(|d| d.exists()))
        {
            store = Some(crate::storage::Store::open(
                &path.with_extension("sqlite3"),
            )?);
        }
    }
    {
        let mut st = state.lock();
        st.follow_owner_rotation();
        st.initialize_memberships();
        if let Some(database) = store.as_mut() {
            if database.load_state::<Saved>()?.is_none() {
                if let Some(path) = &st.save_path {
                    if path.exists() {
                        let backup = path.with_extension("pre-sqlite.json");
                        if !backup.exists() {
                            std::fs::copy(path, backup)?;
                        }
                    }
                }
                database.import(&st.saved(false), &st.messages)?;
            }
            st.messages = database.recent(4000)?;
        }
        let sink = match &store {
            Some(db) => db.upload_sink()?,
            None => None,
        };
        st.store = store.map(|db| Arc::new(Mutex::new(db)));
        if let Some(shared) = &st.store {
            st.pending_uploads.attach(shared.clone(), sink);
        }
        if migrated {
            st.save_to_disk()?;
        }
    }
    let host_key = Arc::new(match save_path_for_key(&state) {
        Some(path) => crate::auth::transport::HostKey::load_or_create(&path)?,
        None => crate::auth::transport::HostKey::generate()?,
    });
    let fingerprint = host_key.fingerprint();
    let dispatch_state = state.clone();
    let media_router = state.lock().media_router.clone();
    let media = Arc::new(crate::net::datagram::ServerMedia::bind(
        listener.local_addr()?,
        move |user, message| {
            if let Some(message) = media_router.dispatch(user, message) {
                handle_msg(&dispatch_state, user, message);
            }
        },
    )?);
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));

    {
        let state = state.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("host-maintenance".into())
            .spawn(move || {
                let mut maintenance_at = Instant::now();
                while !stop.load(Ordering::SeqCst) {
                    if maintenance_at.elapsed() >= Duration::from_secs(60) {
                        let uploads = state.lock().pending_uploads.clone();
                        if let Err(error) = uploads.maintain() {
                            eprintln!("Upload maintenance: {error}");
                        }
                        maintenance_at = Instant::now();
                    }
                    {
                        let mut st = state.lock();
                        let expired: Vec<_> = st
                            .call_deadlines
                            .iter()
                            .filter(|(_, deadline)| **deadline <= Instant::now())
                            .map(|(call, _)| *call)
                            .collect();
                        for call in expired {
                            let users: Vec<_> = st
                                .voice
                                .iter()
                                .filter(|(_, v)| v.channel == Some(call))
                                .map(|(u, _)| *u)
                                .collect();
                            for user in users {
                                st.leave_voice(user);
                            }
                            st.end_call(call);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })?;
    }

    let connections = Arc::new(Mutex::new(HashMap::<u64, TcpStream>::new()));
    // Accept loop.
    let accept_thread = {
        let state = state.clone();
        let stop = stop.clone();
        let connections = connections.clone();
        let host_key = host_key.clone();
        let media = media.clone();
        let max_connections = options.max_connections.clamp(1, 1024);
        std::thread::Builder::new()
            .name("server-accept".into())
            .spawn(move || {
                let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let mut clients = Vec::<std::thread::JoinHandle<()>>::new();
                let mut connection_id = 0u64;
                while !stop.load(Ordering::SeqCst) {
                    let stream = match listener.accept() {
                        Ok((s, _)) => s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                            continue;
                        }
                        Err(_) => break,
                    };
                    if stop.load(Ordering::SeqCst) {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        break;
                    }
                    if active.load(Ordering::SeqCst) >= max_connections {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        continue;
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let id = connection_id;
                    connection_id = connection_id.wrapping_add(1);
                    let Ok(raw) = stream.try_clone() else {
                        active.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    };
                    connections.lock().insert(id, raw);
                    let (state, host_key, media, active, connections, connection_stop) = (
                        state.clone(),
                        host_key.clone(),
                        media.clone(),
                        active.clone(),
                        connections.clone(),
                        stop.clone(),
                    );
                    clients.retain(|thread| !thread.is_finished());
                    clients.push(std::thread::spawn(move || {
                        handle_client(stream, state, host_key, media, connection_stop);
                        connections.lock().remove(&id);
                        active.fetch_sub(1, Ordering::SeqCst);
                    }));
                }
                for socket in connections.lock().values() {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
                // Close every client on shutdown.
                for out in state.lock().clients.values() {
                    out.close();
                }
                for client in clients {
                    if !join_bounded(client, Duration::from_secs(3)) {
                        eprintln!("[host {port}] a client thread did not stop in time; detaching");
                    }
                }
            })?
    };

    // Announce ourselves on the local network.
    {
        let stop = stop.clone();
        let name = name.clone();
        std::thread::Builder::new()
            .name("server-announce".into())
            .spawn(move || {
                let bind_ip = if loopback_only { "127.0.0.1" } else { "0.0.0.0" };
                let Ok(sock) = UdpSocket::bind((bind_ip, 0)) else {
                    return;
                };
                let _ = sock.set_broadcast(!loopback_only);
                let msg = format!("{DISCOVERY_MAGIC}|{port}|{name}");
                while !stop.load(Ordering::SeqCst) {
                    if !loopback_only {
                        let _ = sock.send_to(msg.as_bytes(), ("255.255.255.255", DISCOVERY_PORT));
                    }
                    let _ = sock.send_to(msg.as_bytes(), ("127.0.0.1", DISCOVERY_PORT));
                    for _ in 0..40 {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            })?;
    }

    Ok(ServerHandle {
        port,
        name,
        stop,
        accept_thread: Mutex::new(Some(accept_thread)),
        registration_token: options.registration_token,
        fingerprint,
        media: Mutex::new(Some(media)),
        connections,
    })
}

pub(super) fn save_path_for_key(state: &Arc<Mutex<State>>) -> Option<PathBuf> {
    state
        .lock()
        .save_path
        .as_ref()
        .filter(|p| p.parent().is_some_and(|d| d.exists()))
        .map(|p| p.with_extension("host-key"))
}

pub(super) fn token_matches(expected: &str, actual: &str) -> bool {
    // Kept from the previous name on purpose; see branding.rs.
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"Huddle enrollment token");
    let tag = ring::hmac::sign(&key, expected.as_bytes());
    ring::hmac::verify(&key, actual.as_bytes(), tag.as_ref()).is_ok()
}
