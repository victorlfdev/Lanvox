use super::*;

impl AudioShared {
    pub fn set_mode(&self, mode: InputMode) {
        self.push_to_talk
            .store(mode == InputMode::PushToTalk, Ordering::Relaxed);
        self.smart_vad
            .store(mode == InputMode::SmartVoice, Ordering::Relaxed);
    }
    /// How far above the room's noise floor speech must rise in smart mode.
    pub fn set_smart_margin_db(&self, db: f32) {
        f32_store(&self.smart_margin_db, db.clamp(6.0, 35.0));
    }
    pub fn set_threshold_db(&self, db: f32) {
        f32_store(&self.threshold_db, db);
    }
    pub fn level_db(&self) -> f32 {
        f32_load(&self.level_db)
    }
    pub fn set_input_gain(&self, g: f32) {
        f32_store(&self.input_gain, g);
    }

    pub(super) fn ptt_held(&self) -> bool {
        if self.ptt_window.load(Ordering::Relaxed) {
            return true;
        }
        global_key_down(self.ptt_vk.load(Ordering::Relaxed))
    }
}

impl Default for AudioShared {
    fn default() -> Self {
        let s = Self {
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            push_to_talk: AtomicBool::new(false),
            smart_vad: AtomicBool::new(false),
            threshold_db: AtomicU32::new(0),
            smart_margin_db: AtomicU32::new(0),
            level_db: AtomicU32::new(0),
            speaking: AtomicBool::new(false),
            ptt_window: AtomicBool::new(false),
            ptt_vk: AtomicU32::new(0),
            mic_test: AtomicBool::new(false),
            sender: Mutex::new(None),
            error: Mutex::new(None),
            seq: AtomicU32::new(0),
            input_gain: AtomicU32::new(0),
            device_fault: AtomicBool::new(false),
            processor: crate::media::processing::AudioProcessor::new(),
        };
        f32_store(&s.threshold_db, VAD_THRESHOLD_DB);
        f32_store(&s.smart_margin_db, SMART_DEFAULT_MARGIN_DB);
        f32_store(&s.level_db, -100.0);
        f32_store(&s.input_gain, 1.0);
        s
    }
}

#[cfg(windows)]
pub(super) fn global_key_down(vk: u32) -> bool {
    if vk == 0 {
        return false;
    }
    // High bit set means the key is down right now, whichever window has focus.
    let state =
        unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) };
    (state as u16 & 0x8000) != 0
}

#[cfg(not(windows))]
pub(super) fn global_key_down(_vk: u32) -> bool {
    false
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

impl AudioEngine {
    /// Open the microphone and speakers (named, or the system defaults).
    pub fn start(
        shared: Arc<AudioShared>,
        mixer: Arc<Mixer>,
        input: Option<String>,
        output: Option<String>,
    ) -> AudioEngine {
        *mixer.processor.lock() = Some(shared.processor.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let mut last_problem = None;
                while !stop2.load(Ordering::SeqCst) {
                    shared.device_fault.store(false, Ordering::Relaxed);
                    mixer.device_fault.store(false, Ordering::Relaxed);
                    let host = cpal::default_host();
                    let mut problems = Vec::new();
                    let in_dev = match &input {
                        Some(name) => host
                            .input_devices()
                            .ok()
                            .and_then(|mut it| {
                                it.find(|d| d.name().ok().as_deref() == Some(name.as_str()))
                            })
                            .or_else(|| host.default_input_device()),
                        None => host.default_input_device(),
                    };
                    let out_dev = match &output {
                        Some(name) => host
                            .output_devices()
                            .ok()
                            .and_then(|mut it| {
                                it.find(|d| d.name().ok().as_deref() == Some(name.as_str()))
                            })
                            .or_else(|| host.default_output_device()),
                        None => host.default_output_device(),
                    };
                    let in_stream = match in_dev {
                        Some(d) => match build_input(&d, shared.clone(), mixer.clone()) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                problems.push(format!("Microphone: {e}"));
                                None
                            }
                        },
                        None => {
                            problems.push("No microphone found.".into());
                            None
                        }
                    };
                    let out_stream = match out_dev {
                        Some(d) => match build_output(&d, mixer.clone()) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                problems.push(format!("Speakers: {e}"));
                                None
                            }
                        },
                        None => {
                            problems.push("No speakers found.".into());
                            None
                        }
                    };
                    let problem = if problems.is_empty() {
                        None
                    } else {
                        Some(problems.join(" "))
                    };
                    if problem != last_problem {
                        *shared.error.lock() = problem.clone();
                        last_problem = problem.clone();
                    }
                    let opened = Instant::now();
                    while !stop2.load(Ordering::SeqCst)
                        && !shared.device_fault.load(Ordering::Relaxed)
                        && !mixer.device_fault.load(Ordering::Relaxed)
                        && (problem.is_none() || opened.elapsed() < Duration::from_secs(3))
                    {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    drop(in_stream);
                    drop(out_stream);
                    if !stop2.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            })
            .ok();
        AudioEngine { stop }
    }
}
