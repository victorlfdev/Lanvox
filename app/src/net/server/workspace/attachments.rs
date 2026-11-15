use super::*;

impl State {
    pub(super) fn find_attachment_info(&self, id: u64) -> std::io::Result<AttachmentInfo> {
        if let Some((info, _)) = self.uploads.get(&id) {
            return Ok(info.clone());
        }
        self.store
            .as_ref()
            .map(|s| s.lock().attachment_info(id))
            .transpose()?
            .flatten()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "Anexo não encontrado")
            })
    }

    pub(super) fn read_attachment_chunk(
        &self,
        id: u64,
        offset: u64,
        len: usize,
    ) -> std::io::Result<Vec<u8>> {
        if let Some((_, bytes)) = self.uploads.get(&id) {
            let start = (offset as usize).min(bytes.len());
            let end = start.saturating_add(len).min(bytes.len());
            return Ok(bytes[start..end].to_vec());
        }
        self.store
            .as_ref()
            .map(|s| s.lock().read_attachment(id, offset, len))
            .transpose()?
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "Anexo não encontrado")
            })
    }

    pub(super) fn stored_attachment_bytes(&self) -> std::io::Result<u64> {
        if let Some(store) = &self.store {
            return Ok(store.lock().attachment_bytes()? + self.pending_uploads.unwritten_bytes());
        }
        let done: u64 = self.uploads.values().map(|(_, b)| b.len() as u64).sum();
        Ok(done + self.pending_uploads.unwritten_bytes())
    }

    pub(super) fn previous_upload(
        &self,
        user: UserId,
        operation: &str,
    ) -> std::io::Result<Option<AttachmentInfo>> {
        Ok(if let Some(store) = &self.store {
            store.lock().uploaded(user, operation)?
        } else {
            self.workspace
                .upload_operations
                .iter()
                .find(|(u, o, _)| *u == user && o == operation)
                .and_then(|(_, _, id)| self.uploads.get(id).map(|(i, _)| i.clone()))
        })
    }

    pub(super) fn finish_chunked_upload(
        &mut self,
        user: UserId,
        operation: &str,
        pending: &mut PendingUpload,
    ) -> std::io::Result<()> {
        require(pending.received == pending.size, "Envio incompleto")?;
        self.require_write(user, pending.channel)?;
        require(
            self.has_permission(user, pending.channel, permission::ATTACH),
            "Anexos não permitidos",
        )?;
        if let Some(expected) = pending.digest {
            let actual = if let Some(store) = &self.store {
                store.lock().partial_digest(pending.owner)?
            } else {
                let hash = ring::digest::digest(&ring::digest::SHA256, &pending.buffer);
                let mut bytes = [0; 32];
                bytes.copy_from_slice(hash.as_ref());
                bytes
            };
            require(
                actual == expected,
                "O arquivo não corresponde ao envio iniciado",
            )?;
        }
        // The id was reserved when the upload began (other uploads may have reserved later
        // ones since), so `next_attachment` is left alone here.
        let id = pending.owner;
        let info = AttachmentInfo {
            id,
            channel: pending.channel,
            uploader: user,
            name: pending.name.clone(),
            media_type: pending.media_type.clone(),
            size: pending.size,
        };
        if let Some(store) = &self.store {
            store
                .lock()
                .finish_chunked(&info, operation, &self.saved(false))?;
            self.pending_uploads.checkpoint_later();
        } else {
            let previous = self.workspace.clone();
            let bytes = std::mem::take(&mut pending.buffer);
            self.uploads.insert(id, (info.clone(), bytes));
            self.workspace
                .upload_operations
                .push((user, operation.to_string(), id));
            if let Err(e) = self.persist_workspace(previous) {
                self.uploads.remove(&id);
                return Err(e);
            }
        }
        self.send_to(
            user,
            &ServerMsg::Workspace(WorkspaceEvent::Uploaded {
                operation: operation.to_string(),
                info,
            }),
        );
        Ok(())
    }

    pub(super) fn find_attachment(&self, id: u64) -> std::io::Result<(AttachmentInfo, Vec<u8>)> {
        if let Some(data) = self.uploads.get(&id) {
            return Ok(data.clone());
        }
        self.store
            .as_ref()
            .map(|s| s.lock().attachment(id))
            .transpose()?
            .flatten()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "Anexo não encontrado")
            })
    }
}
