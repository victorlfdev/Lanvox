//! Chunked uploads in progress. Pieces are written here, on the connection's reader thread and
//! through a dedicated database connection, so a large upload never holds the host state lock
//! (which every ping, message and call goes through) while it hits the disk.
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::model::*;
use crate::storage::{Store, UploadSink};

/// A chunked upload that has been announced but not finished. With a database the pieces live in
/// `attachment_chunks` under `owner` (the id the attachment will get); without one (tests,
/// ephemeral hosts) they accumulate in `buffer`.
pub(super) struct PendingUpload {
    pub owner: u64,
    pub channel: ChannelId,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub received: u64,
    pub buffer: Vec<u8>,
    pub started: Instant,
    pub digest: Option<[u8; 32]>,
}

#[derive(Default)]
pub(super) struct UploadRouter {
    pending: Mutex<HashMap<(UserId, String), PendingUpload>>,
    store: Mutex<Option<Arc<Mutex<Store>>>>,
    sink: Mutex<Option<Arc<Mutex<UploadSink>>>>,
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}

impl UploadRouter {
    pub fn attach(&self, store: Arc<Mutex<Store>>, sink: Option<UploadSink>) {
        *self.store.lock() = Some(store);
        *self.sink.lock() = sink.map(|s| Arc::new(Mutex::new(s)));
    }

    fn has_db(&self) -> bool {
        self.store.lock().is_some()
    }

    /// Bytes announced by uploads in progress that the database does not hold yet.
    pub fn unwritten_bytes(&self) -> u64 {
        let db = self.has_db();
        self.pending
            .lock()
            .values()
            .map(|p| {
                if db && p.digest.is_some() {
                    0
                } else if db {
                    p.size - p.received
                } else {
                    p.size
                }
            })
            .sum()
    }

    pub fn count_for(&self, user: UserId) -> usize {
        self.pending
            .lock()
            .keys()
            .filter(|(u, _)| *u == user)
            .count()
    }

    fn write(&self, owner: u64, seq: u64, data: &[u8]) -> io::Result<()> {
        let sink = self.sink.lock().clone();
        if let Some(sink) = sink {
            return sink.lock().put_chunk(owner, seq, data);
        }
        let store = self.store.lock().clone();
        match store {
            Some(store) => store.lock().put_chunk(owner, seq, data),
            None => Err(io::Error::other("sem armazenamento")),
        }
    }

    fn delete(&self, owner: u64) {
        if !self.has_db() {
            return;
        }
        let sink = self.sink.lock().clone();
        if let Some(sink) = sink {
            sink.lock().delete_chunks(owner);
        } else if let Some(store) = self.store.lock().clone() {
            store.lock().delete_chunks(owner);
        }
    }

    /// Folds the write-ahead log back after an upload (on its own thread: it may wait for readers).
    pub fn checkpoint_later(&self) {
        if let Some(sink) = self.sink.lock().clone() {
            std::thread::spawn(move || sink.lock().checkpoint());
        }
    }

    /// Registers an upload; a previous one with the same key is discarded.
    pub fn begin(&self, user: UserId, operation: String, upload: PendingUpload) {
        let old = self.pending.lock().insert((user, operation), upload);
        if let Some(old) = old {
            self.delete(old.owner);
        }
    }

    /// Reusing an active operation is only allowed for the same immutable file metadata.
    #[allow(clippy::too_many_arguments)]
    pub fn resume_active(
        &self,
        user: UserId,
        operation: &str,
        channel: ChannelId,
        name: &str,
        media_type: &str,
        size: u64,
        digest: &[u8; 32],
    ) -> io::Result<Option<u64>> {
        let mut pending = self.pending.lock();
        let Some(p) = pending.get_mut(&(user, operation.to_string())) else {
            return Ok(None);
        };
        if p.channel != channel
            || p.name != name
            || p.media_type != media_type
            || p.size != size
            || p.digest.as_ref() != Some(digest)
        {
            return Err(invalid("O envio parcial pertence a outro arquivo"));
        }
        p.started = Instant::now();
        Ok(Some(p.received))
    }

    pub fn expire(&self, older_than: Duration) {
        let stale: Vec<PendingUpload> = {
            let mut pending = self.pending.lock();
            let keys: Vec<_> = pending
                .iter()
                .filter(|(_, p)| {
                    p.started.elapsed()
                        > if p.digest.is_some() {
                            Duration::from_secs(7 * 24 * 60 * 60)
                        } else {
                            older_than
                        }
                })
                .map(|(k, _)| k.clone())
                .collect();
            keys.iter().filter_map(|k| pending.remove(k)).collect()
        };
        for p in stale {
            self.discard(&p);
        }
    }

    /// Stores the next piece of an upload and returns how many bytes are now held. Pieces must
    /// arrive in order, `ATTACHMENT_CHUNK_BYTES` each (only the last may be shorter).
    pub fn chunk(
        &self,
        user: UserId,
        operation: &str,
        offset: u64,
        data: &[u8],
    ) -> io::Result<u64> {
        let key = (user, operation.to_string());
        let chunk = ATTACHMENT_CHUNK_BYTES as u64;
        let (owner, size) = {
            let mut pending = self.pending.lock();
            let Some(p) = pending.get_mut(&key) else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Envio não iniciado",
                ));
            };
            let len = data.len() as u64;
            if offset != p.received {
                return Err(invalid("Pedaço fora de ordem"));
            }
            if len == 0 || len > chunk || !offset.is_multiple_of(chunk) || p.received + len > p.size
            {
                return Err(invalid("Pedaço inválido"));
            }
            if len != chunk && p.received + len != p.size {
                return Err(invalid("Pedaço incompleto no meio do arquivo"));
            }
            if self.sink.lock().is_none() && self.store.lock().is_none() {
                p.buffer.extend_from_slice(data);
                p.received += len;
                return Ok(p.received);
            }
            (p.owner, p.size)
        };
        self.write(owner, offset / chunk, data)?;
        let mut pending = self.pending.lock();
        match pending.get_mut(&key) {
            Some(p) if p.owner == owner && p.size == size => {
                p.received += data.len() as u64;
                Ok(p.received)
            }
            _ => {
                drop(pending);
                self.delete(owner);
                Err(io::Error::new(io::ErrorKind::NotFound, "Envio cancelado"))
            }
        }
    }

    pub fn take(&self, user: UserId, operation: &str) -> Option<PendingUpload> {
        self.pending.lock().remove(&(user, operation.to_string()))
    }

    /// Drops what an upload stored.
    pub fn discard(&self, upload: &PendingUpload) {
        if upload.digest.is_some() {
            if let Some(store) = self.store.lock().clone() {
                let _ = store.lock().remove_resumable(upload.owner);
                return;
            }
        }
        self.delete(upload.owner);
    }

    pub fn drop_user(&self, user: UserId) {
        let gone: Vec<PendingUpload> = {
            let mut pending = self.pending.lock();
            let keys: Vec<_> = pending
                .iter()
                .filter(|((u, _), upload)| *u == user && upload.digest.is_none())
                .map(|(key, _)| key.clone())
                .collect();
            keys.iter().filter_map(|k| pending.remove(k)).collect()
        };
        for p in gone {
            self.delete(p.owner);
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.pending.lock().is_empty()
    }
}
