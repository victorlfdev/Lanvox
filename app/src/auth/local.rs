//! Per-host credential rotation. A new key is durably staged before the request;
//! the current key remains usable until an authenticated host confirms commit.

use super::Identity;
use crate::model::{identity_rotation_transcript, UserId, WorkspaceCommand};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub struct ConnectionIdentity {
    pub identity: Identity,
    pub name: String,
    pub expected_user: Option<UserId>,
    pub pending: bool,
}

#[derive(Clone, Debug)]
pub struct PendingRotationInfo {
    pub public_key: [u8; 32],
    pub old_public_key: [u8; 32],
    pub nonce: [u8; 32],
    pub user: UserId,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    host: String,
    user: UserId,
    name: String,
    old_public_key: [u8; 32],
    new_document: Vec<u8>,
    nonce: [u8; 32],
}

struct LocalStore {
    directory: PathBuf,
    /// Test-only fault injection: fail with "interrupted" once the given commit step is reached.
    #[cfg(test)]
    crash_at: std::cell::Cell<u8>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn host_name(address: &str) -> String {
    let address = address.trim().to_ascii_lowercase();
    if address.starts_with('[') && address.ends_with(']') {
        format!("{address}:{}", crate::protocol::DEFAULT_PORT)
    } else if address
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_ipv6())
    {
        format!("[{address}]:{}", crate::protocol::DEFAULT_PORT)
    } else if address.contains(':') {
        address
    } else {
        format!("{address}:{}", crate::protocol::DEFAULT_PORT)
    }
}

impl LocalStore {
    fn system() -> io::Result<Self> {
        let directory = dirs::config_dir()
            .ok_or_else(|| invalid("Pasta de configuração indisponível."))?
            .join("Lanvox");
        std::fs::create_dir_all(directory.join("identities"))?;
        crate::secret_store::remove_stale_temporaries(
            &directory.join("identities"),
            std::time::Duration::from_secs(60),
        );
        Ok(Self::at(directory))
    }
    fn at(directory: PathBuf) -> Self {
        Self {
            directory,
            #[cfg(test)]
            crash_at: std::cell::Cell::new(0),
        }
    }
    #[cfg(test)]
    fn step(&self, number: u8) -> io::Result<()> {
        if self.crash_at.get() == number {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "simulated crash",
            ))
        } else {
            Ok(())
        }
    }
    #[cfg(not(test))]
    #[inline(always)]
    fn step(&self, _number: u8) -> io::Result<()> {
        Ok(())
    }
    fn path(&self, host: &str, suffix: &str) -> PathBuf {
        let hash = ring::digest::digest(&ring::digest::SHA256, host_name(host).as_bytes());
        let name: String = hash
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.directory
            .join("identities")
            .join(format!("{name}.{suffix}"))
    }
    fn active(&self, host: &str) -> io::Result<Identity> {
        let path = self.path(host, "key");
        match std::fs::metadata(&path) {
            Ok(_) => Identity::load_or_create(&path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Identity::load_or_create(&self.directory.join("identity.key"))
            }
            Err(error) => Err(error),
        }
    }
    fn pending(&self, host: &str) -> io::Result<Option<Pending>> {
        let bytes = match crate::secret_store::read(&self.path(host, "pending")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let pending: Pending = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if pending.host != host_name(host)
            || pending.name.is_empty()
            || pending.name.chars().count() > 32
        {
            return Err(invalid("Registro de rotação não corresponde a este host."));
        }
        Identity::from_document(&pending.new_document)?;
        Ok(Some(pending))
    }
    fn attempts(&self, host: &str, name: &str) -> io::Result<Vec<ConnectionIdentity>> {
        let active = self.active(host)?;
        let pending = match self.pending(host) {
            Ok(Some(pending)) => pending,
            // An unreadable staging record must never lock the user out of the active key.
            // The file is left untouched so it can still be recovered manually.
            Ok(None) | Err(_) => {
                return Ok(vec![ConnectionIdentity {
                    identity: active,
                    name: name.into(),
                    expected_user: None,
                    pending: false,
                }])
            }
        };
        let new = Identity::from_document(&pending.new_document)?;
        let mut result = vec![ConnectionIdentity {
            identity: new,
            name: pending.name.clone(),
            expected_user: Some(pending.user),
            pending: true,
        }];
        result.push(ConnectionIdentity {
            identity: active,
            name: pending.name,
            expected_user: Some(pending.user),
            pending: false,
        });
        Ok(result)
    }
    fn begin(&self, host: &str, user: UserId, name: &str) -> io::Result<WorkspaceCommand> {
        let old = self.active(host)?;
        let pending = if let Some(pending) = self.pending(host)? {
            if pending.user != user || pending.old_public_key != old.public_key() {
                return Err(invalid("Há uma rotação pendente. Reconecte para confirmar a identidade antes de iniciar outra."));
            }
            pending
        } else {
            let (_, document) = Identity::generate()?;
            let pending = Pending {
                host: host_name(host),
                user,
                name: name.into(),
                old_public_key: old.public_key(),
                new_document: document,
                nonce: super::challenge()?,
            };
            // Never send the rotation until its recovery credential is durable.
            crate::secret_store::write_new(
                &self.path(host, "pending"),
                &serde_json::to_vec(&pending).map_err(io::Error::other)?,
            )?;
            self.step(1)?;
            pending
        };
        let new = Identity::from_document(&pending.new_document)?;
        let transcript = identity_rotation_transcript(
            user,
            &pending.old_public_key,
            &new.public_key(),
            &pending.nonce,
        );
        Ok(WorkspaceCommand::RotateIdentity {
            old_public_key: pending.old_public_key,
            new_public_key: new.public_key(),
            nonce: pending.nonce,
            old_signature: old.sign(&transcript),
            new_signature: new.sign(&transcript),
        })
    }
    fn confirm(&self, host: &str, public_key: [u8; 32], nonce: [u8; 32]) -> io::Result<[u8; 32]> {
        let Some(pending) = self.pending(host)? else {
            if self.active(host)?.public_key() == public_key {
                return Ok(public_key);
            }
            return Err(invalid("Não há rotação correspondente neste dispositivo."));
        };
        if pending.nonce != nonce
            || Identity::from_document(&pending.new_document)?.public_key() != public_key
        {
            return Err(invalid("A confirmação não corresponde à rotação pendente."));
        }
        let active = self.active(host)?;
        if active.public_key() != pending.old_public_key && active.public_key() != public_key {
            return Err(invalid("A identidade ativa mudou durante a rotação."));
        }
        if active.public_key() != public_key {
            let old_path = self.path(host, "key");
            let old_bytes = if old_path.exists() {
                crate::secret_store::read(&old_path)?
            } else {
                crate::secret_store::read(&self.directory.join("identity.key"))?
            };
            // Commit order: keep the old key, install the new one, then drop the journal.
            // Every prefix of this sequence is recoverable: `pending` is only removed once the
            // new key is the active one, and re-running the sequence is idempotent.
            crate::secret_store::replace(&self.path(host, "previous.key"), &old_bytes)?;
            self.step(2)?;
            crate::secret_store::replace(&old_path, &pending.new_document)?;
            self.step(3)?;
        }
        std::fs::remove_file(self.path(host, "pending"))?;
        Ok(pending.old_public_key)
    }
}

/// The key this device uses to authenticate to `address` (the rotated one after a commit).
pub fn host_identity(address: &str) -> io::Result<Identity> {
    LocalStore::system()?.active(address)
}

pub fn connection_attempts(address: &str, name: &str) -> io::Result<Vec<ConnectionIdentity>> {
    LocalStore::system()?.attempts(address, name)
}

pub fn begin_rotation(address: &str, user: UserId, name: &str) -> io::Result<WorkspaceCommand> {
    LocalStore::system()?.begin(address, user, name)
}

pub fn pending_rotation(address: &str) -> io::Result<Option<PendingRotationInfo>> {
    LocalStore::system()?
        .pending(address)?
        .map(|pending| {
            Ok(PendingRotationInfo {
                public_key: Identity::from_document(&pending.new_document)?.public_key(),
                old_public_key: pending.old_public_key,
                nonce: pending.nonce,
                user: pending.user,
            })
        })
        .transpose()
}

pub fn finish_rotation(
    address: &str,
    public_key: [u8; 32],
    nonce: [u8; 32],
) -> io::Result<[u8; 32]> {
    LocalStore::system()?.confirm(address, public_key, nonce)
}

pub fn confirm_connection(address: &str, public_key: [u8; 32], user: UserId) -> io::Result<()> {
    let store = LocalStore::system()?;
    if let Some(pending) = store.pending(address)? {
        if Identity::from_document(&pending.new_document)?.public_key() == public_key {
            if user != pending.user {
                return Err(invalid("O host não recuperou o perfil original."));
            }
            store.confirm(address, public_key, pending.nonce)?;
        }
    }
    Ok(())
}

pub fn export_host_identity(address: &str, destination: &Path) -> io::Result<()> {
    let store = LocalStore::system()?;
    let path = store.path(address, "key");
    let base_path = store.directory.join("identity.key");
    let document = crate::secret_store::read(if path.exists() { &path } else { &base_path })?;
    Identity::from_document(&document)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(destination)?;
    file.write_all(&document)?;
    file.sync_all()
}

pub fn import_host_identity(address: &str, source: &Path) -> io::Result<()> {
    let store = LocalStore::system()?;
    if store.pending(address)?.is_some() {
        return Err(invalid(
            "Reconecte e conclua a rotação pendente antes de importar outra identidade.",
        ));
    }
    let bytes = crate::secret_store::read(source)?;
    Identity::from_document(&bytes)?;
    let destination = store.path(address, "key");
    if destination.exists() {
        crate::secret_store::replace(
            &store.path(address, "previous.key"),
            &crate::secret_store::read(&destination)?,
        )?;
    }
    crate::secret_store::replace(&destination, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> LocalStore {
        let directory =
            std::env::temp_dir().join(format!("lanvox-rotation-local-{}", rand::random::<u64>()));
        std::fs::create_dir_all(directory.join("identities")).unwrap();
        LocalStore::at(directory)
    }
    fn cleanup(store: LocalStore) {
        for file in std::fs::read_dir(store.directory.join("identities")).unwrap() {
            std::fs::remove_file(file.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(store.directory.join("identities")).unwrap();
        let _ = std::fs::remove_file(store.directory.join("identity.key"));
        std::fs::remove_dir(store.directory).unwrap();
    }
    #[test]
    fn pending_rotation_survives_restart_and_only_confirms_matching_proof() {
        let store = fixture();
        let old = store.active("friends").unwrap().public_key();
        let request = store.begin("friends", 7, "Ana").unwrap();
        let WorkspaceCommand::RotateIdentity {
            old_public_key,
            new_public_key,
            nonce,
            ..
        } = request
        else {
            panic!()
        };
        assert_eq!(old, old_public_key);
        assert_ne!(old, new_public_key);
        let attempts = store.attempts("friends:47800", "OtherName").unwrap();
        assert_eq!(attempts[0].identity.public_key(), new_public_key);
        assert_eq!(attempts[0].expected_user, Some(7));
        assert_eq!(attempts[0].name, "Ana");
        assert_eq!(attempts[1].identity.public_key(), old);
        assert!(store.confirm("friends", new_public_key, [0; 32]).is_err());
        assert_eq!(store.active("friends").unwrap().public_key(), old);
        store.confirm("friends", new_public_key, nonce).unwrap();
        assert_eq!(
            store.active("friends").unwrap().public_key(),
            new_public_key
        );
        assert_eq!(store.active("different-host").unwrap().public_key(), old);
        assert!(store.pending("friends").unwrap().is_none());
        cleanup(store);
    }

    /// Simulates a crash after each durable step of the rotation commit, then a restart.
    /// The device must stay able to authenticate and converge on the new key without data loss.
    #[test]
    fn crash_during_commit_recovers_to_new_key_without_lockout() {
        for crash in 2..=3u8 {
            let store = fixture();
            let old = store.active("h").unwrap().public_key();
            let WorkspaceCommand::RotateIdentity {
                new_public_key,
                nonce,
                ..
            } = store.begin("h", 3, "Ana").unwrap()
            else {
                panic!()
            };
            store.crash_at.set(crash);
            assert!(
                store.confirm("h", new_public_key, nonce).is_err(),
                "crash {crash}"
            );
            store.crash_at.set(0);
            // Restart: the pending journal survives and the new key is offered first.
            let attempts = store.attempts("h", "x").unwrap();
            assert_eq!(attempts.len(), 2);
            assert_eq!(attempts[0].identity.public_key(), new_public_key);
            let active = attempts[1].identity.public_key();
            assert_eq!(
                active,
                if crash == 2 { old } else { new_public_key },
                "crash {crash}"
            );
            // The old key is preserved on disk before it can be replaced.
            assert_eq!(
                Identity::from_document(
                    &crate::secret_store::read(&store.path("h", "previous.key")).unwrap()
                )
                .unwrap()
                .public_key(),
                old
            );
            // Host answers with the committed rotation: recovery converges and is repeatable.
            store.confirm("h", new_public_key, nonce).unwrap();
            assert_eq!(store.active("h").unwrap().public_key(), new_public_key);
            assert!(store.pending("h").unwrap().is_none());
            assert_eq!(
                store.confirm("h", new_public_key, nonce).unwrap(),
                new_public_key
            );
            cleanup(store);
        }
    }

    #[test]
    fn interrupted_rotation_before_host_commit_keeps_old_key_and_allows_retry() {
        let store = fixture();
        let old = store.active("h").unwrap().public_key();
        store.crash_at.set(1);
        assert!(store.begin("h", 3, "Ana").is_err());
        store.crash_at.set(0);
        // The staged key is durable; the request is rebuilt identically on retry.
        let first = store.pending("h").unwrap().unwrap();
        let WorkspaceCommand::RotateIdentity {
            old_public_key,
            nonce,
            ..
        } = store.begin("h", 3, "Ana").unwrap()
        else {
            panic!()
        };
        assert_eq!(old_public_key, old);
        assert_eq!(nonce, first.nonce);
        assert_eq!(store.active("h").unwrap().public_key(), old);
        // Host never committed: the old key is still offered.
        assert_eq!(
            store.attempts("h", "Ana").unwrap()[1].identity.public_key(),
            old
        );
        cleanup(store);
    }

    #[test]
    fn unreadable_pending_record_does_not_lock_out_the_active_key() {
        let store = fixture();
        let old = store.active("h").unwrap().public_key();
        std::fs::write(store.path("h", "pending"), b"HUDDLE-DPAPI-1\0garbage").unwrap();
        let attempts = store.attempts("h", "Ana").unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].identity.public_key(), old);
        assert!(!attempts[0].pending);
        assert!(store.path("h", "pending").exists());
        std::fs::remove_file(store.path("h", "pending")).unwrap();
        cleanup(store);
    }

    #[test]
    fn owner_key_for_a_host_follows_a_committed_rotation() {
        let store = fixture();
        let WorkspaceCommand::RotateIdentity {
            new_public_key,
            nonce,
            ..
        } = store.begin("h", 1, "Owner").unwrap()
        else {
            panic!()
        };
        store.confirm("h", new_public_key, nonce).unwrap();
        assert_eq!(
            store.active("h:47800").unwrap().public_key(),
            new_public_key
        );
        cleanup(store);
    }
}
