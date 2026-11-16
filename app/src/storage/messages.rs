use super::*;

impl Store {
    /// Message, metadata and idempotency key commit in one transaction.
    #[cfg(test)]
    pub fn insert<T: Serialize>(
        &mut self,
        message: &Message,
        operation: Option<&str>,
        state: &T,
    ) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        tx.execute(
            "INSERT INTO messages(id,channel,author,text,time,body) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                message.id as i64,
                message.channel,
                message.author,
                message.text,
                message.time,
                serde_json::to_vec(message).map_err(error)?
            ],
        )
        .map_err(error)?;
        if let Some(operation) = operation {
            tx.execute(
                "INSERT INTO operations(author,operation,message) VALUES(?1,?2,?3)",
                params![message.author, operation, message.id as i64],
            )
            .map_err(error)?;
        }
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }

    pub fn history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: usize,
    ) -> io::Result<Vec<Message>> {
        let before = before.unwrap_or(i64::MAX as u64).min(i64::MAX as u64) as i64;
        let mut statement = self
            .connection
            .prepare(
                "SELECT body FROM messages WHERE channel=?1 AND id<?2 ORDER BY id DESC LIMIT ?3",
            )
            .map_err(error)?;
        let rows = statement
            .query_map(params![channel, before, limit as i64], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .map_err(error)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_slice(&row.map_err(error)?).map_err(error)?);
        }
        out.reverse();
        Ok(out)
    }

    /// Context following an anchor, in chronological order.
    pub fn history_after(
        &self,
        channel: ChannelId,
        after: MessageId,
        limit: usize,
    ) -> io::Result<Vec<Message>> {
        let mut q = self
            .connection
            .prepare(
                "SELECT body FROM messages WHERE channel=?1 AND id>?2 ORDER BY id ASC LIMIT ?3",
            )
            .map_err(error)?;
        let rows = q
            .query_map(
                params![
                    channel,
                    after.min(i64::MAX as u64) as i64,
                    limit.min(101) as i64
                ],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .map_err(error)?;
        rows.map(|r| serde_json::from_slice(&r.map_err(error)?).map_err(error))
            .collect()
    }

    /// Exact durable counts, independent of the client's bounded history cache.
    pub fn unread_summary(
        &self,
        user: UserId,
        channel: ChannelId,
        after: MessageId,
    ) -> io::Result<crate::model::UnreadSummary> {
        self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(EXISTS(SELECT 1 FROM message_mentions mm WHERE mm.message=m.id AND mm.user=?1)),0), MIN(m.id), MAX(m.id) FROM messages m WHERE m.channel=?2 AND m.id>?3 AND m.author<>?1",
            params![user,channel,after.min(i64::MAX as u64) as i64], |r| {
                Ok(crate::model::UnreadSummary { channel, count:r.get::<_,u32>(0)?, mentions:r.get::<_,u32>(1)?, first:r.get::<_,Option<i64>>(2)?.map(|n|n as u64), latest:r.get::<_,Option<i64>>(3)?.map(|n|n as u64) })
            }).map_err(error)
    }

    pub fn recent(&self, limit: usize) -> io::Result<Vec<Message>> {
        let mut statement = self
            .connection
            .prepare("SELECT body FROM messages ORDER BY id DESC LIMIT ?1")
            .map_err(error)?;
        let rows = statement
            .query_map([limit as i64], |r| r.get::<_, Vec<u8>>(0))
            .map_err(error)?;
        let mut messages: Vec<Message> = rows
            .map(|r| serde_json::from_slice(&r.map_err(error)?).map_err(error))
            .collect::<io::Result<_>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn pinned(&self, channel: ChannelId) -> io::Result<Vec<Message>> {
        let mut statement=self.connection.prepare("SELECT m.body FROM messages m JOIN message_details d ON m.id=d.id WHERE m.channel=?1 AND json_extract(CAST(d.body AS TEXT),'$.pinned')=1 ORDER BY m.id DESC LIMIT 100").map_err(error)?;
        let rows = statement
            .query_map([channel], |r| r.get::<_, Vec<u8>>(0))
            .map_err(error)?;
        rows.map(|r| serde_json::from_slice(&r.map_err(error)?).map_err(error))
            .collect()
    }

    pub fn message(&self, id: MessageId) -> io::Result<Option<Message>> {
        let body: Option<Vec<u8>> = self
            .connection
            .query_row("SELECT body FROM messages WHERE id=?1", [id as i64], |r| {
                r.get(0)
            })
            .optional()
            .map_err(error)?;
        body.map(|b| serde_json::from_slice(&b).map_err(error))
            .transpose()
    }

    #[cfg(test)]
    pub fn update<T: Serialize>(&mut self, message: &Message, state: &T) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        tx.execute(
            "UPDATE messages SET text=?2,body=?3 WHERE id=?1",
            params![
                message.id as i64,
                message.text,
                serde_json::to_vec(message).map_err(error)?
            ],
        )
        .map_err(error)?;
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }

    pub fn delete<T: Serialize>(&mut self, id: MessageId, state: &T) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        tx.execute("DELETE FROM messages WHERE id=?1", [id as i64])
            .map_err(error)?;
        tx.execute("DELETE FROM message_details WHERE id=?1", [id as i64])
            .map_err(error)?;
        write_state(&tx, state)?;
        tx.commit().map_err(error)
    }

    pub fn operation(&self, author: UserId, operation: &str) -> io::Result<Option<MessageId>> {
        self.connection
            .query_row(
                "SELECT message FROM operations WHERE author=?1 AND operation=?2",
                params![author, operation],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|v| v.map(|n| n as u64))
            .map_err(error)
    }

    pub fn import<T: Serialize>(&mut self, state: &T, messages: &[Message]) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        write_state(&tx, state)?;
        for m in messages {
            tx.execute("INSERT OR IGNORE INTO messages(id,channel,author,text,time,body) VALUES(?1,?2,?3,?4,?5,?6)", params![m.id as i64,m.channel,m.author,m.text,m.time,serde_json::to_vec(m).map_err(error)?]).map_err(error)?;
        }
        tx.commit().map_err(error)
    }
}
