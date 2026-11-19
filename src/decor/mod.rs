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

pub mod canvas;
pub mod frame;
pub mod icons;
pub mod protocol;
pub mod render;
pub mod text;
pub mod theme;

use std::cell::{Ref, RefCell, RefMut};

use smithay::{
    desktop::{Space, Window},
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{decor::theme::Theme, grabs::ResizeEdge};

/// Height of the titlebar, border widths are symmetric for now.
pub const TITLEBAR_HEIGHT: i32 = 28;
pub const BORDER_WIDTH: i32 = 4;
/// Width of a titlebar button and its distance from the right edge.
pub const BUTTON_WIDTH: i32 = 28;
pub const BUTTON_OFFSET: i32 = 2;

/// Titlebar buttons Mriya knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameButton {
    Close,
    Maximize,
    Hide,
}

impl FrameButton {
    /// Buttons placed from the right edge, in this order, when no theme is
    /// loaded.
    pub const ALL: [FrameButton; 2] = [FrameButton::Close, FrameButton::Maximize];

    /// Part name prefix in the theme (`close-active`, `maximize-prelight`).
    pub fn name(&self) -> &'static str {
        match self {
            FrameButton::Close => "close",
            FrameButton::Maximize => "maximize",
            FrameButton::Hide => "hide",
        }
    }

    /// Letter used in the `button_layout` string.
    pub fn letter(&self) -> char {
        match self {
            FrameButton::Close => 'C',
            FrameButton::Maximize => 'M',
            FrameButton::Hide => 'H',
        }
    }

    pub fn from_letter(letter: char) -> Option<Self> {
        match letter {
            'C' => Some(FrameButton::Close),
            'M' => Some(FrameButton::Maximize),
            'H' => Some(FrameButton::Hide),
            _ => None,
        }
    }
}

/// Buttons of one window frame and the titlebar areas they leave free.
#[derive(Debug, Clone, Default)]
pub struct ButtonStrip {
    pub buttons: Vec<(FrameButton, Rectangle<i32, Logical>)>,
    /// Where the left button group ends (or the left edge).
    pub left_end: i32,
    /// Where the right button group starts (or the right edge).
    pub right_start: i32,
}

/// Button layout for a themed frame, following `button_layout` like xfwm4:
/// letters before `|` go on the left, letters after it on the right, in layout
/// order; the last letter of the right group ends up rightmost.
pub fn button_layout(
    theme: &Theme,
    state: &FrameState,
    outer_size: (i32, i32),
    geometry: FrameGeometry,
) -> ButtonStrip {
    let (width, _height) = outer_size;
    let active = state.active;
    let params = &theme.params;
    let layout: Vec<char> = params.button_layout.chars().collect();

    let image_size = |button: FrameButton| {
        theme
            .button(button.name(), button_state(state, button))
            .or_else(|| theme.button(button.name(), if active { "active" } else { "inactive" }))
            .map(|image| (image.width, image.height))
            .unwrap_or((0, 0))
    };

    let offset = if state.maximized {
        params.maximized_offset
    } else {
        params.button_offset
    };
    let spacing = params.button_spacing;

    let mut strip = ButtonStrip {
        left_end: 0,
        right_start: width,
        ..ButtonStrip::default()
    };

    // Left group.
    let mut x = (geometry.left + offset).max(0);
    let mut index = 0;
    let mut left_seen = false;
    while index < layout.len() {
        let letter = layout[index];
        index += 1;
        if letter == '|' {
            break;
        }
        let Some(button) = FrameButton::from_letter(letter) else {
            continue;
        };
        left_seen = true;
        let (button_width, button_height) = image_size(button);
        if button_width <= 0 || x + button_width + spacing >= width - geometry.right {
            continue;
        }
        strip.buttons.push((
            button,
            Rectangle::new(
                Point::from((x, ((geometry.top - button_height + 1) / 2).max(0))),
                Size::from((button_width, button_height)),
            ),
        ));
        x += button_width + spacing;
    }
    strip.left_end = if left_seen { x } else { 0 };

    // Right group, from the end of the layout string backwards.
    let mut x = width - geometry.right + spacing - offset;
    for letter in layout.iter().rev() {
        if *letter == '|' {
            break;
        }
        let Some(button) = FrameButton::from_letter(*letter) else {
            continue;
        };
        let (button_width, button_height) = image_size(button);
        if button_width <= 0 || x - button_width - spacing <= strip.left_end {
            continue;
        }
        x -= button_width + spacing;
        strip.buttons.push((
            button,
            Rectangle::new(
                Point::from((x, ((geometry.top - button_height + 1) / 2).max(0))),
                Size::from((button_width, button_height)),
            ),
        ));
    }
    strip.right_start = x;

    strip
}

/// Theme part suffix for a button in its current state.
pub fn button_state(state: &FrameState, button: FrameButton) -> &'static str {
    if state.pressed == Some(button) {
        "pressed"
    } else if state.hover == Some(button) {
        "prelight"
    } else if state.active {
        "active"
    } else {
        "inactive"
    }
}

/// Per-window decoration state, stored in the window's user data.
#[derive(Debug, Clone)]
pub struct FrameState {
    /// Effective mode, the one used for drawing.
    pub mode: Mode,
    /// Mode the client asked for through `xdg-decoration`, if it speaks it.
    pub requested_mode: Option<Mode>,
    /// Mode forced by a `window-rule`, if any.
    pub rule_mode: Option<Mode>,
    /// The client draws its own decorations; detected from its window geometry.
    pub client_decorations: bool,
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
            // Mriya decorates by default; clients that bring their own
            // decorations (detected on commit) or ask for them keep them.
            mode: Mode::ServerSide,
            requested_mode: None,
            rule_mode: None,
            client_decorations: false,
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

    /// Recompute the effective mode from the rule, the protocol request and what
    /// the client actually draws. Returns whether it changed.
    ///
    /// Precedence: an explicit `window-rule`, then what the client told us over
    /// `xdg-decoration`, then automatic detection.
    pub fn refresh_mode(&mut self) -> bool {
        let effective =
            self.rule_mode
                .or(self.requested_mode)
                .unwrap_or(if self.client_decorations {
                    Mode::ClientSide
                } else {
                    Mode::ServerSide
                });

        let changed = self.mode != effective;
        self.mode = effective;
        changed
    }
}

/// Does the client draw its own decorations?
///
/// Such a client has to report a window geometry that excludes them
/// (`xdg_toplevel.set_window_geometry`): either offset from the surface origin
/// (titlebar, shadow above/left) or smaller than the surface (borders, shadow
/// below/right). Server-decorated clients report the whole surface, so geometry
/// and the surface tree match. Measured on this system: foot/kitty report the
/// full surface, ghostty/thunar/appfinder report an inset one.
pub fn detects_client_decorations(window: &Window) -> bool {
    geometry_looks_inset(window.geometry(), window.bbox())
}

/// The comparison behind [`detects_client_decorations`], split out for tests.
fn geometry_looks_inset(
    geometry: Rectangle<i32, Logical>,
    surface: Rectangle<i32, Logical>,
) -> bool {
    // Before the first buffer commit both rectangles are empty; a client that
    // has not drawn anything yet is not evidence of anything.
    if surface.size.w <= 0 || surface.size.h <= 0 {
        return false;
    }
    geometry.loc != Point::from((0, 0)) || geometry.size != surface.size
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
    /// Frame around a window, using the theme's sizes when one is loaded.
    pub fn of(theme: Option<&Theme>, state: &FrameState) -> Self {
        if !state.decorated() {
            return Self::default();
        }

        // Geometry follows the active images, like `frameDecoration*` in xfwm4,
        // so the frame does not change size when focus moves.
        let mut geometry = match theme {
            Some(theme) => Self {
                left: theme.border_left(true),
                top: theme.title_height(true),
                right: theme.border_right(true),
                bottom: theme.border_bottom(true),
            },
            None => Self {
                left: BORDER_WIDTH,
                top: TITLEBAR_HEIGHT,
                right: BORDER_WIDTH,
                bottom: BORDER_WIDTH,
            },
        };

        // A theme that provides nothing usable still needs a titlebar to grab.
        if geometry.top <= 0 {
            geometry.top = TITLEBAR_HEIGHT;
        }

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
pub fn button_rects(
    outer: Rectangle<i32, Logical>,
    title_height: i32,
) -> [(FrameButton, Rectangle<i32, Logical>); 2] {
    let y = outer.loc.y;
    let right = outer.loc.x + outer.size.w - BUTTON_OFFSET;
    let mut rects = [(FrameButton::Maximize, Rectangle::default()); 2];

    for (index, button) in FrameButton::ALL.into_iter().enumerate() {
        let x = right - (index as i32 + 1) * BUTTON_WIDTH - index as i32 * BUTTON_OFFSET;
        rects[index] = (
            button,
            Rectangle::new(
                Point::from((x, y)),
                Size::from((BUTTON_WIDTH, title_height)),
            ),
        );
    }
    rects
}

/// Topmost window frame under `position`, in space coordinates.
pub fn hit_test(
    theme: Option<&Theme>,
    text: &mut text::TextRenderer,
    icons: &mut icons::IconCache,
    space: &Space<Window>,
    position: Point<f64, Logical>,
) -> Option<FrameHit> {
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

        let geometry = FrameGeometry::of(theme, &frame);
        let outer = geometry.outer_rect(content);
        if !outer.to_f64().contains(position) {
            continue;
        }

        let part = if content.to_f64().contains(position) {
            FramePart::Content
        } else if let Some(theme) = theme {
            let strip = button_layout(theme, &frame, (outer.size.w, outer.size.h), geometry);
            if let Some((button, _)) = strip.buttons.iter().find(|(_, rect)| {
                Rectangle::new(outer.loc + rect.loc, rect.size)
                    .to_f64()
                    .contains(position)
            }) {
                FramePart::Button(*button)
            } else {
                let cache =
                    frame::textures(window, theme, text, icons, &frame, content.size, geometry);
                let local_x = (position.x - outer.loc.x as f64) as i32;
                let local_y = (position.y - outer.loc.y as f64) as i32;
                if !frame::frame_opaque(
                    &cache.textures,
                    geometry,
                    (outer.size.w, outer.size.h),
                    local_x,
                    local_y,
                ) {
                    // Transparent part of the frame (a rounded corner, an empty
                    // button area): the click belongs to whatever is below.
                    continue;
                }
                if position.y < content.loc.y as f64 {
                    FramePart::Titlebar
                } else {
                    FramePart::Edge(edge_at(&content, geometry, position))
                }
            }
        } else if position.y < content.loc.y as f64 {
            button_rects(outer, geometry.top)
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
        let geometry = FrameGeometry::of(None, &server_side());
        assert_eq!(geometry.top, TITLEBAR_HEIGHT);
        assert_eq!(geometry.left, BORDER_WIDTH);
        assert_eq!(
            geometry.content_offset(),
            Point::from((BORDER_WIDTH, TITLEBAR_HEIGHT))
        );
    }

    #[test]
    fn outer_size_includes_the_frame() {
        let geometry = FrameGeometry::of(None, &server_side());
        assert_eq!(
            geometry.outer_size(Size::from((100, 50))),
            Size::from((100 + 2 * BORDER_WIDTH, 50 + TITLEBAR_HEIGHT + BORDER_WIDTH))
        );
    }

    #[test]
    fn outer_rect_is_anchored_around_the_content() {
        let geometry = FrameGeometry::of(None, &server_side());
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
        assert!(FrameGeometry::of(None, &ml).is_empty());

        let mut csd = server_side();
        csd.mode = Mode::ClientSide;
        assert!(FrameGeometry::of(None, &csd).is_empty());
    }

    #[test]
    fn maximized_keeps_only_the_titlebar() {
        let mut state = server_side();
        state.maximized = true;
        let geometry = FrameGeometry::of(None, &state);
        assert_eq!(geometry.top, TITLEBAR_HEIGHT);
        assert_eq!((geometry.left, geometry.right, geometry.bottom), (0, 0, 0));
    }

    #[test]
    fn full_surface_geometry_means_no_client_decorations() {
        let surface = Rectangle::new(Point::from((0, 0)), Size::from((696, 496)));
        assert!(!geometry_looks_inset(surface, surface));
    }

    #[test]
    fn empty_surface_is_not_a_client_decoration_signal() {
        // Before the first buffer commit both rectangles are empty: a client
        // that asked for server-side decorations must not be flipped to
        // client-side because nothing has been drawn yet.
        let empty = Rectangle::new(Point::from((0, 0)), Size::from((0, 0)));
        assert!(!geometry_looks_inset(empty, empty));
        let geometry = Rectangle::new(Point::from((0, 0)), Size::from((696, 496)));
        assert!(!geometry_looks_inset(geometry, empty));
    }

    #[test]
    fn inset_geometry_means_client_decorations() {
        // ghostty on this system: a 12/14 px shadow around the content.
        let surface = Rectangle::new(Point::from((0, 0)), Size::from((828, 629)));
        let geometry = Rectangle::new(Point::from((14, 12)), Size::from((800, 600)));
        assert!(geometry_looks_inset(geometry, surface));

        // Offset only (a titlebar without side borders).
        let geometry = Rectangle::new(Point::from((0, 26)), Size::from((828, 603)));
        assert!(geometry_looks_inset(geometry, surface));

        // Smaller only (borders without an offset).
        let geometry = Rectangle::new(Point::from((0, 0)), Size::from((820, 621)));
        assert!(geometry_looks_inset(geometry, surface));
    }

    #[test]
    fn buttons_are_laid_out_from_the_right_edge() {
        let outer = Rectangle::new(Point::from((0, 0)), Size::from((400, 200)));
        let rects = button_rects(outer, TITLEBAR_HEIGHT);

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
