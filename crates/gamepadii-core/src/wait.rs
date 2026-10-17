//! Waiting for something to show up, by polling: the pad re-enumerating
//! after a reboot, or the user switching it into web-config. Time comes
//! from a [`Clock`], so tests use a [`ManualClock`] whose `sleep` only moves
//! its hands and never blocks.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Monotonic time, sleeping, and the wall clock (for timestamps).
pub trait Clock {
    /// Time since the clock was made; only differences mean anything.
    fn elapsed(&self) -> Duration;
    fn sleep(&self, d: Duration);
    fn wall(&self) -> SystemTime;
}

/// The real clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
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
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A clock for tests: `sleep` advances it instantly; `wall` is a fixed
/// start time plus the elapsed time.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<Duration>,
    wall_start: SystemTime,
}

impl ManualClock {
    /// Starting at `wall_start` (seconds since the epoch).
    pub fn at(unix_secs: u64) -> Self {
        Self {
            now: Mutex::new(Duration::ZERO),
            wall_start: UNIX_EPOCH + Duration::from_secs(unix_secs),
        }
    }

    pub fn advance(&self, d: Duration) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) += d;
    }
}

impl Clock for ManualClock {
    fn elapsed(&self) -> Duration {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn sleep(&self, d: Duration) {
        self.advance(d);
    }
    fn wall(&self) -> SystemTime {
        self.wall_start + self.elapsed()
    }
}

/// How often to look, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Poll {
    pub interval: Duration,
    pub timeout: Duration,
}

impl Poll {
    /// Every 200 ms: sysfs reads are cheap, and re-enumeration takes about
    /// a second.
    pub const INTERVAL: Duration = Duration::from_millis(200);

    pub fn new(timeout: Duration) -> Self {
        Self {
            interval: Self::INTERVAL,
            timeout,
        }
    }
}

/// Calls `check` until it returns `Some`, sleeping `poll.interval` between
/// tries, and gives up with `None` once `poll.timeout` has passed (after one
/// last try at the deadline). `tick` hears the elapsed time before every
/// sleep, for a spinner or a countdown. `check` always runs at least once,
/// so a zero timeout is a single look.
pub fn wait_for<T>(
    clock: &dyn Clock,
    poll: Poll,
    mut check: impl FnMut() -> Option<T>,
    mut tick: impl FnMut(Duration),
) -> Option<T> {
    let start = clock.elapsed();
    loop {
        if let Some(found) = check() {
            return Some(found);
        }
        let elapsed = clock.elapsed().saturating_sub(start);
        if elapsed >= poll.timeout {
            return None;
        }
        tick(elapsed);
        let left = poll.timeout - elapsed;
        clock.sleep(poll.interval.min(left).max(Duration::from_millis(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_it_on_the_nth_look() {
        let clock = ManualClock::at(0);
        let mut looks = 0;
        let mut ticks = Vec::new();
        let found = wait_for(
            &clock,
            Poll::new(Duration::from_secs(10)),
            || {
                looks += 1;
                (looks == 4).then_some("pad")
            },
            |e| ticks.push(e.as_millis()),
        );
        assert_eq!(found, Some("pad"));
        assert_eq!(ticks, [0, 200, 400]);
        assert_eq!(clock.elapsed(), Duration::from_millis(600));
    }

    #[test]
    fn gives_up_at_the_deadline() {
        let clock = ManualClock::at(0);
        let mut looks = 0;
        let found: Option<()> = wait_for(
            &clock,
            Poll::new(Duration::from_millis(500)),
            || {
                looks += 1;
                None
            },
            |_| {},
        );
        assert_eq!(found, None);
        // 0, 200, 400, then a last look at 500.
        assert_eq!(looks, 4);
        assert_eq!(clock.elapsed(), Duration::from_millis(500));
    }

    #[test]
    fn zero_timeout_looks_once() {
        let clock = ManualClock::at(0);
        let mut looks = 0;
        let found: Option<()> = wait_for(
            &clock,
            Poll::new(Duration::ZERO),
            || {
                looks += 1;
                None
            },
            |_| {},
        );
        assert_eq!((found, looks), (None, 1));
        assert_eq!(clock.elapsed(), Duration::ZERO);
    }

    #[test]
    fn manual_wall_clock_moves_with_sleep() {
        let clock = ManualClock::at(1_792_256_223);
        clock.sleep(Duration::from_secs(2));
        assert_eq!(
            clock.wall(),
            UNIX_EPOCH + Duration::from_secs(1_792_256_225)
        );
    }
}
