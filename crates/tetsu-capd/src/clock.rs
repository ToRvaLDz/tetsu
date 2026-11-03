//! The time source, so tests can set the clock.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Where "now" comes from.
pub trait Clock {
    /// The current time.
    fn now(&self) -> SystemTime;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A clock that only moves when told to. Clones share the time.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct FixedClock(Rc<Cell<SystemTime>>);

impl FixedClock {
    /// A clock at `start` seconds after the Unix epoch.
    pub fn at(start: u64) -> Self {
        Self(Rc::new(Cell::new(UNIX_EPOCH + Duration::from_secs(start))))
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        self.0.set(self.0.get() + by);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0.get()
    }
}

/// RFC 3339 UTC with milliseconds, for the wire.
pub fn rfc3339(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    tetsu_tx::serve::convert::rfc3339_ms(secs)
}
