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

/// One output for [`place`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceIn<'a> {
    /// The output's id.
    pub id: &'a str,
    /// Logical width.
    pub w: i64,
    /// Logical height.
    pub h: i64,
    /// Whether the output is lit.
    pub enabled: bool,
    /// The position the user pinned, if any.
    pub pin: Option<(i64, i64)>,
}

fn overlap((ax, ay, aw, ah): (i64, i64, i64, i64), (bx, by, bw, bh): (i64, i64, i64, i64)) -> bool {
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

/// Places enabled outputs, in the order given. A pinned output goes where
/// it is pinned, unless that overlaps (by a positive area) an earlier
/// pinned one: it then counts as automatic, and its id is returned in the
/// second list. Automatic outputs are packed at `y = 0` from the right edge
/// of the rightmost pinned output (0 when none is pinned). Disabled
/// outputs are left out.
pub fn place(outputs: &[PlaceIn<'_>]) -> (Vec<(String, i64, i64)>, Vec<String>) {
    let mut taken: Vec<(i64, i64, i64, i64)> = Vec::new();
    let mut pinned: Vec<Option<(i64, i64)>> = Vec::with_capacity(outputs.len());
    let mut fell_back = Vec::new();
    for out in outputs {
        let accepted = out.pin.filter(|_| out.enabled).and_then(|(x, y)| {
            let rect = (x, y, out.w, out.h);
            if taken.iter().any(|t| overlap(*t, rect)) {
                fell_back.push(out.id.to_owned());
                None
            } else {
                taken.push(rect);
                Some((x, y))
            }
        });
        pinned.push(accepted);
    }
    let mut next = taken.iter().map(|(x, _, w, _)| x + w).max().unwrap_or(0);
    let mut placed = Vec::new();
    for (out, pin) in outputs.iter().zip(pinned) {
        if !out.enabled {
            continue;
        }
        match pin {
            Some((x, y)) => placed.push((out.id.to_owned(), x, y)),
            None => {
                placed.push((out.id.to_owned(), next, 0));
                next += out.w;
            }
        }
    }
    (placed, fell_back)
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
