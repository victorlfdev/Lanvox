//! At-rest credentials. DPAPI binds Windows records to the current user;
//! Unix records use owner-only permissions. Portable exports are explicit.

use std::io::{self, Read, Write};
use std::path::Path;

const MAGIC: &[u8] = b"HUDDLE-DPAPI-1\0";
const MAX_SECRET: usize = 1024 * 1024;
const TEMP_PREFIX: &str = ".secret-";

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take((MAX_SECRET + 4097) as u64)
        .read_to_end(&mut data)?;
    if data.len() > MAX_SECRET + 4096 {
        return Err(invalid("Arquivo de credencial excede o limite."));
    }
    unprotect(&data)
}

/// Creates `path` atomically: the record is fully written and synced to a
/// temporary file, then linked into place only if `path` does not exist. A crash
/// never leaves a torn credential at `path`.
pub fn write_new(path: &Path, plain: &[u8]) -> io::Result<()> {
    let protected = protect(plain)?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let temporary = parent.join(format!("{TEMP_PREFIX}{}.tmp", rand::random::<u64>()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(&protected)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    let result = match std::fs::hard_link(&temporary, path) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            // Filesystems without hard links: rename cannot overwrite on this path.
            if path.exists() {
                Err(io::Error::from(io::ErrorKind::AlreadyExists))
            } else {
                std::fs::rename(&temporary, path)
            }
        }
        other => other,
    };
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Removes temporaries left by an interrupted write. They only ever hold a
/// complete or partial copy of a record that was never published.
pub fn remove_stale_temporaries(directory: &Path, older_than: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(TEMP_PREFIX) && name.ends_with(".tmp")) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= older_than);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn replace(path: &Path, plain: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Pasta de credencial inválida."))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!("{TEMP_PREFIX}{}.tmp", rand::random::<u64>()));
    write_new(&temporary, plain)?;
    let result = std::fs::rename(&temporary, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Call only after validating the decoded credential's content. An unreadable
/// or invalid file must never be silently replaced by a generated identity.
pub fn migrate_validated(path: &Path, plain: &[u8]) -> io::Result<()> {
    #[cfg(windows)]
    {
        let current = std::fs::read(path)?;
        if !current.starts_with(MAGIC) {
            if current != plain {
                return Err(invalid("A credencial foi alterada durante a leitura."));
            }
            replace(path, plain)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = plain;
        if std::fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn protect(plain: &[u8]) -> io::Result<Vec<u8>> {
    if plain.len() > MAX_SECRET {
        return Err(invalid("Credencial excede o limite."));
    }
    #[cfg(windows)]
    {
        let mut result = MAGIC.to_vec();
        result.extend(dpapi(plain, false)?);
        Ok(result)
    }
    #[cfg(not(windows))]
    {
        Ok(plain.to_vec())
    }
}

fn unprotect(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if let Some(cipher) = bytes.strip_prefix(MAGIC) {
        #[cfg(windows)]
        {
            return dpapi(cipher, true);
        }
        #[cfg(not(windows))]
        {
            let _ = cipher;
            return Err(invalid("Credencial protegida pelo Windows. Importe uma exportação portátil na máquina de origem."));
        }
    }
    if bytes.len() > MAX_SECRET {
        return Err(invalid("Credencial excede o limite."));
    }
    Ok(bytes.to_vec())
}

#[cfg(windows)]
fn dpapi(input: &[u8], decrypt: bool) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let data = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr().cast_mut(),
    };
    // Kept from the previous name on purpose; see branding.rs.
    let entropy = b"Huddle private credentials v1";
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        if decrypt {
            CryptUnprotectData(
                &data,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &data,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,
            "Não foi possível abrir a credencial protegida para este usuário do Windows. Restaure uma exportação portátil."));
    }
    if output.pbData.is_null() {
        return Err(invalid("O Windows retornou uma credencial vazia."));
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    if result.len() > MAX_SECRET {
        return Err(invalid("Credencial decifrada excede o limite."));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_secret_round_trip_and_no_overwrite() {
        let path =
            std::env::temp_dir().join(format!("lanvox-secret-{}.key", rand::random::<u64>()));
        write_new(&path, b"private test credential").unwrap();
        assert_eq!(read(&path).unwrap(), b"private test credential");
        assert!(write_new(&path, b"replacement").is_err());
        #[cfg(windows)]
        assert!(!std::fs::read(&path)
            .unwrap()
            .windows(23)
            .any(|part| part == b"private test credential"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn migration_preserves_plaintext_value_and_corrupt_protection_is_rejected() {
        let path = std::env::temp_dir().join(format!(
            "lanvox-secret-migrate-{}.key",
            rand::random::<u64>()
        ));
        std::fs::write(&path, b"legacy key bytes").unwrap();
        migrate_validated(&path, b"legacy key bytes").unwrap();
        assert_eq!(read(&path).unwrap(), b"legacy key bytes");
        let corrupt = [MAGIC, b"invalid protected data"].concat();
        std::fs::write(&path, &corrupt).unwrap();
        assert!(read(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn write_new_leaves_no_temporaries_and_stale_ones_are_collected() {
        let dir = std::env::temp_dir().join(format!("lanvox-secret-dir-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("a.key");
        write_new(&path, b"one").unwrap();
        assert!(write_new(&path, b"two").is_err());
        assert_eq!(read(&path).unwrap(), b"one");
        std::fs::write(dir.join(".secret-123.tmp"), b"torn").unwrap();
        std::fs::write(dir.join("keep.key"), b"x").unwrap();
        remove_stale_temporaries(&dir, std::time::Duration::from_secs(3600));
        assert!(dir.join(".secret-123.tmp").exists());
        remove_stale_temporaries(&dir, std::time::Duration::ZERO);
        assert!(!dir.join(".secret-123.tmp").exists());
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
