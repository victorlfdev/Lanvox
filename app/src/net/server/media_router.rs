//! Real-time routing never takes the host's persistence/state mutex.
//! A routing read lease is held through enqueue, so revocation waits for in-flight
//! forwarding and no packet can be enqueued using an obsolete authorization.
use super::*;
use parking_lot::RwLock;

#[derive(Default)]
pub(super) struct MediaRouter {
    routes: RwLock<HashMap<UserId, Route>>,
}

struct Route {
    sender: Outbox,
    voice: Vec<Outbox>,
    speaking: Vec<Outbox>,
    screen: Vec<Outbox>,
    camera: Vec<Outbox>,
    stream_audio: Vec<Outbox>,
    screen_active: bool,
    camera_active: bool,
    last_keyframe: Mutex<HashMap<StreamKind, Instant>>,
    budget: Arc<Mutex<Budget>>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
    rate: f64,
    capacity: f64,
}
impl Bucket {
    fn new(rate: f64, capacity: f64, now: Instant) -> Self {
        Self {
            tokens: capacity,
            last: now,
            rate,
            capacity,
        }
    }
    fn take(&mut self, cost: f64, now: Instant) -> bool {
        self.tokens = (self.tokens
            + now.saturating_duration_since(self.last).as_secs_f64() * self.rate)
            .min(self.capacity);
        self.last = now;
        if cost > self.tokens {
            return false;
        }
        self.tokens -= cost;
        true
    }
}
struct Budget {
    classes: [Bucket; 5],
    bytes: Bucket,
}
impl Budget {
    fn new(now: Instant) -> Self {
        Self {
            classes: [
                Bucket::new(80.0, 100.0, now),
                Bucket::new(80.0, 100.0, now),
                Bucket::new(20.0, 20.0, now),
                Bucket::new(100.0, 120.0, now),
                // Pointer updates: up to ~100 a second from a sharer.
                Bucket::new(130.0, 130.0, now),
            ],
            bytes: Bucket::new(8.0 * 1024.0 * 1024.0, 4.0 * 1024.0 * 1024.0, now),
        }
    }
    fn take(&mut self, class: usize, bytes: usize, now: Instant) -> bool {
        self.classes[class].take(1.0, now) && self.bytes.take(bytes as f64, now)
    }
}

impl MediaRouter {
    pub(super) fn update(&self, state: &State) {
        let budgets: HashMap<_, _> = self
            .routes
            .read()
            .iter()
            .map(|(user, route)| (*user, route.budget.clone()))
            .collect();
        let mut routes = HashMap::new();
        for (user, out) in &state.clients {
            let peers = state.voice_peers(*user);
            let channel = state.voice.get(user).and_then(|v| v.channel);
            let may_speak =
                channel.is_none_or(|c| c >= CALL_ID_BASE || state.can_transmit_voice(*user, c));
            let may_stream = channel.is_none_or(|c| {
                c >= CALL_ID_BASE || state.has_permission(*user, c, permission::STREAM)
            });
            let muted = state.voice.get(user).is_none_or(|v| v.muted) || !may_speak;
            let voice = if muted {
                Vec::new()
            } else {
                peers
                    .iter()
                    .filter(|p| !state.voice.get(p).is_some_and(|v| v.deafened))
                    .filter_map(|p| state.clients.get(p).cloned())
                    .collect()
            };
            let screen_users: Vec<_> = state
                .watchers
                .get(&(*user, StreamKind::Screen))
                .map(|watchers| {
                    watchers
                        .iter()
                        .filter(|viewer| state.can_watch(**viewer, *user, StreamKind::Screen))
                        .copied()
                        .collect()
                })
                .unwrap_or_default();
            routes.insert(
                *user,
                Route {
                    sender: out.clone(),
                    voice,
                    speaking: if may_speak {
                        peers
                            .iter()
                            .filter_map(|p| state.clients.get(p).cloned())
                            .collect()
                    } else {
                        Vec::new()
                    },
                    screen: screen_users
                        .iter()
                        .filter_map(|p| state.clients.get(p).cloned())
                        .collect(),
                    camera: peers
                        .iter()
                        .filter_map(|p| state.clients.get(p).cloned())
                        .collect(),
                    stream_audio: screen_users
                        .iter()
                        .filter(|p| !state.voice.get(p).is_some_and(|v| v.deafened))
                        .filter_map(|p| state.clients.get(p).cloned())
                        .collect(),
                    screen_active: may_stream
                        && state.streams.contains_key(&(*user, StreamKind::Screen)),
                    camera_active: may_stream
                        && state.streams.contains_key(&(*user, StreamKind::Camera)),
                    last_keyframe: Mutex::new(HashMap::new()),
                    budget: budgets
                        .get(user)
                        .cloned()
                        .unwrap_or_else(|| Arc::new(Mutex::new(Budget::new(Instant::now())))),
                },
            );
        }
        *self.routes.write() = routes;
    }

    /// A control packet is returned to the state actor; media is fully handled here.
    pub(super) fn dispatch(&self, user: UserId, message: ClientMsg) -> Option<ClientMsg> {
        if !matches!(
            &message,
            ClientMsg::Audio { .. }
                | ClientMsg::AudioEncoded { .. }
                | ClientMsg::StreamAudio { .. }
                | ClientMsg::Frame { .. }
                | ClientMsg::EncodedFrame { .. }
                | ClientMsg::Cursor(_)
                | ClientMsg::Speaking(_)
        ) {
            return Some(message);
        }
        let routes = self.routes.read();
        let route = routes.get(&user)?;
        let (class, size) = match &message {
            ClientMsg::Audio { pcm, .. } => (0, pcm.len().saturating_mul(2)),
            ClientMsg::AudioEncoded { opus, .. } => (0, opus.len()),
            ClientMsg::StreamAudio { packet, .. } => (1, packet.len()),
            ClientMsg::Speaking(_) => (2, 1),
            ClientMsg::Frame { frame, .. } => (3, frame.byte_size()),
            ClientMsg::EncodedFrame { frame, .. } => (3, frame.data.len()),
            ClientMsg::Cursor(state) => (4, 32 + state.shape.as_ref().map_or(0, |s| s.rgba.len())),
            _ => return None,
        };
        if !route.budget.lock().take(class, size, Instant::now()) {
            return None;
        }
        match message {
            ClientMsg::Audio { seq, pcm } if pcm.len() == 480 => {
                let packet = encode_packet(&ServerMsg::Audio {
                    from: user,
                    seq,
                    pcm,
                });
                for out in &route.voice {
                    out.push_realtime(packet.clone());
                }
            }
            ClientMsg::AudioEncoded {
                seq,
                sent_micros,
                opus,
            } if !opus.is_empty() && opus.len() <= 1275 => {
                let packet = encode_packet(&ServerMsg::AudioEncoded {
                    from: user,
                    seq,
                    sent_micros,
                    opus,
                });
                for out in &route.voice {
                    out.push_realtime(packet.clone());
                }
            }
            ClientMsg::StreamAudio {
                seq,
                sent_micros,
                packet,
            } if route.screen_active && !packet.is_empty() && packet.len() <= 1275 => {
                let packet = encode_packet(&ServerMsg::StreamAudio {
                    from: user,
                    seq,
                    sent_micros,
                    packet,
                });
                for out in &route.stream_audio {
                    out.push_realtime(packet.clone());
                }
            }
            ClientMsg::Speaking(speaking) => {
                let packet = encode_packet(&ServerMsg::Speaking { user, speaking });
                for out in &route.speaking {
                    out.push_urgent(packet.clone());
                }
            }
            ClientMsg::Cursor(state) if route.screen_active && state.valid() => {
                let packet = encode_packet(&ServerMsg::Cursor { owner: user, state });
                for out in &route.screen {
                    out.push_cursor(packet.clone());
                }
            }
            ClientMsg::Frame { kind, frame } if frame.validate() && active(route, kind) => {
                let packet = encode_packet(&ServerMsg::Frame {
                    owner: user,
                    kind,
                    frame,
                });
                send_video(route, kind, packet);
            }
            ClientMsg::EncodedFrame { kind, frame }
                if active(route, kind)
                    && picture_bytes(frame.width, frame.height).is_some()
                    && !frame.data.is_empty()
                    && frame.data.len() <= 2 * 1024 * 1024 =>
            {
                let packet = encode_packet(&ServerMsg::EncodedFrame {
                    owner: user,
                    kind,
                    frame,
                });
                send_video(route, kind, packet);
            }
            _ => {}
        }
        None
    }
}

fn active(route: &Route, kind: StreamKind) -> bool {
    match kind {
        StreamKind::Screen => route.screen_active,
        StreamKind::Camera => route.camera_active,
    }
}
fn send_video(route: &Route, kind: StreamKind, packet: Vec<u8>) {
    let targets = match kind {
        StreamKind::Screen => &route.screen,
        StreamKind::Camera => &route.camera,
    };
    let mut dropped = false;
    for out in targets {
        dropped |= out.push_bulk(packet.clone());
    }
    if dropped {
        let mut last = route.last_keyframe.lock();
        if last
            .get(&kind)
            .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
        {
            last.insert(kind, Instant::now());
            route
                .sender
                .push_urgent(encode_packet(&ServerMsg::KeyframeWanted { kind }));
        }
    }
}

impl State {
    pub(super) fn refresh_media_routes(&self) {
        self.media_router.update(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn connection() -> (Outbox, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        (Outbox::start(server), peer)
    }
    #[test]
    fn voice_routes_while_persistence_is_locked_and_revocation_blocks_further_forwarding() {
        let mut st = super::super::tests::fixture();
        let (sender, _sender_peer) = connection();
        let (listener, mut receiver) = connection();
        st.clients.insert(1, sender);
        st.clients.insert(2, listener);
        for user in [1, 2] {
            st.voice.insert(
                user,
                VoiceState {
                    channel: Some(3),
                    ..Default::default()
                },
            );
        }
        st.refresh_media_routes();
        let router = st.media_router.clone();
        let state = Arc::new(Mutex::new(st));
        let mut locked = state.lock();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = router.clone();
        std::thread::spawn(move || {
            worker.dispatch(
                1,
                ClientMsg::Audio {
                    seq: 1,
                    pcm: vec![7; 480],
                },
            );
            tx.send(()).unwrap();
        });
        // Deliberately retain StateMutex to simulate a slow filesystem transaction.
        rx.recv_timeout(Duration::from_secs(1))
            .expect("media must not acquire persistence mutex");
        assert!(matches!(
            read_packet::<_, ServerMsg>(&mut receiver).unwrap(),
            ServerMsg::Audio {
                from: 1,
                seq: 1,
                ..
            }
        ));
        locked.voice.remove(&2);
        locked.refresh_media_routes();
        router.dispatch(
            1,
            ClientMsg::Audio {
                seq: 2,
                pcm: vec![9; 480],
            },
        );
        assert!(read_packet::<_, ServerMsg>(&mut receiver).is_err());
        for out in locked.clients.values() {
            out.close();
        }
    }

    #[test]
    fn media_budgets_bound_floods_and_survive_authorization_refreshes() {
        let now = Instant::now();
        let mut budget = Budget::new(now);
        assert_eq!((0..2000).filter(|_| budget.take(0, 200, now)).count(), 100);
        assert_eq!((0..100).filter(|_| budget.take(2, 1, now)).count(), 20);
        assert!(budget.take(3, 2 * 1024 * 1024, now));
        assert!(!budget.take(3, 3 * 1024 * 1024, now));
        assert!(budget.take(0, 200, now + Duration::from_secs(1)));
        let mut st = super::super::tests::fixture();
        let (out, _) = connection();
        st.clients.insert(1, out);
        st.refresh_media_routes();
        let original = st.media_router.routes.read()[&1].budget.clone();
        st.refresh_media_routes();
        assert!(Arc::ptr_eq(
            &original,
            &st.media_router.routes.read()[&1].budget
        ));
        for out in st.clients.values() {
            out.close();
        }
    }
}
