//! Server-side decorations.
//!
//! Stage A: the frame is drawn with flat colours, geometry and hit-testing are
//! already the ones an xfwm4 theme will drive later (titlebar height and border
//! widths will come from the theme images instead of the constants below).
//!
//! Every window carries a [`FrameState`] in its user data; the frame is drawn
//! around the client content, which keeps `Space<Window>` (and therefore all the
//! existing placement/grab code) unchanged: a window's location stays the
//! location of its content area.

pub mod protocol;
pub mod render;

use std::cell::{Ref, RefCell, RefMut};

use smithay::{
    desktop::{Space, Window},
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::grabs::ResizeEdge;

/// Height of the titlebar, border widths are symmetric for now.
pub const TITLEBAR_HEIGHT: i32 = 28;
pub const BORDER_WIDTH: i32 = 4;
/// Width of a titlebar button and its distance from the right edge.
pub const BUTTON_WIDTH: i32 = 28;
pub const BUTTON_OFFSET: i32 = 2;

/// Titlebar buttons Mriya knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameButton {
    Maximize,
    Close,
}

impl FrameButton {
    /// Buttons are placed from the right edge of the titlebar in this order.
    pub const ALL: [FrameButton; 2] = [FrameButton::Close, FrameButton::Maximize];
}

/// Per-window decoration state, stored in the window's user data.
#[derive(Debug, Clone)]
pub struct FrameState {
    pub mode: Mode,
    pub active: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub title: String,
    pub app_id: String,
    /// Button the pointer currently hovers.
    pub hover: Option<FrameButton>,
    /// Button held down right now.
    pub pressed: Option<FrameButton>,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            // Mriya decorates by default; a client that explicitly asks for
            // client-side decorations keeps them.
            mode: Mode::ServerSide,
            active: false,
            maximized: false,
            fullscreen: false,
            title: String::new(),
            app_id: String::new(),
            hover: None,
            pressed: None,
        }
    }
}

impl FrameState {
    /// Whether Mriya draws a frame for this window at all.
    pub fn decorated(&self) -> bool {
        self.mode == Mode::ServerSide && !self.fullscreen
    }
}

/// Border widths around the client content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameGeometry {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl FrameGeometry {
    pub fn of(state: &FrameState) -> Self {
        if !state.decorated() {
            return Self::default();
        }

        let mut geometry = Self {
            left: BORDER_WIDTH,
            top: TITLEBAR_HEIGHT,
            right: BORDER_WIDTH,
            bottom: BORDER_WIDTH,
        };
        if state.maximized {
            // Like xfwm4's `borderless_maximize`: no side borders, the titlebar
            // stays on top of the screen.
            geometry.left = 0;
            geometry.right = 0;
            geometry.bottom = 0;
        }
        geometry
    }

    pub fn is_empty(&self) -> bool {
        self.left == 0 && self.top == 0 && self.right == 0 && self.bottom == 0
    }

    /// Size of the whole frame for the given content size.
    pub fn outer_size(&self, content: Size<i32, Logical>) -> Size<i32, Logical> {
        Size::from((
            content.w + self.left + self.right,
            content.h + self.top + self.bottom,
        ))
    }

    /// Offset of the client content inside the frame.
    pub fn content_offset(&self) -> Point<i32, Logical> {
        Point::from((self.left, self.top))
    }

    /// Frame rectangle for a content rectangle in space coordinates.
    pub fn outer_rect(&self, content: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        Rectangle::new(
            Point::from((content.loc.x - self.left, content.loc.y - self.top)),
            self.outer_size(content.size),
        )
    }
}

/// State accessors for a window.
pub fn state(window: &Window) -> Ref<'_, FrameState> {
    window
        .user_data()
        .insert_if_missing(RefCell::<FrameState>::default);
    window
        .user_data()
        .get::<RefCell<FrameState>>()
        .expect("frame state was just inserted")
        .borrow()
}

/// Mutable state accessors for a window.
pub fn state_mut(window: &Window) -> RefMut<'_, FrameState> {
    window
        .user_data()
        .insert_if_missing(RefCell::<FrameState>::default);
    window
        .user_data()
        .get::<RefCell<FrameState>>()
        .expect("frame state was just inserted")
        .borrow_mut()
}

/// Part of a window frame under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePart {
    /// Client content, handled by the normal input path.
    Content,
    Button(FrameButton),
    /// Empty part of the titlebar: drag to move, double click to maximize.
    Titlebar,
    /// A frame border: start an interactive resize.
    Edge(ResizeEdge),
}

/// Window frame found under the pointer.
#[derive(Debug, Clone)]
pub struct FrameHit {
    pub window: Window,
    pub part: FramePart,
    /// Location of the content area in space coordinates.
    pub content_location: Point<i32, Logical>,
    pub content_size: Size<i32, Logical>,
}

/// Rectangle of every titlebar button for a frame with the given outer rect.
pub fn button_rects(outer: Rectangle<i32, Logical>) -> [(FrameButton, Rectangle<i32, Logical>); 2] {
    let y = outer.loc.y;
    let right = outer.loc.x + outer.size.w - BUTTON_OFFSET;
    let mut rects = [(FrameButton::Maximize, Rectangle::default()); 2];

    for (index, button) in FrameButton::ALL.into_iter().enumerate() {
        let x = right - (index as i32 + 1) * BUTTON_WIDTH - index as i32 * BUTTON_OFFSET;
        rects[index] = (
            button,
            Rectangle::new(
                Point::from((x, y)),
                Size::from((BUTTON_WIDTH, TITLEBAR_HEIGHT)),
            ),
        );
    }
    rects
}

/// Topmost window frame under `position`, in space coordinates.
pub fn hit_test(space: &Space<Window>, position: Point<f64, Logical>) -> Option<FrameHit> {
    for window in space.elements().rev() {
        let Some(content) = space.element_geometry(window) else {
            continue;
        };
        let frame = state(window);
        if !frame.decorated() {
            // No frame: only the content area counts, and the regular path
            // handles it.
            if content.to_f64().contains(position) {
                return Some(FrameHit {
                    window: window.clone(),
                    part: FramePart::Content,
                    content_location: content.loc,
                    content_size: content.size,
                });
            }
            continue;
        }

        let geometry = FrameGeometry::of(&frame);
        let outer = geometry.outer_rect(content);
        if !outer.to_f64().contains(position) {
            continue;
        }

        let part = if content.to_f64().contains(position) {
            FramePart::Content
        } else if position.y < content.loc.y as f64 {
            // Titlebar band, corners included.
            button_rects(outer)
                .into_iter()
                .find(|(_, rect)| rect.to_f64().contains(position))
                .map(|(button, _)| FramePart::Button(button))
                .unwrap_or(FramePart::Titlebar)
        } else {
            FramePart::Edge(edge_at(&content, geometry, position))
        };

        return Some(FrameHit {
            window: window.clone(),
            part,
            content_location: content.loc,
            content_size: content.size,
        });
    }
    None
}

fn edge_at(
    content: &Rectangle<i32, Logical>,
    geometry: FrameGeometry,
    position: Point<f64, Logical>,
) -> ResizeEdge {
    let mut edge = ResizeEdge::empty();
    if position.x < content.loc.x as f64 {
        edge |= ResizeEdge::LEFT;
    }
    if position.x >= (content.loc.x + content.size.w) as f64 {
        edge |= ResizeEdge::RIGHT;
    }
    if position.y < content.loc.y as f64 {
        edge |= ResizeEdge::TOP;
    }
    if position.y >= (content.loc.y + content.size.h) as f64 {
        edge |= ResizeEdge::BOTTOM;
    }
    let _ = geometry;
    edge
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_side() -> FrameState {
        FrameState::default()
    }

    #[test]
    fn content_offset_matches_border_widths() {
        let geometry = FrameGeometry::of(&server_side());
        assert_eq!(geometry.top, TITLEBAR_HEIGHT);
        assert_eq!(geometry.left, BORDER_WIDTH);
        assert_eq!(
            geometry.content_offset(),
            Point::from((BORDER_WIDTH, TITLEBAR_HEIGHT))
        );
    }

    #[test]
    fn outer_size_includes_the_frame() {
        let geometry = FrameGeometry::of(&server_side());
        assert_eq!(
            geometry.outer_size(Size::from((100, 50))),
            Size::from((100 + 2 * BORDER_WIDTH, 50 + TITLEBAR_HEIGHT + BORDER_WIDTH))
        );
    }

    #[test]
    fn outer_rect_is_anchored_around_the_content() {
        let geometry = FrameGeometry::of(&server_side());
        let content = Rectangle::new(Point::from((100, 100)), Size::from((200, 100)));
        let outer = geometry.outer_rect(content);
        assert_eq!(
            outer.loc,
            Point::from((100 - BORDER_WIDTH, 100 - TITLEBAR_HEIGHT))
        );
        assert_eq!(outer.size, geometry.outer_size(content.size));
    }

    #[test]
    fn fullscreen_and_client_side_have_no_frame() {
        let mut ml = server_side();
        ml.fullscreen = true;
        assert!(FrameGeometry::of(&ml).is_empty());

        let mut csd = server_side();
        csd.mode = Mode::ClientSide;
        assert!(FrameGeometry::of(&csd).is_empty());
    }

    #[test]
    fn maximized_keeps_only_the_titlebar() {
        let mut state = server_side();
        state.maximized = true;
        let geometry = FrameGeometry::of(&state);
        assert_eq!(geometry.top, TITLEBAR_HEIGHT);
        assert_eq!((geometry.left, geometry.right, geometry.bottom), (0, 0, 0));
    }

    #[test]
    fn buttons_are_laid_out_from_the_right_edge() {
        let outer = Rectangle::new(Point::from((0, 0)), Size::from((400, 200)));
        let rects = button_rects(outer);

        // Close is the rightmost button, maximize sits to its left.
        let (close_button, close) = rects[0];
        assert_eq!(close_button, FrameButton::Close);
        assert_eq!(close.loc.x + close.size.w, 400 - BUTTON_OFFSET);
        assert_eq!(close.loc.y, outer.loc.y);
        assert_eq!(close.size, Size::from((BUTTON_WIDTH, TITLEBAR_HEIGHT)));

        let (maximize_button, maximize) = rects[1];
        assert_eq!(maximize_button, FrameButton::Maximize);
        assert_eq!(maximize.loc.x, close.loc.x - BUTTON_WIDTH - BUTTON_OFFSET);
    }
}
