use super::*;

impl SmartVad {
    pub(super) fn new() -> Self {
        Self {
            floor_db: -60.0,
            active: false,
            hangover: 0,
            onset_frames: 0,
            frames: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.active = false;
        self.hangover = 0;
        self.onset_frames = 0;
    }

    /// `probability` is RNNoise's speech probability or negative if unknown.
    pub(super) fn update(&mut self, db: f32, probability: f32, margin_db: f32) -> bool {
        // The floor falls fast and rises slowly, so speech never lifts it much.
        // The first second calibrates quickly; while speaking it still creeps
        // up so steady noise that opened the gate eventually closes it.
        self.frames = self.frames.saturating_add(1);
        let rate = if self.frames < 50 {
            0.1
        } else if db < self.floor_db {
            0.2
        } else if self.active {
            0.002
        } else {
            0.01
        };
        self.floor_db += (db - self.floor_db) * rate;
        self.floor_db = self.floor_db.clamp(-90.0, -35.0);
        let above = db - self.floor_db;
        let speechlike = if probability >= 0.0 {
            // Neural detector decides; energy only has to clear the floor a bit.
            let needed = if self.active { 0.35 } else { 0.7 };
            // Background speech (a TV) looks like speech to RNNoise, so the
            // level margin over the room is what separates it from the user.
            probability >= needed
                && above
                    > if self.active {
                        margin_db * 0.5
                    } else {
                        margin_db
                    }
        } else {
            above
                > if self.active {
                    margin_db * 0.5
                } else {
                    margin_db
                }
        };
        if speechlike {
            self.onset_frames += 1;
            // A single loud click is not speech; two frames in a row are.
            if self.active || self.onset_frames >= 2 || probability >= 0.9 {
                self.active = true;
                self.hangover = SMART_HANGOVER_FRAMES;
            }
        } else {
            self.onset_frames = 0;
            if self.hangover > 0 {
                self.hangover -= 1;
            } else {
                self.active = false;
            }
        }
        self.active || self.hangover > 0
    }
}

impl<const C: usize> Resampler<C> {
    pub(super) fn new(in_rate: f64, out_rate: f64) -> Self {
        let step = in_rate / out_rate;
        let fc = (1.0 / step).min(1.0);
        let half = (RS_ZEROS / fc).ceil() as usize;
        let span = half as f64;
        let len = 2 * half * RS_PHASES + 1;
        let mut table = Vec::with_capacity(len);
        for i in 0..len {
            let x = i as f64 / RS_PHASES as f64 - span;
            let arg = std::f64::consts::PI * fc * x;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { arg.sin() / arg };
            let t = (x + span) / (2.0 * span);
            let window = 0.42 - 0.5 * (std::f64::consts::TAU * t).cos()
                + 0.08 * (2.0 * std::f64::consts::TAU * t).cos();
            table.push((fc * sinc * window) as f32);
        }
        Self {
            step,
            pos: 0.0,
            half,
            hist: VecDeque::from(vec![[0.0; C]; 2 * half]),
            table,
        }
    }

    pub(super) fn advance(&mut self, frame: [f32; C]) {
        self.hist.pop_front();
        self.hist.push_back(frame);
    }

    /// Output located `frac` (0..1) after the older centre sample.
    pub(super) fn compute(&self, frac: f64) -> [f32; C] {
        let mut out = [0.0f32; C];
        let c = (self.half - 1) as f64;
        let span = self.half as f64;
        for (k, frame) in self.hist.iter().enumerate() {
            let x = k as f64 - c - frac;
            let idx = ((x + span) * RS_PHASES as f64).round();
            if idx < 0.0 {
                continue;
            }
            let w = self.table.get(idx as usize).copied().unwrap_or(0.0);
            for (o, v) in out.iter_mut().zip(frame) {
                *o += v * w;
            }
        }
        out
    }

    /// Push one input frame, emit every output frame that became available.
    pub(super) fn push(&mut self, frame: [f32; C], mut emit: impl FnMut([f32; C])) {
        self.advance(frame);
        while self.pos < 1.0 {
            emit(self.compute(self.pos));
            self.pos += self.step;
        }
        self.pos -= 1.0;
    }

    /// Produce one output frame, pulling input frames from `source` as needed.
    pub(super) fn pull(&mut self, mut source: impl FnMut() -> [f32; C]) -> [f32; C] {
        while self.pos >= 1.0 {
            self.pos -= 1.0;
            self.advance(source());
        }
        let out = self.compute(self.pos);
        self.pos += self.step;
        out
    }
}

/// Soft limiter: transparent below 0.7, smooth compression above, never past 1.
pub(super) fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.7;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}
