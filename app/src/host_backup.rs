//! Portable host backups retain the database and the host's cryptographic
//! identity. Archive extraction accepts a fixed set of regular filenames only.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const DATABASE: &str = "hosted-server.sqlite3";
const FILES: &[&str] = &[
    DATABASE,
    "hosted-server.host-key",
    "hosted-server.host-token",
    "host-config.json",
    "registration-token.secret",
];

fn is_secret(name: &str) -> bool {
    matches!(
        name,
        "hosted-server.host-key" | "hosted-server.host-token" | "registration-token.secret"
    )
}

fn private_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

struct Staging(PathBuf);
impl Staging {
    fn new(parent: &Path) -> io::Result<Self> {
        let path = parent.join(format!(".lanvox-backup-{}", rand::random::<u64>()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        for name in FILES
            .iter()
            .copied()
            .chain(std::iter::once("verified.sqlite3"))
        {
            let path = self.0.join(name);
            let _ = std::fs::remove_file(&path);
            for suffix in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(path.with_file_name(format!("{name}{suffix}")));
            }
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

pub fn backup(data: &Path, destination: &Path) -> io::Result<()> {
    if destination
        .extension()
        .is_some_and(|extension| extension == "sqlite3")
    {
        return crate::storage::backup_database(&data.join(DATABASE), destination);
    }
    let staging = Staging::new(data)?;
    crate::storage::backup_database(&data.join(DATABASE), &staging.0.join(DATABASE))?;
    let file = private_file(destination)?;
    let result = (|| {
        let mut archive = tar::Builder::new(file);
        archive.append_path_with_name(staging.0.join(DATABASE), DATABASE)?;
        for name in &FILES[1..] {
            let path = data.join(name);
            if path.is_file() {
                if is_secret(name) {
                    let plain = crate::secret_store::read(&path)?;
                    let mut header = tar::Header::new_gnu();
                    header.set_size(plain.len() as u64);
                    header.set_mode(0o600);
                    header.set_cksum();
                    archive.append_data(&mut header, name, plain.as_slice())?;
                } else {
                    archive.append_path_with_name(path, name)?;
                }
            }
        }
        let file = archive.into_inner()?;
        file.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

pub fn restore(source: &Path, data: &Path) -> io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut input = std::fs::File::open(source)?;
    let mut signature = [0; 16];
    input.read_exact(&mut signature)?;
    input.seek(SeekFrom::Start(0))?;
    if &signature == b"SQLite format 3\0" {
        return crate::storage::restore_database(source, &data.join(DATABASE));
    }
    let staging = Staging::new(data)?;
    let mut archive = tar::Archive::new(input);
    let mut names = std::collections::HashSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let name = path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Nome inválido no backup.")
        })?;
        if !FILES.contains(&name)
            || !entry.header().entry_type().is_file()
            || !names.insert(name.to_string())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "O backup contém arquivos desconhecidos, repetidos ou caminhos inválidos.",
            ));
        }
        let maximum = if name == DATABASE {
            32 * 1024 * 1024 * 1024
        } else {
            1024 * 1024
        };
        if entry.size() > maximum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Arquivo do backup excede o limite.",
            ));
        }
        if data.join(name).exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "O destino já contém dados. Escolha uma pasta nova.",
            ));
        }
        let mut output = private_file(&staging.0.join(name))?;
        io::copy(&mut entry, &mut output)?;
        output.flush()?;
        output.sync_all()?;
    }
    if !names.contains(DATABASE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "O backup não contém o banco Lanvox.",
        ));
    }
    // Validate the complete database before changing any destination files.
    let verified = staging.0.join("verified.sqlite3");
    let verify = crate::storage::backup_database(&staging.0.join(DATABASE), &verified);
    if let Err(error) = verify {
        let _ = std::fs::remove_file(verified);
        return Err(error);
    }
    let mut created = Vec::new();
    let result = (|| {
        for name in FILES
            .iter()
            .filter(|name| **name != DATABASE && names.contains(**name))
        {
            let bytes = std::fs::read(staging.0.join(name))?;
            let destination = data.join(name);
            if is_secret(name) {
                crate::secret_store::write_new(&destination, &bytes)?;
                created.push(destination);
            } else {
                let mut file = private_file(&destination)?;
                created.push(destination);
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
        }
        crate::storage::restore_database(&verified, &data.join(DATABASE))?;
        Ok(())
    })();
    let _ = std::fs::remove_file(verified);
    if result.is_err() {
        for path in created {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_restores_messages_and_host_identity_without_overwriting() {
        let root =
            std::env::temp_dir().join(format!("lanvox-full-backup-{}", rand::random::<u64>()));
        let original = root.join("original");
        let restored = root.join("restored");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir(&restored).unwrap();
        let archive = root.join("host.lanvox-backup");
        {
            let mut store = crate::storage::Store::open(&original.join(DATABASE)).unwrap();
            let state = serde_json::json!({"public_keys":{},"channels":[],"users":[],"messages":[],"next_channel":1,"next_user":1,"next_message":2});
            store.save_state(&state).unwrap();
            let message = crate::model::Message {
                id: 1,
                channel: 1,
                author: 1,
                author_name: "Ana".into(),
                author_color: 0,
                time: 1,
                text: "Mensagem durável".into(),
            };
            store
                .insert(&message, Some("one-operation"), &state)
                .unwrap();
            std::fs::write(
                original.join("hosted-server.host-key"),
                b"private host identity",
            )
            .unwrap();
            std::fs::write(
                original.join("hosted-server.host-token"),
                b"private enrollment token",
            )
            .unwrap();
            backup(&original, &archive).unwrap();
        }
        restore(&archive, &restored).unwrap();
        assert_eq!(
            crate::secret_store::read(&restored.join("hosted-server.host-key")).unwrap(),
            b"private host identity"
        );
        assert_eq!(
            crate::secret_store::read(&restored.join("hosted-server.host-token")).unwrap(),
            b"private enrollment token"
        );
        {
            let store = crate::storage::Store::open(&restored.join(DATABASE)).unwrap();
            assert_eq!(
                store.history(1, None, 10).unwrap()[0].text,
                "Mensagem durável"
            );
            assert_eq!(store.operation(1, "one-operation").unwrap(), Some(1));
        }
        assert!(restore(&archive, &restored).is_err());
        assert_eq!(
            crate::secret_store::read(&restored.join("hosted-server.host-key")).unwrap(),
            b"private host identity"
        );
        for dir in [&original, &restored] {
            for name in FILES {
                let _ = std::fs::remove_file(dir.join(name));
            }
            for suffix in ["hosted-server.sqlite3-wal", "hosted-server.sqlite3-shm"] {
                let _ = std::fs::remove_file(dir.join(suffix));
            }
            std::fs::remove_dir(dir).unwrap();
        }
        std::fs::remove_file(archive).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn restore_rejects_unexpected_archive_entries_without_writing_them() {
        let data =
            std::env::temp_dir().join(format!("lanvox-bad-backup-{}", rand::random::<u64>()));
        std::fs::create_dir(&data).unwrap();
        let source = data.join("unknown.tar");
        let file = std::fs::File::create(&source).unwrap();
        let mut archive = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o600);
        header.set_cksum();
        archive
            .append_data(&mut header, "outside.txt", b"oops".as_slice())
            .unwrap();
        archive.finish().unwrap();
        drop(archive);
        assert!(restore(&source, &data).is_err());
        assert!(!data.join("outside.txt").exists());
        std::fs::remove_file(source).unwrap();
        std::fs::remove_dir(data).unwrap();
    }
}
