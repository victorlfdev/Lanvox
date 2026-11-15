//! A sending queue for one TCP connection with two lanes: urgent packets (voice,
//! chat, control) always go before bulk packets (video), and bulk packets that
//! pile up are dropped oldest-first. Negotiated encrypted UDP media bypasses
//! this TCP queue; TCP priority alone cannot preempt an in-flight video record.

use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};

/// How many video packets may wait before the oldest is thrown away.
const MAX_BULK: usize = 4;
const MAX_URGENT: usize = 512;
const MAX_URGENT_BYTES: usize = 16 * 1024 * 1024;
/// Queued file pieces allowed before the connection is considered stuck.
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct Lanes {
    urgent: VecDeque<Vec<u8>>,
    bulk: VecDeque<Vec<u8>>,
    /// Newest pointer update; a newer one replaces it, so it never queues up.
    cursor: Option<Vec<u8>>,
    /// File pieces: lowest priority, never dropped, so they cannot delay pings or chat.
    file: VecDeque<Vec<u8>>,
    closed: bool,
    urgent_bytes: usize,
    file_bytes: usize,
}

#[derive(Clone)]
pub struct Outbox {
    inner: Arc<(Mutex<Lanes>, Condvar)>,
    socket: Arc<TcpStream>,
    media: Arc<Mutex<Option<super::datagram::MediaSender>>>,
    camera_keyframe: Arc<AtomicBool>,
}

impl Outbox {
    /// Start a writer thread for `stream` and return its queue.
    #[cfg(test)]
    pub fn start(stream: TcpStream) -> Outbox {
        let socket = stream.try_clone().expect("clone writer socket");
        Self::start_writer(stream, socket)
    }

    pub fn start_secure(stream: crate::auth::transport::SecureStream) -> std::io::Result<Outbox> {
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        let socket = stream.raw_clone()?;
        Ok(Self::start_writer(stream, socket))
    }

    fn start_writer(mut stream: impl Write + Send + 'static, socket: TcpStream) -> Outbox {
        let _ = socket.set_write_timeout(Some(std::time::Duration::from_secs(5)));
        let outbox = Outbox {
            inner: Arc::new((Mutex::new(Lanes::default()), Condvar::new())),
            socket: Arc::new(socket),
            media: Arc::new(Mutex::new(None)),
            camera_keyframe: Arc::new(AtomicBool::new(true)),
        };
        let me = outbox.clone();
        std::thread::Builder::new()
            .name("net-writer".into())
            .spawn(move || loop {
                let packet = {
                    let (lock, cv) = &*me.inner;
                    let mut lanes = lock.lock();
                    loop {
                        if lanes.closed {
                            return;
                        }
                        if let Some(p) = lanes.urgent.pop_front() {
                            lanes.urgent_bytes -= p.len();
                            break p;
                        }
                        if let Some(p) = lanes.cursor.take() {
                            break p;
                        }
                        if let Some(p) = lanes.bulk.pop_front() {
                            break p;
                        }
                        if let Some(p) = lanes.file.pop_front() {
                            lanes.file_bytes -= p.len();
                            break p;
                        }
                        cv.wait(&mut lanes);
                    }
                };
                if stream.write_all(&packet).is_err() {
                    me.close();
                    let _ = me.socket.shutdown(std::net::Shutdown::Both);
                    return;
                }
            })
            .expect("spawn writer");
        outbox
    }

    pub fn push_urgent(&self, packet: Vec<u8>) {
        let (lock, cv) = &*self.inner;
        let mut lanes = lock.lock();
        if lanes.closed {
            return;
        }
        if lanes.urgent.len() >= MAX_URGENT
            || lanes.urgent_bytes.saturating_add(packet.len()) > MAX_URGENT_BYTES
        {
            drop(lanes);
            self.close();
            return;
        }
        lanes.urgent_bytes += packet.len();
        lanes.urgent.push_back(packet);
        cv.notify_one();
    }

    /// Queue a piece of a file transfer. It goes out only when nothing urgent is waiting.
    pub fn push_file(&self, packet: Vec<u8>) {
        let (lock, cv) = &*self.inner;
        let mut lanes = lock.lock();
        if lanes.closed {
            return;
        }
        if lanes.file_bytes.saturating_add(packet.len()) > MAX_FILE_BYTES {
            drop(lanes);
            self.close();
            return;
        }
        lanes.file_bytes += packet.len();
        lanes.file.push_back(packet);
        cv.notify_one();
    }

    /// Queue a video packet. Returns true when an older one had to be dropped.
    pub fn push_bulk(&self, packet: Vec<u8>) -> bool {
        // Video only takes the datagram path once the peer has proven it can receive it;
        // otherwise (UDP blocked, or not yet established) it stays on the TCP lane.
        let media = self.media.lock().clone();
        if let Some(media) = media.filter(|media| media.stats().active) {
            return media.send(packet, false);
        }
        let (lock, cv) = &*self.inner;
        let mut lanes = lock.lock();
        if lanes.closed {
            return false;
        }
        let mut dropped = false;
        while lanes.bulk.len() >= MAX_BULK {
            lanes.bulk.pop_front();
            dropped = true;
        }
        lanes.bulk.push_back(packet);
        cv.notify_one();
        dropped
    }

    /// Queue a pointer update. Over UDP it is sent like voice (small, never held back by
    /// video); over TCP only the newest one waits, because an old position is useless.
    pub fn push_cursor(&self, packet: Vec<u8>) {
        let media = self.media.lock().clone();
        if let Some(media) = media.filter(|media| media.stats().active) {
            media.send(packet, true);
            return;
        }
        let (lock, cv) = &*self.inner;
        let mut lanes = lock.lock();
        if lanes.closed {
            return;
        }
        lanes.cursor = Some(packet);
        cv.notify_one();
    }

    pub fn close(&self) {
        let media = self.media.lock().take();
        if let Some(media) = media {
            media.close();
        }
        let (lock, cv) = &*self.inner;
        {
            let mut lanes = lock.lock();
            lanes.closed = true;
            lanes.urgent.clear();
            lanes.bulk.clear();
            lanes.file.clear();
            lanes.cursor = None;
            lanes.urgent_bytes = 0;
            lanes.file_bytes = 0;
        }
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        cv.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.inner.0.lock().closed
    }

    pub fn attach_media(&self, sender: super::datagram::MediaSender) {
        let previous = self.media.lock().replace(sender);
        if let Some(previous) = previous {
            previous.close();
        }
    }

    pub fn push_realtime(&self, packet: Vec<u8>) {
        let media = self.media.lock().clone();
        if let Some(media) = media.filter(|media| media.stats().active) {
            media.send(packet, true);
        } else {
            self.push_urgent(packet);
        }
    }

    pub fn media_stats(&self) -> super::datagram::MediaStats {
        let mut stats = self
            .media
            .lock()
            .as_ref()
            .map(|sender| sender.stats())
            .unwrap_or_default();
        if !stats.active {
            let lanes = self.inner.0.lock();
            stats.queued_bytes =
                lanes.urgent_bytes + lanes.bulk.iter().map(Vec::len).sum::<usize>();
        }
        stats
    }

    pub fn probe_media(&self, packet: Vec<u8>) {
        let media = self.media.lock().clone();
        if let Some(media) = media {
            media.send(packet, true);
        }
    }

    pub fn request_camera_keyframe(&self) {
        self.camera_keyframe.store(true, Ordering::Relaxed);
    }
    pub fn take_camera_keyframe(&self) -> bool {
        self.camera_keyframe.swap(false, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn closing_the_outbox_closes_both_socket_directions() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut reader = stream.try_clone().unwrap();
        reader
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let outbox = Outbox::start(stream);
        outbox.close();
        let mut byte = [0];
        assert_eq!(peer.read(&mut byte).unwrap(), 0);
        assert!(!matches!(reader.read(&mut byte), Ok(n) if n > 0));
        assert!(outbox.is_closed());
    }

    #[test]
    fn urgent_overflow_disconnects_instead_of_growing_without_limit() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let outbox = Outbox::start(stream);
        outbox.push_urgent(vec![0; MAX_URGENT_BYTES + 1]);
        assert!(outbox.is_closed());
        assert!(outbox.inner.0.lock().urgent.is_empty());
    }
}
