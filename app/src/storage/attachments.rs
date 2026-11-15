use super::*;

impl Store {
    /// Reserve an attachment id together with the durable host state, before accepting pieces.
    pub fn begin_resumable<T: Serialize>(
        &mut self,
        partial: &PartialUpload,
        state: &T,
    ) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        tx.execute("INSERT INTO resumable_uploads(owner,author,operation,metadata,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![partial.info.id as i64, partial.user, partial.operation, serde_json::to_string(partial).map_err(error)?, partial.created_at]).map_err(error)?;
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }

    pub fn resumable(
        &self,
        user: UserId,
        operation: &str,
    ) -> io::Result<Option<(PartialUpload, u64)>> {
        let data: Option<String> = self
            .connection
            .query_row(
                "SELECT metadata FROM resumable_uploads WHERE author=?1 AND operation=?2",
                params![user, operation],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        let Some(data) = data else { return Ok(None) };
        let partial: PartialUpload = serde_json::from_str(&data).map_err(error)?;
        let mut stmt = self
            .connection
            .prepare("SELECT seq, length(data) FROM attachment_chunks WHERE owner=?1 ORDER BY seq")
            .map_err(error)?;
        let rows = stmt
            .query_map([partial.info.id as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(error)?;
        let mut received = 0u64;
        for (index, row) in rows.enumerate() {
            let (seq, len) = row.map_err(error)?;
            if seq != index as i64
                || len <= 0
                || len as u64 > ATTACHMENT_CHUNK_BYTES as u64
                || received.saturating_add(len as u64) > partial.info.size
                || (len as usize != ATTACHMENT_CHUNK_BYTES
                    && received + len as u64 != partial.info.size)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Blocos do envio parcial estão inconsistentes",
                ));
            }
            received += len as u64;
        }
        Ok(Some((partial, received)))
    }

    pub fn partial_digest(&self, owner: u64) -> io::Result<[u8; 32]> {
        let mut stmt = self
            .connection
            .prepare("SELECT data FROM attachment_chunks WHERE owner=?1 ORDER BY seq")
            .map_err(error)?;
        let rows = stmt
            .query_map([owner as i64], |r| r.get::<_, Vec<u8>>(0))
            .map_err(error)?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        for row in rows {
            context.update(&row.map_err(error)?);
        }
        let hash = context.finish();
        let mut result = [0; 32];
        result.copy_from_slice(hash.as_ref());
        Ok(result)
    }

    pub fn remove_resumable(&self, owner: u64) -> io::Result<()> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        tx.execute(
            "DELETE FROM resumable_uploads WHERE owner=?1",
            [owner as i64],
        )
        .map_err(error)?;
        tx.execute("DELETE FROM attachment_chunks WHERE owner=?1 AND owner NOT IN (SELECT id FROM attachments)", [owner as i64]).map_err(error)?;
        tx.commit().map_err(error)
    }

    pub fn expire_resumable(&self, before: i64) -> io::Result<()> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        tx.execute("DELETE FROM attachment_chunks WHERE owner IN (SELECT owner FROM resumable_uploads WHERE created_at < ?1)", [before]).map_err(error)?;
        tx.execute(
            "DELETE FROM resumable_uploads WHERE created_at < ?1",
            [before],
        )
        .map_err(error)?;
        tx.commit().map_err(error)
    }

    pub fn resumable_count(&self, user: UserId) -> io::Result<usize> {
        self.connection
            .query_row(
                "SELECT COUNT(*) FROM resumable_uploads WHERE author=?1",
                [user],
                |r| r.get::<_, u32>(0),
            )
            .map(|count| count as usize)
            .map_err(error)
    }

    pub fn touch_resumable(&self, owner: u64) -> io::Result<()> {
        self.connection
            .execute(
                "UPDATE resumable_uploads SET created_at=?1 WHERE owner=?2",
                params![crate::model::now_secs(), owner as i64],
            )
            .map_err(error)?;
        Ok(())
    }
    pub fn upload<T: Serialize>(
        &mut self,
        info: &AttachmentInfo,
        bytes: &[u8],
        operation: &str,
        state: &T,
    ) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        tx.execute(
            "INSERT INTO attachments(id,channel,uploader,info,content) VALUES(?1,?2,?3,?4,?5)",
            params![
                info.id as i64,
                info.channel,
                info.uploader,
                serde_json::to_vec(info).map_err(error)?,
                bytes
            ],
        )
        .map_err(error)?;
        tx.execute(
            "INSERT INTO upload_operations(author,operation,attachment) VALUES(?1,?2,?3)",
            params![info.uploader, operation, info.id as i64],
        )
        .map_err(error)?;
        tx.execute(
            "DELETE FROM resumable_uploads WHERE owner=?1",
            [info.id as i64],
        )
        .map_err(error)?;
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }

    pub fn uploaded(&self, user: UserId, operation: &str) -> io::Result<Option<AttachmentInfo>> {
        let data:Option<Vec<u8>>=self.connection.query_row("SELECT a.info FROM attachments a JOIN upload_operations o ON a.id=o.attachment WHERE o.author=?1 AND o.operation=?2",params![user,operation],|r|r.get(0)).optional().map_err(error)?;
        data.map(|b| serde_json::from_slice(&b).map_err(error))
            .transpose()
    }

    pub fn attachment(&self, id: u64) -> io::Result<Option<(AttachmentInfo, Vec<u8>)>> {
        let Some(info) = self.attachment_info(id)? else {
            return Ok(None);
        };
        let bytes = self.read_attachment(id, 0, info.size as usize)?;
        Ok(Some((info, bytes)))
    }

    pub fn details(&self, id: MessageId) -> io::Result<Option<MessageDetails>> {
        let body: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT body FROM message_details WHERE id=?1",
                [id as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        body.map(|b| serde_json::from_slice(&b).map_err(error))
            .transpose()
    }

    /// Bytes held by stored attachments (legacy inline blobs and chunk rows) plus uploads
    /// still being received.
    pub fn attachment_bytes(&self) -> io::Result<u64> {
        self.connection
            .query_row(
                "SELECT (SELECT COALESCE(SUM(length(content)),0) FROM attachments) + (SELECT COALESCE(SUM(length(data)),0) FROM attachment_chunks WHERE owner NOT IN (SELECT owner FROM resumable_uploads)) + (SELECT COALESCE(SUM(json_extract(metadata, '$.info.size')),0) FROM resumable_uploads)",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u64)
            .map_err(error)
    }

    /// Stores one piece of an upload in progress (`seq` counts `ATTACHMENT_CHUNK_BYTES`).
    pub fn put_chunk(&self, owner: u64, seq: u64, data: &[u8]) -> io::Result<()> {
        put_chunk_on(&self.connection, owner, seq, data)
    }

    pub fn delete_chunks(&self, owner: u64) {
        delete_chunks_on(&self.connection, owner);
    }

    /// A second connection for upload pieces: its commits skip the per-write fsync, and it
    /// never waits on (or holds) the host state lock. None for in-memory databases.
    pub fn upload_sink(&self) -> io::Result<Option<UploadSink>> {
        let Some(path) = self.connection.path().filter(|p| !p.is_empty()) else {
            return Ok(None);
        };
        let connection = Connection::open(path).map_err(error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(30))
            .map_err(error)?;
        connection
            .execute_batch("PRAGMA synchronous=NORMAL;")
            .map_err(error)?;
        Ok(Some(UploadSink { connection }))
    }

    /// Turns the pieces stored under `info.id` into an attachment, atomically with the host
    /// state. The pieces must add up to exactly `info.size` bytes.
    pub fn finish_chunked<T: Serialize>(
        &mut self,
        info: &AttachmentInfo,
        operation: &str,
        state: &T,
    ) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        let (count, bytes): (i64, i64) = tx
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(data)),0) FROM attachment_chunks WHERE owner=?1",
                [info.id as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(error)?;
        let expected = info.size.div_ceil(ATTACHMENT_CHUNK_BYTES as u64) as i64;
        if bytes as u64 != info.size || count != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Envio incompleto",
            ));
        }
        tx.execute(
            "DELETE FROM resumable_uploads WHERE owner=?1",
            [info.id as i64],
        )
        .map_err(error)?;
        tx.execute(
            "INSERT INTO attachments(id,channel,uploader,info,content) VALUES(?1,?2,?3,?4,x'')",
            params![
                info.id as i64,
                info.channel,
                info.uploader,
                serde_json::to_vec(info).map_err(error)?
            ],
        )
        .map_err(error)?;
        tx.execute(
            "INSERT INTO upload_operations(author,operation,attachment) VALUES(?1,?2,?3)",
            params![info.uploader, operation, info.id as i64],
        )
        .map_err(error)?;
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }

    pub fn attachment_info(&self, id: u64) -> io::Result<Option<AttachmentInfo>> {
        let info: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT info FROM attachments WHERE id=?1",
                [id as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        info.map(|b| serde_json::from_slice(&b).map_err(error))
            .transpose()
    }

    /// Up to `len` bytes of an attachment starting at `offset` (empty past the end). Reads only
    /// the pieces that cover the range, whether stored as pieces or as one legacy value.
    pub fn read_attachment(&self, id: u64, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let inline: Option<i64> = self
            .connection
            .query_row(
                "SELECT length(content) FROM attachments WHERE id=?1",
                [id as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        let Some(inline) = inline else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Anexo não encontrado",
            ));
        };
        if inline > 0 {
            // Attachments stored before pieces existed are one big value: read just the range
            // (substr() would load the whole value on every call).
            let blob = self
                .connection
                .blob_open("main", "attachments", "content", id as i64, true)
                .map_err(error)?;
            let start = offset as usize;
            let take = blob.len().saturating_sub(start).min(len);
            let mut out = vec![0u8; take];
            if take > 0 {
                blob.read_at_exact(&mut out, start).map_err(error)?;
            }
            return Ok(out);
        }
        let chunk = ATTACHMENT_CHUNK_BYTES as u64;
        let (first, last) = (
            offset / chunk,
            (offset + len as u64).saturating_sub(1) / chunk,
        );
        let mut statement = self
            .connection
            .prepare("SELECT data FROM attachment_chunks WHERE owner=?1 AND seq BETWEEN ?2 AND ?3 ORDER BY seq")
            .map_err(error)?;
        let rows = statement
            .query_map(params![id as i64, first as i64, last as i64], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .map_err(error)?;
        let mut joined = Vec::new();
        for row in rows {
            joined.extend(row.map_err(error)?);
        }
        let skip = (offset - first * chunk) as usize;
        Ok(joined.into_iter().skip(skip).take(len).collect())
    }

    /// Copies an attachment inside the database (used by forwarding) without loading it.
    pub fn copy_attachment<T: Serialize>(
        &mut self,
        source: u64,
        info: &AttachmentInfo,
        operation: &str,
        state: &T,
    ) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        let copied = tx
            .execute(
                "INSERT INTO attachments(id,channel,uploader,info,content) SELECT ?1,?2,?3,?4,content FROM attachments WHERE id=?5",
                params![
                    info.id as i64,
                    info.channel,
                    info.uploader,
                    serde_json::to_vec(info).map_err(error)?,
                    source as i64
                ],
            )
            .map_err(error)?;
        if copied != 1 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Anexo não encontrado",
            ));
        }
        tx.execute(
            "INSERT INTO attachment_chunks(owner,seq,data) SELECT ?1,seq,data FROM attachment_chunks WHERE owner=?2",
            params![info.id as i64, source as i64],
        )
        .map_err(error)?;
        tx.execute(
            "INSERT INTO upload_operations(author,operation,attachment) VALUES(?1,?2,?3)",
            params![info.uploader, operation, info.id as i64],
        )
        .map_err(error)?;
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }
}
