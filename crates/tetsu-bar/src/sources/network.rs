//! The network segment, from `/sys/class/net` and `/proc/net/wireless`.
//! No SSID: that needs nl80211 or NetworkManager.

use std::fs;
use std::path::Path;

/// The best link that is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// A wired interface that is up.
    Wired {
        /// Its name.
        iface: String,
    },
    /// A wireless interface that is up.
    Wifi {
        /// Its name.
        iface: String,
        /// Link quality in percent, when `/proc/net/wireless` lists it.
        quality: Option<u8>,
    },
    /// No physical link is up.
    Offline,
}

impl Network {
    /// `eth <iface>`, `wifi <q>%` (`wifi <iface>` without a quality) or
    /// `offline`.
    pub fn label(&self) -> String {
        match self {
            Self::Wired { iface } => format!("eth {iface}"),
            Self::Wifi {
                quality: Some(q), ..
            } => format!("wifi {q}%"),
            Self::Wifi { iface, .. } => format!("wifi {iface}"),
            Self::Offline => "offline".into(),
        }
    }
}

/// The `link` quality of `/proc/net/wireless` is out of this many.
const FULL_LINK: u32 = 70;

/// `iface -> percent` from one `/proc/net/wireless` line, if it is `iface`'s.
fn quality_of(line: &str, iface: &str) -> Option<u8> {
    let (name, rest) = line.split_once(':')?;
    if name.trim() != iface {
        return None;
    }
    let link = rest.split_whitespace().nth(1)?;
    let link: f32 = link.trim_end_matches('.').parse().ok()?;
    let percent = (link * 100.0 / FULL_LINK as f32).round().clamp(0.0, 100.0);
    Some(percent as u8)
}

fn wifi_quality(root: &Path, iface: &str) -> Option<u8> {
    let table = fs::read_to_string(root.join("proc/net/wireless")).ok()?;
    table.lines().find_map(|line| quality_of(line, iface))
}

/// An interface that is up, as `(name, is_wifi)`; loopback and virtual
/// interfaces (no `device` link) are left out.
fn physical_up(dir: &Path, name: &str) -> Option<(String, bool)> {
    if name == "lo" || fs::symlink_metadata(dir.join("device")).is_err() {
        return None;
    }
    let state = fs::read_to_string(dir.join("operstate")).ok()?;
    if state.trim() != "up" {
        return None;
    }
    let wifi = dir.join("wireless").exists() || dir.join("phy80211").exists();
    Some((name.to_owned(), wifi))
}

/// Reads the links under `root`: a wired one that is up wins over Wi-Fi.
pub fn read(root: &Path) -> Network {
    let Ok(entries) = fs::read_dir(root.join("sys/class/net")) else {
        return Network::Offline;
    };
    let mut up: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            physical_up(&entry.path(), &name)
        })
        .collect();
    up.sort();
    if let Some((iface, _)) = up.iter().find(|(_, wifi)| !wifi) {
        return Network::Wired {
            iface: iface.clone(),
        };
    }
    match up.into_iter().next() {
        Some((iface, _)) => Network::Wifi {
            quality: wifi_quality(root, &iface),
            iface,
        },
        None => Network::Offline,
    }
}
