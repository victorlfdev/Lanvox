//! Session-bound AEAD datagrams for real-time media. Session secrets travel only
//! inside Noise. Direction-specific keys, unique packet nonces and a replay
//! window protect reordered/lost UDP; partial video expires instead of blocking
//! audio. Each fragment is <= 1200 bytes to avoid normal VPN MTU fragmentation.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::rand::{SecureRandom, SystemRandom};

use crate::model::UserId;
use crate::protocol::{read_packet_limit, ClientMsg, ServerMsg};

const HEADER: usize = 40;
const CHUNK: usize = 1200 - HEADER - 16;
const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const MAX_ASSEMBLY: usize = 16 * 1024 * 1024;
/// Audio and control datagrams are useless after this long.
const MEDIA_AGE: Duration = Duration::from_millis(250);
/// Video messages (a 1080p IDR is hundreds of KB and many fragments) get longer
/// to be sent and reassembled; 250 ms made large keyframes die in flight.
const VIDEO_AGE: Duration = Duration::from_millis(700);
/// Bulk (video) fragments are paced to this rate so an IDR burst does not
/// overflow router queues or the receiver's UDP buffer.
const PACING_BYTES_PER_SEC: f64 = 6.0 * 1024.0 * 1024.0;
const MAX_BULK_MESSAGES: usize = 8;
const PATH_CHALLENGE: u64 = 1 << 63;

#[derive(Clone, Copy, Debug, Default)]
pub struct MediaStats {
    pub active: bool,
    pub sent_bytes: u64,
    pub received_bytes: u64,
    pub dropped: u64,
    pub queued_bytes: usize,
}

#[derive(Default)]
struct Counters {
    sent: AtomicU64,
    received: AtomicU64,
    dropped: AtomicU64,
}

#[derive(Default)]
struct Replay {
    highest: Option<u64>,
    bits: [u64; 16],
}

impl Replay {
    fn accept(&mut self, value: u64) -> bool {
        if let Some(highest) = self.highest {
            if value <= highest {
                let delta = highest - value;
                if delta >= 1024 {
                    return false;
                }
                let (slot, bit) = ((delta / 64) as usize, delta % 64);
                if self.bits[slot] & (1 << bit) != 0 {
                    return false;
                }
                self.bits[slot] |= 1 << bit;
                return true;
            }
            let shift = (value - highest).min(1024) as usize;
            let previous = self.bits;
            self.bits.fill(0);
            // Shift whole words instead of visiting all 1024 sequence slots for
            // every received fragment. Keep the same replay/reordering window.
            let words = shift / 64;
            let bits = shift % 64;
            for dest in words..self.bits.len() {
                let source = dest - words;
                self.bits[dest] = previous[source] << bits;
                if bits != 0 && source > 0 {
                    self.bits[dest] |= previous[source - 1] >> (64 - bits);
                }
            }
        }
        self.highest = Some(value);
        self.bits[0] |= 1;
        true
    }
}

struct Assembly {
    created: Instant,
    pieces: Vec<Option<Vec<u8>>>,
    bytes: usize,
    remaining: usize,
}

struct Session {
    token: [u8; 16],
    encrypt: LessSafeKey,
    decrypt: LessSafeKey,
    sequence: AtomicU64,
    message: AtomicU64,
    replay: Mutex<Replay>,
    assemblies: Mutex<HashMap<u64, Assembly>>,
    peer: Mutex<Option<SocketAddr>>,
    last_received: Mutex<Option<Instant>>,
    alive: AtomicBool,
    counters: Counters,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
    client: bool,
    peer_verified: AtomicBool,
    path_challenge: Mutex<Option<PathChallenge>>,
}

struct PathChallenge {
    address: SocketAddr,
    nonce: u64,
    original_ping: u64,
    created: Instant,
}

fn derive(master: &[u8; 32], token: &[u8; 16], direction: &[u8]) -> LessSafeKey {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, master);
    // Kept from the previous name on purpose; see branding.rs.
    let mut context = b"Huddle datagram v3\0".to_vec();
    context.extend_from_slice(token);
    context.extend_from_slice(direction);
    let material = ring::hmac::sign(&key, &context);
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, material.as_ref()).expect("32 byte key"))
}

impl Session {
    fn new(token: [u8; 16], master: [u8; 32], client: bool, peer: Option<SocketAddr>) -> Self {
        let (send, receive): (&[u8], &[u8]) = if client {
            (b"client-to-host", b"host-to-client")
        } else {
            (b"host-to-client", b"client-to-host")
        };
        Self {
            token,
            encrypt: derive(&master, &token, send),
            decrypt: derive(&master, &token, receive),
            sequence: AtomicU64::new(0),
            message: AtomicU64::new(0),
            replay: Mutex::new(Replay::default()),
            assemblies: Mutex::new(HashMap::new()),
            peer: Mutex::new(peer),
            last_received: Mutex::new(None),
            alive: AtomicBool::new(true),
            counters: Counters::default(),
            writer: Mutex::new(None),
            client,
            peer_verified: AtomicBool::new(client),
            path_challenge: Mutex::new(None),
        }
    }

    fn stop(&self) {
        self.alive.store(false, Ordering::Relaxed);
        let mut writer = self.writer.lock();
        if let Some(thread) = writer.take() {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }

    fn seal(&self, id: u64, index: u16, count: u16, piece: &[u8]) -> Option<Vec<u8>> {
        let seq = self
            .sequence
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .ok()?;
        let mut header = Vec::with_capacity(HEADER);
        header.extend_from_slice(b"HD3U");
        header.extend_from_slice(&self.token);
        header.extend_from_slice(&seq.to_le_bytes());
        header.extend_from_slice(&id.to_le_bytes());
        header.extend_from_slice(&index.to_le_bytes());
        header.extend_from_slice(&count.to_le_bytes());
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&seq.to_le_bytes());
        let mut body = piece.to_vec();
        self.encrypt
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&header),
                &mut body,
            )
            .ok()?;
        header.extend_from_slice(&body);
        Some(header)
    }

    fn open(&self, packet: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
        if packet.len() < HEADER + 16
            || packet.len() > 1200
            || &packet[..4] != b"HD3U"
            || packet[4..20] != self.token
        {
            return None;
        }
        let seq = u64::from_le_bytes(packet[20..28].try_into().ok()?);
        let id = u64::from_le_bytes(packet[28..36].try_into().ok()?);
        let index = usize::from(u16::from_le_bytes(packet[36..38].try_into().ok()?));
        let count = usize::from(u16::from_le_bytes(packet[38..40].try_into().ok()?));
        if count == 0 || count > MAX_MESSAGE.div_ceil(CHUNK) || index >= count {
            return None;
        }
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&seq.to_le_bytes());
        let mut body = packet[HEADER..].to_vec();
        let plain = self
            .decrypt
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&packet[..HEADER]),
                &mut body,
            )
            .ok()?;
        if !self.replay.lock().accept(seq) {
            return None;
        }
        if !self.client && *self.peer.lock() != Some(from) {
            self.peer_verified.store(false, Ordering::Relaxed);
        }
        *self.last_received.lock() = Some(Instant::now());
        self.counters
            .received
            .fetch_add(packet.len() as u64, Ordering::Relaxed);
        if count == 1 {
            return Some(plain.to_vec());
        }
        let mut map = self.assemblies.lock();
        map.retain(|_, assembly| assembly.created.elapsed() < VIDEO_AGE);
        if map.values().map(|assembly| assembly.bytes).sum::<usize>() + plain.len() > MAX_ASSEMBLY
            || (map.len() >= 8 && !map.contains_key(&id))
        {
            return None;
        }
        let assembly = map.entry(id).or_insert_with(|| Assembly {
            created: Instant::now(),
            pieces: vec![None; count],
            bytes: 0,
            remaining: count,
        });
        if assembly.pieces.len() != count || assembly.pieces[index].is_some() {
            return None;
        }
        assembly.bytes += plain.len();
        if assembly.bytes > MAX_MESSAGE {
            map.remove(&id);
            return None;
        }
        assembly.pieces[index] = Some(plain.to_vec());
        assembly.remaining -= 1;
        if assembly.remaining != 0 {
            return None;
        }
        let complete = map.remove(&id)?;
        Some(complete.pieces.into_iter().flatten().flatten().collect())
    }

    fn verified_peer(&self, address: SocketAddr) -> bool {
        self.peer_verified.load(Ordering::Relaxed) && *self.peer.lock() == Some(address)
    }

    fn path_probe(&self, address: SocketAddr, value: u64) -> Option<u64> {
        if self.verified_peer(address) {
            return Some(value & !PATH_CHALLENGE);
        }
        let mut pending = self.path_challenge.lock();
        if pending.as_ref().is_some_and(|probe| {
            probe.address == address
                && probe.nonce == value
                && probe.created.elapsed() < Duration::from_secs(3)
        }) {
            let probe = pending.take()?;
            *self.peer.lock() = Some(address);
            self.peer_verified.store(true, Ordering::Relaxed);
            return Some(probe.original_ping);
        }
        // One pending address and at most two tiny challenges per second per
        // authenticated session. A spoofed source never becomes a media target.
        if pending
            .as_ref()
            .is_some_and(|probe| probe.created.elapsed() < Duration::from_millis(500))
        {
            return None;
        }
        let mut entropy = [0; 8];
        SystemRandom::new().fill(&mut entropy).ok()?;
        let nonce = u64::from_le_bytes(entropy) | PATH_CHALLENGE;
        let original_ping = if value & PATH_CHALLENGE != 0 {
            pending
                .as_ref()
                .filter(|probe| probe.address == address)
                .map(|probe| probe.original_ping)
                .unwrap_or(0)
        } else {
            value
        };
        *pending = Some(PathChallenge {
            address,
            nonce,
            original_ping,
            created: Instant::now(),
        });
        Some(nonce)
    }
}

struct Pending {
    packet: Vec<u8>,
    created: Instant,
    id: u64,
    index: usize,
}

#[derive(Default)]
struct Queue {
    urgent: VecDeque<Pending>,
    bulk: VecDeque<Pending>,
    bytes: usize,
}

#[derive(Clone)]
pub struct MediaSender {
    session: Arc<Session>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
}

impl MediaSender {
    fn start(socket: Arc<UdpSocket>, session: Arc<Session>) -> Self {
        let sender = Self {
            session,
            queue: Arc::new((Mutex::new(Queue::default()), Condvar::new())),
        };
        let worker = sender.clone();
        let thread = std::thread::Builder::new()
            .name("media-send".into())
            .spawn(move || {
                // Earliest time the next bulk fragment may leave (token pacing).
                let mut next_bulk = Instant::now();
                while worker.session.alive.load(Ordering::Relaxed) {
                    let (pending, bulk) = {
                        let (lock, cv) = &*worker.queue;
                        let mut queue = lock.lock();
                        loop {
                            if !worker.session.alive.load(Ordering::Relaxed) {
                                return;
                            }
                            if let Some(p) = queue.urgent.pop_front() {
                                break (Some(p), false);
                            }
                            if !queue.bulk.is_empty() {
                                let wait = next_bulk.saturating_duration_since(Instant::now());
                                if wait.is_zero() {
                                    break (queue.bulk.pop_front(), true);
                                }
                                // Scheduler delays are recovered by the bounded
                                // 5 ms allowance below, never by spinning.
                                // Windows condvar timeouts round up to coarse
                                // system ticks. Rust's sleep uses a high resolution
                                // waitable timer; cap it at 1 ms so audio and close
                                // remain responsive even without a notification.
                                #[cfg(windows)]
                                {
                                    drop(queue);
                                    std::thread::sleep(wait.min(Duration::from_millis(1)));
                                    queue = lock.lock();
                                }
                                #[cfg(not(windows))]
                                cv.wait_for(&mut queue, wait);
                            } else {
                                cv.wait_for(&mut queue, Duration::from_millis(100));
                            }
                        }
                    };
                    let Some(mut pending) = pending else { continue };
                    let max_age = if bulk { VIDEO_AGE } else { MEDIA_AGE };
                    if pending.created.elapsed() > max_age {
                        let mut queue = worker.queue.0.lock();
                        queue.bytes = queue.bytes.saturating_sub(pending.packet.len());
                        worker
                            .session
                            .counters
                            .dropped
                            .fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let count = pending.packet.len().div_ceil(CHUNK);
                    let begin = pending.index * CHUNK;
                    let end = (begin + CHUNK).min(pending.packet.len());
                    let peer = *worker.session.peer.lock();
                    if let (Some(peer), Some(packet)) = (
                        peer,
                        worker.session.seal(
                            pending.id,
                            pending.index as u16,
                            count as u16,
                            &pending.packet[begin..end],
                        ),
                    ) {
                        if let Ok(n) = socket.send_to(&packet, peer) {
                            worker
                                .session
                                .counters
                                .sent
                                .fetch_add(n as u64, Ordering::Relaxed);
                        }
                    }
                    if bulk {
                        let slot = Duration::from_secs_f64(CHUNK as f64 / PACING_BYTES_PER_SEC);
                        let now = Instant::now();
                        let floor = now.checked_sub(Duration::from_millis(5)).unwrap_or(now);
                        next_bulk = next_bulk.max(floor) + slot;
                    }
                    pending.index += 1;
                    let mut queue = worker.queue.0.lock();
                    if pending.index < count {
                        queue.bulk.push_front(pending);
                    } else {
                        queue.bytes = queue.bytes.saturating_sub(pending.packet.len());
                    }
                }
            })
            .expect("spawn media writer");
        *sender.session.writer.lock() = Some(thread);
        sender
    }

    /// True means a media update was dropped; callers should force a keyframe.
    pub fn send(&self, packet: Vec<u8>, urgent: bool) -> bool {
        if !self.session.alive.load(Ordering::Relaxed)
            || packet.is_empty()
            || packet.len() > MAX_MESSAGE
        {
            return true;
        }
        let mut queue = self.queue.0.lock();
        let mut dropped = false;
        while queue.bytes.saturating_add(packet.len()) > MAX_ASSEMBLY
            || if urgent {
                queue.urgent.len() >= 64
            } else {
                queue.bulk.len() >= MAX_BULK_MESSAGES
            }
        {
            let old = queue.bulk.pop_front().or_else(|| queue.urgent.pop_front());
            let Some(old) = old else { return true };
            queue.bytes = queue.bytes.saturating_sub(old.packet.len());
            self.session
                .counters
                .dropped
                .fetch_add(1, Ordering::Relaxed);
            dropped = true;
        }
        let pending = Pending {
            packet,
            created: Instant::now(),
            id: self.session.message.fetch_add(1, Ordering::Relaxed),
            index: 0,
        };
        queue.bytes += pending.packet.len();
        if urgent {
            queue.urgent.push_back(pending);
        } else {
            queue.bulk.push_back(pending);
        }
        self.queue.1.notify_one();
        dropped
    }

    pub fn stats(&self) -> MediaStats {
        MediaStats {
            active: self.session.peer_verified.load(Ordering::Relaxed)
                && self
                    .session
                    .last_received
                    .lock()
                    .is_some_and(|last| last.elapsed() < Duration::from_secs(10)),
            sent_bytes: self.session.counters.sent.load(Ordering::Relaxed),
            received_bytes: self.session.counters.received.load(Ordering::Relaxed),
            dropped: self.session.counters.dropped.load(Ordering::Relaxed),
            queued_bytes: self.queue.0.lock().bytes,
        }
    }

    pub fn close(&self) {
        self.session.alive.store(false, Ordering::Relaxed);
        self.queue.1.notify_all();
        self.session.stop();
    }
}

type Sessions = Arc<Mutex<HashMap<[u8; 16], (UserId, Arc<Session>)>>>;

pub struct ServerMedia {
    socket: Arc<UdpSocket>,
    sessions: Sessions,
    alive: Arc<AtomicBool>,
    receiver: Option<std::thread::JoinHandle<()>>,
}

impl ServerMedia {
    pub fn bind(
        address: SocketAddr,
        dispatch: impl Fn(UserId, ClientMsg) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let socket = Arc::new(UdpSocket::bind(address)?);
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        socket.set_write_timeout(Some(Duration::from_millis(100)))?;
        let sessions = Arc::new(Mutex::new(
            HashMap::<[u8; 16], (UserId, Arc<Session>)>::new(),
        ));
        let alive = Arc::new(AtomicBool::new(true));
        let (reader, entries, running) = (socket.clone(), sessions.clone(), alive.clone());
        let receiver = std::thread::Builder::new()
            .name("media-host".into())
            .spawn(move || {
                let mut packet = [0; 1201];
                while running.load(Ordering::Relaxed) {
                    let Ok((n, from)) = reader.recv_from(&mut packet) else {
                        for (_, session) in entries.lock().values() {
                            session
                                .assemblies
                                .lock()
                                .retain(|_, assembly| assembly.created.elapsed() < VIDEO_AGE);
                        }
                        continue;
                    };
                    if !(HEADER + 16..=1200).contains(&n) {
                        continue;
                    }
                    let Ok(token) = <[u8; 16]>::try_from(&packet[4..20]) else {
                        continue;
                    };
                    let Some((user, session)) = entries.lock().get(&token).cloned() else {
                        continue;
                    };
                    if let Some(plain) = session.open(&packet[..n], from) {
                        if let Ok(message) =
                            read_packet_limit::<_, ClientMsg>(&mut plain.as_slice(), MAX_MESSAGE)
                        {
                            if let ClientMsg::Ping(value) = message {
                                let Some(value) = session.path_probe(from, value) else {
                                    continue;
                                };
                                let packet =
                                    crate::protocol::encode_packet(&ServerMsg::Pong(value));
                                let id = session.message.fetch_add(1, Ordering::Relaxed);
                                if let Some(packet) = session.seal(id, 0, 1, &packet) {
                                    let _ = reader.send_to(&packet, from);
                                }
                                continue;
                            }
                            if !session.verified_peer(from) {
                                continue;
                            }
                            if matches!(
                                message,
                                ClientMsg::AudioEncoded { .. }
                                    | ClientMsg::StreamAudio { .. }
                                    | ClientMsg::Audio { .. }
                                    | ClientMsg::Frame { .. }
                                    | ClientMsg::EncodedFrame { .. }
                                    | ClientMsg::Cursor(_)
                                    | ClientMsg::Speaking(_)
                                    | ClientMsg::Ping(_)
                            ) {
                                dispatch(user, message);
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            socket,
            sessions,
            alive,
            receiver: Some(receiver),
        })
    }

    pub fn register(&self, user: UserId) -> io::Result<(ServerMsg, MediaSender)> {
        self.unregister(user);
        let (mut token, mut key) = ([0; 16], [0; 32]);
        SystemRandom::new()
            .fill(&mut token)
            .map_err(|_| io::Error::other("session entropy unavailable"))?;
        SystemRandom::new()
            .fill(&mut key)
            .map_err(|_| io::Error::other("session entropy unavailable"))?;
        let session = Arc::new(Session::new(token, key, false, None));
        self.sessions.lock().insert(token, (user, session.clone()));
        Ok((
            ServerMsg::MediaSession {
                port: self.socket.local_addr()?.port(),
                token,
                key,
            },
            MediaSender::start(self.socket.clone(), session),
        ))
    }

    pub fn unregister(&self, user: UserId) {
        let mut removed = Vec::new();
        self.sessions.lock().retain(|_, (id, session)| {
            if *id == user {
                removed.push(session.clone());
                false
            } else {
                true
            }
        });
        for session in removed {
            session.stop();
        }
    }
}

impl Drop for ServerMedia {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        for (_, session) in self.sessions.lock().values() {
            session.stop();
        }
        if let Some(thread) = self.receiver.take() {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(feature = "gui")]
pub fn connect(
    address: SocketAddr,
    token: [u8; 16],
    key: [u8; 32],
    dispatch: impl Fn(ServerMsg) + Send + 'static,
) -> io::Result<MediaSender> {
    // Loopback targets bind to loopback so no firewall prompt is triggered.
    let bind = match (address.is_ipv4(), address.ip().is_loopback()) {
        (true, true) => "127.0.0.1:0",
        (true, false) => "0.0.0.0:0",
        (false, true) => "[::1]:0",
        (false, false) => "[::]:0",
    };
    let socket = Arc::new(UdpSocket::bind(bind)?);
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    socket.set_write_timeout(Some(Duration::from_millis(100)))?;
    let session = Arc::new(Session::new(token, key, true, Some(address)));
    let sender = MediaSender::start(socket.clone(), session.clone());
    std::thread::Builder::new()
        .name("media-reader".into())
        .spawn(move || {
            let mut packet = [0; 1201];
            while session.alive.load(Ordering::Relaxed) {
                let Ok((n, from)) = socket.recv_from(&mut packet) else {
                    session
                        .assemblies
                        .lock()
                        .retain(|_, assembly| assembly.created.elapsed() < VIDEO_AGE);
                    continue;
                };
                if from != address {
                    continue;
                }
                if let Some(plain) = session.open(&packet[..n], from) {
                    if let Ok(message) =
                        read_packet_limit::<_, ServerMsg>(&mut plain.as_slice(), MAX_MESSAGE)
                    {
                        if let ServerMsg::Pong(value) = message {
                            if value & PATH_CHALLENGE != 0 {
                                let packet =
                                    crate::protocol::encode_packet(&ClientMsg::Ping(value));
                                let id = session.message.fetch_add(1, Ordering::Relaxed);
                                if let Some(packet) = session.seal(id, 0, 1, &packet) {
                                    let _ = socket.send_to(&packet, address);
                                }
                                continue;
                            }
                        }
                        dispatch(message);
                    }
                }
            }
        })?;
    sender.send(crate::protocol::encode_packet(&ClientMsg::Ping(0)), true);
    Ok(sender)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_window_matches_sequence_set_across_word_boundaries() {
        let mut replay = Replay::default();
        let mut seen = std::collections::HashSet::new();
        let mut highest = 0u64;
        let mut seed = 17u64;
        let mut values = vec![
            0, 1, 63, 64, 65, 127, 128, 1023, 1024, 1025, 64, 4096, 3073, 3072,
        ];
        for _ in 0..10_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            highest += seed % 4;
            values.push(highest.saturating_sub((seed >> 32) % 1200));
        }
        highest = 0;
        for value in values {
            highest = highest.max(value);
            let expected = highest - value < 1024 && seen.insert(value);
            assert_eq!(
                replay.accept(value),
                expected,
                "sequence {value}, highest {highest}"
            );
        }
    }

    #[test]
    #[ignore = "manual CPU benchmark; reports throughput without network or capture"]
    fn benchmark_media_reassembly() {
        let client = Session::new([23; 16], [29; 32], true, None);
        let server = Session::new([23; 16], [29; 32], false, None);
        let address = "127.0.0.1:47800".parse().unwrap();
        for size in [25_000usize, 250_000, 1_000_000] {
            let data = vec![7u8; size];
            let started = Instant::now();
            let frames = 100;
            let mut completed = 0;
            for id in 0..frames {
                let count = size.div_ceil(CHUNK);
                for (index, chunk) in data.chunks(CHUNK).enumerate() {
                    let packet = client.seal(id, index as u16, count as u16, chunk).unwrap();
                    if let Some(complete) = server.open(&packet, address) {
                        assert_eq!(complete, data);
                        completed += 1;
                    }
                }
            }
            assert_eq!(completed, frames);
            eprintln!(
                "media reassembly {size} bytes: {:.1} fps, {:.1} Mb/s",
                frames as f64 / started.elapsed().as_secs_f64(),
                (frames as f64 * size as f64 * 8.0) / started.elapsed().as_secs_f64() / 1e6
            );
        }
    }

    #[test]
    #[ignore = "manual paced UDP sender benchmark; opens loopback sockets"]
    fn benchmark_media_sender() {
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        sink.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let address = sink.local_addr().unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let receiving = alive.clone();
        let reader = std::thread::spawn(move || {
            let mut buffer = [0u8; 1500];
            let mut bytes = 0usize;
            while receiving.load(Ordering::Relaxed) {
                if let Ok(n) = sink.recv(&mut buffer) {
                    bytes += n;
                }
            }
            bytes
        });
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
        let session = Arc::new(Session::new([31; 16], [37; 32], true, Some(address)));
        let sender = MediaSender::start(socket, session);
        let start = Instant::now();
        for i in 0..120 {
            let due = start + Duration::from_secs_f64(i as f64 / 60.0);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            sender.send(vec![7; 75_000], false);
        }
        while sender.stats().queued_bytes > 0 && start.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let elapsed = start.elapsed();
        let stats = sender.stats();
        sender.close();
        alive.store(false, Ordering::Relaxed);
        let received = reader.join().unwrap();
        eprintln!(
            "paced sender: {:.3}s, {} sent, {} received, {} dropped",
            elapsed.as_secs_f64(),
            stats.sent_bytes,
            received,
            stats.dropped
        );
        assert_eq!(stats.dropped, 0);
        assert_eq!(stats.queued_bytes, 0);
    }

    #[test]
    fn authenticates_binds_direction_and_rejects_tampering_replay_and_other_sessions() {
        let client = Session::new([1; 16], [7; 32], true, None);
        let server = Session::new([1; 16], [7; 32], false, None);
        let address = "127.0.0.1:47800".parse().unwrap();
        let packet = client.seal(1, 0, 1, b"voice").unwrap();
        let mut tampered = packet.clone();
        tampered[HEADER] ^= 1;
        assert!(server.open(&tampered, address).is_none());
        assert!(client.open(&packet, address).is_none());
        assert_eq!(server.open(&packet, address).unwrap(), b"voice");
        assert!(server.open(&packet, address).is_none());
        let other = Session::new([2; 16], [7; 32], false, None);
        assert!(other.open(&packet, address).is_none());
    }

    #[test]
    fn assembles_reordered_fragments_and_drops_replayed_fragment() {
        let client = Session::new([3; 16], [9; 32], true, None);
        let server = Session::new([3; 16], [9; 32], false, None);
        let address = "127.0.0.1:47800".parse().unwrap();
        let first = client.seal(1, 0, 2, b"first").unwrap();
        let second = client.seal(1, 1, 2, b"second").unwrap();
        assert!(server.open(&second, address).is_none());
        assert!(server.open(&second, address).is_none());
        assert_eq!(server.open(&first, address).unwrap(), b"firstsecond");
    }

    #[test]
    fn duplicate_fragment_with_fresh_nonce_does_not_complete_assembly_early() {
        let client = Session::new([41; 16], [43; 32], true, None);
        let server = Session::new([41; 16], [43; 32], false, None);
        let address = "127.0.0.1:47800".parse().unwrap();
        for _ in 0..3 {
            let repeated = client.seal(1, 1, 3, b"middle").unwrap();
            assert!(server.open(&repeated, address).is_none());
        }
        let last = client.seal(1, 2, 3, b"last").unwrap();
        assert!(server.open(&last, address).is_none());
        let first = client.seal(1, 0, 3, b"first").unwrap();
        assert_eq!(server.open(&first, address).unwrap(), b"firstmiddlelast");
    }

    #[test]
    fn stopping_host_releases_udp_port_even_with_a_sender_clone() {
        let host = ServerMedia::bind("127.0.0.1:0".parse().unwrap(), |_, _| {}).unwrap();
        let address = host.socket.local_addr().unwrap();
        let (_, sender) = host.register(1).unwrap();
        drop(host);
        let rebound = UdpSocket::bind(address)
            .expect("media reader and writer must release their shared socket before stop returns");
        assert_eq!(rebound.local_addr().unwrap(), address);
        sender.close();
    }

    #[test]
    fn source_spoofing_and_rebinding_require_a_fresh_return_path_proof() {
        let server = Session::new([4; 16], [8; 32], false, None);
        let client = Session::new([4; 16], [8; 32], true, None);
        let first: SocketAddr = "127.0.0.1:40100".parse().unwrap();
        let second: SocketAddr = "127.0.0.1:40200".parse().unwrap();
        let challenge = server.path_probe(first, 42).unwrap();
        assert_ne!(challenge & PATH_CHALLENGE, 0);
        assert!(!server.verified_peer(first));
        assert_eq!(*server.peer.lock(), None);
        assert!(
            server.path_probe(first, 43).is_none(),
            "challenge requests must be rate limited"
        );
        assert!(
            server.path_probe(second, challenge).is_none(),
            "proof is bound to its source address"
        );
        assert_eq!(server.path_probe(first, challenge), Some(42));
        assert!(server.verified_peer(first));
        // A valid session ciphertext from a different source address is still
        // not evidence that its owner can receive packets at that address.
        let packet = client.seal(1, 0, 1, b"media").unwrap();
        assert!(server.open(&packet, second).is_some());
        assert!(!server.verified_peer(second));
        assert_eq!(*server.peer.lock(), Some(first));
        let next = server.path_probe(second, 99).unwrap();
        assert_eq!(server.path_probe(second, next), Some(99));
        assert!(server.verified_peer(second));
    }

    #[test]
    fn expired_path_proofs_are_replaced_instead_of_accepted() {
        let server = Session::new([5; 16], [9; 32], false, None);
        let address: SocketAddr = "127.0.0.1:40100".parse().unwrap();
        let old = server.path_probe(address, 1).unwrap();
        server.path_challenge.lock().as_mut().unwrap().created =
            Instant::now() - Duration::from_secs(4);
        let fresh = server.path_probe(address, old).unwrap();
        assert!(!server.verified_peer(address));
        assert_ne!(fresh, old);
        assert_eq!(server.path_probe(address, fresh), Some(1));
        assert!(server.verified_peer(address));
    }

    #[test]
    #[cfg(feature = "gui")]
    fn encrypted_udp_sessions_exchange_real_media_after_authenticated_probe() {
        let (host_tx, host_rx) = crossbeam_channel::bounded(4);
        let host = ServerMedia::bind("127.0.0.1:0".parse().unwrap(), move |user, packet| {
            let _ = host_tx.send((user, packet));
        })
        .unwrap();
        let (ServerMsg::MediaSession { port, token, key }, sender) = host.register(42).unwrap()
        else {
            panic!("media session")
        };
        let (client_tx, client_rx) = crossbeam_channel::bounded(4);
        let client = connect(
            SocketAddr::from(([127, 0, 0, 1], port)),
            token,
            key,
            move |packet| {
                let _ = client_tx.send(packet);
            },
        )
        .unwrap();
        assert!(matches!(
            client_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            ServerMsg::Pong(0)
        ));
        assert!(client.stats().active);
        assert!(sender.stats().active);
        client.send(
            crate::protocol::encode_packet(&ClientMsg::AudioEncoded {
                seq: 8,
                sent_micros: 123,
                opus: vec![1, 2, 3],
            }),
            true,
        );
        let (user, message) = host_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(user, 42);
        assert!(matches!(message, ClientMsg::AudioEncoded { seq: 8, .. }));
        sender.send(
            crate::protocol::encode_packet(&ServerMsg::AudioEncoded {
                from: 42,
                seq: 8,
                sent_micros: 123,
                opus: vec![1, 2, 3],
            }),
            true,
        );
        assert!(matches!(
            client_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            ServerMsg::AudioEncoded {
                from: 42,
                seq: 8,
                ..
            }
        ));
        host.unregister(42);
        client.close();
    }
}
