use super::*;
use std::collections::VecDeque;
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::SHCreateMemStream;

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const MEDIASOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;
/// Frames wider than this are decimated before upload.
const MAX_WIDTH: usize = 960;
/// How much decoded audio to keep queued ahead of playback.
const AUDIO_LEAD_SECS: f32 = 1.0;

struct MfRuntime;
impl MfRuntime {
    fn start() -> Result<Self, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string())?;
        }
        Ok(Self)
    }
}
impl Drop for MfRuntime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// Interleaved stereo f32 queue shared with the audio callback.
struct Ring {
    queue: Mutex<VecDeque<f32>>,
    go: AtomicBool,
    paused: AtomicBool,
    consumed_frames: AtomicU64,
}

struct Audio {
    ring: Arc<Ring>,
    rate: u32,
    _stream: cpal::Stream,
}

fn fill(ring: &Ring, shared: &Shared, out: &mut [f32], channels: usize) {
    let volume = if shared.muted.load(Ordering::Relaxed) {
        0.0
    } else {
        f32::from_bits(shared.volume.load(Ordering::Relaxed))
    };
    let active = ring.go.load(Ordering::Relaxed) && !ring.paused.load(Ordering::Relaxed);
    let mut queue = ring.queue.lock();
    for frame in out.chunks_mut(channels) {
        let (l, r) = if active && queue.len() >= 2 {
            ring.consumed_frames.fetch_add(1, Ordering::Relaxed);
            (
                queue.pop_front().unwrap_or(0.0),
                queue.pop_front().unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0)
        };
        let (l, r) = ((l * volume).clamp(-1.0, 1.0), (r * volume).clamp(-1.0, 1.0));
        if channels == 1 {
            frame[0] = (l + r) * 0.5;
        } else {
            frame[0] = l;
            frame[1] = r;
            for extra in frame.iter_mut().skip(2) {
                *extra = 0.0;
            }
        }
    }
}

fn start_audio(shared: &Arc<Shared>) -> Option<Audio> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let device = cpal::default_host().default_output_device()?;
    let config = device.default_output_config().ok()?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;
    let ring = Arc::new(Ring {
        queue: Mutex::new(VecDeque::new()),
        go: AtomicBool::new(false),
        paused: AtomicBool::new(false),
        consumed_frames: AtomicU64::new(0),
    });
    let cfg: cpal::StreamConfig = config.clone().into();
    let (r, s) = (ring.clone(), shared.clone());
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &cfg,
            move |data: &mut [f32], _| fill(&r, &s, data, channels),
            |_| {},
            None,
        ),
        cpal::SampleFormat::I16 => {
            let mut scratch: Vec<f32> = Vec::new();
            device.build_output_stream(
                &cfg,
                move |data: &mut [i16], _| {
                    scratch.resize(data.len(), 0.0);
                    fill(&r, &s, &mut scratch, channels);
                    for (o, v) in data.iter_mut().zip(&scratch) {
                        *o = (*v * 32767.0) as i16;
                    }
                },
                |_| {},
                None,
            )
        }
        _ => return None,
    }
    .ok()?;
    stream.play().ok()?;
    Some(Audio {
        ring,
        rate,
        _stream: stream,
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn open_reader(source: &VideoSource) -> Result<IMFSourceReader, String> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 4).map_err(|e| e.to_string())?;
    let attrs: IMFAttributes = attrs.ok_or("MFCreateAttributes")?;
    attrs
        .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
        .map_err(|e| e.to_string())?;
    // Fewer buffered frames in the decoder (smaller reorder queue, no frame threading).
    let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
    match source {
        VideoSource::Bytes { name, data } => {
            let stream = SHCreateMemStream(Some(data)).ok_or("memória insuficiente")?;
            let bytes = MFCreateMFByteStreamOnStream(&stream).map_err(|e| e.to_string())?;
            if let Ok(a) = bytes.cast::<IMFAttributes>() {
                let origin = wide(name);
                let _ = a.SetString(&MF_BYTESTREAM_ORIGIN_NAME, PCWSTR(origin.as_ptr()));
            }
            MFCreateSourceReaderFromByteStream(&bytes, &attrs)
        }
        VideoSource::Url(url) => {
            let url = wide(url);
            MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), &attrs)
        }
        VideoSource::File(path) => {
            let path = wide(&path.to_string_lossy());
            MFCreateSourceReaderFromURL(PCWSTR(path.as_ptr()), &attrs)
        }
    }
    .map_err(|e| format!("Formato não suportado ou arquivo inacessível ({e})"))
}

struct VideoFormat {
    width: usize,
    height: usize,
    stride: i32,
}

unsafe fn configure_video(reader: &IMFSourceReader) -> Result<VideoFormat, String> {
    let wanted = MFCreateMediaType().map_err(|e| e.to_string())?;
    wanted
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|e| e.to_string())?;
    wanted
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
        .map_err(|e| e.to_string())?;
    reader
        .SetStreamSelection(VIDEO, true)
        .map_err(|_| "O arquivo não tem trilha de vídeo")?;
    reader
        .SetCurrentMediaType(VIDEO, None, &wanted)
        .map_err(|e| format!("Codec de vídeo indisponível ({e})"))?;
    video_format(reader)
}

unsafe fn video_format(reader: &IMFSourceReader) -> Result<VideoFormat, String> {
    let current = reader
        .GetCurrentMediaType(VIDEO)
        .map_err(|e| e.to_string())?;
    let packed = current
        .GetUINT64(&MF_MT_FRAME_SIZE)
        .map_err(|e| e.to_string())?;
    let (width, height) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err("Dimensões de vídeo inválidas".into());
    }
    let stride = current
        .GetUINT32(&MF_MT_DEFAULT_STRIDE)
        .map(|s| s as i32)
        .unwrap_or(width as i32 * 4);
    Ok(VideoFormat {
        width,
        height,
        stride: if stride == 0 {
            width as i32 * 4
        } else {
            stride
        },
    })
}

unsafe fn configure_audio(reader: &IMFSourceReader, rate: u32) -> bool {
    let Ok(wanted) = MFCreateMediaType() else {
        return false;
    };
    let ok = wanted
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
        .and_then(|_| wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float))
        .and_then(|_| wanted.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2))
        .and_then(|_| wanted.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate))
        .and_then(|_| wanted.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32))
        .and_then(|_| wanted.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 8))
        .and_then(|_| wanted.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * 8))
        .and_then(|_| wanted.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1))
        .is_ok();
    ok && reader.SetStreamSelection(AUDIO, true).is_ok()
        && reader.SetCurrentMediaType(AUDIO, None, &wanted).is_ok()
}

struct Read {
    flags: u32,
    ts: i64,
    sample: Option<IMFSample>,
}

unsafe fn read(reader: &IMFSourceReader, stream: u32) -> Result<Read, String> {
    let (mut flags, mut ts) = (0u32, 0i64);
    let mut sample = None;
    reader
        .ReadSample(
            stream,
            0,
            None,
            Some(&mut flags),
            Some(&mut ts),
            Some(&mut sample),
        )
        .map_err(|e| format!("Falha ao decodificar o vídeo ({e})"))?;
    Ok(Read { flags, ts, sample })
}

fn has(flags: u32, flag: MF_SOURCE_READER_FLAG) -> bool {
    flags & flag.0 as u32 != 0
}

/// Copies a decoded RGB32 sample into `out` as RGBA, decimating by `factor`.
unsafe fn to_rgba(
    sample: &IMFSample,
    format: &VideoFormat,
    factor: usize,
    out: &mut Vec<u8>,
) -> Result<[usize; 2], String> {
    let buffer = sample
        .ConvertToContiguousBuffer()
        .map_err(|e| e.to_string())?;
    let (w, h) = (format.width, format.height);
    let (dw, dh) = (w / factor, h / factor);
    out.clear();
    out.resize(dw * dh * 4, 255);
    let mut convert = |top: *const u8, pitch: isize| {
        for y in 0..dh {
            let row = top.offset((y * factor) as isize * pitch);
            let dst = &mut out[y * dw * 4..(y + 1) * dw * 4];
            for x in 0..dw {
                let p = row.add(x * factor * 4);
                dst[x * 4] = *p.add(2);
                dst[x * 4 + 1] = *p.add(1);
                dst[x * 4 + 2] = *p;
            }
        }
    };
    if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
        let (mut top, mut pitch) = (std::ptr::null_mut(), 0i32);
        b2.Lock2D(&mut top, &mut pitch).map_err(|e| e.to_string())?;
        if top.is_null() || (pitch.unsigned_abs() as usize) < w * 4 {
            let _ = b2.Unlock2D();
            return Err("Quadro de vídeo inválido".into());
        }
        convert(top, pitch as isize);
        let _ = b2.Unlock2D();
    } else {
        let (mut base, mut len) = (std::ptr::null_mut(), 0u32);
        buffer
            .Lock(&mut base, None, Some(&mut len))
            .map_err(|e| e.to_string())?;
        let pitch = format.stride.unsigned_abs() as usize;
        if base.is_null() || pitch < w * 4 || (len as usize) < pitch * h {
            let _ = buffer.Unlock();
            return Err("Quadro de vídeo inválido".into());
        }
        if format.stride < 0 {
            convert(base.add(pitch * (h - 1)), -(pitch as isize));
        } else {
            convert(base, pitch as isize);
        }
        let _ = buffer.Unlock();
    }
    Ok([dw, dh])
}

unsafe fn push_audio(sample: &IMFSample, ring: &Ring) {
    let Ok(buffer) = sample.ConvertToContiguousBuffer() else {
        return;
    };
    let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
    if buffer.Lock(&mut data, None, Some(&mut len)).is_err() {
        return;
    }
    if !data.is_null() {
        let samples = std::slice::from_raw_parts(data as *const u8, len as usize);
        let mut queue = ring.queue.lock();
        queue.extend(
            samples
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        );
        // Bound memory if the clock stalls.
        while queue.len() > 48_000 * 2 * 20 {
            queue.pop_front();
        }
    }
    let _ = buffer.Unlock();
}

unsafe fn duration_ticks(reader: &IMFSourceReader) -> i64 {
    reader
        .GetPresentationAttribute(MEDIASOURCE, &MF_PD_DURATION)
        .ok()
        .and_then(|v| u64::try_from(&v).ok())
        .unwrap_or(0) as i64
}

pub fn grab(source: VideoSource) -> Result<VideoFrame, String> {
    let _runtime = MfRuntime::start()?;
    unsafe {
        let reader = open_reader(&source)?;
        let format = configure_video(&reader)?;
        let _ = reader.SetStreamSelection(AUDIO, false);
        let duration = duration_ticks(&reader);
        let mut target = (duration / 10).min(20_000_000);
        if target > 0
            && reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(target))
                .is_err()
        {
            target = 0;
        }
        let factor = format.width.div_ceil(480).max(1);
        let mut rgba = Vec::new();
        for _ in 0..900 {
            let r = read(&reader, VIDEO)?;
            if let Some(sample) = r.sample {
                if r.ts >= target {
                    let size = to_rgba(&sample, &format, factor, &mut rgba)?;
                    return Ok(VideoFrame { size, rgba, seq: 0 });
                }
            }
            if has(r.flags, MF_SOURCE_READERF_ENDOFSTREAM) || has(r.flags, MF_SOURCE_READERF_ERROR)
            {
                break;
            }
        }
        Err("Sem quadro para miniatura".into())
    }
}

pub fn download(url: &str) -> Result<Vec<u8>, String> {
    use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
    let path = std::env::temp_dir().join(format!("lanvox-thumb-{:016x}", rand::random::<u64>()));
    let wpath = wide(&path.to_string_lossy());
    let wurl = wide(url);
    let result =
        unsafe { URLDownloadToFileW(None, PCWSTR(wurl.as_ptr()), PCWSTR(wpath.as_ptr()), 0, None) };
    let bytes = result
        .map_err(|e| e.to_string())
        .and_then(|_| std::fs::read(&path).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&path);
    bytes.and_then(|b| {
        if b.len() <= 4 * 1024 * 1024 {
            Ok(b)
        } else {
            Err("grande demais".into())
        }
    })
}

pub fn run(shared: Arc<Shared>, source: VideoSource) {
    let _runtime = match MfRuntime::start() {
        Ok(r) => r,
        Err(e) => return shared.fail(format!("Media Foundation indisponível ({e})")),
    };
    if let Err(e) = unsafe { play(&shared, &source) } {
        if !shared.stop.load(Ordering::Relaxed) {
            shared.fail(e);
        }
    }
}

/// Audio-only containers use the output callback's consumed frame count as
/// their clock. Pausing and starvation therefore never advance position.
unsafe fn play_audio(
    shared: &Arc<Shared>,
    reader: &IMFSourceReader,
    audio: Audio,
) -> Result<(), String> {
    let duration = duration_ticks(reader).max(0);
    shared
        .duration_ms
        .store((duration / 10_000) as u64, Ordering::Relaxed);
    let mut eos = false;
    let mut base_ticks = 0i64;
    let mut skip_until = None;
    let mut initialized = false;
    shared.set_state(PlayState::Loading);
    while !shared.stop.load(Ordering::Relaxed) {
        let target = shared.seek_to.swap(NO_SEEK, Ordering::Relaxed);
        if target != NO_SEEK {
            let target = if duration > 0 {
                target.clamp(0, duration)
            } else {
                target.max(0)
            };
            audio.ring.go.store(false, Ordering::Relaxed);
            if reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(target))
                .is_ok()
            {
                audio.ring.queue.lock().clear();
                audio.ring.consumed_frames.store(0, Ordering::Relaxed);
                base_ticks = target;
                skip_until = Some(target);
                eos = false;
                initialized = true;
                shared
                    .position_ms
                    .store((target / 10_000) as u64, Ordering::Relaxed);
            }
        }
        let touched = shared.last_touch_ms.load(Ordering::Relaxed);
        if touched > 0 && (shared.epoch.elapsed().as_millis() as u64).saturating_sub(touched) > 1500
        {
            shared.want_paused.store(true, Ordering::Relaxed);
        }
        let paused = shared.want_paused.load(Ordering::Relaxed);
        audio.ring.paused.store(paused, Ordering::Relaxed);
        if paused {
            shared.set_state(PlayState::Paused);
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let queued = audio.ring.queue.lock().len();
        if !eos && (queued as f32) / (audio.rate as f32 * 2.0) < AUDIO_LEAD_SECS {
            let packet = read(reader, AUDIO)?;
            if has(packet.flags, MF_SOURCE_READERF_ERROR) {
                return Err("O codec não conseguiu decodificar o áudio.".into());
            }
            eos = has(packet.flags, MF_SOURCE_READERF_ENDOFSTREAM);
            if let Some(sample) = packet.sample {
                if skip_until.is_none_or(|target| packet.ts >= target) {
                    if !initialized {
                        base_ticks = packet.ts.max(0);
                        initialized = true;
                    }
                    skip_until = None;
                    push_audio(&sample, &audio.ring);
                    audio.ring.go.store(true, Ordering::Relaxed);
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(8));
        }
        let consumed = audio.ring.consumed_frames.load(Ordering::Relaxed);
        let position = (base_ticks.max(0) as u64 / 10_000)
            .saturating_add(consumed.saturating_mul(1000) / audio.rate.max(1) as u64);
        shared.position_ms.store(position, Ordering::Relaxed);
        if eos && audio.ring.queue.lock().len() < 2 {
            shared.set_state(PlayState::Ended);
            shared.position_ms.store(
                if duration > 0 {
                    (duration / 10_000) as u64
                } else {
                    position
                },
                Ordering::Relaxed,
            );
            std::thread::sleep(Duration::from_millis(20));
        } else {
            shared.set_state(if consumed == 0 && queued == 0 {
                PlayState::Loading
            } else {
                PlayState::Playing
            });
        }
    }
    Ok(())
}

unsafe fn play(shared: &Arc<Shared>, source: &VideoSource) -> Result<(), String> {
    let reader = open_reader(source)?;
    let mut format = match configure_video(&reader) {
        Ok(format) => format,
        Err(video_error) => {
            let _ = reader.SetStreamSelection(VIDEO, false);
            let audio=start_audio(shared).filter(|a|configure_audio(&reader,a.rate))
                .ok_or_else(||format!("Não foi possível reproduzir o arquivo. {video_error}. Verifique o codec e o dispositivo de saída."))?;
            shared.audio_only.store(true, Ordering::Relaxed);
            return play_audio(shared, &reader, audio);
        }
    };
    let duration = duration_ticks(&reader);
    shared
        .duration_ms
        .store((duration / 10_000).max(0) as u64, Ordering::Relaxed);

    let audio = start_audio(shared).filter(|a| configure_audio(&reader, a.rate));
    if audio.is_none() {
        let _ = reader.SetStreamSelection(AUDIO, false);
    }

    let mut factor = format.width.div_ceil(MAX_WIDTH).max(1);
    let mut rgba: Vec<u8> = Vec::new();
    let mut seq = 0u64;
    // A decoded frame waiting for its due time.
    let mut pending: Option<(i64, [usize; 2], Vec<u8>)> = None;
    let (mut video_eos, mut audio_eos) = (false, audio.is_none());
    let mut skip_until: Option<i64> = None;
    // Wall clock anchor: `anchor_ts` (ticks) was on screen at `anchor_wall`.
    let mut anchor: Option<(Instant, i64)> = None;
    let mut position: i64 = 0;
    let mut paused = false;
    let mut ended = false;
    shared.set_state(PlayState::Playing);

    while !shared.stop.load(Ordering::Relaxed) {
        // Pause when the UI is no longer drawing us.
        let idle = (shared.epoch.elapsed().as_millis() as u64)
            .saturating_sub(shared.last_touch_ms.load(Ordering::Relaxed));
        if idle > 1500 && shared.last_touch_ms.load(Ordering::Relaxed) > 0 {
            shared.want_paused.store(true, Ordering::Relaxed);
        }

        let target = shared.seek_to.swap(NO_SEEK, Ordering::Relaxed);
        if target != NO_SEEK {
            let target = if duration > 0 {
                target.clamp(0, duration)
            } else {
                target.max(0)
            };
            let value = PROPVARIANT::from(target);
            if reader.SetCurrentPosition(&GUID::zeroed(), &value).is_ok() {
                pending = None;
                video_eos = false;
                audio_eos = audio.is_none();
                ended = false;
                skip_until = Some(target);
                anchor = None;
                position = target;
                shared
                    .position_ms
                    .store((target / 10_000) as u64, Ordering::Relaxed);
                if let Some(a) = &audio {
                    a.ring.go.store(false, Ordering::Relaxed);
                    a.ring.queue.lock().clear();
                }
            }
        }

        let want_paused = shared.want_paused.load(Ordering::Relaxed);
        if want_paused != paused {
            paused = want_paused;
            if let Some(a) = &audio {
                a.ring.paused.store(paused, Ordering::Relaxed);
            }
            if !paused {
                // Resume from where the picture stopped.
                if anchor.is_some() {
                    anchor = Some((Instant::now(), position));
                }
            }
        }
        if ended || paused {
            shared.set_state(if ended && !paused {
                PlayState::Ended
            } else {
                PlayState::Paused
            });
            if ended && !paused {
                // Replaying is requested through a seek; wait for it.
                shared.set_state(PlayState::Ended);
            }
            std::thread::sleep(Duration::from_millis(25));
            continue;
        }
        shared.set_state(if anchor.is_some() {
            PlayState::Playing
        } else {
            PlayState::Loading
        });

        let mut worked = false;

        // Keep about a second of audio queued.
        if let Some(a) = &audio {
            let queued = a.ring.queue.lock().len() as f32 / (a.rate as f32 * 2.0);
            if !audio_eos && queued < AUDIO_LEAD_SECS {
                let r = read(&reader, AUDIO)?;
                if has(r.flags, MF_SOURCE_READERF_ENDOFSTREAM)
                    || has(r.flags, MF_SOURCE_READERF_ERROR)
                {
                    audio_eos = true;
                }
                if let Some(sample) = r.sample {
                    let stale = skip_until.is_some_and(|t| r.ts < t);
                    if !stale {
                        push_audio(&sample, &a.ring);
                    }
                }
                worked = true;
            }
        }

        if pending.is_none() && !video_eos {
            let r = read(&reader, VIDEO)?;
            if has(r.flags, MF_SOURCE_READERF_ENDOFSTREAM) || has(r.flags, MF_SOURCE_READERF_ERROR)
            {
                video_eos = true;
            }
            if has(r.flags, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) {
                if let Ok(f) = video_format(&reader) {
                    factor = f.width.div_ceil(MAX_WIDTH).max(1);
                    format = f;
                }
            }
            if let Some(sample) = r.sample {
                if !skip_until.is_some_and(|t| r.ts < t) {
                    let size = to_rgba(&sample, &format, factor, &mut rgba)?;
                    pending = Some((r.ts, size, std::mem::take(&mut rgba)));
                }
            }
            worked = true;
        }

        if let Some((ts, _, _)) = &pending {
            let due = match anchor {
                Some((wall, base)) => {
                    wall + Duration::from_nanos(((*ts - base).max(0) as u64) * 100)
                }
                None => Instant::now(),
            };
            if Instant::now() >= due {
                let (ts, size, data) = pending.take().unwrap();
                if anchor.is_none() {
                    anchor = Some((Instant::now(), ts));
                    skip_until = None;
                    if let Some(a) = &audio {
                        a.ring.go.store(true, Ordering::Relaxed);
                    }
                }
                position = ts;
                seq += 1;
                shared
                    .position_ms
                    .store((ts.max(0) / 10_000) as u64, Ordering::Relaxed);
                let mut slot = shared.frame.lock();
                // Recycle the previous frame's buffer when nobody else holds it.
                if let Some(old) = slot.take().and_then(|f| Arc::try_unwrap(f).ok()) {
                    rgba = old.rgba;
                }
                *slot = Some(Arc::new(VideoFrame {
                    size,
                    rgba: data,
                    seq,
                }));
                worked = true;
            } else if !worked {
                let wait = due.saturating_duration_since(Instant::now());
                std::thread::sleep(wait.min(Duration::from_millis(8)));
                worked = true;
            }
        }

        if video_eos && pending.is_none() {
            // Let the queued audio drain, then finish.
            let drained = audio
                .as_ref()
                .map(|a| a.ring.queue.lock().len() < 2)
                .unwrap_or(true);
            if drained {
                ended = true;
                position = duration.max(position);
                shared
                    .position_ms
                    .store((position.max(0) / 10_000) as u64, Ordering::Relaxed);
                shared.set_state(PlayState::Ended);
                continue;
            }
        }
        if !worked {
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    Ok(())
}
