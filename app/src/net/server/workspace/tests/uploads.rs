use super::*;

pub(super) fn chunked_fixture(with_store: bool) -> (State, ChannelId) {
    let mut st = fixture();
    if with_store {
        let mut store = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
        store.import(&st.saved(false), &[]).unwrap();
        let shared = Arc::new(Mutex::new(store));
        st.pending_uploads.attach(shared.clone(), None);
        st.store = Some(shared);
    }
    st.workspace_command(
        1,
        WorkspaceCommand::OpenConversation {
            name: "".into(),
            members: vec![2],
            direct: true,
        },
    )
    .unwrap();
    let channel = st.workspace.conversations[0].channel;
    (st, channel)
}

pub(super) fn begin(st: &mut State, channel: ChannelId, size: u64) -> std::io::Result<()> {
    st.workspace_command(
        1,
        WorkspaceCommand::UploadBegin {
            operation: "big".into(),
            channel,
            name: "big.bin".into(),
            media_type: "application/octet-stream".into(),
            size,
        },
    )
}

#[test]
fn resumable_upload_rejects_changed_content_and_finishes_after_reconnect() {
    let (mut st, channel) = chunked_fixture(true);
    let data = vec![19; ATTACHMENT_CHUNK_BYTES + 9];
    let mut digest = [0u8; 32];
    digest.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, &data).as_ref());
    let begin = |digest| WorkspaceCommand::UploadBeginResumable {
        operation: "resume".into(),
        channel,
        name: "a.bin".into(),
        media_type: "application/octet-stream".into(),
        size: data.len() as u64,
        digest,
    };
    st.workspace_command(1, begin(digest)).unwrap();
    st.workspace_command(
        1,
        WorkspaceCommand::UploadChunk {
            operation: "resume".into(),
            offset: 0,
            data: data[..ATTACHMENT_CHUNK_BYTES].to_vec(),
        },
    )
    .unwrap();
    assert!(st.workspace_command(1, begin([0; 32])).is_err());
    // Lose the in-memory router state like a host restart while keeping the SQLite reservation.
    st.pending_uploads.take(1, "resume").unwrap();
    st.workspace_command(1, begin(digest)).unwrap();
    st.workspace_command(
        1,
        WorkspaceCommand::UploadChunk {
            operation: "resume".into(),
            offset: ATTACHMENT_CHUNK_BYTES as u64,
            data: data[ATTACHMENT_CHUNK_BYTES..].to_vec(),
        },
    )
    .unwrap();
    st.workspace_command(
        1,
        WorkspaceCommand::UploadFinish {
            operation: "resume".into(),
        },
    )
    .unwrap();
    let id = st.workspace.next_attachment - 1;
    assert_eq!(st.read_attachment_chunk(id, 0, data.len()).unwrap(), data);
}

#[test]
fn chunked_upload_is_stored_in_pieces_and_read_back_in_pieces() {
    for with_store in [true, false] {
        let (mut st, channel) = chunked_fixture(with_store);
        let data: Vec<u8> = (0..(ATTACHMENT_CHUNK_BYTES * 2 + 12_345))
            .map(|i| (i % 251) as u8)
            .collect();
        begin(&mut st, channel, data.len() as u64).unwrap();
        let mut offset = 0;
        for piece in data.chunks(ATTACHMENT_CHUNK_BYTES) {
            st.workspace_command(
                1,
                WorkspaceCommand::UploadChunk {
                    operation: "big".into(),
                    offset: offset as u64,
                    data: piece.to_vec(),
                },
            )
            .unwrap();
            offset += piece.len();
        }
        st.workspace_command(
            1,
            WorkspaceCommand::UploadFinish {
                operation: "big".into(),
            },
        )
        .unwrap();
        assert!(st.pending_uploads.is_empty());
        let id = st.workspace.next_attachment - 1;
        let info = st.find_attachment_info(id).unwrap();
        assert_eq!(
            (info.size, info.uploader, info.channel),
            (data.len() as u64, 1, channel)
        );
        let mut back = Vec::new();
        while (back.len() as u64) < info.size {
            let part = st
                .read_attachment_chunk(id, back.len() as u64, ATTACHMENT_CHUNK_BYTES)
                .unwrap();
            assert!(!part.is_empty());
            back.extend(part);
        }
        assert_eq!(back, data);
        st.workspace_command(
            2,
            WorkspaceCommand::DownloadChunk {
                attachment: id,
                offset: 0,
            },
        )
        .unwrap();
        assert!(st
            .workspace_command(
                3,
                WorkspaceCommand::DownloadChunk {
                    attachment: id,
                    offset: 0
                }
            )
            .is_err());
        // Replaying the finished operation is idempotent: no second attachment appears.
        begin(&mut st, channel, data.len() as u64).unwrap();
        assert_eq!(st.workspace.next_attachment - 1, id);
        assert!(st.pending_uploads.is_empty());
    }
}

#[test]
fn chunked_upload_rejects_bad_sequences_and_oversize_files() {
    let (mut st, channel) = chunked_fixture(true);
    assert!(begin(&mut st, channel, 0).is_err());
    assert!(begin(&mut st, channel, MAX_ATTACHMENT_BYTES as u64 + 1).is_err());
    let piece = ATTACHMENT_CHUNK_BYTES;
    begin(&mut st, channel, piece as u64 + 10).unwrap();
    let chunk = |offset: u64, len: usize| WorkspaceCommand::UploadChunk {
        operation: "big".into(),
        offset,
        data: vec![7; len],
    };
    assert!(
        st.workspace_command(1, chunk(5, 5)).is_err(),
        "out of order"
    );
    assert!(
        st.workspace_command(1, chunk(0, piece + 1)).is_err(),
        "bigger than a piece"
    );
    assert!(
        st.workspace_command(1, chunk(0, 4)).is_err(),
        "short piece before the end"
    );
    st.workspace_command(1, chunk(0, piece)).unwrap();
    assert!(
        st.workspace_command(1, chunk(piece as u64, 11)).is_err(),
        "past the announced size"
    );
    assert!(
        st.workspace_command(
            1,
            WorkspaceCommand::UploadFinish {
                operation: "big".into()
            }
        )
        .is_err(),
        "incomplete"
    );
    assert!(
        st.pending_uploads.is_empty(),
        "failed finish drops the upload"
    );
    assert_eq!(st.stored_attachment_bytes().unwrap(), 0);
    assert!(
        st.workspace_command(1, chunk(0, piece)).is_err(),
        "no begin"
    );
    // Legacy single-message uploads stay capped at the inline limit.
    assert!(st
        .workspace_command(
            1,
            WorkspaceCommand::Upload {
                operation: "legacy".into(),
                channel,
                name: "x.bin".into(),
                media_type: "application/octet-stream".into(),
                bytes: vec![1; MAX_INLINE_ATTACHMENT_BYTES + 1],
            }
        )
        .is_err());
    // Someone outside the conversation cannot upload into it.
    assert!(st
        .workspace_command(
            3,
            WorkspaceCommand::UploadBegin {
                operation: "x".into(),
                channel,
                name: "x".into(),
                media_type: "text/plain".into(),
                size: 4
            }
        )
        .is_err());
    // Aborting releases the reservation.
    begin(&mut st, channel, 10).unwrap();
    assert_eq!(
        st.stored_attachment_bytes().unwrap(),
        10,
        "announced bytes count against the quota"
    );
    st.workspace_command(
        1,
        WorkspaceCommand::UploadAbort {
            operation: "big".into(),
        },
    )
    .unwrap();
    assert_eq!(st.stored_attachment_bytes().unwrap(), 0);
}
