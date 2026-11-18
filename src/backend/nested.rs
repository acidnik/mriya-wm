//! Nested backend: a window inside the parent Wayland session.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer, ImportMemWl,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        winit::{
            dpi::LogicalSize, platform::wayland::WindowAttributesExtWayland,
            window::Window as WinitWindow,
        },
    },
    utils::{Rectangle, Transform},
};
use tracing::{info, warn};

use crate::Mriya;

/// Colour of the "desktop" behind all windows.
const BACKGROUND: [f32; 4] = [0.11, 0.11, 0.13, 1.0];
/// Refresh rate advertised for the nested output.
const REFRESH_MHZ: i32 = 60_000;
/// Size of the nested window when the parent compositor lets us choose.
const DEFAULT_SIZE: (u32, u32) = (1280, 800);

pub fn run(mut event_loop: EventLoop<Mriya>, mut state: Mriya) -> Result<()> {
    let attributes = WinitWindow::default_attributes()
        .with_title("Mriya WM")
        .with_name("mriya-wm", "mriya-wm")
        .with_inner_size(LogicalSize::new(
            f64::from(DEFAULT_SIZE.0),
            f64::from(DEFAULT_SIZE.1),
        ))
        .with_visible(true);

    let (mut backend, winit) = winit::init_from_attributes::<GlesRenderer>(attributes)
        .map_err(|err| anyhow!("failed to initialize the nested winit backend: {err}"))?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: REFRESH_MHZ,
    };

    let output = Output::new(
        "mriya-nested".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Mriya".into(),
            model: "Nested".into(),
        },
    );
    output.create_global::<Mriya>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));
    state
        .shm_state
        .update_formats(backend.renderer().shm_formats());

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    let output_for_events = output.clone();
    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            match event {
                WinitEvent::Resized { size, .. } => {
                    output_for_events.change_current_state(
                        Some(Mode {
                            size,
                            refresh: REFRESH_MHZ,
                        }),
                        None,
                        None,
                        None,
                    );
                    state.request_redraw();
                }
                WinitEvent::Input(event) => {
                    state.process_input_event(event);
                    state.request_redraw();
                }
                WinitEvent::Redraw => {
                    match render_frame(&mut backend, &mut damage_tracker, state, &output_for_events)
                    {
                        Ok(()) => state.needs_redraw = false,
                        Err(err) => warn!(%err, "failed to render a frame"),
                    }
                }
                WinitEvent::CloseRequested => state.loop_signal.stop(),
                _ => {}
            }

            // Winit only sends `Redraw` when we ask for it, so a dirty scene
            // has to be turned into an explicit request right here.
            if state.needs_redraw {
                backend.window().request_redraw();
            }
        })
        .expect("failed to insert the winit event source");

    // From here on child processes (and anything launched from a shell we
    // start) have to talk to us instead of the parent compositor. `winit` is
    // already connected to the parent at this point.
    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    info!(
        "nested compositor is running, clients use WAYLAND_DISPLAY={}",
        state.socket_name.to_string_lossy()
    );

    event_loop
        .run(None, &mut state, |_| {})
        .context("event loop failed")
}

fn render_frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: &mut OutputDamageTracker,
    state: &mut Mriya,
    output: &Output,
) -> Result<()> {
    let size = backend.window_size();
    let damage = Rectangle::from_size(size);

    {
        let (renderer, mut framebuffer) = backend.bind()?;
        smithay::desktop::space::render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _>(
            output,
            renderer,
            &mut framebuffer,
            1.0,
            0,
            [&state.space],
            &[],
            damage_tracker,
            BACKGROUND,
        )
        .map_err(|err| anyhow!("render error: {err}"))?;
    }

    backend.submit(Some(&[damage]))?;

    // Clients drive their animation from frame callbacks; without this they
    // would render exactly once and then wait forever.
    for window in state.space.elements() {
        window.send_frame(
            output,
            state.start_time.elapsed(),
            Some(Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    }

    Ok(())
}
