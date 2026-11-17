//! The connection from this app to a server.
//!
//! Voice packets go straight to the audio mixer and video packets straight to the
//! picture decoder, so the interface thread only handles chat and state changes.

use std::io::BufReader;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver};

use super::outbox::Outbox;
use crate::media::audio::Mixer;
use crate::media::video::Decoder;
use crate::model::*;
use crate::protocol::*;

pub enum NetEvent {
    Welcome {
        you: UserId,
        snapshot: ServerSnapshot,
    },
    Server(ServerMsg),
    Latency(f32),
    Closed(String),
}

/// Flags the capture threads read to know a full picture is wanted.
#[derive(Default)]
pub struct KeyframeFlags {
    pub screen: AtomicBool,
    pub camera: AtomicBool,
    pub screen_viewers: AtomicU32,
}

impl KeyframeFlags {
    pub fn take(&self, kind: StreamKind) -> bool {
        match kind {
            StreamKind::Screen => self.screen.swap(false, Ordering::SeqCst),
            StreamKind::Camera => self.camera.swap(false, Ordering::SeqCst),
        }
    }
}

pub struct Client {
    outbox: Outbox,
    pub events: Receiver<NetEvent>,
    pub address: String,
    pub keyframes: Arc<KeyframeFlags>,
    alive: Arc<AtomicBool>,
    pub host_fingerprint: String,
    pub public_key: [u8; 32],
    decoder: Decoder,
}

#[derive(Clone, Default)]
pub struct ConnectOptions {
    pub invite: String,
    pub expected_fingerprint: Option<String>,
}

struct Authenticated {
    stream: crate::auth::transport::SecureStream,
    welcome: NetEvent,
    fingerprint: String,
    peer: std::net::SocketAddr,
    public_key: [u8; 32],
}

fn authenticate(
    address: &str,
    name: &str,
    color: u32,
    options: &ConnectOptions,
) -> std::io::Result<Authenticated> {
    let candidates = crate::auth::local::connection_attempts(address, name)?;
    let mut failure = std::io::Error::other("Nenhuma identidade local disponível");
    for candidate in candidates {
        let result = (|| {
            let mut connection = None;
            let mut connect_error =
                std::io::Error::new(std::io::ErrorKind::NotFound, "Endereço não encontrado");
            for endpoint in address.to_socket_addrs()?.take(8) {
                match TcpStream::connect_timeout(&endpoint, Duration::from_secs(4)) {
                    Ok(stream) => {
                        connection = Some(stream);
                        break;
                    }
                    Err(error) => connect_error = error,
                }
            }
            let socket = connection.ok_or(connect_error)?;
            socket.set_nodelay(true)?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            socket.set_write_timeout(Some(Duration::from_secs(5)))?;
            let (mut stream, host_key) = crate::auth::transport::client_handshake(socket)?;
            let peer = stream.peer_addr()?;
            let fingerprint = crate::auth::transport::pin_host(
                address,
                &host_key,
                options.expected_fingerprint.as_deref(),
            )?;
            let challenge = read_packet_limit::<_, ServerMsg>(&mut stream, 4096)?;
            let ServerMsg::Challenge {
                version: crate::auth::PROTOCOL_VERSION,
                nonce,
            } = challenge
            else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Servidor Lanvox incompatível. Atualize cliente e host juntos.",
                ));
            };
            let hello = candidate.identity.hello(&candidate.name, color, &nonce);
            let hello = if options.invite.is_empty() {
                hello
            } else {
                ClientMsg::Authenticate {
                    hello: Box::new(hello),
                    token: options.invite.clone(),
                }
            };
            write_packet(&mut stream, &hello)?;
            let (you, snapshot) = match read_packet::<_, ServerMsg>(&mut stream)? {
                ServerMsg::Welcome { you, snapshot } => (you, snapshot),
                ServerMsg::Error(reason) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        reason,
                    ))
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "O host não confirmou sua identidade",
                    ))
                }
            };
            if candidate
                .expected_user
                .is_some_and(|expected| expected != you)
            {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "A rotação não recuperou o perfil original. A sessão foi recusada para preservar sua identidade."));
            }
            let public_key = candidate.identity.public_key();
            crate::auth::local::confirm_connection(address, public_key, you)?;
            Ok(Authenticated {
                stream,
                welcome: NetEvent::Welcome { you, snapshot },
                fingerprint,
                peer,
                public_key,
            })
        })();
        match result {
            Ok(connection) => return Ok(connection),
            Err(error) if candidate.pending => failure = error,
            Err(error) => return Err(error),
        }
    }
    Err(failure)
}

impl Client {
    pub fn connect_with_options(
        address: &str,
        name: &str,
        color: u32,
        ctx: egui::Context,
        mixer: Arc<Mixer>,
        decoder: Decoder,
        options: ConnectOptions,
    ) -> std::io::Result<Client> {
        let address = if address.starts_with('[') && address.ends_with(']') {
            format!("{address}:{DEFAULT_PORT}")
        } else if address
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_ipv6())
        {
            format!("[{address}]:{DEFAULT_PORT}")
        } else if address.contains(':') {
            address.to_string()
        } else {
            format!("{address}:{DEFAULT_PORT}")
        };
        let Authenticated {
            stream,
            welcome,
            fingerprint: host_fingerprint,
            peer,
            public_key,
        } = authenticate(&address, name, color, &options)?;
        stream.set_read_timeout(None)?;
        let read_half = stream.try_clone()?;
        let outbox = Outbox::start_secure(stream)?;

        let (tx, rx) = bounded(1024);
        let _ = tx.send(welcome);
        ctx.request_repaint();
        let keyframes = Arc::new(KeyframeFlags::default());
        let alive = Arc::new(AtomicBool::new(true));
        let start = std::time::Instant::now();
        let last_pong = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
        decoder.reset();
        let decoder_handle = decoder.clone();

        // Reader.
        {
            let keyframes = keyframes.clone();
            let alive = alive.clone();
            let outbox = outbox.clone();
            let last_pong = last_pong.clone();
            std::thread::Builder::new()
                .name("net-reader".into())
                .spawn(move || {
                    let mut reader = BufReader::with_capacity(256 * 1024, read_half);
                    let reason = loop {
                        let msg = match read_packet::<_, ServerMsg>(&mut reader) {
                            Ok(m) => m,
                            Err(e) => break e.to_string(),
                        };
                        match msg {
                            ServerMsg::MediaSession { port, token, key } => {
                                let mut address = peer;
                                address.set_port(port);
                                let (mixer, decoder, callback_outbox) =
                                    (mixer.clone(), decoder.clone(), outbox.clone());
                                let media =
                                    super::datagram::connect(address, token, key, move |message| {
                                        let sender = ClientSender::new(callback_outbox.clone());
                                        match message {
                                            ServerMsg::AudioEncoded {
                                                from,
                                                seq,
                                                sent_micros,
                                                opus,
                                            } => mixer.push_encoded(from, seq, sent_micros, &opus),
                                            ServerMsg::StreamAudio {
                                                from,
                                                seq,
                                                sent_micros,
                                                packet,
                                            } => mixer.push_stream(from, seq, sent_micros, &packet),
                                            ServerMsg::Audio { from, seq, pcm } => {
                                                mixer.push(from, seq, &pcm)
                                            }
                                            ServerMsg::Frame { owner, kind, frame } => {
                                                decoder.submit(owner, kind, frame, sender)
                                            }
                                            ServerMsg::EncodedFrame { owner, kind, frame } => {
                                                decoder.submit_encoded(owner, kind, frame, sender)
                                            }
                                            ServerMsg::Cursor { owner, state } => {
                                                decoder.submit_cursor(owner, state)
                                            }
                                            _ => {}
                                        }
                                    });
                                if let Ok(media) = media {
                                    outbox.attach_media(media);
                                }
                            }
                            ServerMsg::AudioEncoded {
                                from,
                                seq,
                                sent_micros,
                                opus,
                            } => {
                                mixer.push_encoded(from, seq, sent_micros, &opus);
                            }
                            ServerMsg::StreamAudio {
                                from,
                                seq,
                                sent_micros,
                                packet,
                            } => {
                                mixer.push_stream(from, seq, sent_micros, &packet);
                            }
                            ServerMsg::StreamStopped { owner, kind } => {
                                if kind == StreamKind::Screen {
                                    mixer.stop_stream(owner);
                                }
                                decoder.forget(owner, kind, ClientSender::new(outbox.clone()));
                                let _ = tx.send(NetEvent::Server(ServerMsg::StreamStopped {
                                    owner,
                                    kind,
                                }));
                                ctx.request_repaint();
                            }
                            ServerMsg::Audio { from, seq, pcm } if pcm.len() == 480 => {
                                mixer.push(from, seq, &pcm)
                            }
                            ServerMsg::Audio { .. } => {}
                            ServerMsg::Frame { owner, kind, frame } => decoder.submit(
                                owner,
                                kind,
                                frame,
                                ClientSender {
                                    outbox: outbox.clone(),
                                },
                            ),
                            ServerMsg::EncodedFrame { owner, kind, frame } => decoder
                                .submit_encoded(
                                    owner,
                                    kind,
                                    frame,
                                    ClientSender::new(outbox.clone()),
                                ),
                            ServerMsg::Cursor { owner, state } => {
                                decoder.submit_cursor(owner, state)
                            }
                            ServerMsg::KeyframeWanted { kind } => match kind {
                                StreamKind::Screen => {
                                    keyframes.screen.store(true, Ordering::SeqCst)
                                }
                                StreamKind::Camera => {
                                    keyframes.camera.store(true, Ordering::SeqCst);
                                    outbox.request_camera_keyframe();
                                }
                            },
                            ServerMsg::Viewers { kind, count } => {
                                if kind == StreamKind::Screen {
                                    keyframes.screen_viewers.store(count, Ordering::Relaxed);
                                }
                                let _ =
                                    tx.send(NetEvent::Server(ServerMsg::Viewers { kind, count }));
                                ctx.request_repaint();
                            }
                            ServerMsg::VideoControl {
                                kind,
                                max_bps,
                                congested,
                                healthy,
                                overcommitted,
                            } => {
                                outbox.update_video_control(
                                    kind,
                                    max_bps,
                                    congested,
                                    healthy,
                                    overcommitted,
                                );
                            }
                            ServerMsg::Pong(t) => {
                                *last_pong
                                    .lock()
                                    .unwrap_or_else(|poison| poison.into_inner()) =
                                    std::time::Instant::now();
                                let now = start.elapsed().as_micros() as u64;
                                let _ = tx
                                    .send(NetEvent::Latency(now.saturating_sub(t) as f32 / 1000.0));
                            }
                            ServerMsg::Welcome { you, snapshot } => {
                                let _ = tx.send(NetEvent::Welcome { you, snapshot });
                                ctx.request_repaint();
                            }
                            other => {
                                let _ = tx.send(NetEvent::Server(other));
                                ctx.request_repaint();
                            }
                        }
                    };
                    alive.store(false, Ordering::SeqCst);
                    outbox.close();
                    let _ = tx.send(NetEvent::Closed(reason));
                    ctx.request_repaint();
                })?;
        }

        // Latency pings every 2 s.
        {
            let outbox = outbox.clone();
            let alive = alive.clone();
            let last_pong = last_pong.clone();
            std::thread::spawn(move || {
                while alive.load(Ordering::SeqCst) {
                    if last_pong
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .elapsed()
                        > Duration::from_secs(12)
                    {
                        alive.store(false, Ordering::SeqCst);
                        outbox.close();
                        break;
                    }
                    let t = start.elapsed().as_micros() as u64;
                    outbox.push_urgent(encode_packet(&ClientMsg::Ping(t)));
                    outbox.probe_media(encode_packet(&ClientMsg::Ping(t)));
                    std::thread::sleep(Duration::from_secs(2));
                }
            });
        }

        Ok(Client {
            outbox,
            events: rx,
            address,
            keyframes,
            alive,
            host_fingerprint,
            public_key,
            decoder: decoder_handle,
        })
    }

    pub fn send(&self, msg: ClientMsg) {
        self.outbox.push_urgent(encode_packet(&msg));
    }

    /// Sends a piece of a file transfer behind everything urgent (pings, chat, calls).
    pub fn send_file(&self, msg: ClientMsg) {
        self.outbox.push_file(encode_packet(&msg));
    }

    pub fn sender(&self) -> ClientSender {
        ClientSender::new(self.outbox.clone())
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn media_stats(&self) -> super::datagram::MediaStats {
        self.outbox.media_stats()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        self.decoder.reset();
        self.outbox.close();
    }
}

/// A cheap handle capture threads use to send packets.
#[derive(Clone)]
pub struct ClientSender {
    outbox: Outbox,
}

impl ClientSender {
    pub(crate) fn new(outbox: Outbox) -> Self {
        Self { outbox }
    }
    pub fn send(&self, msg: &ClientMsg) {
        if matches!(
            msg,
            ClientMsg::Audio { .. }
                | ClientMsg::AudioEncoded { .. }
                | ClientMsg::StreamAudio { .. }
                | ClientMsg::Speaking(_)
        ) {
            self.outbox.push_realtime(encode_packet(msg));
        } else {
            self.outbox.push_urgent(encode_packet(msg));
        }
    }
    pub fn send_frame(&self, kind: StreamKind, frame: crate::protocol::VideoFrame) -> bool {
        self.outbox
            .push_bulk(encode_packet(&ClientMsg::Frame { kind, frame }))
    }
    pub fn send_encoded_frame(
        &self,
        kind: StreamKind,
        frame: crate::protocol::EncodedVideoFrame,
    ) -> bool {
        self.outbox
            .push_bulk(encode_packet(&ClientMsg::EncodedFrame { kind, frame }))
    }
    pub fn send_cursor(&self, state: crate::protocol::CursorState) {
        self.outbox
            .push_cursor(encode_packet(&ClientMsg::Cursor(state)));
    }
    pub fn take_camera_keyframe(&self) -> bool {
        self.outbox.take_camera_keyframe()
    }
    pub fn video_control(&self, kind: StreamKind) -> super::outbox::VideoControl {
        self.outbox.video_control(kind)
    }
    pub fn request_keyframe(&self, owner: UserId, kind: StreamKind) {
        self.send(&ClientMsg::RequestKeyframe { owner, kind });
    }
    pub fn is_closed(&self) -> bool {
        self.outbox.is_closed()
    }

    pub fn media_stats(&self) -> super::datagram::MediaStats {
        self.outbox.media_stats()
    }
}
