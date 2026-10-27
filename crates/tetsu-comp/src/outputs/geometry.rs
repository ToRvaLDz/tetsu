//! Logical sizes and the automatic layout of outputs.

use smithay::utils::Transform as SmithayTransform;
use tetsu_bus::output::{Mode, Transform};

/// The size of an output in the logical space: the mode divided by the
/// scale (a scale that is not finite or not positive counts as 1), with
/// the sides swapped for the 90 and 270 degree transforms.
pub fn logical_size(mode: &Mode, scale: f64, transform: Transform) -> (i64, i64) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let side = |pixels: i64| ((pixels as f64) / scale).round() as i64;
    let (w, h) = (side(mode.width), side(mode.height));
    match transform {
        Transform::Rotate90
        | Transform::Rotate270
        | Transform::Flipped90
        | Transform::Flipped270 => (h, w),
        _ => (w, h),
    }
}

/// Packs outputs left to right at `y = 0`, in the order given. Each entry
/// is an id, a logical width and whether the output is enabled; disabled
/// ones are left out of the result.
pub fn pack(outputs: &[(&str, i64, bool)]) -> Vec<(String, i64, i64)> {
    let mut x = 0;
    let mut packed = Vec::new();
    for &(id, width, enabled) in outputs {
        if enabled {
            packed.push((id.to_owned(), x, 0));
            x += width;
        }
    }
    packed
}

/// The Wayland transform of an `output.v1` one; unknown values do not
/// transform.
pub fn to_smithay_transform(transform: Transform) -> SmithayTransform {
    match transform {
        Transform::Rotate90 => SmithayTransform::_90,
        Transform::Rotate180 => SmithayTransform::_180,
        Transform::Rotate270 => SmithayTransform::_270,
        Transform::Flipped => SmithayTransform::Flipped,
        Transform::Flipped90 => SmithayTransform::Flipped90,
        Transform::Flipped180 => SmithayTransform::Flipped180,
        Transform::Flipped270 => SmithayTransform::Flipped270,
        Transform::Normal | Transform::Unknown => SmithayTransform::Normal,
    }
}
