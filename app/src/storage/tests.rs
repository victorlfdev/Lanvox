use super::*;
struct TestDirectory(std::path::PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("lanvox-storage-test-{}", rand::random::<u64>()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path())
            && self
                .0
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("lanvox-storage-test-"))
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn state(next: u64) -> serde_json::Value {
    serde_json::json!({"channels":[],"users":[],"messages":[],"public_keys":{},"next_channel":1,"next_user":1,"next_message":next,"workspace":{"details":[],"memberships":[],"conversations":[],"channel_settings":[],"profiles":[],"read":[],"host_owner":null,"invites":[],"bans":[],"next_attachment":0}})
}

#[test]
fn history_over_five_thousand_paginates_and_survives_restart() {
    let dir = TestDirectory::new();
    let path = dir.0.join("host.sqlite3");
    let mut database = Store::open(&path).unwrap();
    let messages: Vec<_> = (1..=6001)
        .map(|id| Message {
            id,
            channel: 1,
            author: 1,
            author_name: "Ana".into(),
            author_color: 0,
            time: id as i64,
            text: format!("mensagem {id}"),
        })
        .collect();
    database.import(&state(6002), &messages).unwrap();
    drop(database);
    let mut database = Store::open(&path).unwrap();
    let latest = database.history(1, None, 100).unwrap();
    assert_eq!(latest.first().unwrap().id, 5902);
    assert_eq!(latest.last().unwrap().id, 6001);
    assert_eq!(
        database
            .history(1, Some(5902), 100)
            .unwrap()
            .last()
            .unwrap()
            .id,
        5901
    );
    assert_eq!(database.message(1).unwrap().unwrap().text, "mensagem 1");
    let saved: serde_json::Value = database.load_state().unwrap().unwrap();
    assert_eq!(saved["next_message"], 6002);
    let mut next = messages.last().unwrap().clone();
    next.id = 6002;
    database.insert(&next, Some("new"), &state(6003)).unwrap();
    assert_eq!(database.operation(1, "new").unwrap(), Some(6002));
}

fn msg(id: u64, channel: u32, author: u32, time: i64, text: &str) -> Message {
    Message {
        id,
        channel,
        author,
        author_name: "x".into(),
        author_color: 0,
        time,
        text: text.into(),
    }
}
fn req(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        limit: 100,
        ..Default::default()
    }
}
fn ids(r: (Vec<Message>, bool)) -> Vec<u64> {
    r.0.iter().map(|m| m.id).collect()
}
fn detail_state(details: serde_json::Value) -> serde_json::Value {
    let mut s = state(100);
    s["workspace"]["details"] = details;
    s
}

#[test]
fn resumable_upload_preserves_chunks_across_restart_and_is_scoped_to_user() {
    let dir = TestDirectory::new();
    let path = dir.0.join("host.sqlite3");
    let data = vec![42; ATTACHMENT_CHUNK_BYTES + 13];
    let hash = ring::digest::digest(&ring::digest::SHA256, &data);
    let mut digest = [0; 32];
    digest.copy_from_slice(hash.as_ref());
    let partial = PartialUpload {
        user: 1,
        operation: "op".into(),
        info: AttachmentInfo {
            id: 7,
            channel: 2,
            uploader: 1,
            name: "a.bin".into(),
            media_type: "application/octet-stream".into(),
            size: data.len() as u64,
        },
        digest,
        created_at: crate::model::now_secs(),
    };
    let mut db = Store::open(&path).unwrap();
    db.begin_resumable(&partial, &state(1)).unwrap();
    db.put_chunk(7, 0, &data[..ATTACHMENT_CHUNK_BYTES]).unwrap();
    assert_eq!(db.attachment_bytes().unwrap(), data.len() as u64);
    drop(db);
    let mut db = Store::open(&path).unwrap();
    assert!(db.resumable(2, "op").unwrap().is_none());
    assert_eq!(
        db.resumable(1, "op").unwrap().unwrap().1,
        ATTACHMENT_CHUNK_BYTES as u64
    );
    db.put_chunk(7, 1, &data[ATTACHMENT_CHUNK_BYTES..]).unwrap();
    assert_eq!(db.partial_digest(7).unwrap(), digest);
    db.finish_chunked(&partial.info, "op", &state(2)).unwrap();
    assert!(db.resumable(1, "op").unwrap().is_none());
    assert_eq!(db.attachment(7).unwrap().unwrap().1, data);
}

#[test]
fn inbox_pages_only_unread_mentions_in_authorized_channels() {
    let dir = TestDirectory::new();
    let mut db = Store::open(&dir.0.join("host.sqlite3")).unwrap();
    let details = serde_json::json!([
        {"message":2,"mentions":[1]}, {"message":3,"mentions":[1]},
        {"message":4,"mentions":[1]}, {"message":5,"mentions":[1]}
    ]);
    let all: Vec<_> = (1..=5)
        .map(|id| {
            msg(
                id,
                if id == 4 { 2 } else { 1 },
                if id == 5 { 1 } else { 2 },
                id as i64,
                "oi",
            )
        })
        .collect();
    db.import(&detail_state(details), &all).unwrap();
    let (first, more) = db.inbox(1, &[(1, 1)], true, None, 1).unwrap();
    assert!(more);
    assert_eq!(first[0].id, 3);
    let (second, more) = db.inbox(1, &[(1, 1)], true, Some(3), 1).unwrap();
    assert!(!more);
    assert_eq!(second[0].id, 2);
}

#[test]
fn chunked_attachments_read_in_ranges_and_unfinished_pieces_are_dropped_on_open() {
    let dir = TestDirectory::new();
    let path = dir.0.join("host.sqlite3");
    let state = serde_json::json!({"x": 1});
    let mut db = Store::open(&path).unwrap();
    let sink = db
        .upload_sink()
        .unwrap()
        .expect("a file database has a sink");
    let chunk = ATTACHMENT_CHUNK_BYTES;
    let data: Vec<u8> = (0..chunk * 2 + 500).map(|i| (i % 253) as u8).collect();
    for (i, part) in data.chunks(chunk).enumerate() {
        sink.put_chunk(7, i as u64, part).unwrap();
    }
    sink.put_chunk(8, 0, b"orphan").unwrap();
    let info = |id: u64, size: u64| AttachmentInfo {
        id,
        channel: 1,
        uploader: 1,
        name: "a.bin".into(),
        media_type: "application/octet-stream".into(),
        size,
    };
    assert!(
        db.finish_chunked(&info(7, data.len() as u64 + 1), "wrong", &state)
            .is_err(),
        "pieces that do not add up to the announced size are refused"
    );
    db.finish_chunked(&info(7, data.len() as u64), "op", &state)
        .unwrap();
    assert_eq!(db.uploaded(1, "op").unwrap().unwrap().id, 7);
    let at = chunk - 10;
    assert_eq!(
        db.read_attachment(7, at as u64, 20).unwrap(),
        data[at..at + 20]
    );
    let tail = data.len() - 5;
    assert_eq!(
        db.read_attachment(7, tail as u64, 100).unwrap(),
        data[tail..]
    );
    assert!(db
        .read_attachment(7, data.len() as u64, 10)
        .unwrap()
        .is_empty());
    assert_eq!(db.attachment(7).unwrap().unwrap().1, data);
    db.upload(&info(9, 11), b"hello world", "legacy", &state)
        .unwrap();
    assert_eq!(db.read_attachment(9, 6, 5).unwrap(), b"world");
    assert!(db.read_attachment(99, 0, 5).is_err());
    let total = data.len() as u64 + 11;
    assert_eq!(db.attachment_bytes().unwrap(), total + 6);
    db.copy_attachment(7, &info(10, data.len() as u64), "copy", &state)
        .unwrap();
    assert_eq!(db.attachment(10).unwrap().unwrap().1, data);
    drop(sink);
    drop(db);
    let db = Store::open(&path).unwrap();
    assert_eq!(
        db.attachment_bytes().unwrap(),
        total + data.len() as u64,
        "pieces of an upload that never finished are removed"
    );
}

#[test]
fn fts5_is_compiled_in() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE VIRTUAL TABLE t USING fts5(x);")
        .unwrap();
}

#[test]
fn migration_v1_to_v2_backfills_and_is_idempotent() {
    let dir = TestDirectory::new();
    let path = dir.0.join("host.sqlite3");
    // Build a genuine v1 database by hand, with data and no FTS objects.
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE host_state (id INTEGER PRIMARY KEY CHECK(id=1), body BLOB NOT NULL);
             CREATE TABLE messages (id INTEGER PRIMARY KEY, channel INTEGER NOT NULL, author INTEGER NOT NULL, text TEXT NOT NULL, time INTEGER NOT NULL, body BLOB NOT NULL);
             CREATE TABLE message_details (id INTEGER PRIMARY KEY, body BLOB NOT NULL);
             CREATE TABLE operations (author INTEGER NOT NULL, operation TEXT NOT NULL, message INTEGER NOT NULL, PRIMARY KEY(author,operation));
             CREATE TABLE attachments (id INTEGER PRIMARY KEY, channel INTEGER NOT NULL, uploader INTEGER NOT NULL, info BLOB NOT NULL, content BLOB NOT NULL);
             CREATE TABLE upload_operations (author INTEGER NOT NULL, operation TEXT NOT NULL, attachment INTEGER NOT NULL, PRIMARY KEY(author,operation));
             PRAGMA user_version=1;",
        )
        .unwrap();
        for m in [
            msg(1, 1, 1, 10, "Reunião amanhã"),
            msg(2, 1, 2, 20, "foto das férias"),
        ] {
            c.execute(
                "INSERT INTO messages VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    m.id as i64,
                    m.channel,
                    m.author,
                    m.text,
                    m.time,
                    serde_json::to_vec(&m).unwrap()
                ],
            )
            .unwrap();
        }
        c.execute(
            "INSERT INTO message_details VALUES(2,?1)",
            [br#"{"message":2,"attachments":[{"id":1}],"mentions":[7]}"#.to_vec()],
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let version: u32 = store
        .connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    // v1 walks the whole chain: FTS5 (v2), then categories (v3).
    assert_eq!(version, SCHEMA_VERSION);
    // Accent-insensitive prefix match over pre-existing rows.
    assert_eq!(
        ids(store.search_advanced(&[1], &req("reuniao")).unwrap()),
        vec![1]
    );
    let mut with_att = req("");
    with_att.has_attachment = Some(true);
    assert_eq!(
        ids(store.search_advanced(&[1], &with_att).unwrap()),
        vec![2]
    );
    let mut mentioned = req("");
    mentioned.mentions = Some(7);
    assert_eq!(
        ids(store.search_advanced(&[1], &mentioned).unwrap()),
        vec![2]
    );
    drop(store);
    // Reopening is a no-op and keeps everything intact.
    let store = Store::open(&path).unwrap();
    assert_eq!(
        ids(store.search_advanced(&[1], &req("ferias")).unwrap()),
        vec![2]
    );
}

/// A hand-built schema v2 database (FTS5 era) whose host blob still has text categories.
fn build_v2(path: &Path, body: &[u8]) {
    let c = Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE host_state (id INTEGER PRIMARY KEY CHECK(id=1), body BLOB NOT NULL);
         CREATE TABLE messages (id INTEGER PRIMARY KEY, channel INTEGER NOT NULL, author INTEGER NOT NULL, text TEXT NOT NULL, time INTEGER NOT NULL, body BLOB NOT NULL);
         CREATE TABLE message_details (id INTEGER PRIMARY KEY, body BLOB NOT NULL);
         CREATE TABLE operations (author INTEGER NOT NULL, operation TEXT NOT NULL, message INTEGER NOT NULL, PRIMARY KEY(author,operation));
         CREATE TABLE attachments (id INTEGER PRIMARY KEY, channel INTEGER NOT NULL, uploader INTEGER NOT NULL, info BLOB NOT NULL, content BLOB NOT NULL);
         CREATE TABLE upload_operations (author INTEGER NOT NULL, operation TEXT NOT NULL, attachment INTEGER NOT NULL, PRIMARY KEY(author,operation));
         PRAGMA user_version=2;",
    )
    .unwrap();
    c.execute("INSERT INTO host_state VALUES(1,?1)", [body.to_vec()])
        .unwrap();
}
fn version(path: &Path) -> u32 {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
fn v2_body() -> serde_json::Value {
    let ch = |id: u32, guild: u32, kind: &str| serde_json::json!({"id":id,"guild":guild,"name":format!("c{id}"),"kind":kind,"topic":""});
    let st = |channel: u32, category: &str, position: i32| serde_json::json!({"channel":channel,"category":category,"position":position,"allowed":[7]});
    serde_json::json!({
        "channels":[ch(1,1,"Text"),ch(2,1,"Voice"),ch(3,1,"Text"),ch(4,1,"Voice"),ch(5,1,"Text"),ch(6,2,"Text")],
        "users":[],"messages":[],"public_keys":{"1":vec![1u8;32]},"next_channel":7,"next_user":2,"next_message":1,
        "workspace":{"details":[],"memberships":[],"conversations":[],
            "channel_settings":[st(1,"Zeta",0),st(2,"Zeta",0),st(3,"Alfa",5),st(4,"Alfa",1),st(6,"Zeta",0)],
            "profiles":[],"read":[],"host_owner":null,"invites":[],"bans":[],"next_attachment":0}})
}

#[test]
fn migration_v2_to_v3_turns_text_categories_into_entities_without_losing_channels() {
    let dir = TestDirectory::new();
    let path = dir.0.join("host.sqlite3");
    let old = v2_body();
    build_v2(&path, &serde_json::to_vec(&old).unwrap());
    // Old data is still readable by the typed loader (new fields default).
    let _: crate::net::server::Saved = serde_json::from_value(old.clone()).unwrap();

    let store = Store::open(&path).unwrap();
    assert_eq!(version(&path), SCHEMA_VERSION);
    let body: serde_json::Value = store.load_state().unwrap().unwrap();
    let _: crate::net::server::Saved = serde_json::from_value(body.clone()).unwrap();
    let ws = &body["workspace"];
    assert_eq!(ws["categories_ready"], true);
    // untouched data survives
    assert_eq!(body["public_keys"], old["public_keys"]);
    assert_eq!(body["channels"], old["channels"]);

    let channels: Vec<crate::model::Channel> =
        serde_json::from_value(body["channels"].clone()).unwrap();
    let settings: Vec<crate::model::ChannelSettings> =
        serde_json::from_value(ws["channel_settings"].clone()).unwrap();
    let cats: Vec<crate::model::Category> =
        serde_json::from_value(ws["categories"].clone()).unwrap();
    // Same text => same category; one per community; order by text like the old sidebar.
    let names: Vec<(u32, &str)> = {
        let mut v: Vec<_> = cats.iter().map(|c| (c.guild, c.name.as_str())).collect();
        v.sort();
        v
    };
    assert_eq!(names, vec![(1, "Alfa"), (1, "Zeta"), (2, "Zeta")]);
    let layout = crate::categories::layout(1, &channels, &settings, &cats);
    assert_eq!(layout[0].channels, vec![5]);
    assert_eq!(layout[1].category.as_ref().unwrap().name, "Alfa");
    assert_eq!(layout[1].channels, vec![3, 4]);
    assert_eq!(layout[2].channels, vec![1, 2]);
    let placed: usize = layout.iter().map(|s| s.channels.len()).sum();
    assert_eq!(placed, 5, "no channel of the community may be lost");
    let other = crate::categories::layout(2, &channels, &settings, &cats);
    assert_eq!(other[1].channels, vec![6]);
    assert!(settings.iter().all(|s| s.category.is_empty()));
    // access lists are preserved
    assert_eq!(
        settings.iter().find(|s| s.channel == 3).unwrap().allowed,
        vec![7]
    );
    drop(store);

    // Idempotent: reopening changes nothing, and the migration function is a no-op on its output.
    let again = Store::open(&path).unwrap();
    let body2: serde_json::Value = again.load_state().unwrap().unwrap();
    assert_eq!(body, body2);
    let mut copy = body.clone();
    assert!(!migrate_categories(&mut copy).unwrap());
    assert_eq!(copy, body);
}

#[test]
fn migration_v2_to_v3_is_atomic_and_v3_backups_validate() {
    let dir = TestDirectory::new();
    // A blob the migration cannot handle must leave the database at v2, untouched.
    let broken = dir.0.join("broken.sqlite3");
    let mut value = v2_body();
    value["channels"] = serde_json::json!("not a list");
    build_v2(&broken, &serde_json::to_vec(&value).unwrap());
    assert!(Store::open(&broken).is_err());
    assert_eq!(version(&broken), 2);
    // A migrated database passes validation and backs up; a newer one is refused.
    let path = dir.0.join("ok.sqlite3");
    build_v2(&path, &serde_json::to_vec(&v2_body()).unwrap());
    drop(Store::open(&path).unwrap());
    backup_database(&path, &dir.0.join("backup.sqlite3")).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute_batch(&format!("PRAGMA user_version={};", SCHEMA_VERSION + 1))
        .unwrap();
    assert!(backup_database(&path, &dir.0.join("newer.sqlite3")).is_err());
    assert!(Store::open(&path).is_err());
}

#[test]
fn index_follows_insert_edit_delete_and_filters_combine() {
    let dir = TestDirectory::new();
    let mut db = Store::open(&dir.0.join("host.sqlite3")).unwrap();
    db.import(&state(1), &[]).unwrap();
    let details = serde_json::json!([
        {"message":2,"reply_to":null,"edited":null,"attachments":[{"id":1,"channel":1,"uploader":2,"name":"a","media_type":"x","size":1}],"reactions":[],"mentions":[1],"pinned":false}
    ]);
    db.insert(&msg(1, 1, 1, 100, "bom dia pessoal"), None, &state(2))
        .unwrap();
    db.insert(
        &msg(2, 1, 2, 200, "bom almoço @Ana"),
        None,
        &detail_state(details),
    )
    .unwrap();
    db.insert(&msg(3, 2, 2, 300, "bom jantar"), None, &state(4))
        .unwrap();
    assert_eq!(
        ids(db.search_advanced(&[1, 2], &req("bom")).unwrap()),
        vec![3, 2, 1]
    );
    // Channel scoping is enforced by the caller-supplied list.
    assert_eq!(
        ids(db.search_advanced(&[1], &req("bom")).unwrap()),
        vec![2, 1]
    );
    let mut r = req("bom");
    r.author = Some(2);
    assert_eq!(ids(db.search_advanced(&[1, 2], &r).unwrap()), vec![3, 2]);
    r.after = Some(250);
    assert_eq!(ids(db.search_advanced(&[1, 2], &r).unwrap()), vec![3]);
    let mut r = req("bom");
    r.before = Some(200);
    r.order = SearchOrder::Oldest;
    assert_eq!(ids(db.search_advanced(&[1, 2], &r).unwrap()), vec![1, 2]);
    let mut r = req("");
    r.mentions = Some(1);
    assert_eq!(ids(db.search_advanced(&[1, 2], &r).unwrap()), vec![2]);
    r = req("bom");
    r.has_attachment = Some(false);
    assert_eq!(ids(db.search_advanced(&[1, 2], &r).unwrap()), vec![3, 1]);
    // Edit moves the message in the index; delete removes it.
    db.update(&msg(1, 1, 1, 100, "tchau pessoal"), &state(4))
        .unwrap();
    assert_eq!(
        ids(db.search_advanced(&[1, 2], &req("bom")).unwrap()),
        vec![3, 2]
    );
    assert_eq!(
        ids(db.search_advanced(&[1, 2], &req("tchau")).unwrap()),
        vec![1]
    );
    db.delete(2, &state(4)).unwrap();
    assert_eq!(
        ids(db.search_advanced(&[1, 2], &req("bom")).unwrap()),
        vec![3]
    );
    let mut r = req("");
    r.mentions = Some(1);
    assert!(db.search_advanced(&[1, 2], &r).unwrap().0.is_empty());
}

#[test]
fn search_pages_and_hostile_input_is_inert() {
    let dir = TestDirectory::new();
    let mut db = Store::open(&dir.0.join("host.sqlite3")).unwrap();
    let all: Vec<_> = (1..=25)
        .map(|i| msg(i, 1, 1, i as i64, "olá mundo"))
        .collect();
    db.import(&state(26), &all).unwrap();
    let mut r = req("mundo");
    r.limit = 10;
    let (p1, more1) = db.search_advanced(&[1], &r).unwrap();
    assert_eq!((p1.len(), more1, p1[0].id), (10, true, 25));
    r.offset = 20;
    let (p3, more3) = db.search_advanced(&[1], &r).unwrap();
    assert_eq!((p3.len(), more3), (5, false));
    for evil in ["\" OR 1", "mundo*)", "NEAR(", "col:x", "-"] {
        let _ = db.search_advanced(&[1], &req(evil)).unwrap();
    }
}

#[test]
fn search_benchmark_with_many_messages() {
    let dir = TestDirectory::new();
    let mut db = Store::open(&dir.0.join("host.sqlite3")).unwrap();
    let n: u64 = if cfg!(debug_assertions) {
        50_000
    } else {
        200_000
    };
    let words = [
        "alfa", "beta", "gama", "delta", "épsilon", "zeta", "eta", "teta",
    ];
    let all: Vec<_> = (1..=n)
        .map(|i| {
            let text = format!(
                "{} {} {} item{}",
                words[(i % 8) as usize],
                words[(i / 8 % 8) as usize],
                words[(i / 64 % 8) as usize],
                i
            );
            msg(i, (i % 4) as u32 + 1, (i % 50) as u32 + 1, i as i64, &text)
        })
        .collect();
    let t = std::time::Instant::now();
    db.import(&state(n + 1), &all).unwrap();
    let import = t.elapsed();
    let t = std::time::Instant::now();
    let mut hits = 0;
    for _ in 0..20 {
        let mut r = req("item4999");
        r.author = None;
        hits += db.search_advanced(&[1, 2, 3, 4], &r).unwrap().0.len();
        let mut r = req("epsilon beta");
        r.author = Some(7);
        r.order = SearchOrder::Relevance;
        hits += db.search_advanced(&[1, 2, 3, 4], &r).unwrap().0.len();
    }
    let indexed = t.elapsed() / 40;
    let t = std::time::Instant::now();
    let legacy = db.search(1, "item4999", 50).unwrap().len();
    let scan = t.elapsed();
    println!("bench: {n} msgs import={import:?} fts_query={indexed:?} legacy_scan={scan:?} hits={hits} legacy_hits={legacy}");
    assert!(hits > 0);
    assert!(
        indexed < std::time::Duration::from_millis(250),
        "FTS query too slow: {indexed:?}"
    );
}

#[test]
fn failed_insert_rolls_back_message_operation_and_metadata() {
    let dir = TestDirectory::new();
    let path = dir.0.join("host.sqlite3");
    let mut database = Store::open(&path).unwrap();
    database.import(&state(1), &[]).unwrap();
    database.connection.execute_batch("CREATE TRIGGER reject_message BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'disk failure simulation'); END;").unwrap();
    let message = Message {
        id: 1,
        channel: 1,
        author: 1,
        author_name: "Ana".into(),
        author_color: 0,
        time: 0,
        text: "não confirmada".into(),
    };
    assert!(database
        .insert(&message, Some("failed"), &state(2))
        .is_err());
    assert!(database.message(1).unwrap().is_none());
    assert!(database.operation(1, "failed").unwrap().is_none());
    let saved: serde_json::Value = database.load_state().unwrap().unwrap();
    assert_eq!(saved["next_message"], 1);
}

#[test]
fn backup_restore_preserve_data_and_reject_invalid_or_existing_destination() {
    let dir = TestDirectory::new();
    let path = dir.0.join("host.sqlite3");
    let mut database = Store::open(&path).unwrap();
    database
        .import(
            &state(2),
            &[Message {
                id: 1,
                channel: 1,
                author: 1,
                author_name: "Ana".into(),
                author_color: 0,
                time: 0,
                text: "durável".into(),
            }],
        )
        .unwrap();
    let backup = dir.0.join("backup.sqlite3");
    backup_database(&path, &backup).unwrap();
    assert!(backup_database(&path, &backup).is_err());
    let restored = dir.0.join("restored.sqlite3");
    restore_database(&backup, &restored).unwrap();
    let restored = Store::open(&restored).unwrap();
    assert_eq!(restored.message(1).unwrap().unwrap().text, "durável");
    let bad = dir.0.join("bad.sqlite3");
    std::fs::write(&bad, b"not sqlite").unwrap();
    let destination = dir.0.join("untouched.sqlite3");
    assert!(restore_database(&bad, &destination).is_err());
    assert!(!destination.exists());
    assert_eq!(std::fs::read(bad).unwrap(), b"not sqlite");
}
