use super::*;

#[test]
fn a_large_upload_does_not_delay_other_clients_pings() {
    let dir = std::env::temp_dir().join(format!("lanvox-upload-ping-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    let server = start_listener(
        "Test".into(),
        test_listener("127.0.0.1"),
        Some(dir.join("hosted-server.json")),
    )
    .unwrap();
    let (mut aw, mut ar, _a) = client(server.port, "Ana");
    let (mut bw, mut br, _b) = client(server.port, "Ben");
    let pinger = std::thread::spawn(move || {
        let mut worst = Duration::ZERO;
        for i in 0..200u64 {
            let sent = Instant::now();
            write_packet(&mut bw, &ClientMsg::Ping(i)).unwrap();
            wait_for(&mut br, |m| matches!(m, ServerMsg::Pong(n) if *n == i));
            worst = worst.max(sent.elapsed());
            std::thread::sleep(Duration::from_millis(10));
        }
        worst
    });
    let size = 32 * 1024 * 1024usize;
    write_packet(
        &mut aw,
        &ClientMsg::Workspace(WorkspaceCommand::UploadBegin {
            operation: "big".into(),
            channel: 1,
            name: "big.bin".into(),
            media_type: "application/octet-stream".into(),
            size: size as u64,
        }),
    )
    .unwrap();
    wait_for(&mut ar, |m| {
        matches!(
            m,
            ServerMsg::Workspace(
                WorkspaceEvent::UploadProgress { received: 0, .. } | WorkspaceEvent::Error { .. }
            )
        )
    });
    let piece = vec![9u8; ATTACHMENT_CHUNK_BYTES];
    let (mut sent, mut acked) = (0usize, 0usize);
    while acked < size {
        while sent < size && sent - acked < UPLOAD_WINDOW_CHUNKS as usize * ATTACHMENT_CHUNK_BYTES {
            write_packet(
                &mut aw,
                &ClientMsg::Workspace(WorkspaceCommand::UploadChunk {
                    operation: "big".into(),
                    offset: sent as u64,
                    data: piece.clone(),
                }),
            )
            .unwrap();
            sent += piece.len();
        }
        let reply = wait_for(&mut ar, |m| {
            matches!(
                m,
                ServerMsg::Workspace(
                    WorkspaceEvent::UploadProgress { .. } | WorkspaceEvent::Error { .. }
                )
            )
        });
        match reply {
            ServerMsg::Workspace(WorkspaceEvent::UploadProgress { received, .. }) => {
                acked = received as usize
            }
            ServerMsg::Workspace(WorkspaceEvent::Error { reason, .. }) => panic!("{reason}"),
            _ => unreachable!(),
        }
    }
    write_packet(
        &mut aw,
        &ClientMsg::Workspace(WorkspaceCommand::UploadFinish {
            operation: "big".into(),
        }),
    )
    .unwrap();
    let done = wait_for(&mut ar, |m| {
        matches!(
            m,
            ServerMsg::Workspace(WorkspaceEvent::Uploaded { .. } | WorkspaceEvent::Error { .. })
        )
    });
    let ServerMsg::Workspace(WorkspaceEvent::Uploaded { info, .. }) = done else {
        panic!("upload failed: {done:?}")
    };
    assert_eq!(info.size, size as u64);
    // The file comes back piece by piece, byte for byte.
    let mut offset = 0usize;
    while offset < size {
        write_packet(
            &mut aw,
            &ClientMsg::Workspace(WorkspaceCommand::DownloadChunk {
                attachment: info.id,
                offset: offset as u64,
            }),
        )
        .unwrap();
        let reply = wait_for(&mut ar, |m| {
            matches!(
                m,
                ServerMsg::Workspace(
                    WorkspaceEvent::AttachmentChunk { .. } | WorkspaceEvent::Error { .. }
                )
            )
        });
        let ServerMsg::Workspace(WorkspaceEvent::AttachmentChunk {
            offset: at, data, ..
        }) = reply
        else {
            panic!("download failed: {reply:?}")
        };
        assert_eq!(at as usize, offset);
        assert!(!data.is_empty() && data.len() <= DOWNLOAD_PIECE_BYTES);
        assert!(data.iter().all(|b| *b == 9));
        offset += data.len();
    }
    let worst = pinger.join().unwrap();
    assert!(
        worst < Duration::from_millis(400),
        "a ping waited {worst:?} while another client uploaded"
    );
    server.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stopping_a_host_bound_to_another_loopback_address_does_not_hang() {
    let host = start_configured(
        "Bound".into(),
        None,
        ServerOptions {
            bind: "127.0.0.2".into(),
            port: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let port = host.port;
    let pending = TcpStream::connect(("127.0.0.2", port)).unwrap();
    std::thread::sleep(Duration::from_millis(40));
    let start = Instant::now();
    host.stop();
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(pending);
    let restarted = start_configured(
        "Restarted".into(),
        None,
        ServerOptions {
            bind: "127.0.0.2".into(),
            port,
            ..Default::default()
        },
    )
    .unwrap();
    restarted.stop();
}
