//! Format negotiation: CCS filtering, linear-only, color formats, tranches.

use smithay::backend::allocator::{Format, Fourcc, Modifier};
use tetsu_comp::gpu::formats::{color_formats, render_formats, scanout_tranches};

fn f(code: Fourcc, modifier: Modifier) -> Format {
    Format { code, modifier }
}

const CCS_IDS: [u64; 5] = [
    0x100000000000005,
    0x100000000000008,
    0x10000000000000a,
    0x10000000000000b,
    0x10000000000000c,
];

fn ccs_modifiers() -> Vec<Modifier> {
    let mut v = vec![
        Modifier::I915_y_tiled_ccs,
        Modifier::I915_y_tiled_gen12_rc_ccs,
        Modifier::I915_y_tiled_gen12_mc_ccs,
    ];
    v.extend(CCS_IDS.iter().map(|id| Modifier::from(*id)));
    v
}

#[test]
fn render_formats_drop_every_ccs_modifier() {
    let mut input = vec![f(Fourcc::Argb8888, Modifier::Linear)];
    input.extend(ccs_modifiers().into_iter().map(|m| f(Fourcc::Argb8888, m)));
    assert_eq!(
        render_formats(&input, false),
        vec![f(Fourcc::Argb8888, Modifier::Linear)]
    );
}

#[test]
fn render_formats_keep_other_modifiers() {
    let input = vec![
        f(Fourcc::Argb8888, Modifier::Linear),
        f(Fourcc::Argb8888, Modifier::Invalid),
        f(Fourcc::Argb8888, Modifier::from(0x0123_4567_u64)),
    ];
    assert_eq!(render_formats(&input, false), input);
}

#[test]
fn linear_only_keeps_linear_alone() {
    let input = vec![
        f(Fourcc::Argb8888, Modifier::Linear),
        f(Fourcc::Argb8888, Modifier::Invalid),
        f(Fourcc::Xrgb8888, Modifier::Linear),
        f(Fourcc::Xrgb8888, Modifier::from(0x0123_4567_u64)),
    ];
    assert_eq!(
        render_formats(&input, true),
        vec![
            f(Fourcc::Argb8888, Modifier::Linear),
            f(Fourcc::Xrgb8888, Modifier::Linear)
        ]
    );
}

#[test]
fn color_formats_allow_ten_bit_only_on_the_same_gpu() {
    assert_eq!(
        color_formats(false),
        &[Fourcc::Abgr2101010, Fourcc::Argb8888, Fourcc::Abgr8888]
    );
    assert_eq!(color_formats(true), &[Fourcc::Abgr8888, Fourcc::Argb8888]);
}

#[test]
fn tranches_are_the_intersection_with_the_render_formats() {
    let render = vec![
        f(Fourcc::Argb8888, Modifier::Linear),
        f(Fourcc::Argb8888, Modifier::from(7_u64)),
        f(Fourcc::Xrgb8888, Modifier::Linear),
    ];
    let primary = vec![
        f(Fourcc::Argb8888, Modifier::Linear),
        f(Fourcc::Abgr8888, Modifier::Linear),
    ];
    let overlay = vec![f(Fourcc::Xrgb8888, Modifier::Linear)];
    let (p, po) = scanout_tranches(&primary, &overlay, &render, true);
    assert_eq!(p, vec![f(Fourcc::Argb8888, Modifier::Linear)]);
    assert_eq!(
        po,
        vec![
            f(Fourcc::Argb8888, Modifier::Linear),
            f(Fourcc::Xrgb8888, Modifier::Linear)
        ]
    );
}

#[test]
fn tranches_keep_linear_only_across_gpus() {
    let render = vec![
        f(Fourcc::Argb8888, Modifier::Linear),
        f(Fourcc::Argb8888, Modifier::from(7_u64)),
    ];
    let primary = render.clone();
    let (p, po) = scanout_tranches(&primary, &[], &render, false);
    assert_eq!(p, vec![f(Fourcc::Argb8888, Modifier::Linear)]);
    assert_eq!(po, vec![f(Fourcc::Argb8888, Modifier::Linear)]);
    let (p, _) = scanout_tranches(&primary, &[], &render, true);
    assert_eq!(p.len(), 2);
}

#[test]
fn every_scanout_format_is_also_a_render_format() {
    let render = vec![
        f(Fourcc::Argb8888, Modifier::Linear),
        f(Fourcc::Xrgb8888, Modifier::Linear),
    ];
    let plane = vec![
        f(Fourcc::Argb8888, Modifier::Linear),
        f(Fourcc::Nv12, Modifier::Linear),
        f(Fourcc::Xbgr8888, Modifier::Invalid),
    ];
    for same in [true, false] {
        let (p, po) = scanout_tranches(&plane, &plane, &render, same);
        for format in p.iter().chain(&po) {
            assert!(render.contains(format), "{format:?}");
        }
    }
}
