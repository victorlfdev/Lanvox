//! A failed or cancelled download never changes the destination.
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct AtomicFile {
    file: Option<File>,
    temporary: PathBuf,
    destination: PathBuf,
}

impl AtomicFile {
    pub fn new(destination: &Path) -> io::Result<Self> {
        let directory = destination.parent().unwrap_or_else(|| Path::new("."));
        for _ in 0..32 {
            let temporary =
                directory.join(format!(".lanvox-{:032x}.partial", rand::random::<u128>()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temporary) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        temporary,
                        destination: destination.to_owned(),
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other(
            "Não foi possível reservar o arquivo temporário",
        ))
    }

    pub fn temporary_path(&self) -> &Path {
        &self.temporary
    }

    pub fn commit(mut self) -> io::Result<PathBuf> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        // std::fs::rename replaces the destination atomically on supported local filesystems.
        std::fs::rename(&self.temporary, &self.destination)?;
        Ok(self.destination.clone())
    }
}

impl Write for AtomicFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("Arquivo fechado"))?
            .write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("Arquivo fechado"))?
            .flush()
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_replacement_preserves_original_and_removes_only_temporary() {
        let directory =
            std::env::temp_dir().join(format!("lanvox-atomic-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("existing.txt");
        std::fs::write(&target, b"original").unwrap();
        let temporary;
        {
            let mut output = AtomicFile::new(&target).unwrap();
            temporary = output.temporary_path().to_owned();
            output.write_all(b"partial").unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), b"original");
        }
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        let mut output = AtomicFile::new(&target).unwrap();
        output.write_all(b"complete").unwrap();
        output.commit().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"complete");
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn simultaneous_downloads_use_distinct_temporaries() {
        let target =
            std::env::temp_dir().join(format!("lanvox-{:032x}.txt", rand::random::<u128>()));
        let first = AtomicFile::new(&target).unwrap();
        let mut second = AtomicFile::new(&target).unwrap();
        assert_ne!(first.temporary_path(), second.temporary_path());
        second.write_all(b"second").unwrap();
        drop(first);
        second.commit().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        std::fs::remove_file(target).unwrap();
    }
}
