use super::*;

impl CaptureState {
    pub(super) fn feed(&mut self, mono: impl Iterator<Item = f32>) {
        let gain = f32_load(&self.shared.input_gain);
        for s in mono {
            let s = s * gain;
            let frame = &mut self.frame;
            self.resampler.push([s], |v| frame.push(v[0]));
            while self.frame.len() >= FRAME {
                let rest = self.frame.split_off(FRAME);
                self.finish_frame();
                self.frame = rest;
            }
        }
    }

    pub(super) fn finish_frame(&mut self) {
        let sh = &self.shared;
        if let Err(error) = sh.processor.process_capture(&mut self.frame) {
            *sh.error.lock() = Some(error);
        }
        let rms = (self.frame.iter().map(|v| v * v).sum::<f32>() / FRAME as f32).sqrt();
        let db = if rms > 0.0 {
            20.0 * rms.log10()
        } else {
            -100.0
        };
        f32_store(&sh.level_db, db.max(-100.0));

        let push_to_talk = sh.push_to_talk.load(Ordering::Relaxed);
        if push_to_talk {
            self.lead_in.clear();
            self.smart.reset();
        }
        let lead_frames = if sh.smart_vad.load(Ordering::Relaxed) {
            SMART_LEADING_FRAMES
        } else {
            VAD_LEADING_FRAMES
        };
        let active = if push_to_talk {
            if sh.ptt_held() {
                self.ptt_release = PTT_RELEASE_DELAY_MS.div_ceil(20);
                true
            } else if self.ptt_release > 0 {
                self.ptt_release -= 1;
                true
            } else {
                false
            }
        } else if sh.smart_vad.load(Ordering::Relaxed) {
            self.smart.update(
                db,
                sh.processor.speech_probability(),
                f32_load(&sh.smart_margin_db),
            )
        } else if db > f32_load(&sh.threshold_db) {
            self.quiet_frames = 0;
            true
        } else {
            self.quiet_frames += 1;
            self.quiet_frames < VAD_TRAILING_FRAMES
        };
        let silenced = sh.muted.load(Ordering::Relaxed) || sh.deafened.load(Ordering::Relaxed);
        let speaking = active && !silenced;

        let pcm: Vec<i16> = self
            .frame
            .iter()
            .map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        if sh.mic_test.load(Ordering::Relaxed) && active {
            self.mixer.push(SELF_TEST_ID, 0, &pcm);
        }
        let sender = sh.sender.lock().clone();
        if let Some(sender) = sender {
            if speaking != self.was_speaking {
                sender.send(&ClientMsg::Speaking(speaking));
            }
            if speaking {
                // Send the lead-in first when speech starts.
                if !self.was_speaking {
                    for lead in self.lead_in.drain(..) {
                        let seq = sh.seq.fetch_add(1, Ordering::Relaxed);
                        if let Ok(opus) = self.encoder.encode(&lead) {
                            sender.send(&ClientMsg::AudioEncoded {
                                seq,
                                sent_micros: self.started.elapsed().as_micros() as u64,
                                opus,
                            });
                        }
                    }
                }
                let seq = sh.seq.fetch_add(1, Ordering::Relaxed);
                if let Ok(opus) = self.encoder.encode(&pcm) {
                    sender.send(&ClientMsg::AudioEncoded {
                        seq,
                        sent_micros: self.started.elapsed().as_micros() as u64,
                        opus,
                    });
                }
            } else if !silenced && !push_to_talk {
                self.lead_in.push_back(pcm);
                while self.lead_in.len() > lead_frames {
                    self.lead_in.pop_front();
                }
            } else {
                // Audio captured while muted/deafened must never enter the
                // pre-roll that is transmitted when the microphone is reopened.
                self.lead_in.clear();
            }
        }
        sh.speaking.store(speaking, Ordering::Relaxed);
        self.was_speaking = speaking;
        self.frame.clear();
    }
}

pub(super) fn build_input(
    device: &cpal::Device,
    shared: Arc<AudioShared>,
    mixer: Arc<Mixer>,
) -> Result<cpal::Stream, String> {
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;
    let fault_shared = shared.clone();
    let mut st = CaptureState {
        shared,
        mixer,
        resampler: Resampler::new(rate as f64, RATE as f64),
        frame: Vec::with_capacity(FRAME),
        quiet_frames: VAD_TRAILING_FRAMES,
        was_speaking: false,
        lead_in: std::collections::VecDeque::new(),
        ptt_release: 0,
        smart: SmartVad::new(),
        encoder: VoiceEncoder::new()?,
        started: Instant::now(),
    };
    let err = move |e| {
        fault_shared.device_fault.store(true, Ordering::Relaxed);
        *fault_shared.error.lock() = Some(format!(
            "Microfone desconectado: {e}. Tentando recuperar o dispositivo."
        ));
    };
    let cfg: cpal::StreamConfig = config.clone().into();
    let stream =
        match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &cfg,
                move |data: &[f32], _| {
                    st.feed(
                        data.chunks(channels)
                            .map(|c| c.iter().sum::<f32>() / channels as f32),
                    )
                },
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &cfg,
                move |data: &[i16], _| {
                    st.feed(data.chunks(channels).map(|c| {
                        c.iter().map(|s| *s as f32 / 32768.0).sum::<f32>() / channels as f32
                    }))
                },
                err,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &cfg,
                move |data: &[u16], _| {
                    st.feed(data.chunks(channels).map(|c| {
                        c.iter()
                            .map(|s| (*s as f32 - 32768.0) / 32768.0)
                            .sum::<f32>()
                            / channels as f32
                    }))
                },
                err,
                None,
            ),
            other => return Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}
