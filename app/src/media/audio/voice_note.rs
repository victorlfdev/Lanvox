//! Local, explicitly initiated voice-note recording. Microphone samples are never sent
//! to the host until the user accepts the preview and sends the attachment.
use super::*;

const MAX_SECONDS: usize = 120;
const MAX_SAMPLES: usize = RATE as usize * MAX_SECONDS;

struct RecordingData {
    stop: AtomicBool,
    ready: AtomicBool,
    finished: AtomicBool,
    samples: Mutex<Vec<i16>>,
    error: Mutex<Option<String>>,
    voice: Arc<AudioShared>,
}

/// A microphone worker. Dropping it cancels and clears all captured samples.
pub struct Recorder {
    shared: Arc<RecordingData>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Recorder {
    pub fn start(input: Option<String>, voice: Arc<AudioShared>) -> Result<Self, String> {
        let shared = Arc::new(RecordingData {
            stop: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            samples: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            voice,
        });
        let work = shared.clone();
        let worker = std::thread::Builder::new()
            .name("voice-note".into())
            .spawn(move || {
                if let Err(error) = record(input, work.clone()) {
                    *work.error.lock() = Some(error);
                }
                work.finished.store(true, Ordering::Release);
            })
            .map_err(|e| format!("Não foi possível iniciar a gravação: {e}"))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub fn ready(&self) -> bool {
        self.shared.ready.load(Ordering::Acquire)
    }
    pub fn finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }
    pub fn duration_seconds(&self) -> usize {
        self.shared.samples.lock().len() / RATE as usize
    }

    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "A gravação foi interrompida inesperadamente.".to_owned())?;
        }
        if let Some(error) = self.shared.error.lock().take() {
            return Err(error);
        }
        let samples = std::mem::take(&mut *self.shared.samples.lock());
        if samples.is_empty() {
            return Err(
                "Nenhum áudio foi gravado. Verifique o microfone e tente novamente.".into(),
            );
        }
        Ok(wav(&samples))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.shared.samples.lock().clear();
    }
}

struct Capture {
    channels: usize,
    converter: Resampler<1>,
    shared: Arc<RecordingData>,
}

impl Capture {
    fn feed(&mut self, mono: impl Iterator<Item = f32>) {
        if self.shared.stop.load(Ordering::Acquire)
            || self.shared.voice.muted.load(Ordering::Relaxed)
            || self.shared.voice.deafened.load(Ordering::Relaxed)
        {
            self.shared.stop.store(true, Ordering::Release);
            return;
        }
        let mut samples = self.shared.samples.lock();
        for value in mono {
            self.converter.push([value], |[sample]| {
                if samples.len() < MAX_SAMPLES {
                    samples.push((sample.clamp(-1.0, 1.0) * 32767.0) as i16);
                }
            });
            if samples.len() >= MAX_SAMPLES {
                self.shared.stop.store(true, Ordering::Release);
                break;
            }
        }
    }
}

fn record(input: Option<String>, shared: Arc<RecordingData>) -> Result<(), String> {
    let host = cpal::default_host();
    let device = input
        .and_then(|name| {
            host.input_devices()
                .ok()?
                .find(|d| d.name().ok().as_deref() == Some(name.as_str()))
        })
        .or_else(|| host.default_input_device())
        .ok_or_else(|| "Nenhum microfone disponível.".to_owned())?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("Não foi possível configurar o microfone: {e}"))?;
    let channels = config.channels() as usize;
    if channels == 0 {
        return Err("O microfone não possui canais de entrada.".into());
    }
    let mut capture = Capture {
        channels,
        converter: Resampler::new(config.sample_rate().0 as f64, RATE as f64),
        shared: shared.clone(),
    };
    let err_shared = shared.clone();
    let on_error = move |error| {
        *err_shared.error.lock() =
            Some(format!("A gravação do microfone foi interrompida: {error}"));
        err_shared.stop.store(true, Ordering::Release);
    };
    let cfg: cpal::StreamConfig = config.clone().into();
    let stream =
        match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &cfg,
                move |data: &[f32], _| {
                    let channels = capture.channels;
                    capture.feed(
                        data.chunks(channels)
                            .map(|c| c.iter().sum::<f32>() / channels as f32),
                    );
                },
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &cfg,
                move |data: &[i16], _| {
                    let channels = capture.channels;
                    capture.feed(data.chunks(channels).map(|c| {
                        c.iter().map(|s| *s as f32 / 32768.0).sum::<f32>() / channels as f32
                    }));
                },
                on_error,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &cfg,
                move |data: &[u16], _| {
                    let channels = capture.channels;
                    capture.feed(data.chunks(channels).map(|c| {
                        c.iter()
                            .map(|s| (*s as f32 - 32768.0) / 32768.0)
                            .sum::<f32>()
                            / channels as f32
                    }));
                },
                on_error,
                None,
            ),
            other => return Err(format!("Formato de microfone não suportado: {other:?}")),
        }
        .map_err(|e| format!("Não foi possível abrir o microfone: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Não foi possível iniciar o microfone: {e}"))?;
    shared.ready.store(true, Ordering::Release);
    while !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let size = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + size as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + size).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&size.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}
