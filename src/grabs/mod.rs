//! Interactive window move/resize.

mod move_grab;
pub mod resize_grab;

pub use move_grab::MoveSurfaceGrab;
pub use resize_grab::{ResizeEdge, ResizeSurfaceGrab};

use smithay::{
    input::pointer::GrabStartData as PointerGrabStartData,
    utils::{Logical, Point},
};

use crate::state::Mriya;

/// Grab start data for a drag started by the window manager itself
/// (e.g. `Mod` + left button), as opposed to a client-initiated one.
pub fn start_data_for(state: &Mriya, button: u32) -> PointerGrabStartData<Mriya> {
    let location: Point<f64, Logical> = state.pointer_location;
    start_data_at(location, button)
}

/// Grab start data for a drag started at `location` by the window manager.
///
/// Used when a drag starts on a window frame: there is no client surface under
/// the pointer there, so the grab has no focus surface.
pub fn start_data_at(location: Point<f64, Logical>, button: u32) -> PointerGrabStartData<Mriya> {
    PointerGrabStartData {
        button,
        location,
        focus: None,
    }
}
