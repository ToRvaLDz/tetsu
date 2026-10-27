//! Explicit sync (`wp_linux_drm_syncobj_manager_v1`) and how a commit is
//! gated.
//!
//! Ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787` (the NVIDIA >= 555 rule and the
//! syncobj_eventfd check); the gate order follows Smithay's anvil: the
//! acquire point first, then the implicit dmabuf fence, then nothing.

use crate::settings::SyncMode;

/// The first NVIDIA driver with working explicit sync.
const NVIDIA_MIN_MAJOR: u32 = 555;

/// What the render device offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Support {
    /// `supports_syncobj_eventfd` holds on the render device.
    pub eventfd: bool,
    /// The DRM driver name (`nvidia-drm`, `i915`, ...).
    pub driver: String,
    /// The NVIDIA driver version, when the driver is NVIDIA.
    pub version: Option<(u32, u32, u32)>,
}

/// The decision; mirrors `org.tetsu.comp.render.v1` `ExplicitSync` (#73).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitSync {
    /// The configured mode.
    pub mode: SyncMode,
    /// The render device can do it.
    pub supported: bool,
    /// The global is advertised.
    pub enabled: bool,
    /// Why not, when not enabled.
    pub reason: Option<String>,
}

/// Parses `/sys/module/nvidia_drm/version` (`560.35.03\n`).
pub fn parse_driver_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Whether explicit sync is on. `On` cannot force it onto an unsupported
/// device: it reports why instead.
pub fn decide(mode: SyncMode, support: Support) -> ExplicitSync {
    let is_nvidia = support.driver.contains("nvidia");
    let unsupported = if !support.eventfd {
        Some("no syncobj_eventfd")
    } else if is_nvidia
        && support
            .version
            .is_none_or(|(major, _, _)| major < NVIDIA_MIN_MAJOR)
    {
        Some("nvidia < 555")
    } else {
        None
    };
    let supported = unsupported.is_none();
    let (enabled, reason) = match (mode, unsupported) {
        (SyncMode::Off, _) => (false, Some("off in config")),
        (_, Some(why)) => (false, Some(why)),
        (_, None) => (true, None),
    };
    ExplicitSync {
        mode,
        supported,
        enabled,
        reason: reason.map(str::to_owned),
    }
}

/// What the pending buffer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// A dmabuf.
    Dmabuf,
    /// A shared-memory buffer.
    Shm,
    /// No buffer attached.
    None,
}

/// What a commit waits for before it is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// The client's explicit acquire point.
    AcquirePoint,
    /// The dmabuf's implicit fence.
    DmabufFence,
    /// Nothing.
    Immediate,
}

/// How a commit is gated.
pub fn commit_gate(syncobj_enabled: bool, has_acquire_point: bool, buffer: BufferKind) -> Gate {
    match buffer {
        BufferKind::None => Gate::Immediate,
        _ if syncobj_enabled && has_acquire_point => Gate::AcquirePoint,
        BufferKind::Dmabuf => Gate::DmabufFence,
        BufferKind::Shm => Gate::Immediate,
    }
}
