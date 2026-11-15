//! OpenH264 from Cisco's own binary, downloaded straight from Cisco to this device.
//!
//! Cisco pays the H.264 patent royalties for its binary only when (1) the end user's
//! device downloads it separately, (2) the user can turn it on and off and (3) the place
//! where that switch lives shows [`ATTRIBUTION`]. Its license text must also be
//! reproduced where licensing information is shown ([`LICENSE`]). This module never
//! ships the library: it downloads it, checks its SHA-256 against Cisco's published
//! build and only then loads it. Without it, video falls back to JPEG.

use openh264::OpenH264API;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Text the license requires next to the on/off switch.
pub const ATTRIBUTION: &str = "OpenH264 Video Codec provided by Cisco Systems, Inc.";
/// Cisco's license for its binary, reproduced as its condition 4 requires.
pub const LICENSE: &str = include_str!("../../packaging/OPENH264_BINARY_LICENSE.txt");

struct Blob {
    file: &'static str,
    sha256: &'static str,
}

/// Cisco's 2.6.0 build for this platform, with the hash Cisco publishes for it.
#[cfg(all(windows, target_arch = "x86_64"))]
const BLOB: Option<Blob> = Some(Blob {
    file: "openh264-2.6.0-win64.dll",
    sha256: "2076cb5675ec6c1a4c70e7a2a322552f547b6eeed649d6dfcd9e02a543b24691",
});
#[cfg(not(all(windows, target_arch = "x86_64")))]
const BLOB: Option<Blob> = None;

const MAX_COMPRESSED: usize = 8 * 1024 * 1024;
const MAX_LIBRARY: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The user turned the codec off.
    Disabled,
    /// Not downloaded yet.
    Missing,
    Downloading,
    /// Downloaded, verified and usable.
    Ready,
    Failed(String),
    /// Cisco publishes no build this app knows for this platform.
    Unsupported,
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Disabled => "Desativado. Vídeo usa JPEG.".into(),
            Status::Missing => "Não instalado. Vídeo usa JPEG.".into(),
            Status::Downloading => "Baixando da Cisco…".into(),
            Status::Ready => "Instalado e verificado.".into(),
            Status::Failed(error) => format!("Falha ao instalar: {error}"),
            Status::Unsupported => "Indisponível nesta plataforma. Vídeo usa JPEG.".into(),
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<Status> = Mutex::new(Status::Missing);

pub fn status() -> Status {
    STATUS.lock().clone()
}

fn library_path(blob: &Blob) -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join(crate::branding::NAME)
            .join("codecs")
            .join(blob.file),
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verified(blob: &Blob, path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_LIBRARY)
        && std::fs::read(path).is_ok_and(|bytes| sha256_hex(&bytes) == blob.sha256)
}

/// Applies the user's choice: off, or on (verifying an existing copy or downloading it).
pub fn configure(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    let Some(blob) = BLOB.as_ref() else {
        *STATUS.lock() = Status::Unsupported;
        return;
    };
    if !enabled {
        let mut status = STATUS.lock();
        if *status != Status::Downloading {
            *status = Status::Disabled;
        }
        return;
    }
    match library_path(blob) {
        Some(path) if verified(blob, &path) => *STATUS.lock() = Status::Ready,
        Some(_) => download(),
        None => *STATUS.lock() = Status::Failed("pasta de dados indisponível".into()),
    }
}

/// Starts a background download unless one is running or the codec is off.
pub fn download() {
    let Some(blob) = BLOB.as_ref() else {
        return;
    };
    {
        let mut status = STATUS.lock();
        if *status == Status::Downloading || !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        *status = Status::Downloading;
    }
    let spawned = std::thread::Builder::new()
        .name("openh264-download".into())
        .spawn(move || {
            let result = library_path(blob)
                .ok_or_else(|| "pasta de dados indisponível".to_string())
                .and_then(|path| fetch(blob, &path));
            *STATUS.lock() = match result {
                Ok(()) if ENABLED.load(Ordering::Relaxed) => Status::Ready,
                Ok(()) => Status::Disabled,
                Err(error) => {
                    eprintln!("OpenH264: {error}");
                    Status::Failed(error)
                }
            };
        });
    if let Err(error) = spawned {
        *STATUS.lock() = Status::Failed(error.to_string());
    }
}

#[cfg(windows)]
fn fetch(blob: &Blob, path: &std::path::Path) -> Result<(), String> {
    use std::io::Read;
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;

    let dir = path.parent().ok_or("caminho inválido")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let compressed_path = dir.join(format!("{}.bz2.part", blob.file));
    let url = format!("https://ciscobinary.openh264.org/{}.bz2", blob.file);
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() };
    let (wurl, wpath) = (wide(&url), wide(&compressed_path.to_string_lossy()));
    let downloaded =
        unsafe { URLDownloadToFileW(None, PCWSTR(wurl.as_ptr()), PCWSTR(wpath.as_ptr()), 0, None) };
    let compressed = std::fs::read(&compressed_path);
    let _ = std::fs::remove_file(&compressed_path);
    downloaded.map_err(|e| format!("download falhou ({e})"))?;
    let compressed = compressed.map_err(|e| e.to_string())?;
    if compressed.len() > MAX_COMPRESSED {
        return Err("arquivo baixado grande demais".into());
    }
    let mut library = Vec::new();
    bzip2::read::BzDecoder::new(compressed.as_slice())
        .take(MAX_LIBRARY + 1)
        .read_to_end(&mut library)
        .map_err(|e| format!("arquivo corrompido ({e})"))?;
    if library.len() as u64 > MAX_LIBRARY || sha256_hex(&library) != blob.sha256 {
        return Err("o arquivo não corresponde ao binário publicado pela Cisco".into());
    }
    let partial = dir.join(format!("{}.part", blob.file));
    std::fs::write(&partial, &library).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, path).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        e.to_string()
    })
}

#[cfg(not(windows))]
fn fetch(_: &Blob, _: &std::path::Path) -> Result<(), String> {
    Err("download indisponível nesta plataforma".into())
}

/// Placeholder for a stream with no picture yet; mentions the codec when it is missing,
/// because H.264 streams from others cannot be shown without it.
pub fn waiting_text() -> &'static str {
    if status() == Status::Ready {
        "Aguardando transmissão…"
    } else {
        "Aguardando transmissão…
Sem o codec H.264 (Configurações › Vídeo), alguns vídeos não aparecem."
    }
}

/// The library to create encoders and decoders with, or `None` when video must use JPEG.
pub fn api() -> Option<OpenH264API> {
    #[cfg(test)]
    {
        Some(OpenH264API::from_source())
    }
    #[cfg(not(test))]
    {
        if !ENABLED.load(Ordering::Relaxed) || *STATUS.lock() != Status::Ready {
            return None;
        }
        let path = library_path(BLOB.as_ref()?)?;
        // SAFETY: the file at `path` was checked against Cisco's published SHA-256 when the
        // status became Ready, and it lives in this user's own data folder.
        unsafe { OpenH264API::from_blob_path_unchecked(path) }.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_check_rejects_other_files() {
        let blob = Blob {
            file: "x.dll",
            sha256: "2076cb5675ec6c1a4c70e7a2a322552f547b6eeed649d6dfcd9e02a543b24691",
        };
        let path = std::env::temp_dir().join(format!("lanvox-h264-{}.dll", rand::random::<u64>()));
        std::fs::write(&path, b"not cisco").unwrap();
        assert!(!verified(&blob, &path));
        std::fs::remove_file(&path).unwrap();
        assert!(!verified(&blob, &path), "a missing file is not verified");
    }

    /// Downloads Cisco's real binary, so it needs the network: `cargo test -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn cisco_binary_downloads_verifies_and_encodes() {
        use openh264::encoder::{Encoder, EncoderConfig};
        use openh264::formats::{RgbaSliceU8, YUVBuffer};
        let blob = BLOB.as_ref().unwrap();
        let dir = std::env::temp_dir().join(format!("lanvox-cisco-{}", rand::random::<u64>()));
        let path = dir.join(blob.file);
        fetch(blob, &path).unwrap();
        assert!(verified(blob, &path));
        let api = OpenH264API::from_blob_path(&path).unwrap();
        let mut encoder = Encoder::with_api_config(api, EncoderConfig::new()).unwrap();
        let pixels = vec![128u8; 64 * 64 * 4];
        let yuv = YUVBuffer::from_rgba8_source(RgbaSliceU8::new(&pixels, (64, 64)));
        assert!(!encoder.encode(&yuv).unwrap().to_vec().is_empty());
        drop(encoder);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn license_text_is_reproduced() {
        assert!(LICENSE.contains(ATTRIBUTION));
        assert!(LICENSE.contains("Patent Portfolio License Conditions"));
    }
}
