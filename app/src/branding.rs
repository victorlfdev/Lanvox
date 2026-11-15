//! Product name, invitation scheme and the move away from the previous name.
//!
//! The app was called "Huddle" before the rename to Lanvox. Old invitations (`huddle://`), environment
//! variables (`HUDDLE_*`) and data folders are still accepted so nobody loses access.
//! Byte strings used for protocol framing, signatures and encrypted files keep their
//! original values on purpose: changing them would break existing hosts, identities
//! and stored secrets.

use std::path::Path;

/// Name shown to people and used for data folders.
pub const NAME: &str = "Lanvox";
/// Folder name of the standalone host's default data directory.
pub const HOST_DIR: &str = "LanvoxHost";
/// Scheme of invitation and message links.
pub const SCHEME: &str = "lanvox://";

const LEGACY_SCHEME: &str = "huddle://";
const LEGACY_NAME: &str = "Huddle";
const LEGACY_HOST_DIR: &str = "HuddleHost";
const ENV_PREFIX: &str = "LANVOX_";
const LEGACY_ENV_PREFIX: &str = "HUDDLE_";

/// The part of a link after its scheme, for the current and the previous scheme.
pub fn strip_scheme(link: &str) -> Option<&str> {
    link.strip_prefix(SCHEME)
        .or_else(|| link.strip_prefix(LEGACY_SCHEME))
}

/// True when `link` uses the current or the previous scheme.
pub fn is_link(link: &str) -> bool {
    strip_scheme(link).is_some()
}

/// Reads `LANVOX_*`, falling back to the `HUDDLE_*` variable of the previous name.
pub fn env_var(name: &str) -> Option<String> {
    let non_empty = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    non_empty(name).or_else(|| {
        name.strip_prefix(ENV_PREFIX)
            .and_then(|rest| non_empty(&format!("{LEGACY_ENV_PREFIX}{rest}")))
    })
}

/// True when `LANVOX_*` (or its `HUDDLE_*` predecessor) is set at all.
pub fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some()
        || name
            .strip_prefix(ENV_PREFIX)
            .is_some_and(|rest| std::env::var_os(format!("{LEGACY_ENV_PREFIX}{rest}")).is_some())
}

/// Renames the previous name's data folders, once, before anything opens them.
/// A folder is moved only when the new one does not exist yet; failures leave the old
/// folder in place and are reported, so the data is never lost.
pub fn migrate_legacy_folders() {
    let roots = [dirs::config_dir(), dirs::data_local_dir()];
    for root in roots.iter().flatten() {
        migrate(&root.join(LEGACY_NAME), &root.join(NAME));
    }
    if let Some(config) = dirs::config_dir() {
        migrate(&config.join(LEGACY_HOST_DIR), &config.join(HOST_DIR));
    }
}

fn migrate(old: &Path, new: &Path) {
    if new.exists() || !old.is_dir() {
        return;
    }
    if let Err(error) = std::fs::rename(old, new) {
        eprintln!(
            "{NAME}: could not move {} to {}: {error}",
            old.display(),
            new.display()
        );
    }
}

/// Shown wherever someone joins a host or opens a private conversation. The host stores the
/// messages in the clear for its administrator: there is no end-to-end encryption.
pub const ADMIN_VISIBILITY_NOTICE: &str = "O administrador do host pode ler mensagens, anexos e conversas privadas armazenados nele. Não há criptografia de ponta a ponta que o exclua. O Noise protege apenas o caminho entre você e o host.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_schemes_are_accepted() {
        assert_eq!(strip_scheme("lanvox://host:1"), Some("host:1"));
        assert_eq!(strip_scheme("huddle://host:1"), Some("host:1"));
        assert_eq!(strip_scheme("https://host"), None);
    }

    #[test]
    fn migration_moves_only_when_target_is_missing() {
        let root = std::env::temp_dir().join(format!("lanvox-migrate-{}", rand::random::<u64>()));
        let old = root.join("old");
        let new = root.join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("identity.key"), b"x").unwrap();
        migrate(&old, &new);
        assert!(new.join("identity.key").exists());
        assert!(!old.exists());

        std::fs::create_dir_all(&old).unwrap();
        migrate(&old, &new);
        assert!(old.exists(), "an existing target is never overwritten");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
