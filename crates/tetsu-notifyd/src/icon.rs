//! `icon-static`: one still icon per freedesktop notification, from the
//! `image-data` hint, an image path, or an icon name looked up in the
//! hicolor theme and pixmaps. Anything unreadable means no icon.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use zbus::{OwnedVariant, Structure};

use crate::core::Icon;

/// Larger icons are refused rather than decoded.
const MAX_SIDE: i32 = 1024;
/// Larger icon files are refused rather than read.
const MAX_FILE: u64 = 4 * 1024 * 1024;
/// Icon loading gives up after this long.
const LOAD_TIMEOUT: Duration = Duration::from_secs(2);
/// The PNG signature, then the IHDR chunk with width and height.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Theme sizes tried for an icon name, best first.
const THEME_SIZES: [&str; 5] = ["64x64", "48x48", "128x128", "32x32", "256x256"];
/// Data directories when `XDG_DATA_DIRS` is unset.
const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// [`from_hints`] off the runtime thread, abandoned after a timeout, so a
/// slow file system can never stall the daemon.
pub async fn from_hints_bounded(
    hints: HashMap<String, OwnedVariant>,
    app_icon: String,
) -> Option<Icon> {
    let load = tokio::task::spawn_blocking(move || from_hints(&hints, &app_icon));
    tokio::time::timeout(LOAD_TIMEOUT, load).await.ok()?.ok()?
}

/// The icon of a `Notify` call, by the spec's order of precedence.
pub fn from_hints(hints: &HashMap<String, OwnedVariant>, app_icon: &str) -> Option<Icon> {
    ["image-data", "image_data"]
        .iter()
        .find_map(|key| hints.get(*key).and_then(image_data))
        .or_else(|| {
            ["image-path", "image_path"]
                .iter()
                .find_map(|key| hints.get(*key).and_then(|v| <&str>::try_from(v).ok()))
                .and_then(load)
        })
        .or_else(|| (!app_icon.is_empty()).then(|| load(app_icon)).flatten())
        .or_else(|| hints.get("icon_data").and_then(image_data))
}

fn image_data(value: &OwnedVariant) -> Option<Icon> {
    let structure = Structure::try_from(value.try_clone().ok()?).ok()?;
    let fields = structure.fields();
    let [w, h, stride, alpha, bits, channels, data] = fields else {
        return None;
    };
    let data: Vec<u8> = data.try_clone().ok()?.try_into().ok()?;
    from_image_data(
        i32::try_from(w).ok()?,
        i32::try_from(h).ok()?,
        i32::try_from(stride).ok()?,
        bool::try_from(alpha).ok()?,
        i32::try_from(bits).ok()?,
        i32::try_from(channels).ok()?,
        &data,
    )
}

/// Decodes the `(iiibiiay)` image of the spec into RGBA, or `None` when the
/// fields do not describe the data.
pub fn from_image_data(
    width: i32,
    height: i32,
    rowstride: i32,
    has_alpha: bool,
    bits_per_sample: i32,
    channels: i32,
    data: &[u8],
) -> Option<Icon> {
    let sides_ok = (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height);
    let expected_channels = if has_alpha { 4 } else { 3 };
    if !sides_ok || bits_per_sample != 8 || channels != expected_channels {
        return None;
    }
    let (w, h, stride, ch) = (
        width as usize,
        height as usize,
        usize::try_from(rowstride).ok()?,
        channels as usize,
    );
    if stride < w * ch || data.len() < stride * (h - 1) + w * ch {
        return None;
    }
    let rgba = (0..h)
        .flat_map(|y| data[y * stride..y * stride + w * ch].chunks_exact(ch))
        .flat_map(|px| [px[0], px[1], px[2], if ch == 4 { px[3] } else { 255 }])
        .collect::<Vec<u8>>();
    Some(Icon {
        width: width as u32,
        height: height as u32,
        rgba: Arc::from(rgba),
    })
}

/// An image path, `file://` URI or icon name. Only regular PNG files up to
/// [`MAX_FILE`] and [`MAX_SIDE`] are read.
pub fn load(spec: &str) -> Option<Icon> {
    let path = spec.strip_prefix("file://").unwrap_or(spec);
    if path.starts_with('/') {
        return load_png(Path::new(path));
    }
    if path.contains('/') || path.is_empty() {
        return None;
    }
    theme_paths(path).find_map(|p| load_png(&p))
}

fn theme_paths(name: &str) -> impl Iterator<Item = PathBuf> {
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| DEFAULT_DATA_DIRS.to_owned());
    let name = name.to_owned();
    let bases: Vec<PathBuf> = dirs.split(':').map(PathBuf::from).collect();
    let themed: Vec<PathBuf> = bases
        .iter()
        .flat_map(|base| {
            THEME_SIZES
                .iter()
                .map(move |size| base.join(format!("icons/hicolor/{size}/apps")))
        })
        .chain(bases.iter().map(|base| base.join("pixmaps")))
        .map(|dir| dir.join(format!("{name}.png")))
        .collect();
    themed.into_iter()
}

fn load_png(path: &Path) -> Option<Icon> {
    let bytes = read_bounded(path)?;
    let header = bytes.strip_prefix(PNG_SIGNATURE)?;
    let side = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(header.get(at..at + 4)?.try_into().ok()?))
    };
    // IHDR: length (4), type (4), width (4), height (4).
    let (width, height) = (side(8)?, side(12)?);
    if width == 0 || height == 0 || width > MAX_SIDE as u32 || height > MAX_SIDE as u32 {
        return None;
    }
    let pixmap = tiny_skia::Pixmap::decode_png(&bytes).ok()?;
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect::<Vec<u8>>();
    Some(Icon {
        width: pixmap.width(),
        height: pixmap.height(),
        rgba: Arc::from(rgba),
    })
}

/// Reads a regular file of at most [`MAX_FILE`] bytes. The path is first
/// opened `O_PATH`, which touches no device, and only a regular file is then
/// opened for reading, through that very descriptor.
fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let pinned: File = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH)
        .open(path)
        .ok()?;
    let meta = pinned.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let file: File = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
        .ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_FILE).then_some(bytes)
}
