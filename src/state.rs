//! Compositor state and protocol initialization.

use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{keyboard::Keysym, pointer::CursorImageStatus, Seat, SeatState},
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction},
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle, Resource,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

pub struct Mriya {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    /// Stacking order and position of all mapped windows.
    pub space: Space<Window>,
    pub popups: PopupManager,

    /// Last known pointer position, in output coordinates.
    pub pointer_location: Point<f64, Logical>,
    /// Cursor image requested by the focused client.
    pub cursor_status: CursorImageStatus,
    /// Keysym of shortcuts we intercepted on press, so the matching release is
    /// not forwarded to the client.
    pub suppressed_keys: Vec<Keysym>,
    /// Set when the scene changed and a new frame has to be drawn.
    pub needs_redraw: bool,
    /// Backend that draws the scene; set once the backend is initialized.
    pub renderer: Option<super::backend::nested::NestedRenderer>,

    // Smithay protocol state.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    /// Kept alive so the `zxdg_output_manager_v1` global stays advertised.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Mriya>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,
}

impl Mriya {
    pub fn new(event_loop: &mut EventLoop<Self>, display: Display<Self>) -> Self {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let mut seat_state = SeatState::new();

        let mut seat = seat_state.new_wl_seat(&display_handle, "mriya");

        // TODO: keyboard layout/options have to come from the config (kdl).
        seat.add_keyboard(Default::default(), 200, 25)
            .expect("failed to add keyboard to seat");
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();

        Self {
            start_time: Instant::now(),
            socket_name,
            display_handle,
            loop_signal,
            space: Space::default(),
            popups: PopupManager::default(),
            pointer_location: (0.0, 0.0).into(),
            cursor_status: CursorImageStatus::default_named(),
            suppressed_keys: Vec::new(),
            needs_redraw: true,
            renderer: None,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
        }
    }

    fn init_wayland_listener(display: Display<Self>, event_loop: &mut EventLoop<Self>) -> OsString {
        let listening_socket =
            ListeningSocketSource::new_auto().expect("failed to create wayland socket");
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                if let Err(err) = state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                {
                    tracing::warn!(%err, "failed to insert client");
                }
            })
            .expect("failed to insert wayland listening socket");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: the display is never dropped while the loop runs.
                    let display = unsafe { display.get_mut() };
                    if let Err(err) = display.dispatch_clients(state) {
                        tracing::error!(%err, "failed to dispatch clients");
                        return Ok(PostAction::Continue);
                    }
                    state.on_dispatch();
                    Ok(PostAction::Continue)
                },
            )
            .expect("failed to insert wayland display");

        socket_name
    }

    /// Housekeeping after a batch of client requests was processed.
    fn on_dispatch(&mut self) {
        // Unmaps windows whose surface died and drops stale popups.
        self.space.refresh();
        self.popups.cleanup();
    }

    /// Send everything we queued for clients.
    ///
    /// Composable events are generated from many places (input, rendering,
    /// focus changes), not only from the Wayland dispatch source, so the flush
    /// happens once per event loop iteration instead of next to the dispatch.
    pub fn flush_clients(&mut self) {
        if let Err(err) = self.display_handle.flush_clients() {
            tracing::warn!(%err, "failed to flush clients");
        }
    }

    /// Mark the scene as dirty and ask the backend for a frame.
    ///
    /// Client commits are processed in the Wayland event source, so this is the
    /// only way a `commit` turns into a painted frame.
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
        if let Some(renderer) = &self.renderer {
            renderer.request_redraw();
        }
    }

    /// Draw one frame. Called by the backend when it wants a new picture.
    pub fn render_frame(&mut self) {
        let Some(mut renderer) = self.renderer.take() else {
            return;
        };
        self.needs_redraw = false;
        if let Err(err) = renderer.draw(self) {
            tracing::warn!(%err, "failed to render a frame");
        }
        self.renderer = Some(renderer);
    }

    /// Handle a change of the output size reported by the backend.
    pub fn resize_output(&mut self, size: smithay::utils::Size<i32, smithay::utils::Physical>) {
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(size);
        }
        self.request_redraw();
    }

    /// Surface (and its location on screen) below the given position.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, point)| (surface, (point + location).to_f64()))
            })
    }

    /// Window below the given position, together with its location.
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Point<i32, Logical>)> {
        self.space
            .element_under(pos)
            .map(|(window, location)| (window.clone(), location))
    }

    /// Raise `window` and give it keyboard focus.
    pub fn focus_window(&mut self, window: &Window) {
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();

        self.space.raise_element(window, true);
        for other in self.space.elements() {
            let focused = other == window;
            let Some(other_toplevel) = other.toplevel() else {
                continue;
            };
            other_toplevel.with_pending_state(|state| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
                if focused {
                    state.states.set(xdg_toplevel::State::Activated);
                } else {
                    state.states.unset(xdg_toplevel::State::Activated);
                }
            });
            other_toplevel.send_pending_configure();
        }

        tracing::trace!(surface = ?toplevel.wl_surface().id(), "setting keyboard focus");
        self.seat.get_keyboard().unwrap().set_focus(
            self,
            Some(toplevel.wl_surface().clone()),
            serial,
        );
        self.request_redraw();
    }

    /// Window that currently holds keyboard focus, if any.
    pub fn focused_window(&self) -> Option<Window> {
        let surface = self.seat.get_keyboard()?.current_focus()?;
        self.space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel.wl_surface() == &surface)
                    .unwrap_or(false)
            })
            .cloned()
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
