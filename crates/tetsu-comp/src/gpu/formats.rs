//! Format negotiation.
//!
//! Ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787`: the Intel CCS filter, the
//! color formats and the scanout tranches of `surface_dmabuf_feedback`. Data
//! only, over Smithay's `Format`, `Fourcc` and `Modifier`.

use smithay::backend::allocator::{Format, Fourcc, Modifier};

/// 10-bit first, for the render device's own outputs.
const COLOR_SAME_GPU: [Fourcc; 3] = [Fourcc::Abgr2101010, Fourcc::Argb8888, Fourcc::Abgr8888];

/// Across GPUs: an NVIDIA dGPU copying to an iGPU only understands `Abgr`,
/// not `Argb`, and no 10-bit (Smithay #2119, #2154).
const COLOR_CROSS_GPU: [Fourcc; 2] = [Fourcc::Abgr8888, Fourcc::Argb8888];

/// Intel CCS modifiers: they raise the bandwidth enough to break some
/// monitor setups.
fn is_ccs(modifier: Modifier) -> bool {
    matches!(
        modifier,
        Modifier::I915_y_tiled_ccs
            // I915_FORMAT_MOD_Yf_TILED_CCS
            | Modifier::Unrecognized(0x100000000000005)
            | Modifier::I915_y_tiled_gen12_rc_ccs
            | Modifier::I915_y_tiled_gen12_mc_ccs
            // I915_FORMAT_MOD_Y_TILED_GEN12_RC_CCS_CC
            | Modifier::Unrecognized(0x100000000000008)
            // I915_FORMAT_MOD_4_TILED_DG2_RC_CCS
            | Modifier::Unrecognized(0x10000000000000a)
            // I915_FORMAT_MOD_4_TILED_DG2_MC_CCS
            | Modifier::Unrecognized(0x10000000000000b)
            // I915_FORMAT_MOD_4_TILED_DG2_RC_CCS_CC
            | Modifier::Unrecognized(0x10000000000000c)
    )
}

/// The formats an output may be rendered in: no CCS; with `linear_only`
/// (a display-only device) the linear modifier alone.
pub fn render_formats(formats: &[Format], linear_only: bool) -> Vec<Format> {
    formats
        .iter()
        .copied()
        .filter(|format| {
            if linear_only {
                format.modifier == Modifier::Linear
            } else {
                !is_ccs(format.modifier)
            }
        })
        .collect()
}

/// The scanout color formats, in preference order.
pub fn color_formats(cross_gpu: bool) -> &'static [Fourcc] {
    if cross_gpu {
        &COLOR_CROSS_GPU
    } else {
        &COLOR_SAME_GPU
    }
}

/// The scanout tranches of the per-output dmabuf feedback: the primary-plane
/// formats, then primary-or-overlay-plane formats, each limited to what the
/// render device can also render so a fallback path always exists. Off the
/// same GPU only `Linear` is kept (AMD iGPU + dGPU glitch, and display-only
/// devices).
pub fn scanout_tranches(
    primary_plane: &[Format],
    overlay_planes: &[Format],
    render_formats: &[Format],
    same_gpu: bool,
) -> (Vec<Format>, Vec<Format>) {
    let keep = |format: &Format| {
        render_formats.contains(format) && (same_gpu || format.modifier == Modifier::Linear)
    };
    let mut primary = Vec::new();
    for format in primary_plane.iter().filter(|f| keep(f)) {
        if !primary.contains(format) {
            primary.push(*format);
        }
    }
    let mut either = primary.clone();
    for format in overlay_planes.iter().filter(|f| keep(f)) {
        if !either.contains(format) {
            either.push(*format);
        }
    }
    (primary, either)
}
