use super::*;

impl Store {
    /// Completed but unpublished uploads expire after a day; published files are retained.
    pub fn collect_orphan_attachments(&self, before: i64) -> io::Result<()> {
        self.connection.execute("DELETE FROM attachments WHERE id IN (SELECT id FROM attachment_retention WHERE created_at<?1) AND NOT EXISTS(SELECT 1 FROM message_attachments WHERE attachment=attachments.id)", [before]).map_err(error)?;
        Ok(())
    }
}
