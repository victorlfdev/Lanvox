//! Windows process loopback. A screen captures system audio excluding Lanvox;
//! a window captures only its process tree. There is deliberately no fallback
//! to full device loopback, which would retransmit other participants' voices.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

pub struct Loopback {
    stop: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    pub active: Arc<AtomicBool>,
}

impl Loopback {
    /// Callback receives 20-ms blocks: 1920 interleaved f32 samples, stereo 48 kHz.
    pub fn start(process_id: Option<u32>, callback: impl FnMut(&[f32]) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let active = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        {
            let thread_stop = stop.clone();
            let thread_error = error.clone();
            let thread_active = active.clone();
            let spawn = std::thread::Builder::new()
                .name("share-audio".into())
                .spawn(move || {
                    let result = capture(process_id, callback, &thread_stop, &thread_active);
                    thread_active.store(false, Ordering::Relaxed);
                    if let Err(reason) = result {
                        *thread_error.lock() = Some(reason);
                    }
                });
            if let Err(reason) = spawn {
                *error.lock() = Some(reason.to_string());
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (process_id, callback);
            *error.lock() = Some("O áudio compartilhado por aplicativo está disponível nesta versão apenas no Windows.".into());
        }
        Self {
            stop,
            error,
            active,
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(windows)]
fn capture(
    process_id: Option<u32>,
    mut callback: impl FnMut(&[f32]),
    stop: &AtomicBool,
    active: &AtomicBool,
) -> Result<(), String> {
    use std::collections::VecDeque;
    use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat};

    wasapi::initialize_mta()
        .ok()
        .map_err(|error| format!("Não foi possível iniciar a captura de áudio: {error}"))?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            wasapi::deinitialize();
        }
    }
    let _apartment = Apartment;
    let target = process_id.unwrap_or_else(std::process::id);
    if target == 0 {
        return Err("O processo da janela não está disponível.".into());
    }
    let include_tree = process_id.is_some();
    // Windows 10 build 20348+/Windows 11. Failure remains explicit rather than
    // silently sharing the entire device and creating a voice feedback loop.
    let mut client = AudioClient::new_application_loopback_client(target, include_tree)
        .map_err(|error| format!("O Windows não pôde capturar o áudio desse aplicativo. Use Windows 11 ou uma versão com process loopback: {error}"))?;
    let format = WaveFormat::new(32, 32, SampleType::Float, 48_000, 2, None);
    client
        .initialize_client(
            &format,
            Direction::Capture,
            StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: 200_000,
            },
            None,
        )
        .map_err(|error| format!("Não foi possível preparar o áudio compartilhado: {error}"))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|error| error.to_string())?;
    client.start_stream().map_err(|error| error.to_string())?;
    active.store(true, Ordering::Relaxed);
    let mut buffered = VecDeque::with_capacity(3840);
    let mut bytes = Vec::new();
    let result = (|| {
        while !stop.load(Ordering::SeqCst) {
            let frames = capture
                .get_next_packet_size()
                .map_err(|error| error.to_string())?
                .unwrap_or(0) as usize;
            if frames == 0 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }
            if frames > 48_000 {
                return Err("O dispositivo retornou um bloco de áudio grande demais.".into());
            }
            bytes.resize(frames * 8, 0);
            let (read, _) = capture
                .read_from_device(&mut bytes)
                .map_err(|error| error.to_string())?;
            for chunk in bytes[..read as usize * 8].as_chunks::<4>().0 {
                let sample = f32::from_le_bytes(*chunk);
                buffered.push_back(if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                });
            }
            while buffered.len() >= 1920 {
                let mut frame = [0.0; 1920];
                for sample in &mut frame {
                    *sample = buffered.pop_front().unwrap_or(0.0);
                }
                callback(&frame);
                if stop.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
        Ok(())
    })();
    let _ = client.stop_stream();
    result
}
