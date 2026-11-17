//! Timestamps taken when a read from the AT port returns.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// System (realtime) time and monotonic elapsed time of one read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// Nanoseconds since the Unix epoch per the system clock (negative before 1970).
    pub realtime_ns: i128,
    /// Monotonic time since the program started; unaffected by clock steps.
    pub mono: Duration,
}

/// Source of stamps. A trait so tests can supply fixed times.
pub trait Clock {
    fn now(&mut self) -> Stamp;
}

/// The real clocks.
#[derive(Debug)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&mut self) -> Stamp {
        let mono = self.start.elapsed();
        let realtime_ns = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
            Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
        };
        Stamp { realtime_ns, mono }
    }
}
