//! `zxdg_decoration_manager_v1`: who draws the window frame.
//!
//! Policy: Mriya draws a frame for every toplevel (that is the point of this
//! window manager), and honours an explicit client-side request. Clients that do
//! not implement the protocol at all silently keep whatever they draw
//! themselves.

use smithay::{
    delegate_xdg_decoration,
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    wayland::shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface},
};

use crate::{decor, state::Mriya};

impl XdgDecorationHandler for Mriya {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.apply_decoration_mode(&toplevel, Mode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        self.apply_decoration_mode(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.apply_decoration_mode(&toplevel, Mode::ServerSide);
    }
}

delegate_xdg_decoration!(Mriya);

impl Mriya {
    /// Tell the client which decoration mode applies and record it for drawing.
    pub fn apply_decoration_mode(&mut self, toplevel: &ToplevelSurface, mode: Mode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        toplevel.send_pending_configure();

        if let Some(window) = self.window_for_toplevel(toplevel) {
            decor::state_mut(&window).mode = mode;
        }
        self.request_redraw();
    }
}
