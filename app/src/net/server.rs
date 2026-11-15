//! The server one person hosts on their computer. It keeps the channels, people
//! and messages, and relays voice and video between the people in a call.

use std::collections::{HashMap, HashSet};

use std::io::BufReader;

use std::net::{TcpListener, TcpStream, UdpSocket};

use std::path::PathBuf;

use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::Arc;

use std::time::{Duration, Instant};

use parking_lot::Mutex;

use serde::{Deserialize, Serialize};

use super::outbox::Outbox;

mod media_router;

mod uploads;

mod connection;
mod dispatch;
mod listener;
mod state;
mod workspace;

use crate::model::*;

use crate::protocol::*;

use connection::handle_client;
use dispatch::handle_msg;
use listener::*;
use workspace::WorkspaceData;

/// Messages kept per server.
const CACHE_PER_CHANNEL: usize = 200;

/// Messages sent to a newcomer per channel.
const SNAPSHOT_PER_CHANNEL: usize = 200;

#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub bind: String,
    pub port: u16,
    pub registration_token: Option<String>,
    pub owner_key: Option<[u8; 32]>,
    pub allowlist: Vec<[u8; 32]>,
    pub revoked_keys: Vec<[u8; 32]>,
    pub max_connections: usize,
    pub allow_registration: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".into(),
            port: DEFAULT_PORT,
            registration_token: None,
            owner_key: None,
            allowlist: Vec::new(),
            revoked_keys: Vec::new(),
            max_connections: 64,
            allow_registration: false,
        }
    }
}

pub fn start_configured(
    name: String,
    save_path: Option<PathBuf>,
    options: ServerOptions,
) -> std::io::Result<ServerHandle> {
    let listener = TcpListener::bind((options.bind.as_str(), options.port))?;
    start_listener_configured(name, listener, save_path, None, options)
}

pub struct ServerHandle {
    pub port: u16,
    pub name: String,
    stop: Arc<AtomicBool>,
    accept_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub registration_token: Option<String>,
    pub fingerprint: String,
    media: Mutex<Option<Arc<super::datagram::ServerMedia>>>,
    connections: Arc<Mutex<HashMap<u64, TcpStream>>>,
}

const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Joins `thread` if it finishes within `timeout`; otherwise detaches it.
fn join_bounded(thread: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = thread.join();
    true
}

impl ServerHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // Includes sockets still performing Noise or identity authentication.
        for socket in self.connections.lock().values() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(thread) = self.accept_thread.lock().take() {
            if !join_bounded(thread, STOP_JOIN_TIMEOUT) {
                eprintln!(
                    "[host {}] accept thread did not stop within {:?}",
                    self.port, STOP_JOIN_TIMEOUT
                );
            }
        }
        let media = self.media.lock().take();
        drop(media);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What is saved to disk between runs.
#[derive(Clone, Serialize, Deserialize, Default)]
pub(crate) struct Saved {
    #[serde(default)]
    guilds: Vec<Guild>,
    #[serde(default)]
    next_guild: GuildId,
    /// (person, other person, how they relate) for every friend entry.
    #[serde(default)]
    friends: Vec<(UserId, UserId, FriendStatus)>,
    channels: Vec<Channel>,
    users: Vec<User>,
    messages: Vec<Message>,
    next_channel: ChannelId,
    next_user: UserId,
    next_message: MessageId,
    #[serde(default)]
    public_keys: HashMap<UserId, [u8; 32]>,
    #[serde(default)]
    workspace: WorkspaceData,
}

type StreamKey = (UserId, StreamKind);

struct State {
    name: String,
    channels: Vec<Channel>,
    users: HashMap<UserId, User>,
    messages: Vec<Message>,
    voice: HashMap<UserId, VoiceState>,
    stage_speakers: HashMap<ChannelId, HashSet<UserId>>,
    stage_requests: HashMap<ChannelId, HashSet<UserId>>,
    streams: HashMap<StreamKey, StreamInfo>,
    guilds: Vec<Guild>,
    next_guild: GuildId,
    friends: HashMap<UserId, HashMap<UserId, FriendStatus>>,
    calls: HashMap<ChannelId, CallInfo>,
    next_call: ChannelId,
    watchers: HashMap<StreamKey, HashSet<UserId>>,
    clients: HashMap<UserId, Outbox>,
    next_channel: ChannelId,
    next_user: UserId,
    next_message: MessageId,
    last_keyframe_ask: HashMap<StreamKey, Instant>,
    save_path: Option<PathBuf>,
    public_keys: HashMap<UserId, [u8; 32]>,
    workspace: WorkspaceData,
    store: Option<Arc<Mutex<crate::storage::Store>>>,
    options: ServerOptions,
    uploads: HashMap<u64, (AttachmentInfo, Vec<u8>)>,
    /// Chunked uploads still being received (written outside the state lock).
    pending_uploads: Arc<uploads::UploadRouter>,
    operations: HashMap<(UserId, String), MessageId>,
    rates: HashMap<UserId, (Instant, u32)>,
    call_deadlines: HashMap<ChannelId, Instant>,
    media_router: Arc<media_router::MediaRouter>,
}

fn default_channels() -> Vec<Channel> {
    vec![
        Channel {
            id: 1,
            guild: 1,
            name: "general".into(),
            kind: ChannelKind::Text,
            topic: "Say hello!".into(),
        },
        Channel {
            id: 2,
            guild: 1,
            name: "clips-and-links".into(),
            kind: ChannelKind::Text,
            topic: String::new(),
        },
        Channel {
            id: 3,
            guild: 1,
            name: "Lounge".into(),
            kind: ChannelKind::Voice,
            topic: String::new(),
        },
        Channel {
            id: 4,
            guild: 1,
            name: "Gaming".into(),
            kind: ChannelKind::Voice,
            topic: String::new(),
        },
    ]
}

/// Start hosting a server called `name` on `port` (0 picks the default port).
#[cfg(test)]
pub fn start(name: String, port: u16, save_path: Option<PathBuf>) -> std::io::Result<ServerHandle> {
    let port = if port == 0 { DEFAULT_PORT } else { port };
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    start_listener(name, listener, save_path)
}

pub fn start_desktop_configured(
    name: String,
    save_path: Option<PathBuf>,
    owner_name: &str,
    owner_key: [u8; 32],
    mut options: ServerOptions,
) -> std::io::Result<ServerHandle> {
    options.owner_key = Some(owner_key);
    options.allow_registration = false;
    if options.registration_token.is_none() {
        options.registration_token = Some(desktop_token(save_path.as_deref())?);
    }
    let listener = TcpListener::bind((options.bind.as_str(), options.port))?;
    start_listener_configured(
        name,
        listener,
        save_path,
        Some((owner_name, owner_key)),
        options,
    )
}

#[cfg(test)]
mod tests;
