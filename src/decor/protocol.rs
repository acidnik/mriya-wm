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

use crate::{config::DecorationPolicy, decor, state::Mriya};

impl XdgDecorationHandler for Mriya {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.apply_decoration_mode(&toplevel, Mode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        // A window rule is authoritative over the client's preference.
        let effective = self
            .window_for_toplevel(&toplevel)
            .and_then(|window| self.rule_decoration_mode(&window))
            .unwrap_or(mode);
        self.apply_decoration_mode(&toplevel, effective);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.apply_decoration_mode(&toplevel, Mode::ServerSide);
    }
}

delegate_xdg_decoration!(Mriya);

impl Mriya {
    /// Decoration mode a window should use: the last matching `window-rule` wins
    /// over what the client asked for.
    pub fn rule_decoration_mode(&self, window: &smithay::desktop::Window) -> Option<Mode> {
        let (app_id, title) = {
            let state = decor::state(window);
            (state.app_id.clone(), state.title.clone())
        };
        self.config
            .window_rules
            .iter()
            .filter(|rule| rule.matches_window(&app_id, &title))
            .filter_map(|rule| rule.decorations)
            .next_back()
            .map(|policy| match policy {
                DecorationPolicy::ServerSide => Mode::ServerSide,
                DecorationPolicy::ClientSide => Mode::ClientSide,
            })
    }

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
