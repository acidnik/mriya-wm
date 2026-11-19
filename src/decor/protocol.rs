//! `zxdg_decoration_manager_v1`: who draws the window frame.
//!
//! Policy: Mriya draws a frame for every toplevel (that is the point of this
//! window manager), and honours an explicit client-side request. Clients that do
//! not implement the protocol at all silently keep whatever they draw
//! themselves.

use std::cell::RefCell;

use smithay::{
    delegate_xdg_decoration,
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    wayland::{
        compositor,
        shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface},
    },
};

use crate::{config::DecorationPolicy, decor, state::Mriya};

/// Decoration mode the client asked for, kept on its surface.
///
/// The client usually asks right after `get_toplevel`, before the window is
/// mapped and therefore before it exists in the space. Storing the request on
/// the surface keeps it until the window shows up: dropping it made the
/// automatic detection (a guess) win over an explicit request, and a terminal
/// that had asked for server-side decorations ended up drawing its own.
/// `Some(mode)` = asked for it, `Some(None)` = asked for "no preference",
/// `None` = never spoke.
type RequestedMode = Option<Option<Mode>>;

fn remember_requested(surface: &WlSurface, requested: Option<Mode>) {
    compositor::with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing(RefCell::<RequestedMode>::default);
        if let Some(slot) = states.data_map.get::<RefCell<RequestedMode>>() {
            slot.replace(Some(requested));
        }
    });
}

/// Last request of the client, if it ever made one. The outer option tells
/// whether the client spoke at all, the inner one what it asked for.
pub fn requested_decoration(surface: &WlSurface) -> Option<Option<Mode>> {
    compositor::with_states(surface, |states| {
        states
            .data_map
            .get::<RefCell<RequestedMode>>()
            .and_then(|slot| *slot.borrow())
    })
}

impl XdgDecorationHandler for Mriya {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // The client speaks the protocol but has no preference yet: propose
        // server-side, which is what Mriya does by default.
        self.set_requested_decoration(&toplevel, None, Mode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        remember_requested(toplevel.wl_surface(), Some(mode));
        self.set_requested_decoration(&toplevel, Some(mode), mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        remember_requested(toplevel.wl_surface(), None);
        // "No preference": fall back to detection.
        self.set_requested_decoration(&toplevel, None, Mode::ServerSide);
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

    /// Record the client's decoration preference and tell it the effective mode.
    fn set_requested_decoration(
        &mut self,
        toplevel: &ToplevelSurface,
        requested: Option<Mode>,
        propose: Mode,
    ) {
        let Some(window) = self.window_for_toplevel(toplevel) else {
            // The window is not mapped yet: just advertise a mode, the effective
            // one is computed once the window exists.
            toplevel.with_pending_state(|state| {
                state.decoration_mode = Some(propose);
            });
            toplevel.send_pending_configure();
            return;
        };

        let changed = {
            let mut state = decor::state_mut(&window);
            state.requested_mode = requested;
            state.refresh_mode()
        };

        if changed {
            self.sync_decoration_mode(&window);
        }
    }
}
