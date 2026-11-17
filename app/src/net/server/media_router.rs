//! Real-time routing never takes the host's persistence/state mutex.
//! A routing read lease is held through enqueue, so revocation waits for in-flight
//! forwarding and no packet can be enqueued using an obsolete authorization.
use super::*;
use parking_lot::RwLock;

pub(super) struct MediaRouter {
    routes: RwLock<HashMap<UserId, Route>>,
    video_bps: u64,
}

impl Default for MediaRouter {
    fn default() -> Self {
        let video_bps = crate::branding::env_var("LANVOX_VIDEO_BUDGET_MBPS")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| (1..=100_000).contains(v))
            .map(|v| v * 1_000_000)
            .unwrap_or(crate::media::bitrate::DEFAULT_HOST_VIDEO_BPS);
        Self {
            routes: RwLock::new(HashMap::new()),
            video_bps,
        }
    }
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
    viewers: HashMap<StreamKind, HashSet<UserId>>,
    limits: HashMap<StreamKind, u32>,
    overcommitted: bool,
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
    feedback: HashMap<(StreamKind, UserId), (Instant, bool)>,
    notified: HashMap<StreamKind, Instant>,
    drops: HashMap<StreamKind, u64>,
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
            feedback: HashMap::new(),
            notified: HashMap::new(),
            drops: HashMap::new(),
        }
    }
    fn take(&mut self, class: usize, bytes: usize, now: Instant) -> bool {
        self.classes[class].take(1.0, now) && self.bytes.take(bytes as f64, now)
    }
}

impl MediaRouter {
    pub(super) fn update(&self, state: &State) {
        let previous_limits: HashMap<_, _> = self
            .routes
            .read()
            .iter()
            .flat_map(|(user, route)| {
                route
                    .limits
                    .iter()
                    .map(move |(kind, limit)| ((*user, *kind), (*limit, route.overcommitted)))
            })
            .collect();
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
                    viewers: HashMap::from([
                        (StreamKind::Screen, screen_users.iter().copied().collect()),
                        (StreamKind::Camera, peers.iter().copied().collect()),
                    ]),
                    limits: HashMap::new(),
                    overcommitted: false,
                },
            );
        }
        let mut keys = Vec::new();
        let mut streams = Vec::new();
        for (key, info) in &state.streams {
            if let Some(route) = routes.get(&key.0).filter(|r| active(r, key.1)) {
                let copies = route.viewers[&key.1].len();
                keys.push(*key);
                streams.push((
                    key.1,
                    crate::media::bitrate::ceiling(key.1, info.width, info.height, info.fps),
                    copies,
                ));
            }
        }
        let (limits, overcommitted) = crate::media::bitrate::allocate(&streams, self.video_bps);
        for (key, limit) in keys.into_iter().zip(limits) {
            if let Some(route) = routes.get_mut(&key.0) {
                route.limits.insert(key.1, limit);
                route.overcommitted = overcommitted;
                if previous_limits.get(&key) != Some(&(limit, overcommitted)) {
                    // Push changed allocations even for static desktops, without
                    // waiting for another video frame or receiver feedback.
                    route
                        .sender
                        .push_urgent(encode_packet(&ServerMsg::VideoControl {
                            kind: key.1,
                            max_bps: limit,
                            congested: false,
                            healthy: false,
                            overcommitted,
                        }));
                }
            }
        }
        for route in routes.values() {
            route.budget.lock().feedback.retain(|(kind, viewer), _| {
                active(route, *kind) && route.viewers[kind].contains(viewer)
            });
        }
        *self.routes.write() = routes;
    }

    /// A control packet is returned to the state actor; media is fully handled here.
    pub(super) fn dispatch(&self, user: UserId, message: ClientMsg) -> Option<ClientMsg> {
        if let ClientMsg::VideoFeedback {
            owner,
            kind,
            congested,
        } = message
        {
            let routes = self.routes.read();
            if let Some(route) = routes
                .get(&owner)
                .filter(|r| active(r, kind) && r.viewers[&kind].contains(&user))
            {
                let now = Instant::now();
                let mut budget = route.budget.lock();
                let key = (kind, user);
                if budget
                    .feedback
                    .get(&key)
                    .is_none_or(|(at, _)| now.duration_since(*at) >= Duration::from_millis(500))
                {
                    budget.feedback.insert(key, (now, congested));
                } else if congested {
                    // Never let a recent healthy sample hide a subsequent loss.
                    if let Some((_, bad)) = budget.feedback.get_mut(&key) {
                        *bad = true;
                    }
                }
                drop(budget);
                notify_video_control(route, kind, false);
            }
            return None;
        }
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
    notify_video_control(route, kind, dropped);
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

fn notify_video_control(route: &Route, kind: StreamKind, dropped: bool) {
    let targets = match kind {
        StreamKind::Screen => &route.screen,
        StreamKind::Camera => &route.camera,
    };
    let now = Instant::now();
    let mut budget = route.budget.lock();
    // Latch overflow until the next report, including over TCP fallback.
    if dropped {
        budget.drops.insert(kind, u64::MAX);
    }
    if budget
        .notified
        .get(&kind)
        .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(1))
    {
        return;
    }
    budget.notified.insert(kind, now);
    let stats: Vec<_> = targets.iter().map(Outbox::media_stats).collect();
    let drops: u64 = stats.iter().map(|s| s.dropped).fold(0, u64::saturating_add);
    let previous = budget.drops.insert(kind, drops).unwrap_or(drops);
    let viewers = &route.viewers[&kind];
    let healthy = !viewers.is_empty()
        && viewers.iter().all(|user| {
            budget
                .feedback
                .get(&(kind, *user))
                .is_some_and(|(at, bad)| now.duration_since(*at) <= Duration::from_secs(3) && !bad)
        });
    let congested = previous == u64::MAX
        || drops > previous
        || stats.iter().any(|s| s.queued_bytes > 150_000)
        || viewers.iter().any(|user| {
            budget
                .feedback
                .get(&(kind, *user))
                .is_some_and(|(at, bad)| now.duration_since(*at) <= Duration::from_secs(3) && *bad)
        });
    drop(budget);
    route
        .sender
        .push_urgent(encode_packet(&ServerMsg::VideoControl {
            kind,
            max_bps: route
                .limits
                .get(&kind)
                .copied()
                .unwrap_or_else(|| crate::media::bitrate::minimum(kind)),
            congested,
            healthy: healthy && !congested,
            overcommitted: route.overcommitted,
        }));
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

    #[test]
    fn seven_streams_share_budget_and_only_current_viewers_can_report_health() {
        let mut st = super::super::tests::fixture();
        let mut readers = Vec::new();
        for user in 1..=5 {
            let (out, peer) = connection();
            st.clients.insert(user, out);
            readers.push(peer);
            st.voice.insert(
                user,
                VoiceState {
                    channel: Some(3),
                    camera: true,
                    ..Default::default()
                },
            );
            st.streams.insert(
                (user, StreamKind::Camera),
                StreamInfo {
                    owner: user,
                    kind: StreamKind::Camera,
                    label: "camera".into(),
                    width: 1280,
                    height: 720,
                    fps: 30,
                },
            );
            if user <= 2 {
                st.streams.insert(
                    (user, StreamKind::Screen),
                    StreamInfo {
                        owner: user,
                        kind: StreamKind::Screen,
                        label: "screen".into(),
                        width: 1920,
                        height: 1080,
                        fps: 60,
                    },
                );
                st.watchers.insert(
                    (user, StreamKind::Screen),
                    (1..=5).filter(|viewer| *viewer != user).collect(),
                );
            }
        }
        st.refresh_media_routes();
        {
            let routes = st.media_router.routes.read();
            let mut total = 0u64;
            let mut streams = 0;
            for route in routes.values() {
                for (kind, bps) in &route.limits {
                    assert!(*bps >= crate::media::bitrate::minimum(*kind));
                    assert!(!route.overcommitted);
                    assert_eq!(route.viewers[kind].len(), 4);
                    total += u64::from(*bps) * 4;
                    streams += 1;
                }
            }
            assert_eq!(streams, 7);
            assert!((68_000_000..=st.media_router.video_bps).contains(&total));
        }
        for user in 2..=5 {
            assert!(st
                .media_router
                .dispatch(
                    user,
                    ClientMsg::VideoFeedback {
                        owner: 1,
                        kind: StreamKind::Screen,
                        congested: false
                    }
                )
                .is_none());
        }
        {
            let routes = st.media_router.routes.read();
            let route = &routes[&1];
            let mut budget = route.budget.lock();
            assert_eq!(budget.feedback.len(), 4);
            budget.notified.clear();
            drop(budget);
            notify_video_control(route, StreamKind::Screen, false);
        }
        // Initial allocations and the first partial feedback precede the report
        // confirming that all four current screen viewers are healthy.
        let mut healthy = false;
        for _ in 0..4 {
            if matches!(
                read_packet::<_, ServerMsg>(&mut readers[0]).unwrap(),
                ServerMsg::VideoControl {
                    kind: StreamKind::Screen,
                    healthy: true,
                    congested: false,
                    ..
                }
            ) {
                healthy = true;
                break;
            }
        }
        assert!(healthy);
        st.media_router.dispatch(
            99,
            ClientMsg::VideoFeedback {
                owner: 1,
                kind: StreamKind::Screen,
                congested: true,
            },
        );
        st.media_router.dispatch(
            1,
            ClientMsg::VideoFeedback {
                owner: 1,
                kind: StreamKind::Screen,
                congested: true,
            },
        );
        assert_eq!(
            st.media_router.routes.read()[&1]
                .budget
                .lock()
                .feedback
                .len(),
            4
        );
        // An immediate error overrides a just-reported healthy sample.
        st.media_router.dispatch(
            2,
            ClientMsg::VideoFeedback {
                owner: 1,
                kind: StreamKind::Screen,
                congested: true,
            },
        );
        assert!(
            st.media_router.routes.read()[&1].budget.lock().feedback[&(StreamKind::Screen, 2)].1
        );
        {
            let routes = st.media_router.routes.read();
            let route = &routes[&1];
            route.budget.lock().notified.clear();
            notify_video_control(route, StreamKind::Screen, false);
        }
        assert!(matches!(
            read_packet::<_, ServerMsg>(&mut readers[0]).unwrap(),
            ServerMsg::VideoControl {
                healthy: false,
                congested: true,
                ..
            }
        ));
        // Old receiver reports cannot authorize growth.
        {
            let routes = st.media_router.routes.read();
            let route = &routes[&1];
            let mut budget = route.budget.lock();
            for report in budget.feedback.values_mut() {
                *report = (Instant::now() - Duration::from_secs(4), false);
            }
            budget.notified.clear();
            drop(budget);
            notify_video_control(route, StreamKind::Screen, false);
        }
        assert!(matches!(
            read_packet::<_, ServerMsg>(&mut readers[0]).unwrap(),
            ServerMsg::VideoControl { healthy: false, .. }
        ));
        // Leaving the room revokes feedback rights and removes its stored sample.
        st.voice.get_mut(&2).unwrap().channel = Some(4);
        st.refresh_media_routes();
        st.media_router.dispatch(
            2,
            ClientMsg::VideoFeedback {
                owner: 1,
                kind: StreamKind::Screen,
                congested: true,
            },
        );
        assert!(!st.media_router.routes.read()[&1]
            .budget
            .lock()
            .feedback
            .contains_key(&(StreamKind::Screen, 2)));
        for out in st.clients.values() {
            out.close();
        }
    }
}
