use super::*;

impl Mixer {
    pub fn new() -> Self {
        let m = Self::default();
        f32_store(&m.output_volume, 1.0);
        m
    }

    pub fn set_output_volume(&self, v: f32) {
        f32_store(&self.output_volume, v);
    }

    pub fn set_user(&self, user: UserId, volume: f32, muted: bool) {
        self.settings.lock().insert(user, (volume, muted));
    }

    pub fn has_user(&self, user: UserId) -> bool {
        self.settings.lock().contains_key(&user)
    }

    pub fn user(&self, user: UserId) -> (f32, bool) {
        self.settings
            .lock()
            .get(&user)
            .copied()
            .unwrap_or((1.0, false))
    }

    pub fn set_stream_user(&self, user: UserId, volume: f32, muted: bool) {
        self.stream_settings.lock().insert(user, (volume, muted));
    }

    pub fn has_stream_user(&self, user: UserId) -> bool {
        self.stream_settings.lock().contains_key(&user)
    }

    pub fn stream_user(&self, user: UserId) -> (f32, bool) {
        self.stream_settings
            .lock()
            .get(&user)
            .copied()
            .unwrap_or((1.0, false))
    }

    pub fn push(&self, user: UserId, _seq: u32, pcm: &[i16]) {
        if self.deafened.load(Ordering::Relaxed) && user != SELF_TEST_ID {
            return;
        }
        let mut people = self.people.lock();
        let p = people.entry(user).or_insert_with(|| PersonBuffer {
            queue: VecDeque::new(),
            playing: false,
            encoded: None,
        });
        p.queue.extend(pcm.iter().map(|s| *s as f32 / 32768.0));
        while p.queue.len() > MAX_BUFFER {
            p.queue.pop_front();
        }
    }

    pub fn push_encoded(&self, user: UserId, seq: u32, sent_micros: u64, opus: &[u8]) {
        if self.deafened.load(Ordering::Relaxed) || opus.is_empty() || opus.len() > 1275 {
            return;
        }
        let mut people = self.people.lock();
        let person = people.entry(user).or_insert_with(|| PersonBuffer {
            queue: VecDeque::new(),
            playing: false,
            encoded: JitterBuffer::new().ok(),
        });
        if person.encoded.is_none() {
            person.encoded = JitterBuffer::new().ok();
        }
        if let Some(buffer) = &mut person.encoded {
            buffer.enqueue(seq, sent_micros, opus);
        }
    }

    pub fn voice_stats(&self, user: UserId) -> Option<VoiceStats> {
        self.people
            .lock()
            .get(&user)
            .and_then(|person| person.encoded.as_ref().map(JitterBuffer::stats))
    }

    pub fn clear(&self) {
        self.people.lock().clear();
        self.streams.lock().clear();
    }

    pub fn stop_stream(&self, owner: UserId) {
        self.streams.lock().remove(&owner);
    }

    pub fn push_stream(&self, owner: UserId, seq: u32, sent_micros: u64, packet: &[u8]) {
        if self.deafened.load(Ordering::Relaxed) {
            return;
        }
        let mut streams = self.streams.lock();
        if let std::collections::hash_map::Entry::Vacant(entry) = streams.entry(owner) {
            let Ok(jitter) = JitterBuffer::music() else {
                return;
            };
            entry.insert(StreamBuffer {
                queue: VecDeque::new(),
                jitter,
            });
        }
        if let Some(stream) = streams.get_mut(&owner) {
            stream.jitter.enqueue(seq, sent_micros, packet);
        }
    }

    pub(super) fn pull_streams(&self, output: &mut Vec<[f32; 2]>, samples: usize) {
        output.clear();
        output.resize(samples, [0.0; 2]);
        if self.deafened.load(Ordering::Relaxed) {
            return;
        }
        let settings = self.stream_settings.lock().clone();
        let master = f32_load(&self.output_volume);
        let mut streams = self.streams.lock();
        for (owner, stream) in streams.iter_mut() {
            while stream.queue.len() < samples {
                let Some(frame) = stream.jitter.pop() else {
                    break;
                };
                stream
                    .queue
                    .extend(frame.as_chunks::<2>().0.iter().copied());
            }
            let (volume, muted) = settings.get(owner).copied().unwrap_or((1.0, false));
            let gain = if muted { 0.0 } else { volume * master };
            for sample in output.iter_mut() {
                let Some(frame) = stream.queue.pop_front() else {
                    break;
                };
                sample[0] += frame[0] * gain;
                sample[1] += frame[1] * gain;
            }
        }
    }

    /// Mix `n` samples at RATE into `out`.
    pub(super) fn pull(&self, out: &mut Vec<f32>, n: usize) {
        out.clear();
        out.resize(n, 0.0);
        let settings = self.settings.lock().clone();
        let master = f32_load(&self.output_volume);
        let mut people = self.people.lock();
        for (id, p) in people.iter_mut() {
            if let Some(jitter) = &mut p.encoded {
                while p.queue.len() < n {
                    let Some(frame) = jitter.pop() else { break };
                    p.queue.extend(frame);
                }
            }
            if !p.playing {
                if p.queue.len() >= if p.encoded.is_some() { 1 } else { START_BUFFER } {
                    p.playing = true;
                } else {
                    continue;
                }
            }
            let (vol, muted) = settings.get(id).copied().unwrap_or((1.0, false));
            let gain = if muted { 0.0 } else { vol * master };
            for o in out.iter_mut() {
                match p.queue.pop_front() {
                    Some(s) => *o += s * gain,
                    None => {
                        p.playing = false;
                        break;
                    }
                }
            }
        }
        for o in out.iter_mut() {
            // Soft limit so several loud voices don't crackle.
            *o = soft_limit(*o);
        }
    }
}
