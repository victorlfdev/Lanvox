//! Proof of possession of a local Ed25519 identity. Private keys never cross the network.

use std::io;

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};

#[cfg(any(feature = "gui", test))]
pub mod local;
pub mod transport;
pub const PROTOCOL_VERSION: u32 = 7;

fn crypto_error() -> io::Error {
    io::Error::other("Could not initialize the local identity")
}

pub fn challenge() -> io::Result<[u8; 32]> {
    let mut nonce = [0; 32];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| crypto_error())?;
    Ok(nonce)
}

fn transcript(nonce: &[u8; 32], name: &str, color: u32) -> Vec<u8> {
    // Kept from the previous name on purpose; see branding.rs.
    let mut message = b"Huddle identity v2\0".to_vec();
    message.extend_from_slice(nonce);
    message.extend_from_slice(&(name.len() as u64).to_le_bytes());
    message.extend_from_slice(name.as_bytes());
    message.extend_from_slice(&color.to_le_bytes());
    message
}

pub fn verify(key: &[u8; 32], nonce: &[u8; 32], name: &str, color: u32, signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&transcript(nonce, name, color), signature)
        .is_ok()
}

#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub struct Identity(Ed25519KeyPair);

impl Identity {
    #[cfg(any(feature = "gui", test))]
    pub fn from_document(document: &[u8]) -> io::Result<Self> {
        Ed25519KeyPair::from_pkcs8(document).map(Self).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Arquivo de identidade inválido.",
            )
        })
    }

    #[cfg(any(feature = "gui", test))]
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.0.sign(message).as_ref().to_vec()
    }
    pub fn public_key(&self) -> [u8; 32] {
        self.0
            .public_key()
            .as_ref()
            .try_into()
            .expect("Ed25519 public key")
    }
    #[cfg(any(feature = "gui", test))]
    pub fn generate() -> io::Result<(Self, Vec<u8>)> {
        let document =
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| crypto_error())?;
        let key = Ed25519KeyPair::from_pkcs8(document.as_ref()).map_err(|_| crypto_error())?;
        Ok((Self(key), document.as_ref().to_vec()))
    }

    #[cfg(any(feature = "gui", test))]
    pub fn load_or_create(path: &std::path::Path) -> io::Result<Self> {
        match crate::secret_store::read(path) {
            Ok(document) => {
                let identity = Self::from_document(&document)?;
                crate::secret_store::migrate_validated(path, &document)?;
                Ok(identity)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let (identity, document) = Self::generate()?;
                match crate::secret_store::write_new(path, &document) {
                    Ok(()) => Ok(identity),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        Self::load_or_create(path)
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn hello(&self, name: &str, color: u32, nonce: &[u8; 32]) -> crate::protocol::ClientMsg {
        crate::protocol::ClientMsg::Hello {
            name: name.into(),
            color,
            public_key: self.public_key(),
            signature: self
                .0
                .sign(&transcript(nonce, name, color))
                .as_ref()
                .to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_bind_the_challenge_and_profile() {
        let (identity, _) = Identity::generate().unwrap();
        let nonce = challenge().unwrap();
        let crate::protocol::ClientMsg::Hello {
            public_key,
            signature,
            ..
        } = identity.hello("Ana", 42, &nonce)
        else {
            unreachable!()
        };
        assert!(verify(&public_key, &nonce, "Ana", 42, &signature));
        assert!(!verify(
            &public_key,
            &challenge().unwrap(),
            "Ana",
            42,
            &signature
        ));
        assert!(!verify(&public_key, &nonce, "Other", 42, &signature));
        assert!(!verify(&public_key, &nonce, "Ana", 43, &signature));
    }

    #[test]
    fn identity_survives_restart_and_corruption_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("lanvox-key-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("identity.key");
        let a = Identity::load_or_create(&path).unwrap();
        let b = Identity::load_or_create(&path).unwrap();
        assert_eq!(a.0.public_key().as_ref(), b.0.public_key().as_ref());
        std::fs::write(&path, b"corrupt").unwrap();
        assert!(Identity::load_or_create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
