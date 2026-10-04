use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockKind {
    // driven by advanceTime, default in tests
    Virtual,
    // dev only: task.wait really waits
    Real,
}

pub trait Clock {
    // seconds, monotonic
    fn now(&self) -> f64;

    // real clocks ignore this
    fn set_now(&mut self, seconds: f64);

    fn is_virtual(&self) -> bool;

    // used by inspect output and startup banners
    fn kind(&self) -> &'static str;
}

#[derive(Debug, Clone, Default)]
pub struct VirtualClock {
    now: f64,
}

impl VirtualClock {
    pub fn new() -> Self {
        Self { now: 0.0 }
    }

    pub fn advance(&mut self, seconds: f64) {
        self.now += seconds.max(0.0);
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> f64 {
        self.now
    }

    fn set_now(&mut self, seconds: f64) {
        if seconds > self.now {
            self.now = seconds;
        }
    }

    fn is_virtual(&self) -> bool {
        true
    }

    fn kind(&self) -> &'static str {
        "virtual"
    }
}

#[derive(Debug)]
pub struct RealClock {
    start: Instant,
}

impl RealClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn set_now(&mut self, _seconds: f64) {
        // wall time can't be moved; advance_time sleeps instead
    }

    fn is_virtual(&self) -> bool {
        false
    }

    fn kind(&self) -> &'static str {
        "real"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_clock_only_moves_forward() {
        let mut clock = VirtualClock::new();
        assert_eq!(clock.now(), 0.0);
        clock.set_now(5.0);
        assert_eq!(clock.now(), 5.0);
        clock.set_now(2.0);
        assert_eq!(clock.now(), 5.0, "time must not go backwards");
        assert!(clock.is_virtual());
    }

    #[test]
    fn real_clock_ignores_set_now() {
        let mut clock = RealClock::new();
        clock.set_now(1000.0);
        assert!(clock.now() < 1.0);
        assert!(!clock.is_virtual());
    }
}
