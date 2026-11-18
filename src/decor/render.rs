//! Drawing the frame around a window.
//!
//! Stage A uses flat colours from a handful of cached [`SolidColorBuffer`]s.
//! Stage C replaces the source of this data with the xfwm4 theme tiler; the
//! element structure (frame background, titlebar, buttons, client content) and
//! the per-window cache stay the same.

use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        element::{
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
    decor::{theme::Theme, FrameGeometry},
};

use super::{button_rects, state, state_mut, FrameState};

/// Colours of the flat frame, per state.
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

/// Solid colour buffers of one window frame, kept in the window user data so
/// the damage tracker sees stable element ids.
#[derive(Debug, Default)]
struct FrameBuffers {
    background: SolidColorBuffer,
    titlebar: SolidColorBuffer,
    maximize: SolidColorBuffer,
    close: SolidColorBuffer,
}

impl FrameBuffers {
    fn update(
        &mut self,
        geometry: FrameGeometry,
        outer: Rectangle<i32, Logical>,
        buttons: [(super::FrameButton, Rectangle<i32, Logical>); 2],
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
                super::FrameButton::Close => {
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
                super::FrameButton::Maximize => {
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
                super::FrameButton::Maximize => &mut self.maximize,
                super::FrameButton::Close => &mut self.close,
            };
            // Leave a small gap so the buttons do not touch the frame edge.
            let size = Size::from((rect.size.w, (rect.size.h - 2 * super::BUTTON_OFFSET).max(1)));
            buffer.update(size, color);
        }
    }

    fn elements(
        &self,
        outer: Rectangle<i32, Logical>,
        buttons: [(super::FrameButton, Rectangle<i32, Logical>); 2],
        scale: Scale<f64>,
    ) -> Vec<NestedRenderElement<GlesRenderer>> {
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

        push(&self.background, outer.loc);
        let titlebar = Point::from((outer.loc.x, outer.loc.y));
        push(&self.titlebar, titlebar);

        for (button, rect) in buttons {
            let location = Point::from((rect.loc.x, rect.loc.y + super::BUTTON_OFFSET));
            match button {
                super::FrameButton::Maximize => push(&self.maximize, location),
                super::FrameButton::Close => push(&self.close, location),
            }
        }

        elements
    }
}

/// Frame elements for a window whose content area is at `content_location`.
///
/// The elements are emitted before the client's own elements, so the frame is
/// drawn below its client surface but above windows further down the stack.
pub fn elements(
    theme: Option<&Theme>,
    window: &Window,
    content_location: Point<i32, Logical>,
    content_size: Size<i32, Logical>,
    active: bool,
    scale: Scale<f64>,
) -> Vec<NestedRenderElement<GlesRenderer>> {
    if state(window).active != active {
        state_mut(window).active = active;
    }

    let frame = state(window);
    if !frame.decorated() {
        return Vec::new();
    }

    let geometry: FrameGeometry = FrameGeometry::of(theme, &frame);
    if geometry.is_empty() {
        return Vec::new();
    }

    let content = Rectangle::new(content_location, content_size);
    let outer = geometry.outer_rect(content);
    tracing::trace!(
        title = %frame.title,
        app_id = %frame.app_id,
        ?content,
        ?outer,
        "frame geometry"
    );
    let buttons = button_rects(outer, geometry.top);

    window
        .user_data()
        .insert_if_missing(RefCell::<FrameBuffers>::default);
    let mut buffers = window
        .user_data()
        .get::<RefCell<FrameBuffers>>()
        .expect("frame buffers were just inserted")
        .borrow_mut();
    buffers.update(geometry, outer, buttons, &frame);

    buffers.elements(outer, buttons, scale)
}
