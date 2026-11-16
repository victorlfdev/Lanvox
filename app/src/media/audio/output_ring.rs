//! Single producer/consumer ring. The device callback performs no locking or allocation.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

pub(super) struct OutputRing {
    samples: Box<[AtomicU64]>,
    read: AtomicUsize,
    written: AtomicUsize,
    pub stop: AtomicBool,
}
impl OutputRing {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            samples: (0..capacity.next_power_of_two())
                .map(|_| AtomicU64::new(0))
                .collect(),
            read: AtomicUsize::new(0),
            written: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        })
    }
    pub fn buffered(&self) -> usize {
        self.written
            .load(Ordering::Acquire)
            .wrapping_sub(self.read.load(Ordering::Acquire))
    }
    pub fn push(&self, value: [f32; 2]) -> bool {
        let written = self.written.load(Ordering::Relaxed);
        if written.wrapping_sub(self.read.load(Ordering::Acquire)) >= self.samples.len() {
            return false;
        }
        self.samples[written & (self.samples.len() - 1)].store(
            u64::from(value[0].to_bits()) | (u64::from(value[1].to_bits()) << 32),
            Ordering::Relaxed,
        );
        self.written
            .store(written.wrapping_add(1), Ordering::Release);
        true
    }
    pub fn pop(&self) -> Option<[f32; 2]> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.written.load(Ordering::Acquire) {
            return None;
        }
        let value = self.samples[read & (self.samples.len() - 1)].load(Ordering::Relaxed);
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some([
            f32::from_bits(value as u32),
            f32::from_bits((value >> 32) as u32),
        ])
    }
}

pub(super) struct OutputConsumer {
    pub ring: Arc<OutputRing>,
    pub mixer: Arc<super::Mixer>,
}
impl OutputConsumer {
    pub fn next_sample(&mut self) -> [f32; 2] {
        if self.mixer.deafened.load(Ordering::Relaxed) {
            let _ = self.ring.pop();
            return [0.0; 2];
        }
        self.ring.pop().unwrap_or_else(|| {
            self.mixer.underrun_frames.fetch_add(1, Ordering::Relaxed);
            [0.0; 2]
        })
    }
}
impl Drop for OutputConsumer {
    fn drop(&mut self) {
        self.ring.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn producer_consumer_wraparound_preserves_order_and_bounds() {
        let ring = OutputRing::new(8);
        assert_eq!(ring.pop(), None);
        for i in 0..8 {
            assert!(ring.push([i as f32, -(i as f32)]));
        }
        assert!(!ring.push([9.0; 2]));
        for i in 0..8 {
            assert_eq!(ring.pop(), Some([i as f32, -(i as f32)]));
        }
        let producer = ring.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..10000 {
                while !producer.push([i as f32; 2]) {
                    std::thread::yield_now();
                }
            }
        });
        for i in 0..10000 {
            loop {
                if let Some(sample) = ring.pop() {
                    assert_eq!(sample, [i as f32; 2]);
                    break;
                }
                std::thread::yield_now();
            }
        }
        thread.join().unwrap();
        assert_eq!(ring.buffered(), 0);
    }
}
