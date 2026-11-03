//! Tetsu policy for output configuration: the bounds a `ConfigureOutputs`
//! call and the `output.<connector>.*` config keys share.

/// The smallest scale factor.
pub const MIN_SCALE: f64 = 0.25;
/// The largest scale factor.
pub const MAX_SCALE: f64 = 8.0;
/// Positions stay within plus or minus this many logical pixels.
pub const MAX_COORD: i64 = 1 << 20;
/// The longest connector name a config key may hold.
pub const MAX_CONNECTOR_LEN: usize = 32;

/// Whether `name` is a connector name a config key can hold:
/// `[A-Za-z0-9_-]{1,32}`, so the key never has an extra dot.
pub fn valid_connector(name: &str) -> bool {
    (1..=MAX_CONNECTOR_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `scale` is finite and within the bounds.
pub fn scale_ok(scale: f64) -> bool {
    scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(&scale)
}

/// Whether a coordinate is within the bound.
pub fn coord_ok(value: i64) -> bool {
    (-MAX_COORD..=MAX_COORD).contains(&value)
}
