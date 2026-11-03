//! How each output is served relative to the render device.

use tetsu_comp::gpu::assign::{Alloc, OutputPlan, assign};

#[test]
fn the_same_gpu_allocates_on_itself_and_is_not_cross_gpu() {
    assert_eq!(
        assign(Some(1), 1),
        OutputPlan {
            render: 1,
            target: 1,
            cross_gpu: false,
            allocator: Alloc::Own,
            linear_only: false,
        }
    );
}

#[test]
fn the_igpu_panel_while_the_dgpu_renders_is_cross_gpu() {
    let plan = assign(Some(1), 2);
    assert_eq!(plan.render, 2);
    assert_eq!(plan.target, 1);
    assert!(plan.cross_gpu);
    assert_eq!(plan.allocator, Alloc::Own);
    assert!(!plan.linear_only);
}

#[test]
fn a_display_only_device_borrows_the_render_allocator_with_linear_buffers() {
    let plan = assign(None, 2);
    assert_eq!(plan.render, 2);
    assert_eq!(plan.target, 2);
    assert!(plan.cross_gpu);
    assert_eq!(plan.allocator, Alloc::RenderDevice);
    assert!(plan.linear_only);
}
