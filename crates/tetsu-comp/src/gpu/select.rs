//! The render device: chosen once at session start from what is plugged in.
//!
//! Policy ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787`; niri's config and IPC are not.
//! The order: a user override, else the hardware GPU that drives a connected
//! external output, then the most connected outputs, then the udev primary,
//! then the lowest id. Software devices count only when no hardware exists.

/// One GPU that has a render node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The render node's `dev_t`.
    pub id: u64,
    /// The udev primary GPU.
    pub primary: bool,
    /// A software renderer (llvmpipe).
    pub software: bool,
    /// The connected connector names on this card.
    pub outputs: Vec<String>,
}

/// The answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// A device was chosen.
    Device {
        /// The render node's `dev_t`.
        id: u64,
        /// `false` when the user override decided.
        automatic: bool,
        /// Why, for the log.
        reason: &'static str,
    },
    /// No GPU at all.
    NoGpu,
}

/// An internal panel (laptop screen).
pub fn is_internal(name: &str) -> bool {
    ["eDP-", "LVDS-", "DSI-"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn drives_external(c: &Candidate) -> bool {
    c.outputs.iter().any(|name| !is_internal(name))
}

/// Chooses the render device. `override_id`: the outer `None` is no override,
/// `Some(None)` an override that names no present device.
pub fn select(candidates: &[Candidate], override_id: Option<Option<u64>>) -> Selection {
    if candidates.is_empty() {
        return Selection::NoGpu;
    }
    if let Some(Some(id)) = override_id
        && candidates.iter().any(|c| c.id == id)
    {
        return Selection::Device {
            id,
            automatic: false,
            reason: "set in config",
        };
    }

    let mut pool: Vec<&Candidate> = candidates.iter().filter(|c| !c.software).collect();
    if pool.is_empty() {
        pool = candidates.iter().collect();
    }
    // Best first: external output, output count, primary, lowest id.
    pool.sort_by_key(|c| {
        (
            std::cmp::Reverse(drives_external(c)),
            std::cmp::Reverse(c.outputs.len()),
            std::cmp::Reverse(c.primary),
            c.id,
        )
    });
    let best = pool[0];
    let reason = match pool.get(1) {
        None => "only device",
        Some(next) if drives_external(best) != drives_external(next) => "drives an external output",
        Some(next) if best.outputs.len() != next.outputs.len() => "most connected outputs",
        Some(next) if best.primary != next.primary => "primary gpu",
        Some(_) => "lowest id",
    };
    // An override that names nothing present: say so instead of why auto won.
    let reason = if override_id.is_some() {
        "override not found"
    } else {
        reason
    };
    Selection::Device {
        id: best.id,
        automatic: true,
        reason,
    }
}
