//! `zwlr_layer_shell_v1`: panels and bars such as waybar.
//!
//! The per-output bookkeeping is smithay's `LayerMap`; this module wires it into
//! the commit path and popups.

use smithay::{
    delegate_layer_shell,
    desktop::{layer_map_for_output, LayerSurface, PopupKind, WindowSurfaceType},
    output::Output,
    reexports::wayland_server::protocol::{wl_output, wl_surface::WlSurface},
    wayland::{
        compositor,
        shell::{
            wlr_layer::{
                Layer, LayerSurface as LayerShellSurface, LayerSurfaceConfigure, LayerSurfaceData,
                WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::PopupSurface,
        },
    },
};

use crate::state::Mriya;

impl WlrLayerShellHandler for Mriya {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerShellSurface,
        output: Option<wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else {
            tracing::warn!(%namespace, "layer surface without an output, ignoring");
            return;
        };

        let layer = LayerSurface::new(surface, namespace.clone());
        if let Err(err) = layer_map_for_output(&output).map_layer(&layer) {
            tracing::warn!(%namespace, %err, "cannot map the layer surface");
            return;
        }
        // The layer itself is only known after the client's first commit.
        tracing::debug!(%namespace, "layer surface mapped");

        // The size we configure depends on the client's anchors and requested
        // size, so the arrange happens on its first commit, in
        // `handlers::compositor::commit`.
        self.request_redraw();
    }

    fn new_popup(&mut self, _parent: LayerShellSurface, popup: PopupSurface) {
        // Tooltips and menus of the bar: they hang off a layer surface, not off
        // a window.
        self.unconstrain_popup(&popup);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            tracing::warn!(%err, "failed to track a layer shell popup");
            return;
        }
        self.request_redraw();
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: LayerShellSurface) {
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let Some(layer) = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned()
            else {
                continue;
            };
            map.unmap_layer(&layer);
            tracing::debug!(namespace = %layer.namespace(), "layer surface unmapped");
            drop(map);
            self.request_redraw();
            return;
        }
    }
}

delegate_layer_shell!(Mriya);

impl Mriya {
    /// Configure a layer surface whose client just committed.
    ///
    /// Called from the commit path: anchors and the requested size are only
    /// known then, and the client needs a configure before it can attach a
    /// buffer.
    pub fn configure_layer_surface(&mut self, surface: &WlSurface) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        {
            let map = layer_map_for_output(&output);
            if map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .is_none()
            {
                return;
            }
        }

        let initial_sent = compositor::with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|data| data.lock().unwrap().initial_configure_sent)
                .unwrap_or(true)
        });

        // `arrange` derives the geometry from the anchors and writes the size we
        // want into the layer's pending state.
        layer_map_for_output(&output).arrange();

        // Take the handle out of the map before touching the protocol: holding
        // the layer map guard while sending events deadlocks the event loop.
        let layer = {
            let map = layer_map_for_output(&output);
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned()
        };
        if let Some(layer) = layer {
            if !initial_sent {
                layer.layer_surface().send_configure();
                tracing::debug!(
                    namespace = %layer.namespace(),
                    layer = ?layer.layer(),
                    "panel configured"
                );
            }
        }

        self.apply_layer_scale(&output);
        self.request_redraw();
    }

    /// Tell the whole surface tree of every layer surface the output scale.
    pub fn apply_layer_scale(&self, output: &Output) {
        let layers: Vec<_> = {
            let map = layer_map_for_output(output);
            map.layers().cloned().collect()
        };
        for layer in layers {
            // Collect first: `with_surfaces` holds the surface state lock, and
            // `apply_surface_scale` locks it again (same mutex, no reentrancy).
            let surfaces: Vec<WlSurface> = {
                let mut surfaces = Vec::new();
                layer.with_surfaces(|surface, _| surfaces.push(surface.clone()));
                surfaces
            };
            for surface in surfaces {
                self.apply_surface_scale(&surface);
            }
        }
    }
}
