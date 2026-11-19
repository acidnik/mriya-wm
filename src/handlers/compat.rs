//! Protocols that applications expect from a desktop compositor and that are
//! not about drawing anything themselves: primary selection, activation,
//! cursor shapes, fractional scaling and toplevel icons.
//!
//! They are here to make clients stop degrading themselves. foot, for example,
//! announces every missing one of them on startup and falls back to worse
//! behaviour (client-side cursors, no primary selection, no fractional scale).

use smithay::{
    delegate_cursor_shape, delegate_fractional_scale, delegate_primary_selection,
    delegate_viewporter, delegate_xdg_activation, delegate_xdg_toplevel_icon,
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    wayland::{
        fractional_scale::{with_fractional_scale, FractionalScaleHandler},
        selection::primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
        tablet_manager::TabletSeatHandler,
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
        xdg_toplevel_icon::XdgToplevelIconHandler,
    },
};

use crate::state::Mriya;

impl PrimarySelectionHandler for Mriya {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

delegate_primary_selection!(Mriya);

impl XdgActivationHandler for Mriya {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        _token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // Tokens are single use. Everything is accepted for now: a token only
        // means "the user just did something in this client", and clients that
        // ask for attention (a terminal bell, a finished download) would
        // otherwise never reach the user.
        self.activation_state.remove_token(&token);

        let Some(window) = self.window_for_surface(&surface) else {
            tracing::debug!(surface = ?surface.id(), "activation for an unknown surface");
            return;
        };
        tracing::debug!(app_id = %crate::decor::state(&window).app_id, "activation request");
        self.focus_window(&window);
    }
}

delegate_xdg_activation!(Mriya);

impl FractionalScaleHandler for Mriya {}

delegate_fractional_scale!(Mriya);

// `wp_viewporter`: clients scale a surface to a fractional buffer scale with it,
// and the renderer already applies the destination size.
delegate_viewporter!(Mriya);

// `wp_cursor_shape_v1`: the request ends up in `SeatHandler::cursor_image` like
// a client-side cursor surface would, so the theme cursor is used either way.
// Mriya has no graphics tablets, but the delegate wants the (trivial) trait.
impl TabletSeatHandler for Mriya {}

delegate_cursor_shape!(Mriya);

impl XdgToplevelIconHandler for Mriya {
    fn set_icon(
        &mut self,
        _toplevel: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
        surface: WlSurface,
    ) {
        // The icon is committed with the surface, and the frame is cached per
        // window, so drop the cache once the new icon is in place.
        if let Some(window) = self.window_for_surface(&surface) {
            self.invalidate_window_frame(&window);
            self.request_redraw();
        }
    }
}

delegate_xdg_toplevel_icon!(Mriya);

impl Mriya {
    /// Preferred fractional scale for a new surface.
    ///
    /// Clients only learn the scale once: `wp_fractional_scale_v1` gives the
    /// fractional value, `wl_surface.preferred_buffer_scale` the integer
    /// fallback for clients that do not speak it.
    pub fn apply_surface_scale(&self, surface: &WlSurface) {
        let scale = self.output_scale();
        smithay::wayland::compositor::with_states(surface, |states| {
            with_fractional_scale(states, |state| state.set_preferred_scale(scale));
            smithay::wayland::compositor::send_surface_state(
                surface,
                states,
                scale.round().max(1.0) as i32,
                smithay::utils::Transform::Normal,
            );
        });
    }

    /// Scale of the (single) output; 2.0 on a HiDPI host, 1.0 otherwise.
    pub fn output_scale(&self) -> f64 {
        self.space
            .outputs()
            .next()
            .map(|output| output.current_scale().fractional_scale())
            .unwrap_or(1.0)
    }

    /// Window that owns `surface`, whether it is the toplevel or a popup of it.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<smithay::desktop::Window> {
        let mut root = surface.clone();
        while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
            root = parent;
        }
        self.space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel.wl_surface() == &root)
                    .unwrap_or(false)
            })
            .cloned()
    }
}
