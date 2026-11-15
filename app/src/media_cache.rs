//! Disk cache for large videos and audio opened from chat. The file is written piece by piece
//! as it is downloaded and played straight from disk, so a 100 MiB video never has to sit in
//! (several copies of) memory. The folder is emptied every time the app starts.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Cached media above this total is evicted, oldest first.
const MAX_CACHE_BYTES: u64 = 600 * 1024 * 1024;

fn directory() -> PathBuf {
    std::env::temp_dir().join("lanvox-media")
}

/// Removes whatever a previous run left behind and makes sure the folder exists.
pub fn reset() {
    let dir = directory();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
}

/// Where the attachment `id` named `name` is cached. The original extension is kept because
/// the player uses it to recognise the container.
pub fn path_for(id: u64, name: &str) -> PathBuf {
    let extension: String = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(8)
                .collect()
        })
        .unwrap_or_default();
    let file = if extension.is_empty() {
        format!("{id}")
    } else {
        format!("{id}.{extension}")
    };
    directory().join(file)
}

/// Deletes the oldest cached files until the cache fits, never touching `keep` (files that are
/// playing or being downloaded). Files the system refuses to delete are simply left alone.
pub fn trim(keep: &HashSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory()) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| {
                (
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    meta.len(),
                    e.path(),
                )
            })
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, len, path) in files {
        if total <= MAX_CACHE_BYTES {
            break;
        }
        if !keep.contains(&path) && std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_paths_keep_a_sanitised_extension_and_never_escape_the_folder() {
        let plain = path_for(7, "clip.MP4");
        assert_eq!(plain.file_name().unwrap(), "7.MP4");
        assert_eq!(plain.parent().unwrap(), directory());
        let hostile = path_for(8, "..\\..\\evil.m/p4");
        assert_eq!(hostile.parent().unwrap(), directory());
        assert!(path_for(9, "no-extension").file_name().unwrap() == "9");
    }
}
