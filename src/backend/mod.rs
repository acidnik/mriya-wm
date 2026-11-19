//! Compositor backends.
//!
//! * [`nested`] runs Mriya as a window inside another Wayland session (winit).
//! * [`udev`] takes over the GPU on a TTY through libseat/logind, which is what a
//!   display manager starts.
//!
//! Both draw the same scene: [`scene_elements`] collects it once.

pub mod element;
pub mod nested;
pub mod udev;

use smithay::{
    backend::renderer::gles::GlesRenderer, input::pointer::CursorImageStatus, output::Output,
    utils::Scale, wayland::shell::wlr_layer::Layer,
};

use crate::{cursor::Cursor, decor, layers, state::Mriya};

pub use element::NestedRenderElement;

/// Which backend is driving the compositor.
// Both variants are big; they are created once and never moved in a hot path.
#[allow(clippy::large_enum_variant)]
pub enum Backend {
    Nested(nested::NestedRenderer),
    Udev(udev::UdevBackend),
}

impl Backend {
    pub fn draw(&mut self, state: &mut Mriya) -> anyhow::Result<()> {
        match self {
            Backend::Nested(renderer) => renderer.draw(state),
            Backend::Udev(backend) => backend.draw(state),
        }
    }

    pub fn reload_cursor(&mut self, config: &crate::config::CursorConfig) {
        match self {
            Backend::Nested(renderer) => renderer.reload_cursor(config),
            Backend::Udev(backend) => backend.reload_cursor(config),
        }
    }

    /// The nested window was resized; the DRM backend knows its outputs.
    pub fn resize(&mut self, size: smithay::utils::Size<i32, smithay::utils::Physical>) {
        if let Backend::Nested(renderer) = self {
            renderer.resize(size);
        }
    }
}

/// Everything the scene contains, bottom to top.
///
/// Windows with their frames, panels below and above them, and the cursor last.
/// Callers that draw back to front (the damage tracker) reverse the list.
pub fn scene_elements(
    renderer: &mut GlesRenderer,
    state: &mut Mriya,
    output: &Output,
    scale: f64,
    cursor: &mut Cursor,
    cursor_status: &CursorImageStatus,
) -> Vec<NestedRenderElement<GlesRenderer>> {
    let output_scale = Scale::from(scale);
    let cursor_position = state.pointer_location;
    let active_window = state.focused_window();

    let (below_layers, above_layers): (Vec<_>, Vec<_>) = layers::surfaces(&state.space)
        .into_iter()
        .partition(|(layer, _)| !matches!(layer.layer(), Layer::Top | Layer::Overlay));

    let space = &state.space;
    let theme = state.theme.as_ref();
    let text = &mut state.text;
    let icons = &mut state.icons;

    let mut elements: Vec<NestedRenderElement<GlesRenderer>> =
        Vec::with_capacity(space.elements().len() * 4 + 8);

    for (layer, location) in &below_layers {
        elements.extend(panel_elements(renderer, layer, *location, output_scale));
    }

    for window in space.elements_for_output(output) {
        let Some(content_location) = space.element_location(window) else {
            continue;
        };
        let frame = decor::render::elements(
            renderer,
            theme,
            text,
            icons,
            window,
            content_location,
            window.geometry().size,
            active_window.as_ref() == Some(window),
            output_scale,
        );
        // Shadow (and the flat fallback background) below the client surface.
        elements.extend(frame.below);
        let content =
            AsRenderElements::<GlesRenderer>::render_elements::<NestedRenderElement<GlesRenderer>>(
                window,
                renderer,
                content_location.to_physical_precise_round(output_scale),
                output_scale,
                1.0,
            );
        if trace_elements() {
            use smithay::backend::renderer::element::Element;
            let rects: Vec<_> = content
                .iter()
                .map(|element| element.geometry(output_scale))
                .collect();
            tracing::info!(
                app_id = %decor::state(window).app_id,
                count = content.len(),
                ?rects,
                "window content elements"
            );
        }
        elements.extend(content);
        // Titlebar, buttons, borders above it: a client surface can be larger
        // than its reported geometry.
        elements.extend(frame.above);
    }

    for (layer, location) in &above_layers {
        elements.extend(panel_elements(renderer, layer, *location, output_scale));
    }

    elements.extend(cursor.render_elements(renderer, cursor_position, scale, cursor_status));
    elements
}

fn panel_elements(
    renderer: &mut GlesRenderer,
    layer: &smithay::desktop::LayerSurface,
    location: smithay::utils::Point<i32, smithay::utils::Logical>,
    output_scale: Scale<f64>,
) -> Vec<NestedRenderElement<GlesRenderer>> {
    AsRenderElements::<GlesRenderer>::render_elements::<NestedRenderElement<GlesRenderer>>(
        layer,
        renderer,
        location.to_physical_precise_round(output_scale),
        output_scale,
        1.0,
    )
}

use smithay::backend::renderer::element::AsRenderElements;

/// Debug switch: `MRIYA_TRACE_ELEMENTS=1` logs what each window contributes to
/// the frame, which is how a window that draws nothing is told apart from one
/// whose client never sent a buffer.
fn trace_elements() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MRIYA_TRACE_ELEMENTS").is_some())
}
