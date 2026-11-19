//! Layer surfaces (`zwlr_layer_shell_v1`): panels, bars and wallpapers.
//!
//! waybar is the reason this exists. A bar is a layer surface with an
//! `exclusive_zone`, i.e. it asks the compositor to keep windows out of its
//! band; that band is [`usable_area`] and window placement works inside it.

use smithay::{
    desktop::{layer_map_for_output, LayerSurface, Space, Window, WindowSurfaceType},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, Size},
    wayland::shell::wlr_layer::Layer,
};

/// Layer surfaces of the output, bottom (background) to top (overlay).
pub fn surfaces(space: &Space<Window>) -> Vec<(LayerSurface, Point<i32, Logical>)> {
    let Some(output) = space.outputs().next() else {
        return Vec::new();
    };
    let map = layer_map_for_output(output);
    map.layers()
        .filter_map(|layer| {
            let geometry = map.layer_geometry(layer)?;
            Some((layer.clone(), geometry.loc))
        })
        .collect()
}

/// Recompute layer geometry after a surface changed or the output resized.
///
/// Returns whether anything moved.
pub fn arrange(space: &Space<Window>) -> bool {
    let mut changed = false;
    for output in space.outputs() {
        changed |= layer_map_for_output(output).arrange();
    }
    changed
}

/// Output rectangle without the bands reserved by layer surfaces.
pub fn usable_area(space: &Space<Window>) -> Option<Rectangle<i32, Logical>> {
    let output = space.outputs().next()?;
    let output_geo = space.output_geometry(output)?;
    let map = layer_map_for_output(output);
    let zone = map.non_exclusive_zone();
    Some(Rectangle::new(
        output_geo.loc + zone.loc,
        Size::from((
            zone.size.w.max(1).min(output_geo.size.w),
            zone.size.h.max(1).min(output_geo.size.h),
        )),
    ))
}

/// Layer surface under `pos` (space coordinates), looking at `layers` in order.
pub fn surface_under(
    space: &Space<Window>,
    pos: Point<f64, Logical>,
    layers: &[Layer],
) -> Option<(WlSurface, Point<f64, Logical>, LayerSurface)> {
    let output = space.outputs().next()?;
    let output_geo = space.output_geometry(output)?;
    let map = layer_map_for_output(output);
    let local = pos - output_geo.loc.to_f64();

    for layer in layers {
        let Some(found) = map.layer_under(*layer, local) else {
            continue;
        };
        let Some(geometry) = map.layer_geometry(found) else {
            continue;
        };
        if let Some((surface, point)) =
            found.surface_under(local - geometry.loc.to_f64(), WindowSurfaceType::ALL)
        {
            // `point` is where the surface sits inside the layer tree, so the
            // origin in space is the layer's own position plus that offset.
            let origin = output_geo.loc.to_f64() + geometry.loc.to_f64() + point.to_f64();
            return Some((surface, origin, found.clone()));
        }
    }
    None
}

/// Is this surface one of the layer surfaces on the output?
pub fn is_layer_surface(space: &Space<Window>, surface: &WlSurface) -> bool {
    surfaces(space)
        .iter()
        .any(|(layer, _)| layer.wl_surface() == surface)
}
