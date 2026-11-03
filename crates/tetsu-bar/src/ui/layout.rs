//! Where each segment goes, in logical pixels along the bar. Text widths
//! come through [`Measure`], so layout needs no fonts.
//!
//! Workspace cells go left, the clock is centred, the network and the
//! battery are flush right. On a narrow bar the clock is dropped before it
//! would touch a neighbour, then workspace cells that no longer fit.

use crate::model::Bar;

/// Space at both ends of the bar.
pub const PADDING: f32 = 12.0;
/// Space between the network and the battery, and around the clock.
pub const GAP: f32 = 16.0;
/// Space inside a workspace cell, each side of its label.
pub const CELL_PADDING: f32 = 8.0;
/// Space between workspace cells.
pub const CELL_GAP: f32 = 4.0;

/// Measures text.
pub trait Measure {
    /// The width of `text` on one line.
    fn width(&mut self, text: &str) -> f32;
}

/// A horizontal extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// Left edge.
    pub x: f32,
    /// Width.
    pub w: f32,
}

impl Placed {
    fn right(&self) -> f32 {
        self.x + self.w
    }
}

/// The placed segments; absent ones take no room.
#[derive(Debug, Clone, PartialEq)]
pub struct Segments {
    /// One per workspace cell, in order.
    pub workspaces: Vec<Placed>,
    /// The clock, `None` when the bar is too narrow for it.
    pub clock: Option<Placed>,
    /// The network label.
    pub network: Placed,
    /// The battery label, `None` without a battery.
    pub battery: Option<Placed>,
}

/// Places the segments of `bar` on a bar `width` pixels wide.
pub fn layout(bar: &Bar, width: f32, measure: &mut impl Measure) -> Segments {
    // The right group, laid out from the right edge.
    let mut edge = width - PADDING;
    let battery = bar.battery.as_ref().map(|battery| {
        let w = measure.width(&battery.label());
        edge -= w;
        let placed = Placed { x: edge, w };
        edge -= GAP;
        placed
    });
    let w = measure.width(&bar.network.label());
    let network = Placed { x: edge - w, w };
    let right_start = network.x;

    // The workspace cells, as many as fit before the right group.
    let mut workspaces = Vec::new();
    let mut x = PADDING;
    for cell in bar.workspaces.iter().flatten() {
        let w = measure.width(&cell.label) + 2.0 * CELL_PADDING;
        if x + w + GAP > right_start {
            break;
        }
        workspaces.push(Placed { x, w });
        x += w + CELL_GAP;
    }
    let left_end = workspaces.last().map_or(0.0, Placed::right);

    let w = measure.width(&bar.clock);
    let clock = Placed {
        x: (width - w) / 2.0,
        w,
    };
    let fits = clock.x >= left_end + GAP && clock.right() + GAP <= right_start;
    Segments {
        workspaces,
        clock: fits.then_some(clock),
        network,
        battery,
    }
}
