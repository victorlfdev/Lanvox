//! Authenticated, encrypted transport. Noise XX never falls back to plaintext.
//! Host public keys are pinned on first use; invitation fingerprints can pin the
//! first connection out of band. Identity signatures run inside this channel.

use std::io::{self, Read, Write};
#[cfg(test)]
use std::net::Shutdown;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
// Kept from the previous name on purpose; see branding.rs.
const MAGIC: &[u8; 8] = b"HUDDLE4\0";
const MAX_RECORD: usize = 60_000;

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[derive(Serialize, Deserialize)]
pub struct HostKey {
    private: Vec<u8>,
    public: Vec<u8>,
}

impl HostKey {
    pub fn generate() -> io::Result<Self> {
        let pair = snow::Builder::new(PATTERN.parse().map_err(invalid)?)
            .generate_keypair()
            .map_err(invalid)?;
        Ok(Self {
            private: pair.private,
            public: pair.public,
        })
    }

    /// Reads an existing host key without ever creating or migrating one.
    pub fn load_existing(path: &Path) -> io::Result<Self> {
        let bytes = crate::secret_store::read(path)?;
        let key: Self = serde_json::from_slice(&bytes).map_err(invalid)?;
        if key.private.len() != 32 || key.public.len() != 32 {
            return Err(invalid(
                "Invalid host key; restore its backup instead of replacing it",
            ));
        }
        Ok(key)
    }

    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match crate::secret_store::read(path) {
            Ok(bytes) => {
                let key: Self = serde_json::from_slice(&bytes).map_err(invalid)?;
                if key.private.len() != 32 || key.public.len() != 32 {
                    return Err(invalid(
                        "Invalid host key; restore its backup instead of replacing it",
                    ));
                }
                crate::secret_store::migrate_validated(path, &bytes)?;
                Ok(key)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let key = Self::generate()?;
                match crate::secret_store::write_new(
                    path,
                    &serde_json::to_vec(&key).map_err(invalid)?,
                ) {
                    Ok(()) => Ok(key),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        Self::load_or_create(path)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public)
    }
}

pub fn fingerprint(key: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, key);
    let hex: String = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("SHA256:{hex}")
}

fn read_handshake(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut prefix = [0u8; 2];
    stream.read_exact(&mut prefix)?;
    let len = usize::from(u16::from_le_bytes(prefix));
    if !(32..=4096).contains(&len) {
        return Err(invalid("Invalid encrypted handshake"));
    }
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn write_handshake(stream: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(&(bytes.len() as u16).to_le_bytes())?;
    stream.write_all(bytes)
}

/// Clones share only cipher state and record ordering; a blocking socket read
/// never holds the cipher lock needed by the writer.
pub struct SecureStream {
    socket: TcpStream,
    state: Arc<Mutex<snow::TransportState>>,
    write_order: Arc<Mutex<()>>,
    buffered: Vec<u8>,
    offset: usize,
}

impl SecureStream {
    fn new(socket: TcpStream, state: snow::TransportState) -> Self {
        Self {
            socket,
            state: Arc::new(Mutex::new(state)),
            write_order: Arc::new(Mutex::new(())),
            buffered: Vec::new(),
            offset: 0,
        }
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            socket: self.socket.try_clone()?,
            state: self.state.clone(),
            write_order: self.write_order.clone(),
            buffered: Vec::new(),
            offset: 0,
        })
    }
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.socket.set_read_timeout(timeout)
    }
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.socket.set_write_timeout(timeout)
    }
    #[cfg(test)]
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.socket.shutdown(how)
    }
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.socket.peer_addr()
    }
    pub fn raw_clone(&self) -> io::Result<TcpStream> {
        self.socket.try_clone()
    }
}

impl Read for SecureStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.offset == self.buffered.len() {
            let mut prefix = [0; 2];
            self.socket.read_exact(&mut prefix)?;
            let len = usize::from(u16::from_le_bytes(prefix));
            if !(16..=MAX_RECORD + 16).contains(&len) {
                return Err(invalid("Invalid encrypted record"));
            }
            let mut cipher = vec![0; len];
            self.socket.read_exact(&mut cipher)?;
            self.buffered.resize(len, 0);
            let n = self
                .state
                .lock()
                .read_message(&cipher, &mut self.buffered)
                .map_err(invalid)?;
            self.buffered.truncate(n);
            self.offset = 0;
            if n == 0 {
                return Err(invalid("Empty encrypted record"));
            }
        }
        let n = out.len().min(self.buffered.len() - self.offset);
        out[..n].copy_from_slice(&self.buffered[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}

impl Write for SecureStream {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        let _order = self.write_order.lock();
        let n = input.len().min(MAX_RECORD);
        let mut record = vec![0; n + 16];
        let len = self
            .state
            .lock()
            .write_message(&input[..n], &mut record)
            .map_err(invalid)?;
        self.socket.write_all(&(len as u16).to_le_bytes())?;
        self.socket.write_all(&record[..len])?;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
const STOP_POLL: Duration = Duration::from_millis(100);

/// `read_exact` that wakes every `STOP_POLL` to observe `stop` and a deadline.
struct StopReader<'a> {
    socket: &'a mut TcpStream,
    stop: &'a AtomicBool,
    deadline: Instant,
}

impl Read for StopReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "shutdown"));
            }
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "handshake timeout"));
            }
            match self.socket.read(out) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                other => return other,
            }
        }
    }
}

#[cfg(test)]
pub fn server_handshake(socket: TcpStream, key: &HostKey) -> io::Result<SecureStream> {
    server_handshake_with_stop(socket, key, Arc::new(AtomicBool::new(false)))
}

pub fn server_handshake_with_stop(
    mut socket: TcpStream,
    key: &HostKey,
    stop: Arc<AtomicBool>,
) -> io::Result<SecureStream> {
    let previous = socket.read_timeout().ok().flatten();
    socket.set_read_timeout(Some(STOP_POLL))?;
    let result = server_handshake_inner(&mut socket, key, &stop);
    let _ = socket.set_read_timeout(previous);
    let state = result?;
    Ok(SecureStream::new(socket, state))
}

fn server_handshake_inner(
    socket: &mut TcpStream,
    key: &HostKey,
    stop: &AtomicBool,
) -> io::Result<snow::TransportState> {
    let deadline = Instant::now() + HANDSHAKE_DEADLINE;
    let mut magic = [0; 8];
    StopReader {
        socket,
        stop,
        deadline,
    }
    .read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid("This host requires encrypted Lanvox protocol 3"));
    }
    let mut handshake = snow::Builder::new(PATTERN.parse().map_err(invalid)?)
        .prologue(MAGIC)
        .map_err(invalid)?
        .local_private_key(&key.private)
        .map_err(invalid)?
        .build_responder()
        .map_err(invalid)?;
    let mut scratch = [0; 4096];
    let msg = read_handshake(&mut StopReader {
        socket,
        stop,
        deadline,
    })?;
    handshake
        .read_message(&msg, &mut scratch)
        .map_err(invalid)?;
    let n = handshake
        .write_message(&[], &mut scratch)
        .map_err(invalid)?;
    write_handshake(socket, &scratch[..n])?;
    let msg = read_handshake(&mut StopReader {
        socket,
        stop,
        deadline,
    })?;
    handshake
        .read_message(&msg, &mut scratch)
        .map_err(invalid)?;
    handshake.into_transport_mode().map_err(invalid)
}

pub fn client_handshake(mut socket: TcpStream) -> io::Result<(SecureStream, Vec<u8>)> {
    let pair = HostKey::generate()?;
    let mut handshake = snow::Builder::new(PATTERN.parse().map_err(invalid)?)
        .prologue(MAGIC)
        .map_err(invalid)?
        .local_private_key(&pair.private)
        .map_err(invalid)?
        .build_initiator()
        .map_err(invalid)?;
    socket.write_all(MAGIC)?;
    let mut scratch = [0; 4096];
    let n = handshake
        .write_message(&[], &mut scratch)
        .map_err(invalid)?;
    write_handshake(&mut socket, &scratch[..n])?;
    handshake
        .read_message(&read_handshake(&mut socket)?, &mut scratch)
        .map_err(invalid)?;
    let remote = handshake
        .get_remote_static()
        .ok_or_else(|| invalid("Missing host identity"))?
        .to_vec();
    let n = handshake
        .write_message(&[], &mut scratch)
        .map_err(invalid)?;
    write_handshake(&mut socket, &scratch[..n])?;
    Ok((
        SecureStream::new(socket, handshake.into_transport_mode().map_err(invalid)?),
        remote,
    ))
}

/// A changed key fails closed. First contact can be preauthenticated by passing
/// the fingerprint from a trusted invitation instead of relying on TOFU.
#[cfg(feature = "gui")]
pub fn pin_host(address: &str, key: &[u8], expected: Option<&str>) -> io::Result<String> {
    let actual = fingerprint(key);
    if expected.is_some_and(|value| !value.eq_ignore_ascii_case(&actual)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "A identidade do servidor não corresponde ao convite. Conexão recusada.",
        ));
    }
    let dir = dirs::config_dir()
        .ok_or_else(|| invalid("Configuration directory unavailable"))?
        .join("Lanvox");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("known-hosts.json");
    let mut hosts: std::collections::BTreeMap<String, String> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(invalid)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Default::default(),
        Err(error) => return Err(error),
    };
    if hosts.get(address).is_some_and(|pinned| pinned != &actual) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "A chave deste servidor mudou. Confira a impressão digital com o administrador antes de atualizar a confiança."));
    }
    if !hosts.contains_key(address) {
        hosts.insert(address.to_string(), actual.clone());
        let tmp = path.with_extension("tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(&hosts).map_err(invalid)?)?;
        file.sync_all()?;
        std::fs::rename(tmp, path)?;
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn encrypted_stream_round_trip_survives_large_records_and_clones() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let key = HostKey::generate().unwrap();
        let expected = key.fingerprint();
        let thread = std::thread::spawn(move || {
            let mut stream = server_handshake(listener.accept().unwrap().0, &key).unwrap();
            let mut bytes = vec![0; 180_000];
            stream.read_exact(&mut bytes).unwrap();
            assert!(bytes.iter().all(|byte| *byte == 42));
            stream.try_clone().unwrap().write_all(b"confirmed").unwrap();
        });
        let (mut stream, key) = client_handshake(TcpStream::connect(address).unwrap()).unwrap();
        assert_eq!(fingerprint(&key), expected);
        stream.write_all(&vec![42; 180_000]).unwrap();
        let mut answer = [0; 9];
        stream.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"confirmed");
        thread.join().unwrap();
    }

    #[test]
    fn invalid_key_file_is_never_replaced() {
        let path =
            std::env::temp_dir().join(format!("lanvox-invalid-host-key-{}", rand::random::<u64>()));
        std::fs::write(&path, b"broken").unwrap();
        assert!(HostKey::load_or_create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupted_transport_record_is_rejected_before_application_decoding() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let key = HostKey::generate().unwrap();
            let mut stream = server_handshake(listener.accept().unwrap().0, &key).unwrap();
            assert!(stream.read(&mut [0; 32]).is_err());
        });
        let (stream, _) = client_handshake(TcpStream::connect(address).unwrap()).unwrap();
        let mut socket = stream.raw_clone().unwrap();
        socket.write_all(&16u16.to_le_bytes()).unwrap();
        socket.write_all(&[0; 16]).unwrap();
        thread.join().unwrap();
    }
}
