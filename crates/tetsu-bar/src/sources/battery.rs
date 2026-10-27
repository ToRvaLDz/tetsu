//! The battery segment, from `/sys/class/power_supply`.

use std::fs;
use std::path::Path;

/// Whether the battery is filling, emptying or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// At least one battery is charging.
    Charging,
    /// At least one battery is discharging, none is charging.
    Discharging,
    /// Anything else.
    Full,
}

/// The laptop battery as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// The mean capacity of the batteries.
    pub percent: u8,
    /// The combined status.
    pub status: Status,
}

impl Battery {
    /// `BAT 85%`, with a `+` while charging.
    pub fn label(&self) -> String {
        let plus = if self.status == Status::Charging {
            "+"
        } else {
            ""
        };
        format!("BAT {}%{plus}", self.percent)
    }
}

fn text(path: &Path) -> Option<String> {
    Some(fs::read_to_string(path).ok()?.trim().to_owned())
}

/// One supply as `(capacity, status)`; `None` when it is not a system
/// battery or its capacity is unreadable.
fn supply(dir: &Path) -> Option<(u32, Status)> {
    if text(&dir.join("type"))? != "Battery"
        || text(&dir.join("scope")).as_deref() == Some("Device")
    {
        return None;
    }
    let capacity: u32 = text(&dir.join("capacity"))?
        .parse()
        .ok()
        .filter(|c| *c <= 100)?;
    let status = match text(&dir.join("status")).as_deref() {
        Some("Charging") => Status::Charging,
        Some("Discharging") => Status::Discharging,
        _ => Status::Full,
    };
    Some((capacity, status))
}

/// Reads every battery under `root`; `None` with no readable battery.
pub fn read(root: &Path) -> Option<Battery> {
    let entries = fs::read_dir(root.join("sys/class/power_supply")).ok()?;
    let supplies: Vec<(u32, Status)> = entries
        .flatten()
        .filter_map(|entry| supply(&entry.path()))
        .collect();
    if supplies.is_empty() {
        return None;
    }
    let count = supplies.len() as u32;
    let sum: u32 = supplies.iter().map(|(capacity, _)| capacity).sum();
    let has = |wanted| supplies.iter().any(|(_, status)| *status == wanted);
    let status = if has(Status::Charging) {
        Status::Charging
    } else if has(Status::Discharging) {
        Status::Discharging
    } else {
        Status::Full
    };
    Some(Battery {
        percent: ((sum + count / 2) / count) as u8,
        status,
    })
}
