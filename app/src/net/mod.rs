//! Networking: the host (server), the client connection, and finding hosts on the
//! local network.

#[cfg(feature = "gui")]
pub mod client;
pub mod datagram;
#[cfg(feature = "gui")]
pub mod discovery;
pub mod outbox;
pub mod server;
