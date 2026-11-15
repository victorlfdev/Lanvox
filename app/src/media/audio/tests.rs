#[test]
fn smart_vad_adapts_to_floor_and_ignores_single_clicks() {
    let mut vad = SmartVad::new();
    for _ in 0..100 {
        assert!(!vad.update(-70.0, 0.05, SMART_DEFAULT_MARGIN_DB));
    }
    // One loud frame that RNNoise does not call speech is not enough.
    assert!(!vad.update(-20.0, 0.3, SMART_DEFAULT_MARGIN_DB));
    // Sustained speech opens it, and it holds through a short pause.
    assert!(vad.update(-30.0, 0.95, SMART_DEFAULT_MARGIN_DB));
    assert!(vad.update(-30.0, 0.9, SMART_DEFAULT_MARGIN_DB));
    for _ in 0..10 {
        assert!(vad.update(-70.0, 0.05, SMART_DEFAULT_MARGIN_DB));
    }
    for _ in 0..20 {
        vad.update(-70.0, 0.05, SMART_DEFAULT_MARGIN_DB);
    }
    assert!(!vad.update(-70.0, 0.05, SMART_DEFAULT_MARGIN_DB));
    // A noisy room raises the floor so steady noise stays closed.
    let mut noisy = SmartVad::new();
    for _ in 0..600 {
        noisy.update(-45.0, -1.0, SMART_DEFAULT_MARGIN_DB);
    }
    assert!(!noisy.update(-45.0, -1.0, SMART_DEFAULT_MARGIN_DB));
}

use super::*;

#[test]
fn muted_capture_is_never_transmitted_later_as_voice_preroll() {
    use std::net::{TcpListener, TcpStream};
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let outbox = crate::net::outbox::Outbox::start(stream);
    let shared = Arc::new(AudioShared::default());
    shared.processor.configure(false, false);
    shared.set_threshold_db(-100.0);
    shared.muted.store(true, Ordering::Relaxed);
    *shared.sender.lock() = Some(ClientSender::new(outbox.clone()));
    let mixer = Arc::new(Mixer::new());
    let mut capture = CaptureState {
        shared: shared.clone(),
        mixer,
        resampler: Resampler::new(24_000.0, 24_000.0),
        frame: vec![0.25; FRAME],
        quiet_frames: 0,
        was_speaking: false,
        lead_in: std::collections::VecDeque::from([vec![12_000; FRAME]]),
        ptt_release: 0,
        smart: SmartVad::new(),
        encoder: VoiceEncoder::new().unwrap(),
        started: Instant::now(),
    };
    capture.finish_frame();
    assert!(
        capture.lead_in.is_empty(),
        "mute must erase pre-roll and never retain muted samples"
    );
    shared.muted.store(false, Ordering::Relaxed);
    capture.frame = vec![0.25; FRAME];
    capture.finish_frame();
    assert!(matches!(
        crate::protocol::read_packet::<_, ClientMsg>(&mut peer).unwrap(),
        ClientMsg::Speaking(true)
    ));
    assert!(matches!(
        crate::protocol::read_packet::<_, ClientMsg>(&mut peer).unwrap(),
        ClientMsg::AudioEncoded { seq: 0, .. }
    ));
    outbox.close();
    assert!(crate::protocol::read_packet::<_, ClientMsg>(&mut peer).is_err());
}

#[test]
fn capture_makes_20ms_frames_at_24khz_from_48khz() {
    let shared = Arc::new(AudioShared::default());
    let mixer = Arc::new(Mixer::new());
    shared.mic_test.store(true, Ordering::Relaxed);
    shared.processor.configure(false, false);
    shared.set_threshold_db(-100.0);
    let mut st = CaptureState {
        shared: shared.clone(),
        mixer: mixer.clone(),
        resampler: Resampler::new(48_000.0, RATE as f64),
        frame: Vec::new(),
        quiet_frames: VAD_TRAILING_FRAMES,
        was_speaking: false,
        lead_in: std::collections::VecDeque::new(),
        ptt_release: 0,
        smart: SmartVad::new(),
        encoder: VoiceEncoder::new().unwrap(),
        started: Instant::now(),
    };
    // 100 ms of a 440 Hz tone at 48 kHz.
    st.feed((0..4800).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5));
    let mut out = Vec::new();
    mixer.pull(&mut out, FRAME * 5);
    let heard = out.iter().filter(|v| v.abs() > 0.01).count();
    assert!(
        heard > FRAME * 4,
        "expected about 100 ms of audio at 24 kHz, got {heard} samples"
    );
    assert!(shared.level_db() > -10.0);
}
