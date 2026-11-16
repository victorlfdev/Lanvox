use super::*;

pub(super) fn write_state<T: Serialize>(
    tx: &rusqlite::Transaction<'_>,
    state: &T,
) -> io::Result<()> {
    let mut value = serde_json::to_value(state).map_err(error)?;
    // Per-message details live with their own rows, not in the growing host metadata blob.
    if let Some(details) = value
        .get_mut("workspace")
        .and_then(|v| v.get_mut("details"))
    {
        if let Some(entries) = details.as_array() {
            for entry in entries {
                let id = entry
                    .get("message")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| error("Invalid message details"))?;
                tx.execute("INSERT INTO message_details(id,body) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body WHERE body<>excluded.body",params![id as i64,serde_json::to_vec(entry).map_err(error)?]).map_err(error)?;
            }
        }
        *details = serde_json::json!([]);
    }
    if let Some(next) = value.get("next_message").and_then(|v| v.as_i64()) {
        tx.execute("INSERT INTO runtime_state VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET next_message=MAX(next_message,excluded.next_message)",[next]).map_err(error)?;
    }
    if let Some(workspace) = value.get("workspace") {
        for (table, field) in [("channel_reads", "read"), ("last_sends", "last_sends")] {
            if let Some(entries) = workspace.get(field).and_then(|v| v.as_array()) {
                tx.execute(&format!("DELETE FROM {table}"), [])
                    .map_err(error)?;
                for entry in entries {
                    let values: Vec<i64> = entry
                        .as_array()
                        .ok_or_else(|| error("Invalid runtime state"))?
                        .iter()
                        .map(|v| v.as_i64().ok_or_else(|| error("Invalid runtime value")))
                        .collect::<io::Result<_>>()?;
                    if values.len() != 3 {
                        return Err(error("Invalid runtime entry"));
                    }
                    tx.execute(
                        &format!("INSERT INTO {table} VALUES(?1,?2,?3)"),
                        params![values[0], values[1], values[2]],
                    )
                    .map_err(error)?;
                }
            }
        }
    }
    // Only channels explicitly removed from a previously saved state are collected.
    if let Some(channels) = value.get("channels").and_then(|v| v.as_array()) {
        let previous: Option<Vec<u8>> = tx
            .query_row("SELECT body FROM host_state WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(error)?;
        if let Some(previous) = previous {
            let previous: serde_json::Value = serde_json::from_slice(&previous).map_err(error)?;
            let current: std::collections::HashSet<_> =
                channels.iter().filter_map(|c| c["id"].as_u64()).collect();
            if let Some(old) = previous.get("channels").and_then(|v| v.as_array()) {
                for channel in old
                    .iter()
                    .filter_map(|c| c["id"].as_u64())
                    .filter(|id| !current.contains(id))
                {
                    tx.execute("DELETE FROM message_details WHERE id IN (SELECT id FROM messages WHERE channel=?1)",[channel as i64]).map_err(error)?;
                    tx.execute("DELETE FROM messages WHERE channel=?1", [channel as i64])
                        .map_err(error)?;
                    tx.execute("DELETE FROM attachments WHERE channel=?1", [channel as i64])
                        .map_err(error)?;
                    tx.execute("DELETE FROM attachment_chunks WHERE owner IN (SELECT owner FROM resumable_uploads WHERE json_extract(metadata,'$.info.channel')=?1)",[channel as i64]).map_err(error)?;
                    tx.execute("DELETE FROM resumable_uploads WHERE json_extract(metadata,'$.info.channel')=?1",[channel as i64]).map_err(error)?;
                }
            }
        }
    }
    tx.execute("INSERT INTO host_state(id,body) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET body=excluded.body",[serde_json::to_vec(&value).map_err(error)?]).map_err(error)?;
    Ok(())
}

pub fn backup_database(path: &Path, destination: &Path) -> io::Result<()> {
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Banco de dados não encontrado",
        ));
    }
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(error)?;
    validate_database(&connection)?;
    // Reserve the destination atomically; never overwrite an existing database.
    let reservation = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    drop(reservation);
    if let Err(e) = connection
        .backup("main", destination, None::<fn(rusqlite::backup::Progress)>)
        .map_err(error)
    {
        let _ = std::fs::remove_file(destination);
        return Err(e);
    }
    Ok(())
}

pub fn restore_database(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Pare o host e escolha um destino vazio",
        ));
    }
    backup_database(source, destination)
}

pub(super) fn validate_database(connection: &Connection) -> io::Result<()> {
    let check: String = connection
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(error)?;
    if check != "ok" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Banco de dados corrompido",
        ));
    }
    let body: Vec<u8> = connection
        .query_row("SELECT body FROM host_state WHERE id=1", [], |r| r.get(0))
        .map_err(error)?;
    let _: crate::net::server::Saved = serde_json::from_slice(&body).map_err(error)?;
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(error)?;
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Versão de backup Lanvox incompatível",
        ));
    }
    for table in [
        "messages",
        "message_details",
        "operations",
        "attachments",
        "upload_operations",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )
            .map_err(error)?;
        if !exists {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Esquema Lanvox incompleto",
            ));
        }
    }
    Ok(())
}
