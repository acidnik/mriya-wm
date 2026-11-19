//! Drawing the frame around a window.
//!
//! Two paths: the themed one composes the theme images (titlebar tiles, sides,
//! corners, buttons) and caches the result per window; the flat one is used when
//! no theme could be loaded so the compositor stays usable.

use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            Kind,
        },
        gles::GlesRenderer,
    },
    desktop::Window,
    utils::{Logical, Point, Rectangle, Scale, Size},
};

use crate::{
    backend::element::NestedRenderElement,
    decor::{
        button_rects, frame, icons::IconCache, state, state_mut, text::TextRenderer, theme::Theme,
        FrameButton, FrameGeometry, FrameState,
    },
};

/// Colours of the fallback frame drawn without a theme.
const BORDER_COLOR: [f32; 4] = [0.13, 0.13, 0.15, 1.0];
const TITLEBAR_ACTIVE: [f32; 4] = [0.29, 0.30, 0.34, 1.0];
const TITLEBAR_INACTIVE: [f32; 4] = [0.20, 0.20, 0.22, 1.0];
const MAXIMIZE_ACTIVE: [f32; 4] = [0.35, 0.46, 0.63, 1.0];
const MAXIMIZE_INACTIVE: [f32; 4] = [0.26, 0.32, 0.42, 1.0];
const MAXIMIZE_HOVER: [f32; 4] = [0.45, 0.59, 0.80, 1.0];
const CLOSE_ACTIVE: [f32; 4] = [0.66, 0.26, 0.24, 1.0];
const CLOSE_INACTIVE: [f32; 4] = [0.42, 0.22, 0.21, 1.0];
const CLOSE_HOVER: [f32; 4] = [0.85, 0.33, 0.29, 1.0];
const BUTTON_PRESSED: [f32; 4] = [0.18, 0.18, 0.20, 1.0];

/// Solid colour buffers of the fallback frame.
#[derive(Debug, Default)]
struct FlatBuffers {
    background: SolidColorBuffer,
    titlebar: SolidColorBuffer,
    maximize: SolidColorBuffer,
    close: SolidColorBuffer,
}

impl FlatBuffers {
    fn update(
        &mut self,
        geometry: FrameGeometry,
        outer: Rectangle<i32, Logical>,
        buttons: [(FrameButton, Rectangle<i32, Logical>); 2],
        state: &FrameState,
    ) {
        let titlebar = Rectangle::new(outer.loc, Size::from((outer.size.w, geometry.top)));
        let titlebar_color = if state.active {
            TITLEBAR_ACTIVE
        } else {
            TITLEBAR_INACTIVE
        };

        self.background.update(outer.size, BORDER_COLOR);
        self.titlebar.update(titlebar.size, titlebar_color);

        for (button, rect) in buttons {
            let pressed = state.pressed == Some(button);
            let hovered = state.hover == Some(button);
            let color = match button {
                FrameButton::Close => {
                    if pressed {
                        BUTTON_PRESSED
                    } else if hovered {
                        CLOSE_HOVER
                    } else if state.active {
                        CLOSE_ACTIVE
                    } else {
                        CLOSE_INACTIVE
                    }
                }
                FrameButton::Maximize | FrameButton::Hide => {
                    if pressed {
                        BUTTON_PRESSED
                    } else if hovered {
                        MAXIMIZE_HOVER
                    } else if state.active {
                        MAXIMIZE_ACTIVE
                    } else {
                        MAXIMIZE_INACTIVE
                    }
                }
            };

            let buffer = match button {
                FrameButton::Close => &mut self.close,
                FrameButton::Maximize | FrameButton::Hide => &mut self.maximize,
            };
            let size = Size::from((rect.size.w, (rect.size.h - 2).max(1)));
            buffer.update(size, color);
        }
    }
}

/// Frame elements for a window whose content area is at `content_location`.
///
/// The elements are emitted before the client's own elements, so the frame is
/// drawn below its client surface but above windows further down the stack.
#[allow(clippy::too_many_arguments)]
pub fn elements(
    renderer: &mut GlesRenderer,
    theme: Option<&Theme>,
    text: &mut TextRenderer,
    icons: &mut IconCache,
    window: &Window,
    content_location: Point<i32, Logical>,
    content_size: Size<i32, Logical>,
    active: bool,
    scale: Scale<f64>,
) -> Vec<NestedRenderElement<GlesRenderer>> {
    if state(window).active != active {
        state_mut(window).active = active;
    }

    let frame_state = state(window);
    if !frame_state.decorated() {
        return Vec::new();
    }

    let geometry = FrameGeometry::of(theme, &frame_state);
    if geometry.is_empty() {
        return Vec::new();
    }

    match theme {
        Some(theme) => themed_elements(
            renderer,
            theme,
            text,
            icons,
            window,
            content_location,
            content_size,
            &frame_state,
            geometry,
            scale,
        ),
        None => flat_elements(
            window,
            content_location,
            content_size,
            &frame_state,
            geometry,
            scale,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn themed_elements(
    renderer: &mut GlesRenderer,
    theme: &Theme,
    text: &mut TextRenderer,
    icons: &mut IconCache,
    window: &Window,
    content_location: Point<i32, Logical>,
    content_size: Size<i32, Logical>,
    frame_state: &FrameState,
    geometry: FrameGeometry,
    scale: Scale<f64>,
) -> Vec<NestedRenderElement<GlesRenderer>> {
    let cache = frame::textures(
        window,
        theme,
        text,
        icons,
        frame_state,
        content_size,
        geometry,
    );
    let textures = &cache.textures;

    let outer = geometry.outer_rect(Rectangle::new(content_location, content_size));
    let mut elements = Vec::with_capacity(6 + textures.corners.len());

    // Shadow first: it goes under the frame.
    if let Some(shadow) = &textures.shadow {
        let location = Point::<i32, Logical>::from((
            outer.loc.x + shadow.offset.0,
            outer.loc.y + shadow.offset.1,
        ));
        if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            {
                let physical: Point<i32, smithay::utils::Physical> =
                    location.to_physical_precise_round(scale);
                physical.to_f64()
            },
            &shadow.buffer,
            None,
            // `src` is in the texture's own pixels and has to cover it fully;
            // giving only `size` would make the element sample outside of it.
            Some(Rectangle::from_size(Size::from((
                f64::from(shadow.source_size.0),
                f64::from(shadow.source_size.1),
            )))),
            Some(Size::from(shadow.draw_size)),
            Kind::Unspecified,
        ) {
            elements.push(NestedRenderElement::Memory(element));
        }
    }

    let mut push = |buffer: &smithay::backend::renderer::element::memory::MemoryRenderBuffer,
                    location: Point<i32, Logical>| {
        if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            {
                let physical: Point<i32, smithay::utils::Physical> =
                    location.to_physical_precise_round(scale);
                physical.to_f64()
            },
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            elements.push(NestedRenderElement::Memory(element));
        }
    };

    if std::env::var("MRIYA_TRACE_ELEMENTS").is_ok() {
        tracing::debug!(
            app_id = %state(window).app_id,
            active = frame_state.active,
            ?outer,
            titlebar = ?(textures.titlebar_mask.width, textures.titlebar_mask.height),
            corners = textures.corners.len(),
            shadow = textures.shadow.is_some(),
            "frame elements"
        );
    }
    push(&textures.titlebar, outer.loc);
    for (x, y, buffer) in &textures.overlays {
        push(buffer, Point::from((outer.loc.x + x, outer.loc.y + y)));
    }

    let side_height = (outer.size.h - geometry.top - geometry.bottom).max(0);
    let _ = side_height;
    push(
        &textures.left,
        Point::from((outer.loc.x, outer.loc.y + geometry.top)),
    );
    push(
        &textures.right,
        Point::from((
            outer.loc.x + outer.size.w - geometry.right,
            outer.loc.y + geometry.top,
        )),
    );
    push(
        &textures.bottom,
        Point::from((outer.loc.x, outer.loc.y + outer.size.h - geometry.bottom)),
    );

    for corner in &textures.corners {
        push(
            &corner.buffer,
            Point::from((outer.loc.x + corner.x, outer.loc.y + corner.y)),
        );
    }

    elements
}

fn flat_elements(
    window: &Window,
    content_location: Point<i32, Logical>,
    content_size: Size<i32, Logical>,
    frame_state: &FrameState,
    geometry: FrameGeometry,
    scale: Scale<f64>,
) -> Vec<NestedRenderElement<GlesRenderer>> {
    let outer = geometry.outer_rect(Rectangle::new(content_location, content_size));
    let buttons = button_rects(outer, geometry.top);

    window
        .user_data()
        .insert_if_missing(RefCell::<FlatBuffers>::default);
    let mut buffers = window
        .user_data()
        .get::<RefCell<FlatBuffers>>()
        .expect("flat buffers were just inserted")
        .borrow_mut();
    buffers.update(geometry, outer, buttons, frame_state);

    let mut elements = Vec::with_capacity(4);
    let mut push = |buffer: &SolidColorBuffer, location: Point<i32, Logical>| {
        elements.push(NestedRenderElement::Solid(
            SolidColorRenderElement::from_buffer(
                buffer,
                location.to_physical_precise_round(scale),
                scale,
                1.0,
                Kind::Unspecified,
            ),
        ));
    };

    push(&buffers.background, outer.loc);
    push(&buffers.titlebar, outer.loc);

    for (button, rect) in buttons {
        let location = Point::from((rect.loc.x, rect.loc.y + 1));
        match button {
            FrameButton::Maximize | FrameButton::Hide => push(&buffers.maximize, location),
            FrameButton::Close => push(&buffers.close, location),
        }
    }

    elements
}
