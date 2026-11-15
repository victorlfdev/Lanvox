use super::*;

/// Forward-only, transactional migrations keyed on PRAGMA user_version.
pub(super) fn migrate(connection: &mut Connection, from: u32) -> io::Result<()> {
    if from < 2 {
        let tx = connection.transaction().map_err(error)?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text, content='messages', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
             CREATE TABLE IF NOT EXISTS message_meta (id INTEGER PRIMARY KEY, has_attachment INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS message_mentions (message INTEGER NOT NULL, user INTEGER NOT NULL, PRIMARY KEY(user,message)) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS message_mentions_message ON message_mentions(message);
             CREATE INDEX IF NOT EXISTS messages_author_id ON messages(author,id DESC);
             CREATE INDEX IF NOT EXISTS messages_time ON messages(time);
             DELETE FROM message_meta; DELETE FROM message_mentions;
             INSERT INTO message_meta(id,has_attachment) SELECT id, COALESCE(json_array_length(CAST(body AS TEXT),'$.attachments'),0)>0 FROM message_details;
             INSERT OR IGNORE INTO message_mentions(message,user) SELECT d.id, j.value FROM message_details d, json_each(CAST(d.body AS TEXT),'$.mentions') j;
             INSERT INTO messages_fts(messages_fts) VALUES('rebuild');
             CREATE TRIGGER IF NOT EXISTS messages_fts_ai AFTER INSERT ON messages BEGIN
               INSERT INTO messages_fts(rowid,text) VALUES(new.id,new.text);
             END;
             CREATE TRIGGER IF NOT EXISTS messages_fts_ad AFTER DELETE ON messages BEGIN
               INSERT INTO messages_fts(messages_fts,rowid,text) VALUES('delete',old.id,old.text);
             END;
             CREATE TRIGGER IF NOT EXISTS messages_fts_au AFTER UPDATE OF text ON messages BEGIN
               INSERT INTO messages_fts(messages_fts,rowid,text) VALUES('delete',old.id,old.text);
               INSERT INTO messages_fts(rowid,text) VALUES(new.id,new.text);
             END;
             CREATE TRIGGER IF NOT EXISTS details_meta_ai AFTER INSERT ON message_details BEGIN
               INSERT OR REPLACE INTO message_meta(id,has_attachment) VALUES(new.id, COALESCE(json_array_length(CAST(new.body AS TEXT),'$.attachments'),0)>0);
               DELETE FROM message_mentions WHERE message=new.id;
               INSERT OR IGNORE INTO message_mentions(message,user) SELECT new.id, value FROM json_each(CAST(new.body AS TEXT),'$.mentions');
             END;
             CREATE TRIGGER IF NOT EXISTS details_meta_au AFTER UPDATE OF body ON message_details BEGIN
               INSERT OR REPLACE INTO message_meta(id,has_attachment) VALUES(new.id, COALESCE(json_array_length(CAST(new.body AS TEXT),'$.attachments'),0)>0);
               DELETE FROM message_mentions WHERE message=new.id;
               INSERT OR IGNORE INTO message_mentions(message,user) SELECT new.id, value FROM json_each(CAST(new.body AS TEXT),'$.mentions');
             END;
             CREATE TRIGGER IF NOT EXISTS details_meta_ad AFTER DELETE ON message_details BEGIN
               DELETE FROM message_meta WHERE id=old.id;
               DELETE FROM message_mentions WHERE message=old.id;
             END;
             PRAGMA user_version=2;",
        )
        .map_err(|e| {
            error(format!(
                "Migração do índice de busca falhou (FTS5 indisponível?): {e}"
            ))
        })?;
        tx.commit().map_err(error)?;
    }
    if from < 3 {
        // One transaction: rewrite the host metadata blob and bump the version together, so a
        // crash leaves a clean v2 database that simply migrates again.
        let tx = connection.transaction().map_err(error)?;
        let body: Option<Vec<u8>> = tx
            .query_row("SELECT body FROM host_state WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(error)?;
        if let Some(body) = body {
            let mut value: serde_json::Value = serde_json::from_slice(&body).map_err(error)?;
            if migrate_categories(&mut value)? {
                tx.execute(
                    "UPDATE host_state SET body=?1 WHERE id=1",
                    [serde_json::to_vec(&value).map_err(error)?],
                )
                .map_err(error)?;
            }
        }
        tx.execute_batch("PRAGMA user_version=3;").map_err(error)?;
        tx.commit().map_err(error)?;
    }
    if from < 4 {
        connection
            .execute_batch("PRAGMA user_version=4;")
            .map_err(error)?;
    }
    Ok(())
}

/// v2 -> v3 on the host metadata JSON: legacy `channel_settings[].category` text becomes
/// `workspace.categories` entities (see `categories::adopt_text_categories`). Marks
/// `workspace.categories_ready` so it never runs twice; returns whether anything changed.
pub(super) fn migrate_categories(value: &mut serde_json::Value) -> io::Result<bool> {
    use crate::model::{Category, Channel, ChannelSettings};
    let Some(root) = value.as_object_mut() else {
        return Ok(false);
    };
    let channels: Vec<Channel> = match root.get("channels") {
        Some(v) => serde_json::from_value(v.clone()).map_err(error)?,
        None => Vec::new(),
    };
    let workspace = root
        .entry("workspace")
        .or_insert_with(|| serde_json::json!({}));
    let Some(workspace) = workspace.as_object_mut() else {
        return Ok(false);
    };
    if workspace
        .get("categories_ready")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(false);
    }
    let mut settings: Vec<ChannelSettings> = match workspace.get("channel_settings") {
        Some(v) => serde_json::from_value(v.clone()).map_err(error)?,
        None => Vec::new(),
    };
    let mut categories: Vec<Category> = match workspace.get("categories") {
        Some(v) => serde_json::from_value(v.clone()).map_err(error)?,
        None => Vec::new(),
    };
    let mut counter = workspace
        .get("next_category")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as u32;
    crate::categories::adopt_text_categories(
        &channels,
        &mut settings,
        &mut categories,
        &mut counter,
        true,
    );
    workspace.insert(
        "channel_settings".into(),
        serde_json::to_value(&settings).map_err(error)?,
    );
    workspace.insert(
        "categories".into(),
        serde_json::to_value(&categories).map_err(error)?,
    );
    workspace.insert("next_category".into(), counter.into());
    workspace.insert("categories_ready".into(), true.into());
    Ok(true)
}
