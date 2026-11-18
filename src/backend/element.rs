//! Render element types used by the nested backend.
//!
//! Windows and the cursor end up in the same frame, so they have to share one
//! element type. `render_elements!` generates the enum and the `RenderElement`
//! implementation for it.

use smithay::{
    backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, surface::WaylandSurfaceRenderElement},
        ImportAll, ImportMem,
    },
    render_elements,
};

render_elements! {
    /// Everything that can be drawn into the nested window.
    pub NestedRenderElement<R> where R: ImportAll + ImportMem;
    /// Client surfaces: windows, popups, subsurfaces, surface cursors.
    Surface = WaylandSurfaceRenderElement<R>,
    /// Client-side memory buffers, used for cursors from the XCursor theme.
    Memory = MemoryRenderBufferRenderElement<R>,
}
