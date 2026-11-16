//! Signed updates from GitHub Releases.
//!
//! Trust comes from one Ed25519 key, not from GitHub or TLS alone:
//! - Each release carries `update-manifest.json` and `update-manifest.json.sig`. The
//!   manifest names the installer, its SHA-256 and size; the signature covers the exact
//!   manifest bytes.
//! - The public key is compiled into the app ([`PUBLIC_KEY_HEX`]); the private key never
//!   leaves the maintainer's machine (`--update-keygen`, `--update-sign`).
//! - A manifest is rejected unless the signature verifies, it names this product, its
//!   installer URL is a release asset of [`REPO`], and its version is newer than the
//!   running one (no rollback to an old, signed but vulnerable build).
//! - The installer is downloaded only after that, and run only if its hash and size match
//!   the signed manifest.
//!
//! The app checks at startup and every five minutes while open (on by default, can be
//! turned off), downloads signed updates, and installs when no host or connection is active.

use std::io;
use std::path::{Path, PathBuf};

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};

/// GitHub repository that publishes releases (`owner/name`).
pub const REPO: &str = "victorlfdev/Lanvox";
/// Ed25519 public key (64 hex characters) that signs releases. Empty until the maintainer
/// runs `lanvox --update-keygen` and pastes the printed key here; with no key the app
/// refuses to update, never to trust unsigned files.
pub const PUBLIC_KEY_HEX: &str = "32123fc8816678d8d6df9f5c6cee985ac75dacca4d39dace91e30f5d64a57b0f";

pub const MANIFEST_FILE: &str = "update-manifest.json";
pub const SIGNATURE_FILE: &str = "update-manifest.json.sig";
const PRODUCT: &str = "lanvox";
/// Signatures are bound to this label so they cannot be replayed as another kind of data.
const DOMAIN: &[u8] = b"lanvox-update-v1\n";
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_SIGNATURE: u64 = 1024;
const MAX_INSTALLER: u64 = 300 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub product: String,
    pub version: String,
    /// Network protocol of this release; hosts and clients must match.
    pub protocol: u32,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub notes: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// `major.minor.patch` only: a pre-release or odd string is never treated as an update.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let mut next = || -> Option<u64> {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn allowed_url(url: &str) -> bool {
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    let Some(rest) = url.strip_prefix(&prefix) else {
        return false;
    };
    let mut parts = rest.split('/');
    let (Some(tag), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let safe = |text: &str| {
        !text.is_empty()
            && text != "."
            && text != ".."
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    };
    safe(tag) && safe(file) && file.ends_with(".exe")
}

fn signed_message(manifest: &[u8]) -> Vec<u8> {
    let mut message = DOMAIN.to_vec();
    message.extend_from_slice(manifest);
    message
}

/// Checks the signature first, then every field of the manifest. Newness is a separate
/// question ([`is_newer`]) so an old signed manifest reads as "up to date", not as an attack.
pub fn verify_manifest(
    manifest: &[u8],
    signature_hex: &str,
    public_key: &[u8],
) -> Result<Manifest, String> {
    let signature = unhex(signature_hex).ok_or("assinatura ilegível")?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&signed_message(manifest), &signature)
        .map_err(|_| "assinatura inválida: a atualização foi recusada".to_string())?;
    let parsed: Manifest =
        serde_json::from_slice(manifest).map_err(|e| format!("manifesto inválido ({e})"))?;
    if parsed.product != PRODUCT {
        return Err("o manifesto é de outro produto".into());
    }
    if parse_version(&parsed.version).is_none() {
        return Err("versão inválida no manifesto".into());
    }
    if !allowed_url(&parsed.url) {
        return Err("endereço do instalador fora do repositório oficial".into());
    }
    let hash_ok = parsed.sha256.len() == 64
        && parsed
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !hash_ok {
        return Err("hash inválido no manifesto".into());
    }
    if parsed.size == 0 || parsed.size > MAX_INSTALLER {
        return Err("tamanho inválido no manifesto".into());
    }
    Ok(parsed)
}

/// The installer must be exactly what the signed manifest described.
pub fn verify_installer(manifest: &Manifest, path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() != manifest.size {
        return Err("o instalador baixado tem tamanho diferente do assinado".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if sha256_hex(&bytes) != manifest.sha256 {
        return Err("o instalador baixado não corresponde ao hash assinado".into());
    }
    Ok(())
}

fn configured_key() -> Option<Vec<u8>> {
    unhex(PUBLIC_KEY_HEX).filter(|key| key.len() == 32)
}

/// True when this build carries a release key and can therefore verify updates.
pub fn configured() -> bool {
    configured_key().is_some()
}

// ---------------------------------------------------------------------------------------
// Maintainer tools (`--update-keygen`, `--update-sign`).

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == name)?;
    args.get(index + 1)
        .filter(|v| !v.starts_with("--"))
        .cloned()
}

/// Creates a new signing key at `path` (never overwrites) and returns the public key.
pub fn generate_key(path: &Path) -> io::Result<String> {
    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| invalid("sem aleatoriedade"))?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| invalid("chave inválida"))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    io::Write::write_all(&mut file, hex(pkcs8.as_ref()).as_bytes())?;
    Ok(hex(pair.public_key().as_ref()))
}

fn load_key(path: &Path) -> io::Result<Ed25519KeyPair> {
    let text = std::fs::read_to_string(path)?;
    let bytes = unhex(&text).ok_or_else(|| invalid("arquivo de chave inválido"))?;
    Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| invalid("arquivo de chave inválido"))
}

/// Builds and signs the manifest for `installer`, returning (manifest, signature) bytes.
pub fn sign_release(
    installer: &Path,
    key: &Ed25519KeyPair,
    version: &str,
    notes: &str,
) -> io::Result<(Vec<u8>, String)> {
    let file_name = installer
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("nome do instalador inválido"))?;
    let bytes = std::fs::read(installer)?;
    let manifest = Manifest {
        product: PRODUCT.into(),
        version: version.into(),
        protocol: crate::auth::PROTOCOL_VERSION,
        url: format!("https://github.com/{REPO}/releases/download/v{version}/{file_name}"),
        sha256: sha256_hex(&bytes),
        size: bytes.len() as u64,
        notes: notes.into(),
    };
    let json = serde_json::to_vec_pretty(&manifest)?;
    let signature = hex(key.sign(&signed_message(&json)).as_ref());
    // Refuse to emit something the app itself would reject.
    verify_manifest(&json, &signature, key.public_key().as_ref()).map_err(invalid)?;
    Ok((json, signature))
}

/// Handles `--update-keygen` and `--update-sign`; returns false when neither was given.
pub fn run_cli(args: &[String]) -> io::Result<bool> {
    if let Some(path) = flag(args, "--update-keygen") {
        let public = generate_key(Path::new(&path))?;
        println!("Chave privada gravada em {path}. Guarde-a fora do Git e do GitHub, com backup.");
        println!("Chave pública (cole em PUBLIC_KEY_HEX, app/src/updater.rs):\n{public}");
        return Ok(true);
    }
    if let Some(installer) = flag(args, "--update-sign") {
        let key = load_key(Path::new(
            &flag(args, "--update-key").ok_or_else(|| invalid("falta --update-key ARQUIVO"))?,
        ))?;
        let version =
            flag(args, "--update-version").unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        if parse_version(&version).is_none() {
            return Err(invalid("versão deve ser x.y.z"));
        }
        let notes = flag(args, "--update-notes").unwrap_or_default();
        let out = PathBuf::from(flag(args, "--out").unwrap_or_else(|| ".".into()));
        std::fs::create_dir_all(&out)?;
        let (manifest, signature) = sign_release(Path::new(&installer), &key, &version, &notes)?;
        std::fs::write(out.join(MANIFEST_FILE), manifest)?;
        std::fs::write(out.join(SIGNATURE_FILE), signature)?;
        println!(
            "Gerados {MANIFEST_FILE} e {SIGNATURE_FILE} em {}.\nAnexe os dois e o instalador à release v{version}.",
            out.display()
        );
        return Ok(true);
    }
    Ok(false)
}

// ---------------------------------------------------------------------------------------
// Client side.

#[cfg(feature = "gui")]
pub use client::*;

#[cfg(feature = "gui")]
mod client {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum State {
        Idle,
        Checking,
        UpToDate,
        Available(Manifest),
        Downloading(Manifest),
        Ready(Manifest, PathBuf),
        Installing,
        Failed(String),
    }

    static STATE: Mutex<State> = Mutex::new(State::Idle);

    pub fn state() -> State {
        STATE.lock().clone()
    }

    /// Pretends a release is available, for `--screenshot-view update`.
    pub fn show_demo() {
        set(State::Available(Manifest {
            product: PRODUCT.into(),
            version: "0.2.0".into(),
            protocol: crate::auth::PROTOCOL_VERSION,
            url: String::new(),
            sha256: String::new(),
            size: 0,
            notes: String::new(),
        }));
    }

    fn set(state: State) {
        *STATE.lock() = state;
    }

    fn work_dir() -> Result<PathBuf, String> {
        let dir = dirs::data_local_dir()
            .ok_or("pasta de dados indisponível")?
            .join(crate::branding::NAME)
            .join("updates");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    /// Downloads `url` into `dest`, refusing anything larger than `limit` bytes.
    #[cfg(windows)]
    fn fetch(url: &str, dest: &Path, limit: u64) -> Result<(), String> {
        use windows::core::PCWSTR;
        use windows::Win32::Networking::WinInet::DeleteUrlCacheEntryW;
        use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;

        let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() };
        let (wurl, wdest) = (wide(url), wide(&dest.to_string_lossy()));
        let _ = std::fs::remove_file(dest);
        // A cached copy of "latest" would hide a new release.
        unsafe {
            let _ = DeleteUrlCacheEntryW(PCWSTR(wurl.as_ptr()));
        }
        let result = unsafe {
            URLDownloadToFileW(None, PCWSTR(wurl.as_ptr()), PCWSTR(wdest.as_ptr()), 0, None)
        };
        let size = std::fs::metadata(dest).map(|m| m.len());
        if let Err(error) = result {
            let _ = std::fs::remove_file(dest);
            return Err(format!("download falhou ({error})"));
        }
        match size {
            Ok(size) if size <= limit => Ok(()),
            _ => {
                let _ = std::fs::remove_file(dest);
                Err("arquivo baixado grande demais".into())
            }
        }
    }

    #[cfg(not(windows))]
    fn fetch(_: &str, _: &Path, _: u64) -> Result<(), String> {
        Err("atualização automática disponível apenas no Windows".into())
    }

    fn fetch_text(url: &str, name: &str, limit: u64) -> Result<String, String> {
        let path = work_dir()?.join(format!("{name}.part"));
        fetch(url, &path, limit)?;
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&path);
        text
    }

    /// How often a running app looks again, when the person allowed update checks.
    const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

    static LAST_CHECK: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    /// Version already announced with a desktop notification, so it is announced once.
    static ANNOUNCED: Mutex<Option<String>> = Mutex::new(None);

    #[derive(Debug, PartialEq, Eq)]
    enum AutomaticAction {
        Wait,
        Download,
        Install,
    }

    fn automatic_action(state: &State, enabled: bool, can_restart: bool) -> AutomaticAction {
        match (enabled, can_restart, state) {
            (true, _, State::Available(_)) => AutomaticAction::Download,
            (true, true, State::Ready(..)) => AutomaticAction::Install,
            _ => AutomaticAction::Wait,
        }
    }

    /// Advances automatic updates without interrupting an active host or connection.
    pub fn poll(ctx: &egui::Context, enabled: bool, can_restart: bool) {
        if !enabled || !configured() {
            return;
        }
        let current = state();
        if matches!(current, State::Checking | State::Downloading(_)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if let State::Available(manifest) = &current {
            let mut announced = ANNOUNCED.lock();
            if announced.as_deref() != Some(manifest.version.as_str()) {
                crate::notifications::notify(
                    "Nova versão do Lanvox",
                    &format!(
                        "A versão {} será baixada e instalada automaticamente quando não houver sessão ativa.",
                        manifest.version
                    ),
                );
                *announced = Some(manifest.version.clone());
            }
        }
        match automatic_action(&current, enabled, can_restart) {
            AutomaticAction::Download => {
                download();
                ctx.request_repaint();
                return;
            }
            AutomaticAction::Install => {
                match launch_installer() {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(error) => set(State::Failed(error)),
                }
                ctx.request_repaint();
                return;
            }
            AutomaticAction::Wait => {}
        }
        if matches!(current, State::Ready(..) | State::Installing) {
            return;
        }
        let elapsed = LAST_CHECK.lock().map(|at| at.elapsed());
        match elapsed {
            Some(elapsed) if elapsed < RECHECK_EVERY => {
                ctx.request_repaint_after(RECHECK_EVERY - elapsed);
            }
            _ => {
                if matches!(state(), State::Idle | State::UpToDate | State::Failed(_)) {
                    check();
                    ctx.request_repaint_after(RECHECK_EVERY);
                }
            }
        }
    }

    /// Looks for a newer signed release in the background.
    pub fn check() {
        {
            let mut state = STATE.lock();
            if matches!(
                *state,
                State::Checking | State::Downloading(_) | State::Installing
            ) {
                return;
            }
            *state = State::Checking;
        }
        *LAST_CHECK.lock() = Some(std::time::Instant::now());
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(|| set(check_now()));
        if let Err(error) = spawned {
            set(State::Failed(error.to_string()));
        }
    }

    fn check_now() -> State {
        let Some(key) = configured_key() else {
            return State::Failed("esta versão não tem chave de atualização configurada".into());
        };
        let base = format!("https://github.com/{REPO}/releases/latest/download");
        let result = (|| -> Result<State, String> {
            let manifest = fetch_text(
                &format!("{base}/{MANIFEST_FILE}"),
                MANIFEST_FILE,
                MAX_MANIFEST,
            )?;
            let signature = fetch_text(
                &format!("{base}/{SIGNATURE_FILE}"),
                SIGNATURE_FILE,
                MAX_SIGNATURE,
            )?;
            let manifest = verify_manifest(manifest.as_bytes(), &signature, &key)?;
            Ok(if is_newer(&manifest.version, env!("CARGO_PKG_VERSION")) {
                State::Available(manifest)
            } else {
                State::UpToDate
            })
        })();
        result.unwrap_or_else(State::Failed)
    }

    /// Downloads the installer announced by the signed manifest and verifies it.
    pub fn download() {
        let manifest = {
            let mut state = STATE.lock();
            let State::Available(manifest) = &*state else {
                return;
            };
            let manifest = manifest.clone();
            *state = State::Downloading(manifest.clone());
            manifest
        };
        let spawned = std::thread::Builder::new()
            .name("update-download".into())
            .spawn(move || {
                let result = (|| -> Result<PathBuf, String> {
                    let dir = work_dir()?;
                    let part = dir.join(format!("Lanvox-{}-setup.exe.part", manifest.version));
                    let done = dir.join(format!("Lanvox-{}-setup.exe", manifest.version));
                    fetch(&manifest.url, &part, manifest.size)?;
                    if let Err(error) = verify_installer(&manifest, &part) {
                        let _ = std::fs::remove_file(&part);
                        return Err(error);
                    }
                    let _ = std::fs::remove_file(&done);
                    std::fs::rename(&part, &done).map_err(|e| e.to_string())?;
                    Ok(done)
                })();
                set(match result {
                    Ok(path) => State::Ready(manifest, path),
                    Err(error) => State::Failed(error),
                });
            });
        if let Err(error) = spawned {
            set(State::Failed(error.to_string()));
        }
    }

    /// Re-verifies the downloaded installer and starts it. The caller then closes the app
    /// so the installer can replace the files.
    pub fn launch_installer() -> Result<(), String> {
        let State::Ready(manifest, path) = state() else {
            return Err("nenhuma atualização pronta".into());
        };
        verify_installer(&manifest, &path)?;
        #[cfg(windows)]
        {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let install_dir = executable.parent().ok_or("pasta do app indisponível")?;
            std::process::Command::new(&path)
                .args([
                    "/SP-",
                    "/VERYSILENT",
                    "/SUPPRESSMSGBOXES",
                    "/NORESTART",
                    "/CLOSEAPPLICATIONS",
                    "/NORESTARTAPPLICATIONS",
                    "/LANVOXUPDATE=1",
                ])
                .arg(format!("/DIR={}", install_dir.display()))
                .spawn()
                .map(|_| set(State::Installing))
                .map_err(|e| format!("não foi possível abrir o instalador ({e})"))
        }
        #[cfg(not(windows))]
        {
            Err("atualização automática disponível apenas no Windows".into())
        }
    }

    #[cfg(test)]
    mod automatic_tests {
        use super::*;

        #[test]
        fn automatic_update_waits_for_sessions_and_respects_opt_out() {
            let manifest = Manifest {
                product: PRODUCT.into(),
                version: "9.9.9".into(),
                protocol: 1,
                url: String::new(),
                sha256: String::new(),
                size: 1,
                notes: String::new(),
            };
            let available = State::Available(manifest.clone());
            let ready = State::Ready(manifest.clone(), PathBuf::from("setup.exe"));
            assert_eq!(
                automatic_action(&available, true, false),
                AutomaticAction::Download
            );
            assert_eq!(automatic_action(&ready, true, false), AutomaticAction::Wait);
            assert_eq!(
                automatic_action(&ready, true, true),
                AutomaticAction::Install
            );
            for state in [&available, &ready] {
                assert_eq!(automatic_action(state, false, true), AutomaticAction::Wait);
            }
            for state in [
                State::Checking,
                State::Downloading(manifest),
                State::Installing,
                State::Failed("offline".into()),
                State::UpToDate,
                State::Idle,
            ] {
                assert_eq!(automatic_action(&state, true, true), AutomaticAction::Wait);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn installer(bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lanvox-upd-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Lanvox-9.9.9-windows-x64-setup.exe");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn signed(version: &str) -> (Vec<u8>, String, Ed25519KeyPair, PathBuf) {
        let key = key();
        let path = installer(b"installer bytes");
        let (manifest, signature) = sign_release(&path, &key, version, "notas").unwrap();
        (manifest, signature, key, path)
    }

    #[test]
    fn signed_release_round_trips() {
        let (manifest, signature, key, path) = signed("9.9.9");
        let parsed = verify_manifest(&manifest, &signature, key.public_key().as_ref()).unwrap();
        assert_eq!(parsed.version, "9.9.9");
        assert_eq!(parsed.protocol, crate::auth::PROTOCOL_VERSION);
        verify_installer(&parsed, &path).unwrap();
    }

    #[test]
    fn tampered_manifest_is_rejected() {
        let (manifest, signature, key, _) = signed("9.9.9");
        let text = String::from_utf8(manifest)
            .unwrap()
            .replace("9.9.9", "9.9.8");
        assert!(verify_manifest(text.as_bytes(), &signature, key.public_key().as_ref()).is_err());
    }

    #[test]
    fn another_key_is_rejected() {
        let (manifest, signature, _, _) = signed("9.9.9");
        assert!(verify_manifest(&manifest, &signature, self::key().public_key().as_ref()).is_err());
    }

    #[test]
    fn garbage_signature_is_rejected() {
        let (manifest, _, key, _) = signed("9.9.9");
        for bad in ["", "zz", "00", &"00".repeat(64)] {
            assert!(verify_manifest(&manifest, bad, key.public_key().as_ref()).is_err());
        }
    }

    #[test]
    fn signature_is_bound_to_this_use() {
        let key = key();
        let (manifest, _, _, _) = signed("9.9.9");
        // Signed without the domain label, as any other signed blob might be.
        let raw = hex(key.sign(&manifest).as_ref());
        assert!(verify_manifest(&manifest, &raw, key.public_key().as_ref()).is_err());
    }

    #[test]
    fn modified_installer_is_rejected() {
        let (manifest, signature, key, path) = signed("9.9.9");
        let parsed = verify_manifest(&manifest, &signature, key.public_key().as_ref()).unwrap();
        std::fs::write(&path, b"installer bytez").unwrap();
        assert!(verify_installer(&parsed, &path).is_err());
        std::fs::write(&path, b"short").unwrap();
        assert!(verify_installer(&parsed, &path).is_err());
    }

    #[test]
    fn only_newer_plain_versions_count() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"), "no rollback");
        assert!(!is_newer("1.0.0-beta", "0.1.0"));
        assert!(!is_newer("1.0", "0.1.0"));
        assert!(!is_newer("1.0.0.1", "0.1.0"));
        assert!(!is_newer("+1.0.0", "0.1.0"));
    }

    #[test]
    fn installer_url_must_be_an_official_release_asset() {
        let ok =
            format!("https://github.com/{REPO}/releases/download/v1.0.0/Lanvox-1.0.0-setup.exe");
        assert!(allowed_url(&ok));
        for bad in [
            "http://github.com/victorlfdev/Lanvox/releases/download/v1.0.0/a.exe",
            "https://github.com/other/Lanvox/releases/download/v1.0.0/a.exe",
            "https://github.com/victorlfdev/Lanvox/releases/download/v1.0.0/a.zip",
            "https://github.com/victorlfdev/Lanvox/releases/download/../a.exe",
            "https://github.com/victorlfdev/Lanvox/releases/download/v1.0.0/../a.exe",
            "https://github.com/victorlfdev/Lanvox/releases/download/v1.0.0/a.exe?x=1",
            "https://github.com/victorlfdev/Lanvox/releases/download/v1.0.0/a/b.exe",
            "https://github.com.evil.test/victorlfdev/Lanvox/releases/download/v1/a.exe",
            "https://github.com/victorlfdev/Lanvox/releases/download/v1.0.0/",
        ] {
            assert!(!allowed_url(bad), "{bad}");
        }
    }

    #[test]
    fn foreign_manifest_fields_are_rejected() {
        let key = key();
        let sign = |json: &str| hex(key.sign(&signed_message(json.as_bytes())).as_ref());
        let url = format!("https://github.com/{REPO}/releases/download/v1.0.0/a.exe");
        let good = format!(
            r#"{{"product":"lanvox","version":"1.0.0","protocol":5,"url":"{url}","sha256":"{}","size":10}}"#,
            "a".repeat(64)
        );
        assert!(verify_manifest(good.as_bytes(), &sign(&good), key.public_key().as_ref()).is_ok());
        for bad in [
            good.replace("lanvox", "other"),
            good.replace("1.0.0\"", "1.0\""),
            good.replace(&"a".repeat(64), "ABC"),
            good.replace("\"size\":10", "\"size\":0"),
            good.replace("\"size\":10", "\"size\":999999999999"),
            good.replace("\"size\":10", "\"size\":10,\"extra\":1"),
        ] {
            assert!(
                verify_manifest(bad.as_bytes(), &sign(&bad), key.public_key().as_ref()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn keygen_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("lanvox-key-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("update.key");
        let public = generate_key(&path).unwrap();
        assert_eq!(public.len(), 64);
        assert_eq!(hex(load_key(&path).unwrap().public_key().as_ref()), public);
        assert!(generate_key(&path).is_err());
    }

    #[test]
    fn no_key_means_no_updates() {
        // The committed build has no key until the maintainer sets one; that must disable
        // updating, not accept anything.
        if PUBLIC_KEY_HEX.is_empty() {
            assert!(!configured());
        }
    }
}
