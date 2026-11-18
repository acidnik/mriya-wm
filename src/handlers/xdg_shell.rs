//! `xdg_wm_base` toplevel/popup handling.

use smithay::{
    delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, PopupKind, PopupManager, Space, Window,
    },
    input::{
        pointer::{Focus, GrabStartData as PointerGrabStartData},
        Seat,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_seat, wl_surface::WlSurface},
            Resource,
        },
    },
    utils::{Rectangle, Serial},
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};

use crate::{
    decor::{self, FrameGeometry},
    grabs::{MoveSurfaceGrab, ResizeSurfaceGrab},
    shell,
    state::Mriya,
};

impl XdgShellHandler for Mriya {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        // The frame is drawn around the content area, so the cascade position
        // applies to the frame and the content moves inside it.
        let content_offset = FrameGeometry::of(&decor::state(&window)).content_offset();
        let location = shell::location_for_new_window(&self.space) + content_offset;
        self.space.map_element(window.clone(), location, false);
        self.focus_window(&window);
        self.request_redraw();
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_toplevel(&surface) {
            decor::state_mut(&window).title = surface_title(&surface);
        }
        self.request_redraw();
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_toplevel(&surface) {
            decor::state_mut(&window).app_id = surface_app_id(&surface);
        }
        self.request_redraw();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!(%err, "failed to track popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        let wl_surface = surface.wl_surface();

        let Some(start_data) = check_grab(&seat, wl_surface, serial) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        let Some((window, initial_window_location)) = self.window_under(self.pointer_location)
        else {
            return;
        };

        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        let wl_surface = surface.wl_surface();

        let Some(start_data) = check_grab(&seat, wl_surface, serial) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        let Some((window, initial_window_location)) = self.window_under(self.pointer_location)
        else {
            return;
        };
        let initial_window_size = window.geometry().size;

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();

        let grab = ResizeSurfaceGrab::start(
            start_data,
            window,
            edges.into(),
            Rectangle::new(initial_window_location, initial_window_size),
        );
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        match self.window_for_toplevel(&surface) {
            Some(window) => shell::set_maximized(&mut self.space, &window, true),
            None => {
                surface.send_configure();
            }
        }
        self.request_redraw();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        match self.window_for_toplevel(&surface) {
            Some(window) => shell::set_maximized(&mut self.space, &window, false),
            None => {
                surface.send_configure();
            }
        }
        self.request_redraw();
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        match self.window_for_toplevel(&surface) {
            Some(window) => shell::set_fullscreen(&mut self.space, &window, true),
            None => {
                surface.send_configure();
            }
        }
        self.request_redraw();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        match self.window_for_toplevel(&surface) {
            Some(window) => shell::set_fullscreen(&mut self.space, &window, false),
            None => {
                surface.send_configure();
            }
        }
        self.request_redraw();
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        // TODO: no iconified state yet, a minimised window just loses focus.
        surface.send_configure();
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO: popup grabs
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_toplevel(&surface) {
            self.space.unmap_elem(&window);
        }
        self.request_redraw();
    }
}

delegate_xdg_shell!(Mriya);

impl Mriya {
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.space.elements().find(|window| {
            window
                .toplevel()
                .map(|toplevel| toplevel.wl_surface() == &root)
                .unwrap_or(false)
        }) else {
            return;
        };
        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(window) else {
            return;
        };

        // The positioner geometry is relative to the parent window, but the
        // unconstrained geometry has to be relative to the output.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

fn check_grab(
    seat: &Seat<Mriya>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<Mriya>> {
    let pointer = seat.get_pointer()?;

    // The click that started the drag has to be one we saw on this surface.
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }

    Some(start_data)
}

fn surface_title(surface: &ToplevelSurface) -> String {
    with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().unwrap().title.clone())
            .unwrap_or_default()
    })
}

fn surface_app_id(surface: &ToplevelSurface) -> String {
    with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().unwrap().app_id.clone())
            .unwrap_or_default()
    })
}

/// Called on every `WlSurface::commit`.
pub fn handle_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    // First configure has to be sent once the toplevel is mapped.
    if let Some(window) = space.elements().find(|window| {
        window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface() == surface)
            .unwrap_or(false)
    }) {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_configure_sent {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_configure();
            }
        }
    }

    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface) {
        match popup {
            PopupKind::Xdg(ref xdg) => {
                if !xdg.is_initial_configure_sent() {
                    if let Err(err) = xdg.send_configure() {
                        tracing::warn!(%err, "failed to send initial popup configure");
                    }
                }
            }
            PopupKind::InputMethod(_) => {}
        }
    }
}
