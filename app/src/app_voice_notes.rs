//! Private, durable voice drafts. Navigation stops capture without discarding audio.
use super::*;
use std::io::Write;

pub(super) fn draft_key(identity: &str, host: &str, channel: ChannelId) -> String {
    let scope = format!(
        "{identity}|{}|{channel}",
        crate::settings::host_address(host)
    );
    ring::digest::digest(&ring::digest::SHA256, scope.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn draft_path(key: &str) -> std::io::Result<PathBuf> {
    let directory = dirs::data_local_dir()
        .ok_or_else(|| std::io::Error::other("Pasta de dados indisponível"))?
        .join("Lanvox")
        .join("voice-drafts");
    std::fs::create_dir_all(&directory)?;
    Ok(directory.join(format!("{key}.wav")))
}

impl LanvoxApp {
    pub fn save_voice_preview(&mut self, note: &VoiceNote) {
        let Some(bytes) = &note.bytes else { return };
        let key = draft_key(&note.identity, &note.host, note.channel);
        self.voice_drafts.insert(key.clone(), bytes.clone());
        self.voice_drafts_loaded.insert(key.clone());
        if self.demo {
            return;
        }
        let result = (|| -> std::io::Result<()> {
            let mut file = crate::file_transfer::AtomicFile::new(&draft_path(&key)?)?;
            file.write_all(bytes)?;
            file.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            self.notify_error(format!(
                "Áudio preservado nesta sessão; não foi possível salvar no dispositivo: {error}"
            ));
        } else {
            // Keep only unsaved drafts in RAM. Saved recordings are loaded lazily from disk.
            self.voice_drafts.remove(&key);
            self.voice_drafts_loaded.remove(&key);
        }
    }

    pub fn preserve_voice_note(&mut self) {
        let Some(mut note) = self.voice_note.take() else {
            return;
        };
        note.preview = None;
        if let Some(recorder) = note.recorder.take() {
            match recorder.finish() {
                Ok(bytes) => note.bytes = Some(bytes),
                Err(error) => {
                    self.notify(error);
                    return;
                }
            }
        }
        self.save_voice_preview(&note);
    }

    pub fn restore_voice_note(&mut self, channel: ChannelId) {
        if self.voice_note.is_some() {
            return;
        }
        let host = self.current_host();
        let key = draft_key(&self.local_identity, &host, channel);
        if self.voice_drafts_loaded.insert(key.clone()) && !self.demo {
            if let Ok(path) = draft_path(&key) {
                // A two-minute mono recording is under 12 MiB. Never load unbounded files.
                if std::fs::metadata(&path).is_ok_and(|m| m.len() <= 12 * 1024 * 1024) {
                    if let Ok(bytes) = std::fs::read(path) {
                        if bytes.starts_with(b"RIFF") && bytes.len() > 44 {
                            self.voice_drafts.insert(key.clone(), bytes);
                        }
                    }
                }
            }
        }
        if let Some(bytes) = self.voice_drafts.get(&key) {
            self.voice_note = Some(VoiceNote {
                identity: self.local_identity.clone(),
                host,
                channel,
                recorder: None,
                bytes: Some(bytes.clone()),
                preview: None,
            });
        }
    }

    pub fn remove_voice_draft(&mut self, note: &VoiceNote) {
        let key = draft_key(&note.identity, &note.host, note.channel);
        self.voice_drafts.remove(&key);
        self.voice_drafts_loaded.insert(key.clone());
        if !self.demo {
            if let Ok(path) = draft_path(&key) {
                if let Err(error) = std::fs::remove_file(path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        self.notify(format!(
                            "Não foi possível remover o rascunho de áudio: {error}"
                        ));
                    }
                }
            }
        }
    }
}
