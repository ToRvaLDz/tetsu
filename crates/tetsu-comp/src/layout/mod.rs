//! The layout algorithms. The viewport equals the output for now (#22.6);
//! the outputs ticket narrows it by layer-shell zones. Each algorithm maps
//! (viewport, gaps, n, params) to rects and knows nothing else.

pub mod dwindle;
pub mod floating;
pub mod master;

use std::fmt;
use std::str::FromStr;

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl Rect {
    /// The viewport inset by `gaps.outer`; every side stays positive.
    pub(crate) fn inset(self, gaps: Gaps) -> Self {
        Self {
            x: self.x + gaps.outer,
            y: self.y + gaps.outer,
            w: (self.w - 2 * gaps.outer).max(1),
            h: (self.h - 2 * gaps.outer).max(1),
        }
    }
}

/// Space around and between windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gaps {
    /// Between the viewport edge and the windows.
    pub outer: i32,
    /// Between two windows.
    pub inner: i32,
}

impl Gaps {
    /// No gaps.
    pub const NONE: Self = Self { outer: 0, inner: 0 };
}

/// How a workspace arranges its windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutKind {
    /// Each window halves what is left, alternating the split side.
    #[default]
    Dwindle,
    /// One master on the left, the rest stacked on the right.
    Master,
    /// Every window floats.
    Floating,
}

impl LayoutKind {
    /// The next one in the cycle dwindle, master, floating.
    pub fn next(self) -> Self {
        match self {
            Self::Dwindle => Self::Master,
            Self::Master => Self::Floating,
            Self::Floating => Self::Dwindle,
        }
    }

    /// The name used in the config file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
            Self::Floating => "floating",
        }
    }
}

/// A layout name that is none of the three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownLayout(pub String);

impl fmt::Display for UnknownLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown layout {:?} (dwindle, master or floating)",
            self.0
        )
    }
}

impl std::error::Error for UnknownLayout {}

impl FromStr for LayoutKind {
    type Err = UnknownLayout;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "dwindle" => Ok(Self::Dwindle),
            "master" => Ok(Self::Master),
            "floating" => Ok(Self::Floating),
            other => Err(UnknownLayout(other.to_owned())),
        }
    }
}

/// Cuts `len` into `parts` pieces separated by `inner`: `(offset, length)`
/// each. The edges are rounded from `len`, so the pieces and the gaps
/// leave no hole and no overlap, and the last one ends exactly at `len`.
/// Every piece is at least 1 long.
pub fn split(len: i32, parts: usize, inner: i32) -> Vec<(i32, i32)> {
    let Ok(count) = i32::try_from(parts) else {
        return Vec::new();
    };
    let len = len.max(count);
    let edge = |i: i32| (i64::from(len) * i64::from(i) / i64::from(count)) as i32;
    (0..count)
        .map(|i| {
            let (start, end) = (edge(i), edge(i + 1));
            let gap = if i + 1 < count { inner } else { 0 };
            (start, (end - start - gap).max(1))
        })
        .collect()
}
