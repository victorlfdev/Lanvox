//! Validates and reconstructs stream pictures, recovering after missing deltas.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Sender};
use parking_lot::Mutex;
use rayon::prelude::*;

use super::codec::decode_jpeg;
use crate::model::{StreamKind, UserId};
use crate::net::client::ClientSender;
use crate::protocol::{picture_bytes, CursorShape, CursorState, EncodedVideoFrame, VideoFrame};

pub type StreamKey = (UserId, StreamKind);
const MAX_STORE_BYTES: usize = 128 * 1024 * 1024;

pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub version: u64,
    pub fps: f32,
    pub kbps: f32,
    pub decode_ms: f32,
    pub queue_ms: f32,
    frames: u32,
    bytes: usize,
    window: Instant,
    last_seq: Option<u32>,
}

impl Picture {
    fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
            version: 0,
            fps: 0.0,
            kbps: 0.0,
            decode_ms: 0.0,
            queue_ms: 0.0,
            frames: 0,
            bytes: 0,
            window: Instant::now(),
            last_seq: None,
        }
    }

    fn count(&mut self, bytes: usize) {
        self.frames += 1;
        self.bytes += bytes;
        let elapsed = self.window.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            self.fps = self.frames as f32 / elapsed;
            self.kbps = self.bytes as f32 * 8.0 / 1000.0 / elapsed;
            self.frames = 0;
            self.bytes = 0;
            self.window = Instant::now();
        }
    }
}

/// The pointer of a shared screen as the viewer last heard of it.
#[derive(Clone)]
pub struct CursorView {
    pub x: f32,
    pub y: f32,
    pub visible: bool,
    pub area_w: u32,
    pub shape: Option<Arc<CursorShape>>,
    /// Bumps when `shape` changes, so the texture is only rebuilt then.
    pub shape_version: u64,
}

#[derive(Clone, Default)]
pub struct PictureStore {
    inner: Arc<Mutex<HashMap<StreamKey, Picture>>>,
    cursors: Arc<Mutex<HashMap<UserId, CursorView>>>,
}

impl PictureStore {
    pub fn put_cursor(&self, owner: UserId, state: CursorState) {
        if !state.valid() {
            return;
        }
        let mut map = self.cursors.lock();
        let previous = map.get(&owner);
        let (shape, shape_version) = match state.shape {
            Some(new) if previous.and_then(|p| p.shape.as_deref()) != Some(&new) => (
                Some(Arc::new(new)),
                previous.map_or(1, |p| p.shape_version + 1),
            ),
            // Same shape again (the periodic repeat), or none attached: keep what we have.
            _ => previous.map_or((None, 0), |p| (p.shape.clone(), p.shape_version)),
        };
        map.insert(
            owner,
            CursorView {
                x: f32::from(state.x) / 65535.0,
                y: f32::from(state.y) / 65535.0,
                visible: state.visible,
                area_w: state.area_w,
                shape,
                shape_version,
            },
        );
    }

    pub fn cursor(&self, owner: UserId) -> Option<CursorView> {
        self.cursors.lock().get(&owner).cloned()
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut HashMap<StreamKey, Picture>) -> R) -> R {
        f(&mut self.inner.lock())
    }

    pub fn remove(&self, key: StreamKey) {
        self.inner.lock().remove(&key);
        if key.1 == StreamKind::Screen {
            self.cursors.lock().remove(&key.0);
        }
    }

    fn record_decode_timing(&self, key: StreamKey, queue_ms: f32, decode_ms: f32) {
        if let Some(picture) = self.inner.lock().get_mut(&key) {
            picture.queue_ms = queue_ms;
            picture.decode_ms = decode_ms;
        }
    }

    pub fn clear(&self) {
        self.inner.lock().clear();
        self.cursors.lock().clear();
    }

    pub fn put(&self, key: StreamKey, width: u32, height: u32, rgba: Vec<u8>, bytes: usize) {
        if picture_bytes(width, height) != Some(rgba.len()) {
            return;
        }
        let mut map = self.inner.lock();
        if !fits_store(&map, key, rgba.len()) {
            return;
        }
        let p = map
            .entry(key)
            .or_insert_with(|| Picture::new(width, height, Vec::new()));
        p.width = width;
        p.height = height;
        p.rgba = rgba;
        p.version += 1;
        p.count(bytes);
    }

    #[cfg(test)]
    pub fn apply(&self, key: StreamKey, frame: &VideoFrame) -> bool {
        self.apply_if_active(key, frame, || true)
    }

    fn invalidate(&self, key: StreamKey) {
        if let Some(p) = self.inner.lock().get_mut(&key) {
            p.last_seq = None;
        }
    }

    fn apply_if_active(
        &self,
        key: StreamKey,
        frame: &VideoFrame,
        active: impl Fn() -> bool,
    ) -> bool {
        if !active() {
            return true;
        }
        if !frame.validate() {
            self.invalidate(key);
            return false;
        }
        // Decode every tile successfully before changing the visible picture.
        let decoded: Option<Vec<Vec<u8>>> = frame
            .tiles
            .par_iter()
            .map(|t| decode_jpeg(&t.jpeg, u32::from(t.w), u32::from(t.h)))
            .collect();
        let Some(decoded) = decoded else {
            self.invalidate(key);
            return false;
        };
        let mut map = self.inner.lock();
        // Checked under the same lock used by clear(), so disconnected jobs cannot repopulate it.
        if !active() {
            return true;
        }
        if !frame.keyframe {
            let in_sequence = map.get(&key).is_some_and(|p| {
                p.width == frame.width
                    && p.height == frame.height
                    && p.last_seq.is_some_and(|s| s.wrapping_add(1) == frame.seq)
            });
            if !in_sequence {
                if let Some(p) = map.get_mut(&key) {
                    p.last_seq = None;
                }
                return false;
            }
        }
        let size = picture_bytes(frame.width, frame.height).expect("validated picture dimensions");
        if !fits_store(&map, key, size) {
            return false;
        }
        let p = map
            .entry(key)
            .or_insert_with(|| Picture::new(frame.width, frame.height, vec![0; size]));
        if p.width != frame.width || p.height != frame.height {
            p.width = frame.width;
            p.height = frame.height;
            p.rgba.resize(size, 0);
        }
        let stride = frame.width as usize * 4;
        for (tile, rgba) in frame.tiles.iter().zip(decoded) {
            let src_stride = usize::from(tile.w) * 4;
            for row in 0..usize::from(tile.h) {
                let dest = (usize::from(tile.y) + row) * stride + usize::from(tile.x) * 4;
                let src = row * src_stride;
                p.rgba[dest..dest + src_stride].copy_from_slice(&rgba[src..src + src_stride]);
            }
        }
        p.last_seq = Some(frame.seq);
        p.version += 1;
        p.count(frame.byte_size());
        true
    }
}

fn fits_store(map: &HashMap<StreamKey, Picture>, key: StreamKey, bytes: usize) -> bool {
    map.iter()
        .filter(|(k, _)| **k != key)
        .map(|(_, p)| p.rgba.len())
        .sum::<usize>()
        .saturating_add(bytes)
        <= MAX_STORE_BYTES
}

#[derive(Clone)]
pub struct Decoder {
    store: PictureStore,
    ctx: egui::Context,
    tx: Sender<DecodeJob>,
    recovery: Arc<Mutex<HashMap<StreamKey, Instant>>>,
    generation: Arc<AtomicU64>,
}

struct DecodeJob {
    key: StreamKey,
    frame: DecodeFrame,
    sender: ClientSender,
    queued: Instant,
}

enum DecodeFrame {
    Tiles(VideoFrame),
    H264(EncodedVideoFrame),
    Forget,
}

fn request_recovery(
    recovery: &Mutex<HashMap<StreamKey, Instant>>,
    key: StreamKey,
    sender: &ClientSender,
) {
    let mut pending = recovery.lock();
    if pending
        .get(&key)
        .is_none_or(|t| t.elapsed() >= Duration::from_millis(700))
    {
        pending.insert(key, Instant::now());
        sender.request_keyframe(key.0, key.1);
        sender.send(&crate::protocol::ClientMsg::VideoFeedback {
            owner: key.0,
            kind: key.1,
            congested: true,
        });
    }
}

impl Decoder {
    pub fn start(store: PictureStore, ctx: egui::Context) -> Decoder {
        let cursor_store = store.clone();
        let cursor_ctx = ctx.clone();
        let (tx, rx) = bounded::<DecodeJob>(4);
        let recovery = Arc::new(Mutex::new(HashMap::new()));
        let requests = recovery.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let decoder_generation = generation.clone();
        std::thread::Builder::new()
            .name("video-decoder".into())
            .spawn(move || {
                let mut temporal =
                    HashMap::<StreamKey, (super::temporal::TemporalDecoder, usize)>::new();
                let mut current_generation = 0;
                let mut feedback_sent = HashMap::<StreamKey, Instant>::new();
                loop {
                    let latest = decoder_generation.load(Ordering::Relaxed);
                    if latest != current_generation {
                        temporal.clear();
                        feedback_sent.clear();
                        requests.lock().clear();
                        current_generation = latest;
                    }
                    let job = match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(job) => job,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                    if matches!(job.frame, DecodeFrame::Forget) {
                        temporal.remove(&job.key);
                        feedback_sent.remove(&job.key);
                        requests.lock().remove(&job.key);
                        continue;
                    }
                    if job.sender.is_closed() {
                        temporal.remove(&job.key);
                        continue;
                    }
                    if job.queued.elapsed() > Duration::from_millis(600) {
                        request_recovery(&requests, job.key, &job.sender);
                        continue;
                    }
                    let queue_ms = job.queued.elapsed().as_secs_f32() * 1000.0;
                    let decode_started = Instant::now();
                    let mut decoded = false;
                    match job.frame {
                        DecodeFrame::Tiles(frame) => {
                            temporal.remove(&job.key);
                            if !store.apply_if_active(job.key, &frame, || !job.sender.is_closed()) {
                                request_recovery(&requests, job.key, &job.sender);
                            } else {
                                decoded = true;
                                if frame.keyframe {
                                    requests.lock().remove(&job.key);
                                }
                            }
                        }
                        DecodeFrame::H264(frame) => {
                            let Some(size) = picture_bytes(frame.width, frame.height) else {
                                continue;
                            };
                            let budget = size.saturating_mul(2);
                            let other_budget = temporal
                                .iter()
                                .filter(|(key, _)| **key != job.key)
                                .map(|(_, entry)| entry.1)
                                .sum::<usize>();
                            if other_budget.saturating_add(budget) > MAX_STORE_BYTES {
                                request_recovery(&requests, job.key, &job.sender);
                                continue;
                            }
                            if !store.with(|pictures| fits_store(pictures, job.key, size)) {
                                request_recovery(&requests, job.key, &job.sender);
                                continue;
                            }
                            if !temporal.contains_key(&job.key) {
                                if temporal.len() >= 8
                                    || temporal
                                        .values()
                                        .map(|entry| entry.1)
                                        .sum::<usize>()
                                        .saturating_add(budget)
                                        > MAX_STORE_BYTES
                                {
                                    continue;
                                }
                                let Ok(decoder) = super::temporal::TemporalDecoder::new() else {
                                    continue;
                                };
                                temporal.insert(job.key, (decoder, budget));
                            }
                            if let Some((decoder, stored_budget)) = temporal.get_mut(&job.key) {
                                *stored_budget = budget;
                                match decoder.decode(&frame) {
                                    Ok(Some(pixels)) if !job.sender.is_closed() => {
                                        decoded = true;
                                        store.put(
                                            job.key,
                                            frame.width,
                                            frame.height,
                                            pixels,
                                            frame.data.len(),
                                        );
                                        if frame.keyframe {
                                            requests.lock().remove(&job.key);
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(_) => {
                                        request_recovery(&requests, job.key, &job.sender);
                                    }
                                }
                            }
                        }
                        DecodeFrame::Forget => {}
                    }
                    if decoded {
                        store.record_decode_timing(
                            job.key,
                            queue_ms,
                            decode_started.elapsed().as_secs_f32() * 1000.0,
                        );
                    }
                    if decoded
                        && feedback_sent
                            .get(&job.key)
                            .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
                    {
                        job.sender.send(&crate::protocol::ClientMsg::VideoFeedback {
                            owner: job.key.0,
                            kind: job.key.1,
                            congested: job.queued.elapsed() > Duration::from_millis(150),
                        });
                        feedback_sent.insert(job.key, Instant::now());
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn decoder");
        Decoder {
            store: cursor_store,
            ctx: cursor_ctx,
            tx,
            recovery,
            generation,
        }
    }

    /// A pointer update goes straight to the store: it is tiny and not part of the
    /// video stream, so it never waits behind picture decoding.
    pub fn submit_cursor(&self, owner: UserId, state: CursorState) {
        self.store.put_cursor(owner, state);
        self.ctx.request_repaint();
    }

    pub fn submit(&self, owner: UserId, kind: StreamKind, frame: VideoFrame, sender: ClientSender) {
        if let Err(error) = self.tx.try_send(DecodeJob {
            key: (owner, kind),
            frame: DecodeFrame::Tiles(frame),
            sender,
            queued: Instant::now(),
        }) {
            let job = error.into_inner();
            request_recovery(&self.recovery, job.key, &job.sender);
        }
    }

    pub fn submit_encoded(
        &self,
        owner: UserId,
        kind: StreamKind,
        frame: EncodedVideoFrame,
        sender: ClientSender,
    ) {
        if let Err(error) = self.tx.try_send(DecodeJob {
            key: (owner, kind),
            frame: DecodeFrame::H264(frame),
            sender,
            queued: Instant::now(),
        }) {
            let job = error.into_inner();
            request_recovery(&self.recovery, job.key, &job.sender);
        }
    }

    pub fn forget(&self, owner: UserId, kind: StreamKind, sender: ClientSender) {
        if self
            .tx
            .try_send(DecodeJob {
                key: (owner, kind),
                frame: DecodeFrame::Forget,
                sender,
                queued: Instant::now(),
            })
            .is_err()
        {
            self.reset();
        }
    }

    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::codec::TileEncoder;
    use crate::net::outbox::Outbox;
    use crate::protocol::{read_packet, ClientMsg};
    use std::net::{TcpListener, TcpStream};

    fn cursor_state(x: u16, shape: Option<CursorShape>) -> CursorState {
        CursorState {
            x,
            y: 0,
            visible: true,
            area_w: 1920,
            shape_id: 7,
            shape,
        }
    }
    fn arrow() -> CursorShape {
        CursorShape {
            id: 7,
            w: 2,
            h: 2,
            hot_x: 0,
            hot_y: 0,
            rgba: vec![255; 16],
        }
    }

    #[test]
    fn cursor_keeps_its_shape_between_updates_and_dies_with_the_stream() {
        let store = PictureStore::default();
        store.put_cursor(5, cursor_state(100, Some(arrow())));
        let first = store.cursor(5).unwrap();
        assert_eq!(first.shape_version, 1);
        // Position-only updates and the periodic repeat of the same shape change nothing.
        store.put_cursor(5, cursor_state(200, None));
        store.put_cursor(5, cursor_state(300, Some(arrow())));
        let now = store.cursor(5).unwrap();
        assert_eq!(now.shape_version, 1);
        assert!(now.shape.is_some() && now.x > first.x);
        // A different image bumps the version.
        let mut other = arrow();
        other.rgba = vec![0; 16];
        store.put_cursor(5, cursor_state(300, Some(other)));
        assert_eq!(store.cursor(5).unwrap().shape_version, 2);
        // Malformed shapes are refused.
        let mut bad = arrow();
        bad.rgba.pop();
        store.put_cursor(6, cursor_state(1, Some(bad)));
        assert!(store.cursor(6).is_none());
        store.remove((5, StreamKind::Screen));
        assert!(store.cursor(5).is_none());
    }

    #[test]
    fn missing_deltas_are_not_painted_until_a_complete_frame_arrives() {
        let store = PictureStore::default();
        let key = (1, StreamKind::Screen);
        let mut encoder = TileEncoder::new();
        let mut pixels = vec![160; 128 * 64 * 4];
        let full = encoder.encode(&pixels, 128, 64, 80, true).unwrap();
        assert!(store.apply(key, &full));
        let before = store.with(|m| m[&key].rgba.clone());
        pixels[0] = 0;
        let _lost = encoder.encode(&pixels, 128, 64, 80, false).unwrap();
        pixels[64 * 4] = 0;
        let next = encoder.encode(&pixels, 128, 64, 80, false).unwrap();
        assert!(!store.apply(key, &next));
        assert_eq!(store.with(|m| m[&key].rgba.clone()), before);
        let recovered = encoder.encode(&pixels, 128, 64, 80, true).unwrap();
        assert!(store.apply(key, &recovered));
        pixels[1] = 0;
        assert!(store.apply(key, &encoder.encode(&pixels, 128, 64, 80, false).unwrap()));
    }

    #[test]
    fn malformed_images_cannot_allocate_or_partially_modify_the_picture() {
        let store = PictureStore::default();
        let key = (1, StreamKind::Screen);
        let mut encoder = TileEncoder::new();
        let valid = encoder
            .encode(&vec![160; 128 * 64 * 4], 128, 64, 80, true)
            .unwrap();
        assert!(store.apply(key, &valid));
        let before = store.with(|m| (m[&key].rgba.clone(), m[&key].version));
        let mut broken = valid.clone();
        broken.width = u32::MAX;
        broken.height = u32::MAX;
        assert!(!store.apply(key, &broken));
        broken = valid.clone();
        broken.tiles[1].jpeg = vec![0; 16];
        assert!(!store.apply(key, &broken));
        broken = valid.clone();
        broken.tiles[0].jpeg = crate::media::codec::encode_jpeg(&vec![0; 32 * 32 * 4], 32, 32, 80);
        assert!(!store.apply(key, &broken));
        assert_eq!(
            store.with(|m| (m[&key].rgba.clone(), m[&key].version)),
            before
        );
        assert!(store.apply(key, &valid));
    }

    #[test]
    fn a_full_decoder_queue_requests_recovery_immediately() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let outbox = Outbox::start(stream);
        let sender = ClientSender::new(outbox.clone());
        let (tx, _rx) = bounded(1);
        let decoder = Decoder {
            store: PictureStore::default(),
            ctx: egui::Context::default(),
            tx,
            recovery: Arc::new(Mutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
        };
        let frame = TileEncoder::new()
            .encode(&vec![160; 64 * 64 * 4], 64, 64, 80, true)
            .unwrap();
        decoder.submit(7, StreamKind::Screen, frame.clone(), sender.clone());
        decoder.submit(7, StreamKind::Screen, frame, sender);
        assert!(matches!(
            read_packet::<_, ClientMsg>(&mut peer).unwrap(),
            ClientMsg::RequestKeyframe {
                owner: 7,
                kind: StreamKind::Screen
            }
        ));
        assert!(matches!(
            read_packet::<_, ClientMsg>(&mut peer).unwrap(),
            ClientMsg::VideoFeedback {
                owner: 7,
                kind: StreamKind::Screen,
                congested: true
            }
        ));
        outbox.close();
    }
}
