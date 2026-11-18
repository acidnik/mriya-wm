//! Nested backend: a window inside the parent Wayland session.
//!
//! The graphics backend lives in [`Mriya::renderer`] so that any part of the
//! compositor (a client commit, an input event, a keybinding) can ask for a new
//! frame. The winit event source only translates events into state calls.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker, element::AsRenderElements, gles::GlesRenderer, ImportMemWl,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    input::pointer::CursorImageStatus,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        winit::{
            dpi::LogicalSize, platform::wayland::WindowAttributesExtWayland,
            window::Window as WinitWindow,
        },
    },
    utils::{Physical, Scale, Size, Transform},
};
use tracing::info;

use crate::{backend::element::NestedRenderElement, cursor::Cursor, decor, Mriya};

/// Colour of the "desktop" behind all windows.
const BACKGROUND: [f32; 4] = [0.11, 0.11, 0.13, 1.0];
/// Refresh rate advertised for the nested output.
const REFRESH_MHZ: i32 = 60_000;
/// Size of the nested window when the parent compositor lets us choose.
const DEFAULT_SIZE: (u32, u32) = (1280, 800);
/// Frames rendered without damage tracking after a resize.
const FULL_REDRAW_FRAMES: u8 = 2;

pub fn run(mut event_loop: EventLoop<Mriya>, mut state: Mriya) -> Result<()> {
    let attributes = WinitWindow::default_attributes()
        .with_title("Mriya WM")
        .with_name("mriya-wm", "mriya-wm")
        .with_inner_size(LogicalSize::new(
            f64::from(DEFAULT_SIZE.0),
            f64::from(DEFAULT_SIZE.1),
        ))
        .with_visible(true);

    let (graphics, winit) = winit::init_from_attributes::<GlesRenderer>(attributes)
        .map_err(|err| anyhow!("failed to initialize the nested winit backend: {err}"))?;

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
        Some(Mode {
            size: graphics.window_size(),
            refresh: REFRESH_MHZ,
        }),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );

    let mut graphics = graphics;
    state
        .shm_state
        .update_formats(graphics.renderer().shm_formats());
    state.space.map_output(&output, (0, 0));

    let renderer = NestedRenderer {
        damage_tracker: OutputDamageTracker::from_output(&output),
        graphics,
        output,
        full_redraw: FULL_REDRAW_FRAMES,
        cursor: Cursor::new(&state.config.cursor),
        host_cursor_visible: None,
    };
    info!(theme = renderer.cursor.theme_name(), "cursor theme loaded");
    state.renderer = Some(renderer);
    state.request_redraw();

    event_loop
        .handle()
        .insert_source(winit, |event, _, state| match event {
            WinitEvent::Resized { size, .. } => state.resize_output(size),
            WinitEvent::Input(event) => {
                state.process_input_event(event);
                state.request_redraw();
            }
            WinitEvent::Redraw => state.render_frame(),
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
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
        .run(None, &mut state, |state| {
            state.flush_clients();
            state.spawner.reap();
        })
        .context("event loop failed")
}

/// Rendering state of the nested backend.
pub struct NestedRenderer {
    graphics: WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: OutputDamageTracker,
    output: Output,
    /// When non-zero, the next frames are rendered without damage tracking.
    full_redraw: u8,
    cursor: Cursor,
    /// Last cursor visibility we asked the host compositor for.
    host_cursor_visible: Option<bool>,
}

impl NestedRenderer {
    /// Ask the host compositor for another frame.
    pub fn request_redraw(&self) {
        self.graphics.window().request_redraw();
    }

    pub(crate) fn resize(&mut self, size: Size<i32, Physical>) {
        self.output.change_current_state(
            Some(Mode {
                size,
                refresh: REFRESH_MHZ,
            }),
            None,
            None,
            None,
        );
        // Buffer contents are undefined after a resize.
        self.full_redraw = FULL_REDRAW_FRAMES;
    }

    /// Draw one frame into the nested window.
    pub(crate) fn draw(&mut self, state: &mut Mriya) -> Result<()> {
        let scale = self.output.current_scale().fractional_scale();
        let cursor_position = state.pointer_location;
        let cursor_status = state.cursor_status.clone();
        let age = if self.full_redraw > 0 {
            0
        } else {
            self.graphics.buffer_age().unwrap_or(0)
        };

        let damage = {
            let (renderer, mut framebuffer) = self.graphics.bind()?;
            let output_scale = Scale::from(scale);

            let active_window = state.focused_window();
            let text = &mut state.text;
            let mut elements: Vec<NestedRenderElement<GlesRenderer>> =
                Vec::with_capacity(state.space.elements().len() * 4 + 1);

            // Collected bottom to top (frame below its client content, windows
            // in stacking order); the damage tracker wants front to back, so the
            // list is reversed at the end.
            for window in state.space.elements_for_output(&self.output) {
                let Some(content_location) = state.space.element_location(window) else {
                    continue;
                };

                elements.extend(decor::render::elements(
                    renderer,
                    state.theme.as_ref(),
                    text,
                    window,
                    content_location,
                    window.geometry().size,
                    active_window.as_ref() == Some(window),
                    output_scale,
                ));
                elements.extend(AsRenderElements::<GlesRenderer>::render_elements::<
                    NestedRenderElement<GlesRenderer>,
                >(
                    window,
                    renderer,
                    content_location.to_physical_precise_round(output_scale),
                    output_scale,
                    1.0,
                ));
            }

            elements.extend(self.cursor.render_elements(
                renderer,
                cursor_position,
                scale,
                &cursor_status,
            ));

            // OutputDamageTracker draws the list back to front: the last
            // element of the slice ends up at the very bottom.
            elements.reverse();

            let result = self
                .damage_tracker
                .render_output(renderer, &mut framebuffer, age, &elements, BACKGROUND)
                .map_err(|err| anyhow!("render error: {err}"))?;

            result.damage.cloned()
        };

        self.full_redraw = self.full_redraw.saturating_sub(1);

        // One bind/render/submit per frame keeps the damage tracker and the
        // swapchain's buffer ages in sync: skipping a swap desynchronises them
        // and makes the tracker believe stale pixels are already on screen.
        let rects = damage.as_ref().map(|damage| damage.len()).unwrap_or(0);
        tracing::trace!(age, rects, "frame drawn");
        self.graphics.submit(damage.as_deref())?;

        let host_visible = !Self::hides_host_cursor(&self.cursor, &cursor_status);
        if self.host_cursor_visible != Some(host_visible) {
            self.graphics.window().set_cursor_visible(host_visible);
            self.host_cursor_visible = Some(host_visible);
        }

        // Clients drive their animation from frame callbacks; without this they
        // would render exactly once and then wait forever.
        for window in state.space.elements() {
            window.send_frame(
                &self.output,
                state.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(self.output.clone()),
            );
        }

        Ok(())
    }

    /// Whether our own cursor replaces the one the host session draws.
    fn hides_host_cursor(cursor: &Cursor, status: &CursorImageStatus) -> bool {
        match status {
            CursorImageStatus::Hidden => true,
            // Without a cursor theme we have no arrow to draw, so the host
            // cursor stays as the fallback.
            CursorImageStatus::Named(_) => cursor.has_theme(),
            CursorImageStatus::Surface(_) => true,
        }
    }
}
