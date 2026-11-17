//! Shared video targets and a bounded, conservative congestion controller.
use crate::model::StreamKind;

pub const SCREEN_MIN_BPS: u32 = 6_000_000;
pub const SCREEN_LOW_BPS: u32 = 3_000_000;
pub const CAMERA_MIN_BPS: u32 = 1_000_000;
pub const DEFAULT_HOST_VIDEO_BPS: u64 = 72_000_000;

pub fn minimum(kind: StreamKind) -> u32 {
    match kind {
        StreamKind::Screen => SCREEN_MIN_BPS,
        StreamKind::Camera => CAMERA_MIN_BPS,
    }
}

/// Low-resolution screens have a fixed 3 Mb/s target; all higher screen
/// profiles have a ceiling of at least 6 Mb/s, so the ceiling identifies them.
pub fn minimum_for_ceiling(kind: StreamKind, maximum: u32) -> u32 {
    if kind == StreamKind::Screen && maximum == SCREEN_LOW_BPS {
        SCREEN_LOW_BPS
    } else {
        minimum(kind)
    }
}

pub fn ceiling(kind: StreamKind, width: u32, height: u32, fps: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    if kind == StreamKind::Screen && pixels <= 1280 * 720 {
        return SCREEN_LOW_BPS;
    }
    let rate = match kind {
        StreamKind::Screen => {
            let percent = match fps {
                0..=15 => 55,
                16..=30 => 75,
                _ => 100,
            };
            pixels.saturating_mul(5800).saturating_mul(percent) / 100_000
        }
        StreamKind::Camera => pixels.saturating_mul(3).min(6_000_000),
    };
    rate.clamp(u64::from(minimum(kind)), 40_000_000) as u32
}

/// (kind, maximum per source, number of remote copies). Never violates floors,
/// even when the administrator's budget is insufficient; report that explicitly.
pub fn allocate(streams: &[(StreamKind, u32, usize)], budget: u64) -> (Vec<u32>, bool) {
    let floor_cost: u64 = streams
        .iter()
        .map(|(kind, max, copies)| u64::from(minimum_for_ceiling(*kind, *max)) * *copies as u64)
        .sum();
    let headroom: u64 = streams
        .iter()
        .map(|(kind, max, copies)| {
            u64::from(max.saturating_sub(minimum_for_ceiling(*kind, *max))) * *copies as u64
        })
        .sum();
    let spare = budget.saturating_sub(floor_cost).min(headroom);
    let limits = streams
        .iter()
        .map(|(kind, max, copies)| {
            let floor = minimum_for_ceiling(*kind, *max);
            if *copies == 0 || headroom == 0 {
                return (*max).max(floor);
            }
            floor + (u64::from(max.saturating_sub(floor)) * spare / headroom) as u32
        })
        .collect();
    (limits, floor_cost > budget)
}

#[cfg(any(feature = "gui", test))]
pub struct Controller {
    kind: StreamKind,
    target: u32,
    max: u32,
    clean: u32,
    last: std::time::Instant,
}

#[cfg(any(feature = "gui", test))]
impl Controller {
    pub fn new(kind: StreamKind, maximum: u32, now: std::time::Instant) -> Self {
        Self {
            kind,
            target: minimum_for_ceiling(kind, maximum),
            max: maximum.max(minimum_for_ceiling(kind, maximum)),
            clean: 0,
            last: now,
        }
    }
    pub fn target(&self) -> u32 {
        self.target
    }
    pub fn floor(&self) -> u32 {
        minimum_for_ceiling(self.kind, self.max)
    }
    /// Drops by 20% at most once per second. Growth requires five healthy
    /// samples, then adds 5% (at least 50 kbit/s). A host cap is immediate.
    pub fn update(
        &mut self,
        maximum: u32,
        congested: bool,
        healthy: bool,
        now: std::time::Instant,
    ) -> u32 {
        self.max = maximum.clamp(minimum_for_ceiling(self.kind, maximum), 40_000_000);
        self.target = self.target.clamp(self.floor(), self.max);
        if now.saturating_duration_since(self.last) < std::time::Duration::from_secs(1) {
            return self.target;
        }
        self.last = now;
        if congested {
            self.clean = 0;
            self.target = (self.target * 4 / 5).max(self.floor());
        } else if healthy {
            self.clean += 1;
            if self.clean >= 5 {
                self.target = self
                    .target
                    .saturating_add((self.target / 20).max(50_000))
                    .min(self.max);
                self.clean = 0;
            }
        } else {
            self.clean = 0;
        }
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn low_resolution_screens_stay_at_three_megabits_for_all_fps() {
        for fps in [5, 15, 30, 60] {
            for (w, h) in [(854, 480), (1280, 720), (720, 1280)] {
                let max = ceiling(StreamKind::Screen, w, h, fps);
                assert_eq!(max, SCREEN_LOW_BPS);
                let start = Instant::now();
                let mut control = Controller::new(StreamKind::Screen, max, start);
                assert_eq!(control.floor(), SCREEN_LOW_BPS);
                assert_eq!(control.target(), SCREEN_LOW_BPS);
                for second in 1..60 {
                    assert_eq!(
                        control.update(
                            max,
                            second % 2 == 0,
                            second % 2 != 0,
                            start + Duration::from_secs(second)
                        ),
                        SCREEN_LOW_BPS
                    );
                }
            }
        }
        assert_eq!(ceiling(StreamKind::Screen, 1920, 1080, 60), 12_026_880);
        assert_eq!(ceiling(StreamKind::Screen, 2560, 1440, 60), 21_381_120);
        assert_eq!(
            minimum_for_ceiling(StreamKind::Screen, 12_026_880),
            SCREEN_MIN_BPS
        );
        assert_eq!(
            minimum_for_ceiling(StreamKind::Camera, 3_000_000),
            CAMERA_MIN_BPS
        );
    }

    #[test]
    fn two_720p_screens_and_five_cameras_need_44_megabits_of_host_video() {
        let mut streams = vec![(StreamKind::Camera, 2_764_800, 4); 5];
        streams.extend([(StreamKind::Screen, SCREEN_LOW_BPS, 4); 2]);
        let (limits, insufficient) = allocate(&streams, 44_000_000);
        assert!(!insufficient);
        let cost: u64 = limits
            .iter()
            .zip(&streams)
            .map(|(bps, (_, _, copies))| u64::from(*bps) * *copies as u64)
            .sum();
        assert_eq!(cost, 44_000_000);
        for (limit, (kind, max, _)) in limits.iter().zip(&streams) {
            assert_eq!(*limit, minimum_for_ceiling(*kind, *max));
        }
        assert!(allocate(&streams, 43_000_000).1);
    }

    #[test]
    fn resolution_changes_apply_the_matching_floor_without_waiting_for_a_tick() {
        let start = Instant::now();
        let mut control = Controller::new(StreamKind::Screen, 12_000_000, start);
        assert_eq!(
            control.update(SCREEN_LOW_BPS, false, false, start),
            SCREEN_LOW_BPS
        );
        assert_eq!(
            control.update(12_000_000, false, false, start),
            SCREEN_MIN_BPS
        );
    }

    #[test]
    fn five_cameras_two_screens_fit_host_budget_including_all_copies() {
        let mut streams = vec![(StreamKind::Camera, 2_764_800, 4); 5];
        streams.extend([(StreamKind::Screen, 12_026_000, 4); 2]);
        let (limits, insufficient) = allocate(&streams, DEFAULT_HOST_VIDEO_BPS);
        assert!(!insufficient);
        let total: u64 = limits
            .iter()
            .zip(&streams)
            .map(|(bps, (_, _, copies))| u64::from(*bps) * *copies as u64)
            .sum();
        assert!(total <= DEFAULT_HOST_VIDEO_BPS);
        assert!(total >= 68_000_000);
        for (limit, (kind, max, _)) in limits.iter().zip(&streams) {
            assert!(*limit >= minimum(*kind) && *limit <= *max);
        }
        let (limits, insufficient) = allocate(&streams, 50_000_000);
        assert!(insufficient);
        for (limit, (kind, _, _)) in limits.iter().zip(&streams) {
            assert_eq!(*limit, minimum(*kind));
        }
    }

    #[test]
    fn controller_never_oscillates_per_frame_or_crosses_floors_and_caps() {
        for kind in [StreamKind::Screen, StreamKind::Camera] {
            let start = Instant::now();
            let floor = minimum(kind);
            let mut control = Controller::new(kind, floor * 2, start);
            for tick in 1..5000 {
                let now = start + Duration::from_millis(tick * 100);
                let congested = tick > 1000 && tick < 2000;
                let before = control.target();
                let target = control.update(floor * 2, congested, !congested, now);
                assert!((floor..=floor * 2).contains(&target));
                if tick % 10 != 0 {
                    assert_eq!(before, target);
                }
                if tick == 1999 {
                    assert_eq!(target, floor);
                }
            }
            assert_eq!(
                control.update(floor, false, true, start + Duration::from_secs(501)),
                floor
            );
            for second in 502..520 {
                assert_eq!(
                    control.update(floor * 2, false, false, start + Duration::from_secs(second)),
                    floor
                );
            }
        }
    }
}
