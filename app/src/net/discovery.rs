//! Listens for servers announcing themselves on the local network.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::protocol::{DISCOVERY_MAGIC, DISCOVERY_PORT};

#[derive(Clone, Debug)]
pub struct FoundServer {
    pub name: String,
    pub address: String,
    pub seen: Instant,
}

#[derive(Clone, Default)]
pub struct Discovery {
    found: Arc<Mutex<HashMap<String, FoundServer>>>,
    stop: Arc<AtomicBool>,
}

impl Discovery {
    pub fn start(ctx: egui::Context) -> Discovery {
        let d = Discovery::default();
        let found = d.found.clone();
        let stop = d.stop.clone();
        std::thread::Builder::new()
            .name("discovery".into())
            .spawn(move || {
                let Ok(sock) = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) else {
                    return;
                };
                let _ = sock.set_read_timeout(Some(Duration::from_secs(1)));
                let mut buf = [0u8; 512];
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = sock.recv_from(&mut buf) else {
                        found
                            .lock()
                            .retain(|_, server| server.seen.elapsed() < Duration::from_secs(7));
                        continue;
                    };
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let mut parts = text.splitn(3, '|');
                    if parts.next() != Some(DISCOVERY_MAGIC) {
                        continue;
                    }
                    let Some(port) = parts.next().and_then(|p| p.parse::<u16>().ok()) else {
                        continue;
                    };
                    if port == 0 {
                        continue;
                    }
                    let name: String = parts
                        .next()
                        .unwrap_or("Servidor")
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(80)
                        .collect();
                    let ip = if from.ip().is_loopback() {
                        "127.0.0.1".to_string()
                    } else {
                        from.ip().to_string()
                    };
                    let address = format!("{ip}:{port}");
                    let mut map = found.lock();
                    map.retain(|_, server| server.seen.elapsed() < Duration::from_secs(7));
                    if map.len() >= 128 && !map.contains_key(&address) {
                        continue;
                    }
                    // Prefer a network address over the loopback copy of our own server.
                    if from.ip().is_loopback()
                        && map
                            .values()
                            .any(|f| f.name == name && !f.address.starts_with("127."))
                    {
                        continue;
                    }
                    let fresh = !map.contains_key(&address);
                    map.insert(
                        address.clone(),
                        FoundServer {
                            name,
                            address,
                            seen: Instant::now(),
                        },
                    );
                    if fresh {
                        ctx.request_repaint();
                    }
                }
            })
            .ok();
        d
    }

    /// Servers heard from in the last 7 seconds.
    pub fn list(&self) -> Vec<FoundServer> {
        let mut v: Vec<FoundServer> = self
            .found
            .lock()
            .values()
            .filter(|f| f.seen.elapsed() < Duration::from_secs(7))
            .cloned()
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name).then(a.address.cmp(&b.address)));
        v
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if Arc::strong_count(&self.stop) <= 2 {
            self.stop.store(true, Ordering::Relaxed);
        }
    }
}
