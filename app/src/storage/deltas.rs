//! Small, durable transactions for the hot path; image metadata is never reserialized here.
use super::*;

fn put_details(tx: &rusqlite::Transaction<'_>, details: &MessageDetails) -> io::Result<()> {
    tx.execute("INSERT INTO message_details(id,body) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body", params![details.message as i64,serde_json::to_vec(details).map_err(error)?]).map_err(error)?;
    Ok(())
}

impl Store {
    pub fn append_message(
        &mut self,
        message: &Message,
        operation: Option<&str>,
        details: &MessageDetails,
        metadata: Option<&crate::net::server::Saved>,
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
        if let Some(metadata) = metadata {
            write_state(&tx, metadata)?;
        }
        put_details(&tx, details)?;
        tx.execute("INSERT INTO runtime_state VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET next_message=MAX(next_message,excluded.next_message)",[(message.id+1) as i64]).map_err(error)?;
        tx.execute("INSERT INTO last_sends VALUES(?1,?2,?3) ON CONFLICT(user,channel) DO UPDATE SET time=excluded.time",params![message.author,message.channel,message.time]).map_err(error)?;
        tx.commit().map_err(error)
    }

    pub fn set_read(&self, user: UserId, channel: ChannelId, message: MessageId) -> io::Result<()> {
        self.connection.execute("INSERT INTO channel_reads VALUES(?1,?2,?3) ON CONFLICT(user,channel) DO UPDATE SET message=excluded.message",params![user,channel,message as i64]).map_err(error)?;
        Ok(())
    }

    pub fn set_details(&mut self, details: &MessageDetails) -> io::Result<()> {
        let tx = self.connection.transaction().map_err(error)?;
        put_details(&tx, details)?;
        tx.commit().map_err(error)
    }

    pub fn edit_message(&mut self, message: &Message, details: &MessageDetails) -> io::Result<()> {
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
        put_details(&tx, details)?;
        tx.commit().map_err(error)
    }
}
