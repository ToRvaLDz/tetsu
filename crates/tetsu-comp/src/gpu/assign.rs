//! How an output on a given device is served by the render device.
//!
//! Ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787`: a display-only device (no
//! render node) uses the render device's allocator and linear buffers.

/// Whose allocator makes the scanout buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alloc {
    /// The output's own device.
    Own,
    /// The render device, because the output's device cannot allocate.
    RenderDevice,
}

/// How one device's outputs are rendered and scanned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputPlan {
    /// The render node's `dev_t` that composes the frame.
    pub render: u64,
    /// The node that scans out: the device's render node, else the render one.
    pub target: u64,
    /// The frame is copied across GPUs.
    pub cross_gpu: bool,
    /// Who allocates the scanout buffers.
    pub allocator: Alloc,
    /// Only the linear modifier is safe.
    pub linear_only: bool,
}

/// `device_render`: the render node of the output's device, `None` for a
/// display-only device.
pub fn assign(device_render: Option<u64>, render: u64) -> OutputPlan {
    match device_render {
        Some(target) => OutputPlan {
            render,
            target,
            cross_gpu: target != render,
            allocator: Alloc::Own,
            linear_only: false,
        },
        None => OutputPlan {
            render,
            target: render,
            cross_gpu: true,
            allocator: Alloc::RenderDevice,
            linear_only: true,
        },
    }
}
