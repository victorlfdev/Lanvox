use super::*;

/// Draft attachments are scoped to the authenticated identity and host. The filename is an
/// operation generated locally, never an attachment name received from another participant.
fn draft_directory(identity: &str, host: &str) -> std::io::Result<std::path::PathBuf> {
    use ring::digest::{digest, SHA256};
    let root = dirs::data_local_dir()
        .ok_or_else(|| std::io::Error::other("Pasta de dados indisponível"))?
        .join("Lanvox")
        .join("attachment-drafts");
    let scope = format!("{identity}|{}", crate::settings::host_address(host));
    let hash = digest(&SHA256, scope.as_bytes());
    let name = hash
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok(root.join(name))
}

fn draft_path(identity: &str, host: &str, operation: &str) -> std::io::Result<std::path::PathBuf> {
    if !operation.starts_with("upload-")
        || operation.len() != 39
        || !operation[7..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(std::io::Error::other("Identificador de anexo inválido"));
    }
    Ok(draft_directory(identity, host)?.join(format!("{operation}.draft")))
}

fn write_draft(
    identity: &str,
    host: &str,
    channel: ChannelId,
    file: &StagedAttachment,
) -> std::io::Result<()> {
    use std::io::Write;
    let path = draft_path(identity, host, &file.operation)?;
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| std::io::Error::other("Pasta inválida"))?,
    )?;
    let metadata = serde_json::to_vec(&(channel, &file.name)).map_err(std::io::Error::other)?;
    let temp = path.with_extension("partial");
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    out.write_all(&(metadata.len() as u32).to_le_bytes())?;
    out.write_all(&metadata)?;
    out.write_all(&file.bytes)?;
    out.sync_all()?;
    drop(out);
    std::fs::rename(temp, path)
}

impl LanvoxApp {
    pub fn stage_attachment(&mut self, channel: ChannelId, name: String, bytes: Vec<u8>) {
        if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
            self.notify("O arquivo está vazio ou ultrapassa o limite de 100 MiB.");
            return;
        }
        let staged_bytes: usize = self
            .workspace_ui
            .staged
            .values()
            .flatten()
            .map(|f| f.bytes.len())
            .sum();
        if staged_bytes + bytes.len() > 2 * MAX_ATTACHMENT_BYTES {
            self.notify("Muitos arquivos grandes aguardando envio. Envie os atuais primeiro.");
            return;
        }
        let total = self.workspace_ui.staged.get(&channel).map_or(0, Vec::len)
            + self
                .workspace_ui
                .attachments
                .get(&channel)
                .map_or(0, Vec::len)
            + self
                .workspace_ui
                .uploads
                .values()
                .filter(|ch| **ch == channel)
                .count();
        if total >= 10 {
            self.notify("Anexe até 10 arquivos por mensagem.");
            return;
        }
        let file = StagedAttachment {
            operation: format!("upload-{:032x}", rand::random::<u128>()),
            name,
            bytes,
            preview: None,
        };
        if let Err(error) = write_draft(&self.local_identity, &self.current_host(), channel, &file)
        {
            self.notify(format!(
                "Não foi possível preservar o anexo neste dispositivo: {error}"
            ));
            return;
        }
        self.workspace_ui
            .staged
            .entry(channel)
            .or_default()
            .push(file);
    }

    pub fn remove_attachment_draft(&self, operation: &str) {
        if let Ok(path) = draft_path(&self.local_identity, &self.current_host(), operation) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn clear_sent_attachment_drafts(&self, channel: ChannelId) {
        use std::io::Read;
        let Ok(dir) = draft_directory(&self.local_identity, &self.current_host()) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(operation) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|s| s.to_str()) != Some("draft")
                || draft_path(&self.local_identity, &self.current_host(), operation).is_err()
            {
                continue;
            }
            let Ok(mut source) = std::fs::File::open(&path) else {
                continue;
            };
            let mut header = [0; 4];
            if source.read_exact(&mut header).is_err() {
                continue;
            }
            let len = u32::from_le_bytes(header) as usize;
            if len > 8192 {
                continue;
            }
            let mut metadata = vec![0; len];
            if source.read_exact(&mut metadata).is_ok()
                && serde_json::from_slice::<(ChannelId, String)>(&metadata)
                    .is_ok_and(|(id, _)| id == channel)
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// Restore only this identity's drafts. Bounds protect memory even if files were modified.
    pub fn restore_attachment_drafts(&mut self) {
        use std::io::Read;
        let Ok(dir) = draft_directory(&self.local_identity, &self.current_host()) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut total: usize = self
            .workspace_ui
            .staged
            .values()
            .flatten()
            .map(|f| f.bytes.len())
            .sum();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(operation) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
            else {
                continue;
            };
            if path.extension().and_then(|s| s.to_str()) != Some("draft")
                || draft_path(&self.local_identity, &self.current_host(), &operation).is_err()
                || self
                    .workspace_ui
                    .staged
                    .values()
                    .flatten()
                    .any(|f| f.operation == operation)
                || self.workspace_ui.upload_sources.contains_key(&operation)
            {
                continue;
            }
            let Ok(mut source) = std::fs::File::open(&path) else {
                continue;
            };
            let Ok(meta) = source.metadata() else {
                continue;
            };
            if meta.len() > MAX_ATTACHMENT_BYTES as u64 + 8196 {
                continue;
            }
            let mut header = [0; 4];
            if source.read_exact(&mut header).is_err() {
                continue;
            }
            let len = u32::from_le_bytes(header) as usize;
            if len > 8192 {
                continue;
            }
            let mut metadata = vec![0; len];
            if source.read_exact(&mut metadata).is_err() {
                continue;
            }
            let Ok((channel, name)) = serde_json::from_slice::<(ChannelId, String)>(&metadata)
            else {
                continue;
            };
            let mut bytes = Vec::new();
            if source.read_to_end(&mut bytes).is_err()
                || bytes.is_empty()
                || bytes.len() > MAX_ATTACHMENT_BYTES
                || total.saturating_add(bytes.len()) > 2 * MAX_ATTACHMENT_BYTES
                || self
                    .workspace_ui
                    .staged
                    .get(&channel)
                    .is_some_and(|files| files.len() >= 10)
            {
                continue;
            }
            total += bytes.len();
            self.workspace_ui
                .staged
                .entry(channel)
                .or_default()
                .push(StagedAttachment {
                    operation,
                    name,
                    bytes,
                    preview: None,
                });
        }
    }

    pub fn upload_staged(&mut self, channel: ChannelId, index: usize) {
        let Some(list) = self.workspace_ui.staged.get_mut(&channel) else {
            return;
        };
        if index >= list.len() {
            return;
        }
        let file = list.remove(index);
        let operation = file.operation.clone();
        let extension = file
            .name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let media_type = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "ogg" => "audio/ogg",
            "txt" | "md" => "text/plain",
            "pdf" => "application/pdf",
            _ => "application/octet-stream",
        }
        .into();
        self.workspace_ui.uploads.insert(operation.clone(), channel);
        self.workspace_ui
            .upload_names
            .insert(operation.clone(), file.name.clone());
        if file.bytes.len() > MAX_INLINE_ATTACHMENT_BYTES {
            self.workspace_ui.chunk_uploads.insert(
                operation.clone(),
                ChunkUpload {
                    total: file.bytes.len() as u64,
                    sent: 0,
                    acked: 0,
                    finishing: false,
                },
            );
            let hash = ring::digest::digest(&ring::digest::SHA256, &file.bytes);
            let mut digest = [0; 32];
            digest.copy_from_slice(hash.as_ref());
            self.workspace(WorkspaceCommand::UploadBeginResumable {
                operation: operation.clone(),
                channel,
                name: file.name.clone(),
                media_type,
                size: file.bytes.len() as u64,
                digest,
            });
        } else {
            self.workspace(WorkspaceCommand::Upload {
                operation: operation.clone(),
                channel,
                name: file.name.clone(),
                media_type,
                bytes: file.bytes.clone(),
            });
        }
        self.workspace_ui.upload_sources.insert(operation, file);
    }

    /// Sends the next chunks of `operation` while the window allows, then the finish marker.
    pub(super) fn pump_chunks(&mut self, operation: &str) {
        let Some(state) = self.workspace_ui.chunk_uploads.get_mut(operation) else {
            return;
        };
        let Some(file) = self.workspace_ui.upload_sources.get(operation) else {
            return;
        };
        let window = UPLOAD_WINDOW_CHUNKS * ATTACHMENT_CHUNK_BYTES as u64;
        let mut commands = Vec::new();
        while state.sent < state.total && state.sent - state.acked < window {
            let start = state.sent as usize;
            let end = (start + ATTACHMENT_CHUNK_BYTES).min(file.bytes.len());
            commands.push(WorkspaceCommand::UploadChunk {
                operation: operation.to_string(),
                offset: state.sent,
                data: file.bytes[start..end].to_vec(),
            });
            state.sent = end as u64;
        }
        if state.acked == state.total && !state.finishing {
            state.finishing = true;
            commands.push(WorkspaceCommand::UploadFinish {
                operation: operation.to_string(),
            });
        }
        for command in commands {
            self.send_file(ClientMsg::Workspace(command));
        }
    }

    /// Fraction (0..=1) of a chunked upload the host has confirmed.
    pub fn upload_fraction(&self, operation: &str) -> Option<f32> {
        self.workspace_ui
            .chunk_uploads
            .get(operation)
            .map(|c| c.acked as f32 / c.total.max(1) as f32)
    }

    pub fn cancel_upload(&mut self, operation: String) {
        self.remove_attachment_draft(&operation);
        if self.workspace_ui.chunk_uploads.remove(&operation).is_some() {
            // Chunked uploads are paced by the host's acknowledgements, so the slot can be
            // freed right away and the host told to drop what it already received.
            self.workspace(WorkspaceCommand::UploadAbort {
                operation: operation.clone(),
            });
            if let Some(channel) = self.workspace_ui.uploads.remove(&operation) {
                self.workspace_ui.send_after_upload.remove(&channel);
                self.workspace_ui.pending_batches.remove(&channel);
            }
            self.workspace_ui.upload_names.remove(&operation);
            self.workspace_ui.upload_sources.remove(&operation);
            return;
        }
        // Keep the in-flight slot until its ACK/error: removing it here would
        // allow another large upload to overfill the bounded transport queue.
        if let Some(channel) = self.workspace_ui.uploads.get(&operation).copied() {
            self.workspace_ui.send_after_upload.remove(&channel);
            self.workspace_ui.pending_batches.remove(&channel);
        }
        self.workspace_ui.upload_sources.remove(&operation);
        self.workspace_ui.cancelled_uploads.insert(operation);
    }

    pub fn begin_attachment_batch(&mut self, channel: ChannelId) {
        if self.workspace_ui.pending_batches.contains_key(&channel) {
            return;
        }
        self.workspace_ui.pending_batches.insert(
            channel,
            PendingBatch {
                operation: format!("{:032x}", rand::random::<u128>()),
                text: self.compose.get(&channel).cloned().unwrap_or_default(),
                reply_to: self.workspace_ui.reply_to,
            },
        );
        self.workspace_ui.send_after_upload.insert(channel);
        self.pump_uploads();
    }

    pub(super) fn pump_uploads(&mut self) {
        if !self.workspace_ui.uploads.is_empty()
            || !self.client.as_ref().is_some_and(|c| c.is_alive())
        {
            return;
        }
        let mut channels: Vec<_> = self
            .workspace_ui
            .send_after_upload
            .iter()
            .copied()
            .collect();
        channels.sort_unstable();
        for channel in channels {
            if self
                .workspace_ui
                .staged
                .get(&channel)
                .is_some_and(|files| !files.is_empty())
            {
                self.upload_staged(channel, 0);
                return;
            }
            self.workspace_ui.send_after_upload.remove(&channel);
            self.finish_attachment_batch(channel);
        }
    }

    pub(super) fn finish_attachment_batch(&mut self, channel: ChannelId) {
        let Some(batch) = self.workspace_ui.pending_batches.remove(&channel) else {
            return;
        };
        let text = match &self.view {
            Some(view) => {
                crate::mentions::encode(batch.text.trim(), &view.channel_members(channel))
            }
            None => batch.text.trim().into(),
        };
        let attachments = self
            .workspace_ui
            .attachments
            .get(&channel)
            .map(|files| files.iter().map(|f| f.id).collect())
            .unwrap_or_default();
        let pending = crate::settings::PendingMessage {
            operation: batch.operation.clone(),
            host: self.current_host(),
            identity: self.local_identity.clone(),
            channel,
            text,
            reply_to: batch.reply_to,
            attachments,
            error: None,
            attempted_at: now_secs(),
        };
        self.settings.pending.push(pending.clone());
        if let Err(error) = self.settings.try_save() {
            self.settings.pending.pop();
            self.notify(format!("Não foi possível preservar o envio: {error}. Os arquivos e o texto continuam aqui."));
            return;
        }
        self.clear_sent_attachment_drafts(channel);
        self.send_pending(&pending);
        if self.compose.get(&channel) == Some(&batch.text) {
            self.compose.insert(channel, String::new());
        }
        if self.selected == Some(channel) && self.workspace_ui.reply_to == batch.reply_to {
            self.workspace_ui.reply_to = None;
        }
        self.workspace_ui.attachments.remove(&channel);
        self.save_drafts();
        self.scroll_to_bottom = 3;
    }

    pub fn stage_paths(&mut self, channel: ChannelId, paths: Vec<std::path::PathBuf>) {
        if self.file_pending.is_some() {
            self.notify("Aguarde o arquivo atual antes de anexar outro.");
            return;
        }
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        std::thread::spawn(move || {
            let mut files = Vec::new();
            for path in paths.into_iter().take(10) {
                let task = (|| -> Result<(String, Vec<u8>), String> {
                    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                    if !meta.is_file() || meta.len() > MAX_ATTACHMENT_BYTES as u64 {
                        return Err("Escolha arquivos de até 100 MiB.".into());
                    }
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    if bytes.len() > MAX_ATTACHMENT_BYTES {
                        return Err("O arquivo ultrapassa 100 MiB.".into());
                    }
                    Ok((
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        bytes,
                    ))
                })();
                match task {
                    Ok(file) => files.push(file),
                    Err(error) => {
                        let _ = tx.send(FileTask::Error(error));
                        return;
                    }
                }
            }
            let _ = tx.send(FileTask::Uploads { channel, files });
        });
    }

    pub fn paste_image(&mut self, channel: ChannelId) {
        if self.file_pending.is_some() {
            return;
        }
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        std::thread::spawn(move || {
            let task = (|| -> Option<FileTask> {
                let mut clipboard = arboard::Clipboard::new().ok()?;
                let img = clipboard.get_image().ok()?;
                let pixels = img.width.checked_mul(img.height)?;
                if pixels > 16 * 1024 * 1024 || img.bytes.len() != pixels.checked_mul(4)? {
                    return Some(FileTask::Error(
                        "A imagem da área de transferência é grande demais.".into(),
                    ));
                }
                let rgba = image::RgbaImage::from_raw(
                    u32::try_from(img.width).ok()?,
                    u32::try_from(img.height).ok()?,
                    img.bytes.into_owned(),
                )?;
                let mut out = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(rgba)
                    .write_to(&mut out, image::ImageFormat::Png)
                    .ok()?;
                Some(FileTask::Upload {
                    channel,
                    name: format!("captura-{}.png", now_secs()),
                    bytes: out.into_inner(),
                })
            })()
            .unwrap_or(FileTask::Cancelled);
            let _ = tx.send(task);
        });
    }

    pub fn pick_attachment(&mut self, channel: ChannelId) {
        if self.file_pending.is_some() {
            return;
        }
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        std::thread::spawn(move || {
            let task = match rfd::FileDialog::new()
                .set_title("Anexar arquivo (até 100 MiB)")
                .pick_file()
            {
                Some(path) => match std::fs::metadata(&path) {
                    Ok(meta) if meta.len() <= MAX_ATTACHMENT_BYTES as u64 => {
                        match std::fs::read(&path) {
                            Ok(bytes) if bytes.len() <= MAX_ATTACHMENT_BYTES => FileTask::Upload {
                                channel,
                                name: path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                bytes,
                            },
                            Ok(_) => FileTask::Error("O arquivo ultrapassa 100 MiB.".into()),
                            Err(e) => {
                                FileTask::Error(format!("Não foi possível ler o arquivo: {e}"))
                            }
                        }
                    }
                    Ok(_) => FileTask::Error("O arquivo ultrapassa 100 MiB.".into()),
                    Err(e) => FileTask::Error(format!("Não foi possível abrir o arquivo: {e}")),
                },
                None => FileTask::Cancelled,
            };
            let _ = tx.send(task);
        });
    }
}
