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

fn validate_operation(operation: &str) -> std::io::Result<()> {
    if !operation.starts_with("upload-")
        || operation.len() != 39
        || !operation[7..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(std::io::Error::other("Identificador de anexo inválido"));
    }
    Ok(())
}

fn draft_path(identity: &str, host: &str, operation: &str) -> std::io::Result<std::path::PathBuf> {
    validate_operation(operation)?;
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
    let mut out = crate::file_transfer::AtomicFile::new(&path)?;
    out.write_all(&(metadata.len() as u32).to_le_bytes())?;
    out.write_all(&metadata)?;
    out.write_all(&file.bytes)?;
    out.commit()?;
    Ok(())
}
pub(super) fn migrate_attachment_drafts(
    receipt: &crate::settings::IdentityMigration,
) -> std::io::Result<()> {
    copy_draft_directory(
        &draft_directory(&receipt.old_identity, &receipt.host)?,
        &draft_directory(&receipt.new_identity, &receipt.host)?,
    )
}

fn file_hash(path: &std::path::Path) -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let mut digest = [0; 32];
    digest.copy_from_slice(hash.finish().as_ref());
    Ok(digest)
}

fn copy_draft_directory(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry.path().extension().and_then(|e| e.to_str()) != Some("draft")
        {
            continue;
        }
        let path = entry.path();
        let Some(operation) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        validate_operation(operation)?;
        if entry.metadata()?.len() > MAX_ATTACHMENT_BYTES as u64 + 8196 {
            return Err(std::io::Error::other("Rascunho excede o limite"));
        }
        let target = destination.join(format!("{operation}.draft"));
        if target.exists() {
            if file_hash(&target)? != file_hash(&path)? {
                return Err(std::io::Error::other(
                    "Conflito ao recuperar rascunho de anexo",
                ));
            }
        } else {
            let mut output = crate::file_transfer::AtomicFile::new(&target)?;
            std::io::copy(&mut std::fs::File::open(&path)?, &mut output)?;
            output.commit()?;
        }
    }
    Ok(())
}

pub(super) fn prune_migrated_drafts(receipt: &crate::settings::IdentityMigration) {
    let (Ok(source), Ok(destination)) = (
        draft_directory(&receipt.old_identity, &receipt.host),
        draft_directory(&receipt.new_identity, &receipt.host),
    ) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&source) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("draft") {
            continue;
        }
        let target = destination.join(entry.file_name());
        if let (Ok(old), Ok(new)) = (file_hash(&path), file_hash(&target)) {
            if old == new {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let _ = std::fs::remove_dir(source);
}

impl LanvoxApp {
    pub fn stage_attachment(&mut self, channel: ChannelId, name: String, bytes: Vec<u8>) {
        if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES {
            self.notify("O arquivo está vazio ou ultrapassa o limite de 100 MiB.");
            return;
        }
        if !self.workspace_ui.can_stage(channel, bytes.len()) {
            self.notify(
                "Limite de anexos atingido ou preparação em andamento. Envie os atuais primeiro.",
            );
            return;
        }
        let file = StagedAttachment {
            operation: format!("upload-{:032x}", rand::random::<u128>()),
            name,
            bytes,
            preview: None,
            digest: [0; 32],
        };
        let identity = self.local_identity.clone();
        let host = self.current_host();
        let bytes = file.bytes.len();
        let (tx, receiver) = bounded(1);
        self.workspace_ui.draft_pending.push(PendingDraft {
            channel,
            bytes,
            receiver,
        });
        std::thread::spawn(move || {
            let mut file = file;
            file.digest
                .copy_from_slice(ring::digest::digest(&ring::digest::SHA256, &file.bytes).as_ref());
            let result = write_draft(&identity, &host, channel, &file)
                .map(|_| file)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }
    pub(super) fn poll_draft_tasks(&mut self) {
        let mut completed = Vec::new();
        self.workspace_ui
            .draft_pending
            .retain(|pending| match pending.receiver.try_recv() {
                Ok(result) => {
                    completed.push((pending.channel, result));
                    false
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    completed.push((
                        pending.channel,
                        Err("A preparação do arquivo foi interrompida".into()),
                    ));
                    false
                }
                Err(crossbeam_channel::TryRecvError::Empty) => true,
            });
        for (channel, result) in completed {
            match result {
                Ok(file) => self
                    .workspace_ui
                    .staged
                    .entry(channel)
                    .or_default()
                    .push(file),
                Err(error) => self.notify(format!("Não foi possível preservar o anexo: {error}")),
            }
        }
        if let Some(receiver) = &self.workspace_ui.draft_restore {
            match receiver.try_recv() {
                Ok(files) => {
                    self.workspace_ui.draft_restore = None;
                    for (channel, file) in files {
                        let queued = self.settings.draft_is_queued(
                            &self.local_identity,
                            &self.current_host(),
                            &file.operation,
                        );
                        let exists =
                            draft_path(&self.local_identity, &self.current_host(), &file.operation)
                                .is_ok_and(|path| path.exists());
                        if queued || !exists {
                            continue;
                        }
                        if !self
                            .workspace_ui
                            .staged
                            .values()
                            .flatten()
                            .any(|f| f.operation == file.operation)
                            && !self
                                .workspace_ui
                                .upload_sources
                                .contains_key(&file.operation)
                        {
                            self.workspace_ui
                                .staged
                                .entry(channel)
                                .or_default()
                                .push(file);
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.workspace_ui.draft_restore = None;
                    self.notify("A recuperação de anexos foi interrompida.");
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }
    }

    pub fn remove_attachment_draft(&self, operation: &str) {
        if let Ok(path) = draft_path(&self.local_identity, &self.current_host(), operation) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Restore only this identity's drafts. Bounds protect memory even if files were modified.
    pub fn restore_attachment_drafts(&mut self) {
        if self.workspace_ui.draft_restore.is_some() {
            return;
        }
        let identity = self.local_identity.clone();
        let host = self.current_host();
        let initial_bytes: usize = self
            .workspace_ui
            .staged
            .values()
            .flatten()
            .map(|f| f.bytes.len())
            .sum();
        let known: HashSet<_> = self
            .workspace_ui
            .staged
            .values()
            .flatten()
            .map(|f| f.operation.clone())
            .chain(self.workspace_ui.upload_sources.keys().cloned())
            .collect();
        let (tx, rx) = bounded(1);
        self.workspace_ui.draft_restore = Some(rx);
        std::thread::spawn(move || {
            let mut files: Vec<(ChannelId, StagedAttachment)> = Vec::new();
            (|| {
                use std::io::Read;
                let Ok(dir) = draft_directory(&identity, &host) else {
                    return;
                };
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return;
                };
                let mut total = initial_bytes;
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(operation) =
                        path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
                    else {
                        continue;
                    };
                    if path.extension().and_then(|s| s.to_str()) != Some("draft")
                        || draft_path(&identity, &host, &operation).is_err()
                        || known.contains(&operation)
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
                    let Ok((channel, name)) =
                        serde_json::from_slice::<(ChannelId, String)>(&metadata)
                    else {
                        continue;
                    };
                    if meta.len().saturating_sub(4 + len as u64)
                        > (2 * MAX_ATTACHMENT_BYTES).saturating_sub(total) as u64
                    {
                        continue;
                    }
                    let mut bytes = Vec::new();
                    if source
                        .take(MAX_ATTACHMENT_BYTES as u64 + 1)
                        .read_to_end(&mut bytes)
                        .is_err()
                        || bytes.is_empty()
                        || bytes.len() > MAX_ATTACHMENT_BYTES
                        || total.saturating_add(bytes.len()) > 2 * MAX_ATTACHMENT_BYTES
                        || files.iter().filter(|(ch, _)| *ch == channel).count()
                            >= MAX_MESSAGE_ATTACHMENTS
                    {
                        continue;
                    }
                    total += bytes.len();
                    let mut digest = [0; 32];
                    digest.copy_from_slice(
                        ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref(),
                    );
                    files.push((
                        channel,
                        StagedAttachment {
                            operation,
                            name,
                            bytes,
                            preview: None,
                            digest,
                        },
                    ));
                }
            })();
            let _ = tx.send(files);
        });
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
            let digest = file.digest;
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
                .draft_pending
                .iter()
                .any(|p| p.channel == channel)
                || self.workspace_ui.draft_restore.is_some()
            {
                continue;
            }
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
        let attachments: Vec<u64> = self
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
            attachment_drafts: attachments
                .iter()
                .filter_map(|id| self.workspace_ui.upload_receipts.get(id).cloned())
                .collect(),
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
        if self.file_pending.is_some() || self.workspace_ui.draft_restore.is_some() {
            self.notify("Aguarde o arquivo atual antes de anexar outro.");
            return;
        }
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        let occupied = self.workspace_ui.attachment_count(channel);
        let mut remaining =
            (2 * MAX_ATTACHMENT_BYTES).saturating_sub(self.workspace_ui.attachment_bytes());
        self.workspace_ui.file_read_reserved = remaining;
        std::thread::spawn(move || {
            let mut files = Vec::new();
            for path in paths
                .into_iter()
                .take(MAX_MESSAGE_ATTACHMENTS.saturating_sub(occupied))
            {
                let task = (|| -> Result<(String, Vec<u8>), String> {
                    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                    if !meta.is_file() || meta.len() > MAX_ATTACHMENT_BYTES.min(remaining) as u64 {
                        return Err("Escolha arquivos de até 100 MiB.".into());
                    }
                    use std::io::Read;
                    let mut bytes = Vec::new();
                    std::fs::File::open(&path)
                        .map_err(|e| e.to_string())?
                        .take(MAX_ATTACHMENT_BYTES.min(remaining) as u64 + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    if bytes.len() > MAX_ATTACHMENT_BYTES.min(remaining) {
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
                    Ok(file) => {
                        remaining -= file.1.len();
                        files.push(file);
                    }
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
        let remaining = (2 * MAX_ATTACHMENT_BYTES)
            .saturating_sub(self.workspace_ui.attachment_bytes())
            .min(MAX_ATTACHMENT_BYTES);
        self.workspace_ui.file_read_reserved = remaining;
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        std::thread::spawn(move || {
            let task = match rfd::FileDialog::new()
                .set_title("Anexar arquivo (até 100 MiB)")
                .pick_file()
            {
                Some(path) => match std::fs::metadata(&path) {
                    Ok(meta) if meta.len() <= remaining as u64 => match std::fs::read(&path) {
                        Ok(bytes) if bytes.len() <= remaining => FileTask::Upload {
                            channel,
                            name: path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                            bytes,
                        },
                        Ok(_) => FileTask::Error("O arquivo ultrapassa 100 MiB.".into()),
                        Err(e) => FileTask::Error(format!("Não foi possível ler o arquivo: {e}")),
                    },
                    Ok(_) => FileTask::Error("O arquivo ultrapassa 100 MiB.".into()),
                    Err(e) => FileTask::Error(format!("Não foi possível abrir o arquivo: {e}")),
                },
                None => FileTask::Cancelled,
            };
            let _ = tx.send(task);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachment_namespace_migration_is_idempotent_and_preserves_recovery_source() {
        let root =
            std::env::temp_dir().join(format!("lanvox-draft-{:032x}", rand::random::<u128>()));
        let source = root.join("old");
        let destination = root.join("new");
        std::fs::create_dir_all(&source).unwrap();
        let name = format!("upload-{:032x}.draft", rand::random::<u128>());
        std::fs::write(source.join(&name), b"draft payload").unwrap();
        copy_draft_directory(&source, &destination).unwrap();
        copy_draft_directory(&source, &destination).unwrap();
        assert_eq!(
            std::fs::read(destination.join(&name)).unwrap(),
            b"draft payload"
        );
        assert!(source.join(&name).exists());
        std::fs::write(destination.join(&name), b"different").unwrap();
        assert!(copy_draft_directory(&source, &destination).is_err());
        assert_eq!(std::fs::read(source.join(&name)).unwrap(), b"draft payload");
        std::fs::remove_dir_all(root).unwrap();
    }
}
