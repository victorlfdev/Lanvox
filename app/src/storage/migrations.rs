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
    if from < 5 {
        let tx = connection.transaction().map_err(error)?;
        tx.execute_batch("CREATE TABLE message_attachments(message INTEGER NOT NULL, attachment INTEGER NOT NULL, PRIMARY KEY(message,attachment));
            CREATE INDEX message_attachments_attachment ON message_attachments(attachment);
            INSERT OR IGNORE INTO message_attachments SELECT d.id, json_extract(j.value,'$.id') FROM message_details d, json_each(CAST(d.body AS TEXT),'$.attachments') j;
            CREATE TABLE attachment_retention(id INTEGER PRIMARY KEY, created_at INTEGER NOT NULL);
            INSERT INTO attachment_retention SELECT id,unixepoch() FROM attachments;
            CREATE TRIGGER attachment_created AFTER INSERT ON attachments BEGIN INSERT OR REPLACE INTO attachment_retention VALUES(new.id,unixepoch()); END;
            CREATE TRIGGER attachment_deleted AFTER DELETE ON attachments BEGIN
                DELETE FROM attachment_chunks WHERE owner=old.id;
                DELETE FROM upload_operations WHERE attachment=old.id;
                DELETE FROM attachment_retention WHERE id=old.id;
            END;
            CREATE TRIGGER details_attachments_ai AFTER INSERT ON message_details BEGIN
                INSERT OR IGNORE INTO message_attachments SELECT new.id,json_extract(value,'$.id') FROM json_each(CAST(new.body AS TEXT),'$.attachments');
            END;
            CREATE TRIGGER details_attachments_au AFTER UPDATE OF body ON message_details BEGIN
                DELETE FROM message_attachments WHERE message=new.id;
                INSERT OR IGNORE INTO message_attachments SELECT new.id,json_extract(value,'$.id') FROM json_each(CAST(new.body AS TEXT),'$.attachments');
            END;
            CREATE TRIGGER details_attachments_ad AFTER DELETE ON message_details BEGIN
                DELETE FROM attachments WHERE id IN (SELECT attachment FROM message_attachments WHERE message=old.id)
                    AND NOT EXISTS(SELECT 1 FROM message_attachments refs WHERE refs.attachment=attachments.id AND refs.message<>old.id);
                DELETE FROM message_attachments WHERE message=old.id;
            END;
            DROP TRIGGER IF EXISTS details_meta_ai;
            DROP TRIGGER IF EXISTS details_meta_au;
            CREATE TRIGGER details_meta_ai AFTER INSERT ON message_details BEGIN
                INSERT INTO message_meta(id,has_attachment) VALUES(new.id,COALESCE(json_array_length(CAST(new.body AS TEXT),'$.attachments'),0)>0) ON CONFLICT(id) DO UPDATE SET has_attachment=excluded.has_attachment;
                DELETE FROM message_mentions WHERE message=new.id;
                INSERT OR IGNORE INTO message_mentions SELECT new.id,value FROM json_each(CAST(new.body AS TEXT),'$.mentions');
            END;
            CREATE TRIGGER details_meta_au AFTER UPDATE OF body ON message_details BEGIN
                INSERT INTO message_meta(id,has_attachment) VALUES(new.id,COALESCE(json_array_length(CAST(new.body AS TEXT),'$.attachments'),0)>0) ON CONFLICT(id) DO UPDATE SET has_attachment=excluded.has_attachment;
                DELETE FROM message_mentions WHERE message=new.id;
                INSERT OR IGNORE INTO message_mentions SELECT new.id,value FROM json_each(CAST(new.body AS TEXT),'$.mentions');
            END;
            CREATE TABLE runtime_state(id INTEGER PRIMARY KEY CHECK(id=1), next_message INTEGER NOT NULL);
            CREATE TABLE channel_reads(user INTEGER NOT NULL, channel INTEGER NOT NULL, message INTEGER NOT NULL, PRIMARY KEY(user,channel));
            CREATE TABLE last_sends(user INTEGER NOT NULL, channel INTEGER NOT NULL, time INTEGER NOT NULL, PRIMARY KEY(user,channel));
            PRAGMA user_version=5;") .map_err(error)?;
        // Older versions removed channel metadata without collecting the channel's rows.
        // Initialized host state is authoritative; generic storage-only fixtures need not list channels.
        let initialized: bool=tx.query_row("SELECT COALESCE(MAX(json_extract(CAST(body AS TEXT),'$.workspace.initialized')),0) FROM host_state",[],|r|r.get(0)).map_err(error)?;
        if initialized {
            let removed="SELECT m.id FROM messages m WHERE m.channel NOT IN (SELECT json_extract(c.value,'$.id') FROM host_state h,json_each(CAST(h.body AS TEXT),'$.channels') c)";
            tx.execute(
                &format!("DELETE FROM message_details WHERE id IN ({removed})"),
                [],
            )
            .map_err(error)?;
            tx.execute("DELETE FROM messages WHERE channel NOT IN (SELECT json_extract(c.value,'$.id') FROM host_state h,json_each(CAST(h.body AS TEXT),'$.channels') c)",[]).map_err(error)?;
            tx.execute(
                "DELETE FROM message_details WHERE id NOT IN (SELECT id FROM messages)",
                [],
            )
            .map_err(error)?;
            tx.execute("DELETE FROM attachments WHERE channel NOT IN (SELECT json_extract(c.value,'$.id') FROM host_state h,json_each(CAST(h.body AS TEXT),'$.channels') c)",[]).map_err(error)?;
        }
        tx.commit().map_err(error)?;
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
