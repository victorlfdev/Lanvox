use super::*;

impl PlaybackState {
    pub(super) fn next_sample(&mut self) -> [f32; 2] {
        let mixer = &self.mixer;
        let (buf, idx) = (&mut self.buf, &mut self.idx);
        let (render, last_music) = (&mut self.render, &self.last_music);
        let v = self.voice.pull(|| {
            if *idx >= buf.len() {
                mixer.pull(buf, FRAME / 4);
                *idx = 0;
            }
            let next = buf[*idx];
            *idx += 1;
            render.push(next + (last_music[0] + last_music[1]) * 0.5);
            if render.len() == 240 {
                if let Some(processor) = mixer.processor.lock().as_ref() {
                    processor.render(render);
                }
                render.clear();
            }
            [next]
        })[0];
        let (music_buf, music_idx) = (&mut self.music_buf, &mut self.music_idx);
        let m = self.music.pull(|| {
            if *music_idx >= music_buf.len() {
                mixer.pull_streams(music_buf, 240);
                *music_idx = 0;
            }
            let frame = music_buf[*music_idx];
            *music_idx += 1;
            frame
        });
        self.last_music = m;
        [
            soft_limit(v + m[0]).clamp(-1.0, 1.0),
            soft_limit(v + m[1]).clamp(-1.0, 1.0),
        ]
    }
}

pub(super) fn build_output(
    device: &cpal::Device,
    mixer: Arc<Mixer>,
) -> Result<cpal::Stream, String> {
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;
    let mut st = PlaybackState {
        mixer: mixer.clone(),
        voice: {
            let mut r = Resampler::new(RATE as f64, rate as f64);
            r.pos = 1.0;
            r
        },
        buf: Vec::new(),
        idx: 0,
        music: {
            let mut r = Resampler::new(48_000.0, rate as f64);
            r.pos = 1.0;
            r
        },
        music_buf: Vec::new(),
        music_idx: 0,
        last_music: [0.0; 2],
        render: Vec::with_capacity(240),
    };
    let block = (rate as usize / 100).max(1);
    let ring = super::output_ring::OutputRing::new(block * 4);
    let producer = ring.clone();
    let mut consumer = super::output_ring::OutputConsumer {
        ring: ring.clone(),
        mixer: mixer.clone(),
    };
    std::thread::Builder::new()
        .name("audio-mix".into())
        .spawn(move || {
            while !producer.stop.load(Ordering::Acquire) {
                if producer.buffered() < block * 2 {
                    for _ in 0..block {
                        if !producer.push(st.next_sample()) {
                            break;
                        }
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let fault_mixer = mixer.clone();
    let err = move |e| {
        fault_mixer.device_fault.store(true, Ordering::Relaxed);
        ring.stop.store(true, Ordering::Release);
        eprintln!("speaker error: {e}");
    };
    let cfg: cpal::StreamConfig = config.clone().into();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &cfg,
            move |data: &mut [f32], _| {
                for frame in data.chunks_mut(channels) {
                    let v = consumer.next_sample();
                    for (channel, sample) in frame.iter_mut().enumerate() {
                        *sample = if channels == 1 {
                            (v[0] + v[1]) * 0.5
                        } else {
                            v[channel.min(1)]
                        };
                    }
                }
            },
            err,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &cfg,
            move |data: &mut [i16], _| {
                for frame in data.chunks_mut(channels) {
                    let v = consumer.next_sample();
                    for (channel, sample) in frame.iter_mut().enumerate() {
                        let value = if channels == 1 {
                            (v[0] + v[1]) * 0.5
                        } else {
                            v[channel.min(1)]
                        };
                        *sample = (value * 32767.0) as i16;
                    }
                }
            },
            err,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_output_stream(
            &cfg,
            move |data: &mut [u16], _| {
                for frame in data.chunks_mut(channels) {
                    let v = consumer.next_sample();
                    for (channel, sample) in frame.iter_mut().enumerate() {
                        let value = if channels == 1 {
                            (v[0] + v[1]) * 0.5
                        } else {
                            v[channel.min(1)]
                        };
                        *sample = (value * 32767.0 + 32768.0) as u16;
                    }
                }
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
