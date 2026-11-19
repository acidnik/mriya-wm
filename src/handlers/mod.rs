//! Protocol implementations for [`Mriya`].

mod compat;
mod compositor;
mod layer_shell;
mod xdg_shell;

use smithay::{
    delegate_data_device, delegate_output, delegate_seat,
    input::{pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    wayland::{
        output::OutputHandler,
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            SelectionHandler, SelectionSource, SelectionTarget,
        },
    },
};

use crate::state::Mriya;

impl SeatHandler for Mriya {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        tracing::trace!(status = ?image, "cursor image changed");
        self.cursor_status = image;
        // The cursor is part of the scene, so the next frame has to redraw the
        // area it left and the area it moved to.
        self.request_redraw();
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let display_handle = &self.display_handle;
        let client = focused.and_then(|surface| display_handle.get_client(surface.id()).ok());
        set_data_device_focus(display_handle, seat, client.clone());
        // Primary selection follows the keyboard focus the same way; without it
        // no client is ever offered a selection and middle-click paste does
        // nothing.
        smithay::wayland::selection::primary_selection::set_primary_focus(
            display_handle,
            seat,
            client,
        );
    }
}

delegate_seat!(Mriya);

impl SelectionHandler for Mriya {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        tracing::debug!(?ty, owner = source.is_some(), "selection changed");
    }
}

impl DataDeviceHandler for Mriya {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Mriya {}
impl ServerDndGrabHandler for Mriya {}

delegate_data_device!(Mriya);

impl OutputHandler for Mriya {}

delegate_output!(Mriya);
