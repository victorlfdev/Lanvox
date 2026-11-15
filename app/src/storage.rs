//! Transactional host storage. Legacy JSON is imported once and kept as a backup.

use std::io;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::model::{
    AttachmentInfo, ChannelId, Message, MessageDetails, MessageId, SearchOrder, SearchRequest,
    UserId, ATTACHMENT_CHUNK_BYTES,
};

mod attachments;
mod backup;
mod messages;
mod migrations;
mod search;
pub use backup::*;
use migrations::*;

/// Schema version written by this build. v2 adds the FTS5 index and search metadata; v3 turns
/// free-text channel categories into `Category` entities.
const SCHEMA_VERSION: u32 = 4;

/// Default and maximum page size of an advanced search.
pub const SEARCH_DEFAULT_LIMIT: usize = 50;

pub const SEARCH_MAX_LIMIT: usize = 100;

fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

pub struct Store {
    connection: Connection,
}

/// Persistent metadata for a partially received attachment, scoped to its authenticated user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartialUpload {
    pub user: UserId,
    pub operation: String,
    pub info: AttachmentInfo,
    pub digest: [u8; 32],
    pub created_at: i64,
}

fn put_chunk_on(connection: &Connection, owner: u64, seq: u64, data: &[u8]) -> io::Result<()> {
    connection
        .execute(
            "INSERT OR REPLACE INTO attachment_chunks(owner,seq,data) VALUES(?1,?2,?3)",
            params![owner as i64, seq as i64, data],
        )
        .map(|_| ())
        .map_err(error)
}

fn delete_chunks_on(connection: &Connection, owner: u64) {
    let _ = connection.execute(
        "DELETE FROM attachment_chunks WHERE owner=?1",
        [owner as i64],
    );
}

/// Writes upload pieces on its own connection (see [`Store::upload_sink`]).
pub struct UploadSink {
    connection: Connection,
}

impl UploadSink {
    pub fn put_chunk(&self, owner: u64, seq: u64, data: &[u8]) -> io::Result<()> {
        put_chunk_on(&self.connection, owner, seq, data)
    }

    pub fn delete_chunks(&self, owner: u64) {
        delete_chunks_on(&self.connection, owner);
    }

    /// Folds the write-ahead log back into the database so a large upload does not leave a huge
    /// WAL behind. PASSIVE never blocks: TRUNCATE held the writer lock while waiting for readers,
    /// which made the host's own writes and attachment reads fail with "database is locked".
    /// `journal_size_limit` still shrinks the file once the log is fully checkpointed.
    pub fn checkpoint(&self) {
        let _ = self
            .connection
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
    }
}

impl Store {
    pub fn open(path: &Path) -> io::Result<Self> {
        let connection = Connection::open(path).map_err(error)?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(error)?;
        if version > SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Banco criado por uma versão mais recente; atualize o Lanvox",
            ));
        }
        connection
            .busy_timeout(std::time::Duration::from_secs(30))
            .map_err(error)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS host_state (id INTEGER PRIMARY KEY CHECK(id=1), body BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, channel INTEGER NOT NULL, author INTEGER NOT NULL, text TEXT NOT NULL, time INTEGER NOT NULL, body BLOB NOT NULL);
            CREATE INDEX IF NOT EXISTS messages_channel_id ON messages(channel,id DESC);
            CREATE TABLE IF NOT EXISTS message_details (id INTEGER PRIMARY KEY, body BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS operations (author INTEGER NOT NULL, operation TEXT NOT NULL, message INTEGER NOT NULL, PRIMARY KEY(author,operation));
            CREATE TABLE IF NOT EXISTS attachments (id INTEGER PRIMARY KEY, channel INTEGER NOT NULL, uploader INTEGER NOT NULL, info BLOB NOT NULL, content BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS upload_operations (author INTEGER NOT NULL, operation TEXT NOT NULL, attachment INTEGER NOT NULL, PRIMARY KEY(author,operation));
            CREATE TABLE IF NOT EXISTS attachment_chunks (owner INTEGER NOT NULL, seq INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY(owner,seq));
            CREATE TABLE IF NOT EXISTS resumable_uploads (owner INTEGER PRIMARY KEY, author INTEGER NOT NULL, operation TEXT NOT NULL, metadata BLOB NOT NULL, created_at INTEGER NOT NULL, UNIQUE(author, operation));
            PRAGMA journal_size_limit=67108864;
            DELETE FROM attachment_chunks WHERE owner NOT IN (SELECT id FROM attachments) AND owner NOT IN (SELECT owner FROM resumable_uploads);").map_err(error)?;
        let mut connection = connection;
        migrate(&mut connection, version)?;
        let store = Self { connection };
        // Incomplete transfers expire after a week; completed attachments are never touched.
        store.expire_resumable(crate::model::now_secs().saturating_sub(7 * 24 * 60 * 60))?;
        Ok(store)
    }

    pub fn load_state<T: DeserializeOwned>(&self) -> io::Result<Option<T>> {
        let body: Option<Vec<u8>> = self
            .connection
            .query_row("SELECT body FROM host_state WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(error)?;
        body.map(|b| serde_json::from_slice(&b).map_err(error))
            .transpose()
    }

    pub fn save_state<T: Serialize>(&self, state: &T) -> io::Result<()> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }

    pub fn backup(&self, destination: &Path) -> io::Result<()> {
        self.connection
            .backup("main", destination, None::<fn(rusqlite::backup::Progress)>)
            .map_err(error)
    }
}

/// Turns free text into an FTS5 expression: every word becomes a quoted prefix term (AND).
/// Returns None when the text has no searchable words. User input never reaches FTS syntax.
pub fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[cfg(test)]
mod tests;
