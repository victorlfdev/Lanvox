use super::*;
use std::net::TcpStream;

mod call_tests;
mod identity_tests;
mod network_tests;

// TCP and UDP have different excluded-port ranges on Windows. Reserve a
// candidate usable by both instead of assuming the TCP ephemeral port works.
fn test_listener(address: &str) -> TcpListener {
    for _ in 0..128 {
        let listener = TcpListener::bind((address, 0)).unwrap();
        if let Ok(_udp) = UdpSocket::bind(listener.local_addr().unwrap()) {
            return listener;
        }
    }
    panic!("no loopback port available for both TCP and UDP");
}

fn host() -> ServerHandle {
    start_listener("Test".into(), test_listener("127.0.0.1"), None).unwrap()
}

pub(super) fn fixture() -> State {
    State {
        name: "Test".into(),
        channels: default_channels(),
        users: HashMap::new(),
        messages: Vec::new(),
        voice: HashMap::new(),
        stage_speakers: HashMap::new(),
        stage_requests: HashMap::new(),
        streams: HashMap::new(),
        guilds: vec![Guild {
            id: 1,
            name: "Test".into(),
        }],
        next_guild: 2,
        friends: HashMap::new(),
        calls: HashMap::new(),
        next_call: CALL_ID_BASE,
        watchers: HashMap::new(),
        clients: HashMap::new(),
        next_channel: 5,
        next_user: 1,
        next_message: 1,
        last_keyframe_ask: HashMap::new(),
        save_path: None,
        public_keys: HashMap::new(),
        workspace: WorkspaceData::default(),
        store: None,
        options: ServerOptions {
            allow_registration: true,
            ..ServerOptions::default()
        },
        uploads: HashMap::new(),
        pending_uploads: Arc::default(),
        operations: HashMap::new(),
        rates: HashMap::new(),
        call_deadlines: HashMap::new(),
        media_router: Arc::new(media_router::MediaRouter::default()),
    }
}

fn client(
    port: u16,
    name: &str,
) -> (
    crate::auth::transport::SecureStream,
    BufReader<crate::auth::transport::SecureStream>,
    UserId,
) {
    let (identity, _) = crate::auth::Identity::generate().unwrap();
    let (w, r, reply) = login(port, name, &identity);
    let ServerMsg::Welcome { you, .. } = reply else {
        panic!("no welcome")
    };
    (w, r, you)
}

fn login(
    port: u16,
    name: &str,
    identity: &crate::auth::Identity,
) -> (
    crate::auth::transport::SecureStream,
    BufReader<crate::auth::transport::SecureStream>,
    ServerMsg,
) {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let (s, _) = crate::auth::transport::client_handshake(s).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    let ServerMsg::Challenge { nonce, .. } = read_packet::<_, ServerMsg>(&mut r).unwrap() else {
        panic!("no challenge")
    };
    write_packet(&mut w, &identity.hello(name, 0, &nonce)).unwrap();
    let reply = read_packet::<_, ServerMsg>(&mut r).unwrap();
    (w, r, reply)
}

fn wait_for(
    r: &mut BufReader<crate::auth::transport::SecureStream>,
    pred: impl Fn(&ServerMsg) -> bool,
) -> ServerMsg {
    loop {
        let m = read_packet::<_, ServerMsg>(r).expect("server message");
        if pred(&m) {
            return m;
        }
    }
}
