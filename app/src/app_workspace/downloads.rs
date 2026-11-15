use super::*;

impl LanvoxApp {
    /// Saves an attachment to disk. Files up to the inline limit use the single-message
    /// download; larger ones are pulled piece by piece (the next piece is requested as each
    /// arrives).
    pub fn download_attachment(&mut self, attachment: &AttachmentInfo) {
        if attachment.size > MAX_INLINE_ATTACHMENT_BYTES as u64 {
            self.pick_save_target(attachment);
        } else {
            self.workspace_ui.download_requests.insert(attachment.id);
            self.workspace(WorkspaceCommand::Download {
                attachment: attachment.id,
            });
        }
    }

    /// Asks where to save a large attachment before downloading anything, so the pieces can be
    /// written straight to that file.
    pub(super) fn pick_save_target(&mut self, info: &AttachmentInfo) {
        if self.file_pending.is_some() {
            self.notify("Aguarde o arquivo atual antes de baixar outro.");
            return;
        }
        let name = std::path::Path::new(&info.name)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let info = info.clone();
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        std::thread::spawn(move || {
            let task = match rfd::FileDialog::new().set_file_name(name).save_file() {
                Some(path) => FileTask::SaveTarget { info, path },
                None => FileTask::Cancelled,
            };
            let _ = tx.send(task);
        });
    }

    pub(super) fn begin_streamed_save(&mut self, info: AttachmentInfo, path: std::path::PathBuf) {
        match std::fs::File::create(&path) {
            Ok(file) => {
                let id = info.id;
                self.drop_download(id);
                self.workspace_ui.chunk_downloads.insert(
                    id,
                    ChunkDownload {
                        info,
                        file,
                        path,
                        written: 0,
                        purpose: DownloadPurpose::Save,
                        last_progress: Instant::now(),
                    },
                );
                self.notify("Baixando o arquivo…");
                self.workspace(WorkspaceCommand::DownloadChunk {
                    attachment: id,
                    offset: 0,
                });
            }
            Err(e) => self.notify(format!("Não foi possível criar o arquivo: {e}")),
        }
    }

    /// Starts caching a large video or audio file so it can be played from disk.
    pub fn request_media_file(&mut self, info: &AttachmentInfo) {
        let id = info.id;
        if self.workspace_ui.chunk_downloads.contains_key(&id)
            || self
                .workspace_ui
                .media_files
                .get(&id)
                .is_some_and(|p| p.exists())
        {
            return;
        }
        let path = crate::media_cache::path_for(id, &info.name);
        let mut keep: HashSet<std::path::PathBuf> = self
            .workspace_ui
            .media_files
            .iter()
            .filter(|(known, _)| {
                self.videos.wanted.contains(known) || self.videos.is_active(**known)
            })
            .map(|(_, p)| p.clone())
            .collect();
        keep.insert(path.clone());
        keep.extend(
            self.workspace_ui
                .chunk_downloads
                .values()
                .map(|d| d.path.clone()),
        );
        crate::media_cache::trim(&keep);
        self.workspace_ui.media_files.retain(|_, p| p.exists());
        match std::fs::File::create(&path) {
            Ok(file) => {
                self.workspace_ui.chunk_downloads.insert(
                    id,
                    ChunkDownload {
                        info: info.clone(),
                        file,
                        path,
                        written: 0,
                        purpose: DownloadPurpose::Play,
                        last_progress: Instant::now(),
                    },
                );
                self.workspace(WorkspaceCommand::DownloadChunk {
                    attachment: id,
                    offset: 0,
                });
            }
            Err(e) => {
                self.videos.wanted.remove(&id);
                self.notify(format!("Não foi possível preparar o arquivo: {e}"));
            }
        }
    }

    /// Fraction (0..=1) of a chunked download written so far.
    pub fn download_fraction(&self, id: u64) -> Option<f32> {
        self.workspace_ui
            .chunk_downloads
            .get(&id)
            .map(|d| d.written as f32 / d.info.size.max(1) as f32)
    }

    /// Abandons a chunked download and removes what it wrote.
    pub(super) fn drop_download(&mut self, id: u64) {
        self.videos.wanted.remove(&id);
        if let Some(download) = self.workspace_ui.chunk_downloads.remove(&id) {
            let path = download.path.clone();
            drop(download);
            let _ = std::fs::remove_file(path);
        }
    }

    pub(super) fn save_attachment(&mut self, info: AttachmentInfo, bytes: Vec<u8>) {
        let name = std::path::Path::new(&info.name)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        std::thread::spawn(move || {
            let task = if let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() {
                match std::fs::write(&path, bytes) {
                    Ok(()) => FileTask::Saved(format!("Arquivo salvo em {}", path.display())),
                    Err(e) => FileTask::Error(format!("Não foi possível salvar: {e}")),
                }
            } else {
                FileTask::Cancelled
            };
            let _ = tx.send(task);
        });
    }

    pub fn poll_file_task(&mut self, ctx: &egui::Context) {
        let task = self.file_pending.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(task) = task {
            self.file_pending = None;
            match task {
                FileTask::Uploads { channel, files } => {
                    for (name, bytes) in files {
                        self.stage_attachment(channel, name, bytes);
                    }
                }
                FileTask::Upload {
                    channel,
                    name,
                    bytes,
                } => {
                    self.stage_attachment(channel, name, bytes);
                }
                FileTask::Avatar(bytes) => self.workspace_ui.profile_avatar = bytes,
                FileTask::SaveTarget { info, path } => self.begin_streamed_save(info, path),
                FileTask::Error(error) => self.notify(error),
                FileTask::Saved(message) => self.notify(message),
                FileTask::IdentityImported => {
                    self.local_identity =
                        crate::notifications::identity_fingerprint().unwrap_or_default();
                    self.compose.clear();
                    self.notify("Identidade importada. Você já pode se conectar.");
                }
                FileTask::Cancelled => {}
            }
        }
        if self.file_pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
