//! Values read from the kernel's `/sys` and `/proc`. Each reader takes the
//! filesystem root, `/` in production and a tempdir tree in tests.

use std::time::Duration;

pub mod battery;
pub mod network;

/// How often battery and network are sampled. A sample only redraws when
/// its value changed.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
