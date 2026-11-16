use super::*;

impl State {
    pub(super) fn attachment_commands(
        &mut self,
        user: UserId,
        command: WorkspaceCommand,
    ) -> std::io::Result<()> {
        match command {
            WorkspaceCommand::Upload {
                operation,
                channel,
                name,
                media_type,
                bytes,
            } => {
                self.require_write(user, channel)?;
                require(
                    self.has_permission(user, channel, permission::ATTACH),
                    "Anexos não permitidos",
                )?;
                require(
                    !bytes.is_empty() && bytes.len() <= MAX_INLINE_ATTACHMENT_BYTES,
                    "Arquivo deve ter até 8 MiB neste modo; atualize o Lanvox para enviar até 100 MiB",
                )?;
                require(
                    !operation.is_empty() && operation.len() <= 128,
                    "Identificador inválido",
                )?;
                let previous = if let Some(store) = &self.store {
                    store.lock().uploaded(user, &operation)?
                } else {
                    self.workspace
                        .upload_operations
                        .iter()
                        .find(|(u, o, _)| *u == user && *o == operation)
                        .and_then(|(_, _, id)| self.uploads.get(id).map(|(i, _)| i.clone()))
                };
                if let Some(info) = previous {
                    require(
                        info.channel == channel,
                        "Operação pertence a outra conversa",
                    )?;
                    self.send_to(
                        user,
                        &ServerMsg::Workspace(WorkspaceEvent::Uploaded { operation, info }),
                    );
                    return Ok(());
                }
                let total = self.stored_attachment_bytes()?;
                require(
                    total + bytes.len() as u64 <= ATTACHMENT_STORE_LIMIT,
                    "Armazenamento de anexos cheio (4 GiB)",
                )?;
                let name = clean(&name.replace(['/', '\\'], "_"), 180);
                require(!name.is_empty(), "Nome do arquivo vazio")?;
                let id = self.workspace.next_attachment.max(1);
                require(id < i64::MAX as u64, "Limite de anexos atingido")?;
                let info = AttachmentInfo {
                    id,
                    channel,
                    uploader: user,
                    name,
                    media_type: clean(&media_type, 100),
                    size: bytes.len() as u64,
                };
                let previous = self.workspace.clone();
                self.workspace.next_attachment = id + 1;
                if let Some(store) = &self.store {
                    if let Err(e) =
                        store
                            .lock()
                            .upload(&info, &bytes, &operation, &self.saved(false))
                    {
                        self.workspace = previous;
                        return Err(e);
                    }
                } else {
                    self.uploads.insert(id, (info.clone(), bytes));
                    self.workspace
                        .upload_operations
                        .push((user, operation.clone(), id));
                    if let Err(e) = self.persist_workspace(previous) {
                        self.uploads.remove(&id);
                        return Err(e);
                    }
                }
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Uploaded { operation, info }),
                );
            }
            WorkspaceCommand::Download { attachment } => {
                let info = self.find_attachment_info(attachment)?;
                require(self.can_access(user, info.channel), "Anexo não autorizado")?;
                require(
                    info.size <= MAX_INLINE_ATTACHMENT_BYTES as u64,
                    "Arquivo grande demais para prévia; baixe-o pelo menu do anexo",
                )?;
                let (info, bytes) = self.find_attachment(attachment)?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Attachment { info, bytes }),
                );
            }
            WorkspaceCommand::DownloadChunk { attachment, offset } => {
                let info = self.find_attachment_info(attachment)?;
                require(self.can_access(user, info.channel), "Anexo não autorizado")?;
                require(offset < info.size, "Posição inválida")?;
                let data = self.read_attachment_chunk(attachment, offset, DOWNLOAD_PIECE_BYTES)?;
                require(!data.is_empty(), "Anexo truncado")?;
                self.send_file_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::AttachmentChunk { info, offset, data }),
                );
            }
            WorkspaceCommand::UploadBegin {
                operation,
                channel,
                name,
                media_type,
                size,
            } => {
                self.require_write(user, channel)?;
                require(
                    self.has_permission(user, channel, permission::ATTACH),
                    "Anexos não permitidos",
                )?;
                require(
                    size > 0 && size <= MAX_ATTACHMENT_BYTES as u64,
                    "Arquivo deve ter até 100 MiB",
                )?;
                require(
                    !operation.is_empty() && operation.len() <= 128,
                    "Identificador inválido",
                )?;
                if let Some(info) = self.previous_upload(user, &operation)? {
                    require(
                        info.channel == channel,
                        "Operação pertence a outra conversa",
                    )?;
                    self.send_to(
                        user,
                        &ServerMsg::Workspace(WorkspaceEvent::Uploaded { operation, info }),
                    );
                    return Ok(());
                }
                self.pending_uploads.maintain()?;
                let key = (user, operation.clone());
                require(
                    self.pending_uploads.count_for(user) < 3,
                    "Muitos envios simultâneos",
                )?;
                require(
                    self.stored_attachment_bytes()? + size <= ATTACHMENT_STORE_LIMIT,
                    "Armazenamento de anexos cheio (4 GiB)",
                )?;
                let name = clean(&name.replace(['/', '\\'], "_"), 180);
                require(!name.is_empty(), "Nome do arquivo vazio")?;
                let owner = self.workspace.next_attachment.max(1);
                require(owner < i64::MAX as u64, "Limite de anexos atingido")?;
                self.workspace.next_attachment = owner + 1;
                self.pending_uploads.begin(
                    user,
                    key.1,
                    PendingUpload {
                        owner,
                        channel,
                        name,
                        media_type: clean(&media_type, 100),
                        size,
                        received: 0,
                        buffer: Vec::new(),
                        started: Instant::now(),
                        digest: None,
                    },
                );
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::UploadProgress {
                        operation,
                        received: 0,
                    }),
                );
            }
            WorkspaceCommand::UploadBeginResumable {
                operation,
                channel,
                name,
                media_type,
                size,
                digest,
            } => {
                self.pending_uploads.maintain()?;
                self.require_write(user, channel)?;
                require(
                    self.has_permission(user, channel, permission::ATTACH),
                    "Anexos não permitidos",
                )?;
                require(
                    size > 0 && size <= MAX_ATTACHMENT_BYTES as u64,
                    "Arquivo deve ter até 100 MiB",
                )?;
                require(
                    !operation.is_empty() && operation.len() <= 128,
                    "Identificador inválido",
                )?;
                let name = clean(&name.replace(['/', '\\'], "_"), 180);
                let media_type = clean(&media_type, 100);
                require(!name.is_empty(), "Nome do arquivo vazio")?;
                if let Some(info) = self.previous_upload(user, &operation)? {
                    require(
                        info.channel == channel
                            && info.name == name
                            && info.media_type == media_type
                            && info.size == size,
                        "Operação pertence a outro arquivo",
                    )?;
                    self.send_to(
                        user,
                        &ServerMsg::Workspace(WorkspaceEvent::Uploaded { operation, info }),
                    );
                    return Ok(());
                }
                if let Some(received) = self.pending_uploads.resume_active(
                    user,
                    &operation,
                    channel,
                    &name,
                    &media_type,
                    size,
                    &digest,
                )? {
                    self.send_to(
                        user,
                        &ServerMsg::Workspace(WorkspaceEvent::UploadResumePoint {
                            operation,
                            received,
                        }),
                    );
                    return Ok(());
                }
                if let Some(store) = &self.store {
                    let saved = store.lock().resumable(user, &operation)?;
                    if let Some((saved, received)) = saved {
                        require(
                            saved.user == user
                                && saved.info.channel == channel
                                && saved.info.uploader == user
                                && saved.info.name == name
                                && saved.info.media_type == media_type
                                && saved.info.size == size
                                && saved.digest == digest,
                            "O envio parcial pertence a outro arquivo",
                        )?;
                        store.lock().touch_resumable(saved.info.id)?;
                        self.pending_uploads.begin(
                            user,
                            operation.clone(),
                            PendingUpload {
                                owner: saved.info.id,
                                channel,
                                name,
                                media_type,
                                size,
                                received,
                                buffer: Vec::new(),
                                started: Instant::now(),
                                digest: Some(digest),
                            },
                        );
                        self.send_to(
                            user,
                            &ServerMsg::Workspace(WorkspaceEvent::UploadResumePoint {
                                operation,
                                received,
                            }),
                        );
                        return Ok(());
                    }
                    require(
                        store.lock().resumable_count(user)? < 3,
                        "Muitos envios simultâneos",
                    )?;
                } else {
                    require(
                        self.pending_uploads.count_for(user) < 3,
                        "Muitos envios simultâneos",
                    )?;
                }
                require(
                    self.stored_attachment_bytes()?.saturating_add(size) <= ATTACHMENT_STORE_LIMIT,
                    "Armazenamento de anexos cheio (4 GiB)",
                )?;
                let owner = self.workspace.next_attachment.max(1);
                require(owner < i64::MAX as u64, "Limite de anexos atingido")?;
                let previous = self.workspace.clone();
                self.workspace.next_attachment = owner + 1;
                if let Some(store) = &self.store {
                    let partial = crate::storage::PartialUpload {
                        user,
                        operation: operation.clone(),
                        info: AttachmentInfo {
                            id: owner,
                            channel,
                            uploader: user,
                            name: name.clone(),
                            media_type: media_type.clone(),
                            size,
                        },
                        digest,
                        created_at: now_secs(),
                    };
                    if let Err(error) = store.lock().begin_resumable(&partial, &self.saved(false)) {
                        self.workspace = previous;
                        return Err(error);
                    }
                }
                self.pending_uploads.begin(
                    user,
                    operation.clone(),
                    PendingUpload {
                        owner,
                        channel,
                        name,
                        media_type,
                        size,
                        received: 0,
                        buffer: Vec::new(),
                        started: Instant::now(),
                        digest: Some(digest),
                    },
                );
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::UploadResumePoint {
                        operation,
                        received: 0,
                    }),
                );
            }
            WorkspaceCommand::UploadChunk {
                operation,
                offset,
                data,
            } => {
                // Normally handled on the connection's reader thread (see `serve`); this keeps
                // the command meaningful when it arrives through the regular path.
                let received = self
                    .pending_uploads
                    .chunk(user, &operation, offset, &data)?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::UploadProgress {
                        operation,
                        received,
                    }),
                );
            }
            WorkspaceCommand::UploadFinish { operation } => {
                let Some(mut pending) = self.pending_uploads.take(user, &operation) else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Envio não iniciado",
                    ));
                };
                let result = self.finish_chunked_upload(user, &operation, &mut pending);
                if result.is_err() {
                    self.pending_uploads.discard(&pending);
                }
                result?;
            }
            WorkspaceCommand::UploadAbort { operation } => {
                if let Some(pending) = self.pending_uploads.take(user, &operation) {
                    self.pending_uploads.discard(&pending);
                } else if let Some(store) = &self.store {
                    let saved = store.lock().resumable(user, &operation)?;
                    if let Some((partial, _)) = saved {
                        store.lock().remove_resumable(partial.info.id)?;
                    }
                }
            }
            _ => unreachable!("routed by workspacecommand"),
        }
        Ok(())
    }
}
