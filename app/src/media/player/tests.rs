use super::*;

fn private_mb() -> u64 {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Get-Process -Id {}).PrivateMemorySize64",
                std::process::id()
            ),
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        / (1024 * 1024)
}

/// Memory cost of N simultaneous players: `VIDEO_TEST_FILE=clip.mp4 VIDEO_TEST_COUNT=2 ...`.
#[test]
#[ignore]
fn memory_with_players() {
    let path = std::env::var("VIDEO_TEST_FILE").expect("VIDEO_TEST_FILE");
    let count: usize = std::env::var("VIDEO_TEST_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    println!("baseline private MB: {}", private_mb());
    let players: Vec<_> = (0..count)
        .map(|_| VideoPlayer::start(VideoSource::File(path.clone().into())))
        .collect();
    let started = Instant::now();
    let mut peak = 0;
    while started.elapsed() < Duration::from_secs(6) {
        for p in &players {
            p.touch();
        }
        std::thread::sleep(Duration::from_millis(500));
        peak = peak.max(private_mb());
    }
    println!("{count} player(s): peak private MB {peak}");
}

/// Plays a real file headlessly: `VIDEO_TEST_FILE=clip.mp4 cargo test player -- --ignored`.
#[test]
#[ignore]
fn plays_file_to_the_end() {
    let path = std::env::var("VIDEO_TEST_FILE").expect("VIDEO_TEST_FILE");
    let player = VideoPlayer::start(if path.starts_with("http") {
        VideoSource::Url(path)
    } else if std::env::var("VIDEO_TEST_AS_FILE").is_ok() {
        // The path large chat attachments take: played straight from the media cache.
        VideoSource::File(path.into())
    } else {
        VideoSource::Bytes {
            data: std::fs::read(&path).unwrap(),
            name: path,
        }
    });
    let (mut frames, mut seen) = (0, 0);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        player.touch();
        if let Some(f) = player.frame_after(seen) {
            seen = f.seq;
            frames += 1;
            assert_eq!(f.rgba.len(), f.size[0] * f.size[1] * 4);
        }
        if player.state() == PlayState::Ended || player.state() == PlayState::Failed {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "state={:?} frames={frames} dur={:?} pos={:?} err={}",
        player.state(),
        player.duration(),
        player.position(),
        player.error()
    );
    assert_eq!(player.state(), PlayState::Ended, "{}", player.error());
    assert!(frames > 20);
    player.seek(Duration::from_secs(1));
    std::thread::sleep(Duration::from_millis(600));
    println!(
        "after seek: {:?} pos={:?}",
        player.state(),
        player.position()
    );
    assert!(player.position() >= Duration::from_secs(1));
}

#[test]
#[ignore]
fn thumbnail_and_download() {
    let path = std::env::var("VIDEO_TEST_FILE").expect("VIDEO_TEST_FILE");
    let f = thumbnail(VideoSource::Bytes {
        data: std::fs::read(&path).unwrap(),
        name: path,
    })
    .unwrap();
    println!(
        "thumb {:?} nonblack={}",
        f.size,
        f.rgba.iter().any(|b| *b > 40)
    );
    assert!(f.rgba.chunks(4).any(|p| p[0] > 40 || p[1] > 40));
    let jpg = download("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg").unwrap();
    println!("jpg {} bytes", jpg.len());
    assert!(jpg.len() > 1000);
}
