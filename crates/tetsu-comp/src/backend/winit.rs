//! The winit backend: the compositor in a window of the host session, for
//! development.

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent};
use smithay::utils::{Rectangle, Transform};

use crate::input::ModKey;
use crate::lock::glue::View;
use crate::render::{CLEAR_COLOR, LOCK_COLOR};
use crate::state::State;

/// The output's name.
pub const OUTPUT_NAME: &str = "winit";
/// The refresh rate reported to clients, in mHz.
const REFRESH_MHZ: i32 = 60_000;

/// Opens the window, adds its output and starts drawing.
pub fn init(state: &mut State) -> Result<(), Box<dyn std::error::Error>> {
    // The host compositor grabs Super.
    state.mod_key = ModKey::CtrlAlt;
    let (mut backend, winit_events) = winit::init::<GlesRenderer>()?;
    let output = state.add_virtual_output(OUTPUT_NAME, backend.window_size(), REFRESH_MHZ);
    // The winit framebuffer is upside down.
    output.change_current_state(None, Some(Transform::Flipped180), None, None);
    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    state
        .loop_handle
        .insert_source(winit_events, move |event, (), state| match event {
            WinitEvent::Resized { size, .. } => state.resize_output(&output, size, REFRESH_MHZ),
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let size = backend.window_size();
                // The scene is decided once per frame, here and nowhere else.
                let view = state.view(&output);
                let mut drawn = false;
                {
                    let Ok((renderer, mut framebuffer)) = backend.bind() else {
                        return;
                    };
                    let rendered = match &view {
                        View::Normal => smithay::desktop::space::render_output::<
                            _,
                            WaylandSurfaceRenderElement<GlesRenderer>,
                            _,
                            _,
                        >(
                            &output,
                            renderer,
                            &mut framebuffer,
                            1.0,
                            0,
                            [&state.space],
                            &[],
                            &mut damage_tracker,
                            CLEAR_COLOR,
                        )
                        .map(|_| ()),
                        // Locked: the lock surface over a solid colour; no
                        // space, so no window can show.
                        View::Locked(surface) => {
                            let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = surface
                                .as_ref()
                                .map(|surface| {
                                    render_elements_from_surface_tree(
                                        renderer,
                                        surface,
                                        (0, 0),
                                        1.0,
                                        1.0,
                                        Kind::Unspecified,
                                    )
                                })
                                .unwrap_or_default();
                            damage_tracker
                                .render_output(renderer, &mut framebuffer, 0, &elements, LOCK_COLOR)
                                .map(|_| ())
                        }
                    };
                    match rendered {
                        Ok(()) => drawn = true,
                        Err(e) => eprintln!("tetsu-comp: render failed: {e}"),
                    }
                }
                match backend.submit(Some(&[Rectangle::from_size(size)])) {
                    // Presented: only now does a locked frame count.
                    Ok(()) if drawn => {
                        let locked = matches!(view, View::Locked(_));
                        state.send_frames(&output);
                        state.output_presented(&output, locked);
                    }
                    Ok(()) => state.send_frames(&output),
                    Err(e) => {
                        eprintln!("tetsu-comp: swap failed: {e}");
                        state.send_frames(&output);
                    }
                }
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
