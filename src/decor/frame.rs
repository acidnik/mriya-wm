//! Composing a window frame out of theme images.
//!
//! The frame is built the way xfwm4's `frameCreateTitlePixmap` and friends do it:
//! the titlebar strip is tiled from `title-1..title-5` (or stretched when the
//! theme ships `title-*-stretch`), the sides are stretched along their length,
//! corners and buttons are composited on top, and buttons follow the
//! `button_layout` string. The result is a few small textures plus per-pixel
//! alpha masks, which make transparent parts (rounded corners, empty button
//! areas) click-through.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use tracing::trace;

use super::{
    theme::{palette::Rgba, Theme, ThemeImage},
    FrameButton, FrameGeometry, FrameState,
};

/// A composed picture with straight alpha.
#[derive(Debug, Clone)]
struct Canvas {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: i32, height: i32) -> Self {
        let (width, height) = (width.max(0), height.max(0));
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    fn alpha(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        self.pixels[((y * self.width + x) * 4 + 3) as usize]
    }

    /// Source-over one pixel.
    fn blend(&mut self, x: i32, y: i32, src: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let offset = ((y * self.width + x) * 4) as usize;
        let src_alpha = u16::from(src[3]);
        if src_alpha == 0 {
            return;
        }
        if src_alpha == 255 {
            self.pixels[offset..offset + 4].copy_from_slice(&src);
            return;
        }

        let dst_alpha = u16::from(self.pixels[offset + 3]);
        let inverse = 255 - src_alpha;
        let out_alpha = src_alpha + dst_alpha * inverse / 255;
        if out_alpha == 0 {
            return;
        }
        for channel in 0..3 {
            let numerator = u32::from(src[channel]) * u32::from(src_alpha)
                + u32::from(self.pixels[offset + channel])
                    * u32::from(dst_alpha)
                    * u32::from(inverse)
                    / 255;
            self.pixels[offset + channel] = (numerator / u32::from(out_alpha)) as u8;
        }
        self.pixels[offset + 3] = out_alpha as u8;
    }

    fn blit(&mut self, source: &ThemeImage, x: i32, y: i32) {
        for sy in 0..source.height {
            for sx in 0..source.width {
                if let Some(color) = source.pixel(sx, sy) {
                    self.blend(x + sx, y + sy, color.to_array());
                }
            }
        }
    }

    /// Repeat `source` to fill a rectangle, like cairo's `EXTEND_REPEAT`.
    fn blit_tiled(&mut self, source: &ThemeImage, x: i32, y: i32, width: i32, height: i32) {
        if source.width <= 0 || source.height <= 0 || width <= 0 || height <= 0 {
            return;
        }
        for dy in 0..height {
            let sy = dy % source.height;
            for dx in 0..width {
                let sx = dx % source.width;
                if let Some(color) = source.pixel(sx, sy) {
                    self.blend(x + dx, y + dy, color.to_array());
                }
            }
        }
    }

    /// Stretch along one axis (the other keeps the source size), like
    /// `xfwmPixmapFill` with a single scale factor.
    fn blit_stretched(
        &mut self,
        source: &ThemeImage,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        horizontally: bool,
    ) {
        if source.width <= 0 || source.height <= 0 || width <= 0 || height <= 0 {
            return;
        }

        if horizontally {
            for dx in 0..width {
                let position = (dx as f64 + 0.5) * source.width as f64 / width as f64 - 0.5;
                let left = position.floor().max(0.0) as i32;
                let right = (left + 1).min(source.width - 1);
                let ratio = (position - left as f64).clamp(0.0, 1.0);
                for dy in 0..height.min(source.height) {
                    if let Some(color) = mix(source.pixel(left, dy), source.pixel(right, dy), ratio)
                    {
                        self.blend(x + dx, y + dy, color);
                    }
                }
            }
        } else {
            for dy in 0..height {
                let position = (dy as f64 + 0.5) * source.height as f64 / height as f64 - 0.5;
                let top = position.floor().max(0.0) as i32;
                let bottom = (top + 1).min(source.height - 1);
                let ratio = (position - top as f64).clamp(0.0, 1.0);
                for dx in 0..width.min(source.width) {
                    if let Some(color) = mix(source.pixel(dx, top), source.pixel(dx, bottom), ratio)
                    {
                        self.blend(x + dx, y + dy, color);
                    }
                }
            }
        }
    }

    /// Upload as a texture, premultiplied in the byte order `Argb8888` expects.
    fn to_buffer(&self) -> MemoryRenderBuffer {
        let mut bytes = Vec::with_capacity(self.pixels.len());
        for pixel in self.pixels.chunks_exact(4) {
            let (r, g, b, a) = (pixel[0], pixel[1], pixel[2], pixel[3]);
            let scale = |channel: u8| ((u16::from(channel) * u16::from(a)) / 255) as u8;
            // ARGB8888 in memory is B, G, R, A.
            bytes.extend_from_slice(&[scale(b), scale(g), scale(r), a]);
        }

        MemoryRenderBuffer::from_slice(
            &bytes,
            Fourcc::Argb8888,
            (self.width, self.height),
            1,
            Transform::Normal,
            None,
        )
    }

    fn mask(&self) -> AlphaMask {
        let area = (self.width * self.height).max(0) as usize;
        let mut bits = vec![0u64; area.div_ceil(64)];
        for y in 0..self.height {
            for x in 0..self.width {
                if self.alpha(x, y) > 8 {
                    let index = (y * self.width + x) as usize;
                    bits[index / 64] |= 1 << (index % 64);
                }
            }
        }
        AlphaMask {
            width: self.width,
            height: self.height,
            bits,
        }
    }
}

fn mix(from: Option<Rgba>, to: Option<Rgba>, ratio: f64) -> Option<[u8; 4]> {
    let (from, to) = (from?, to?);
    let lerp = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * ratio).round() as u8;
    Some([
        lerp(from.r, to.r),
        lerp(from.g, to.g),
        lerp(from.b, to.b),
        lerp(from.a, to.a),
    ])
}

/// One bit per pixel: where the frame is actually painted.
#[derive(Debug, Clone)]
pub struct AlphaMask {
    width: i32,
    height: i32,
    bits: Vec<u64>,
}

impl AlphaMask {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return false;
        }
        let index = (y * self.width + x) as usize;
        self.bits[index / 64] & (1 << (index % 64)) != 0
    }
}

/// A corner picture placed at a frame-relative position.
#[derive(Debug)]
pub struct CornerTexture {
    pub x: i32,
    pub y: i32,
    pub buffer: MemoryRenderBuffer,
    pub mask: AlphaMask,
}

/// Everything needed to draw and hit-test one window frame.
#[derive(Debug)]
pub struct FrameTextures {
    /// Titlebar strip including buttons, in frame coordinates.
    pub titlebar: MemoryRenderBuffer,
    pub left: MemoryRenderBuffer,
    pub right: MemoryRenderBuffer,
    pub bottom: MemoryRenderBuffer,
    pub corners: Vec<CornerTexture>,
    pub titlebar_mask: AlphaMask,
    pub left_mask: AlphaMask,
    pub right_mask: AlphaMask,
    pub bottom_mask: AlphaMask,
}

/// Is the frame painted at this frame-relative point? The content rect is
/// treated as opaque, everything else is looked up in the part masks.
pub fn frame_opaque(
    textures: &FrameTextures,
    geometry: FrameGeometry,
    outer_size: (i32, i32),
    x: i32,
    y: i32,
) -> bool {
    let (width, height) = outer_size;
    let content_w = width - geometry.left - geometry.right;
    let content_h = height - geometry.top - geometry.bottom;

    let in_content_x = x >= geometry.left && x < geometry.left + content_w;
    let in_content_y = y >= geometry.top && y < geometry.top + content_h;
    if in_content_x && in_content_y {
        return true;
    }

    if textures
        .corners
        .iter()
        .any(|corner| corner.mask.contains(x - corner.x, y - corner.y))
    {
        return true;
    }

    if y < geometry.top {
        return textures.titlebar_mask.contains(x, y);
    }
    if y >= height - geometry.bottom {
        return textures
            .bottom_mask
            .contains(x, y - (height - geometry.bottom));
    }
    if x < geometry.left {
        return textures.left_mask.contains(x, y - geometry.top);
    }
    if x >= width - geometry.right {
        return textures
            .right_mask
            .contains(x - (width - geometry.right), y - geometry.top);
    }

    true
}

/// Cache key of a composed frame: everything that changes its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameKey {
    pub width: i32,
    pub height: i32,
    pub active: bool,
    pub maximized: bool,
    pub hover: Option<FrameButton>,
    pub pressed: Option<FrameButton>,
}

/// A composed frame kept in the window user data.
#[derive(Debug)]
pub struct CachedFrame {
    key: FrameKey,
    pub textures: FrameTextures,
}

/// Textures of a window frame, rebuilt only when something they depend on
/// changes (size, focus, button states). Renderer and hit-testing share them.
pub fn textures<'a>(
    window: &'a smithay::desktop::Window,
    theme: &Theme,
    state: &FrameState,
    content_size: smithay::utils::Size<i32, smithay::utils::Logical>,
    geometry: FrameGeometry,
) -> std::cell::RefMut<'a, CachedFrame> {
    use std::cell::RefCell;

    window
        .user_data()
        .insert_if_missing(RefCell::<Option<CachedFrame>>::default);
    let cell = window
        .user_data()
        .get::<RefCell<Option<CachedFrame>>>()
        .expect("frame cache was just inserted");

    let outer_size = geometry.outer_size(content_size);
    let key = FrameKey {
        width: outer_size.w,
        height: outer_size.h,
        active: state.active,
        maximized: state.maximized,
        hover: state.hover,
        pressed: state.pressed,
    };

    // Rebuild outside of the borrow of `cell` to keep `state` readable while
    // composing (callers hold a borrow of the frame state).
    let needs_build = cell
        .borrow()
        .as_ref()
        .map(|cached| cached.key != key)
        .unwrap_or(true);
    if needs_build {
        let textures = build(theme, state, (outer_size.w, outer_size.h), geometry);
        *cell.borrow_mut() = Some(CachedFrame { key, textures });
    }

    std::cell::RefMut::map(cell.borrow_mut(), |cached| {
        cached.as_mut().expect("frame cache was just filled")
    })
}

/// Compose a window frame.
///
/// `outer_size` is the full frame size, `geometry` its border widths; the
/// content size is derived from them.
pub fn build(
    theme: &Theme,
    state: &FrameState,
    outer_size: (i32, i32),
    geometry: FrameGeometry,
) -> FrameTextures {
    let (width, height) = outer_size;
    let active = state.active;

    let corner_width = |corner: &str| {
        theme
            .corner(corner, active)
            .map(|image| image.width)
            .unwrap_or(0)
    };
    let strip_x = corner_width("top-left");
    let strip_w = (width - strip_x - corner_width("top-right")).max(0);

    let buttons = super::button_layout(theme, state, outer_size, geometry);
    let mut titlebar_canvas = Canvas::new(width, geometry.top);
    compose_titlebar(
        theme,
        state,
        &mut titlebar_canvas,
        strip_x,
        strip_w,
        (buttons.left_end - strip_x).max(0),
        (width - buttons.right_start + strip_x).max(0),
    );

    for (button, rect) in &buttons.buttons {
        if let Some(image) = button_image(theme, state, *button) {
            titlebar_canvas.blit(image, rect.loc.x, rect.loc.y);
        }
    }

    let side_height = (height - geometry.top - geometry.bottom).max(0);
    let left = compose_side(theme, "left", geometry.left, side_height, active, false);
    let right = compose_side(theme, "right", geometry.right, side_height, active, false);
    let bottom = compose_side(theme, "bottom", width, geometry.bottom, active, true);

    let mut corners = Vec::new();
    for (corner, anchor_right, anchor_bottom) in [
        ("top-left", false, false),
        ("top-right", true, false),
        ("bottom-left", false, true),
        ("bottom-right", true, true),
    ] {
        let Some(image) = theme.corner(corner, active) else {
            continue;
        };
        let x = if anchor_right { width - image.width } else { 0 };
        let y = if anchor_bottom {
            height - image.height
        } else {
            0
        };
        corners.push(CornerTexture {
            x,
            y,
            buffer: image_to_buffer(image),
            mask: image_mask(image, x, y),
        });
    }

    let placed: Vec<char> = buttons
        .buttons
        .iter()
        .map(|(button, _)| button.letter())
        .collect();
    trace!(
        theme = %theme.name,
        width,
        height,
        titlebar = geometry.top,
        strip = strip_w,
        ?placed,
        "frame composed"
    );

    FrameTextures {
        titlebar: titlebar_canvas.to_buffer(),
        left: left.to_buffer(),
        right: right.to_buffer(),
        bottom: bottom.to_buffer(),
        corners,
        titlebar_mask: titlebar_canvas.mask(),
        left_mask: left.mask(),
        right_mask: right.mask(),
        bottom_mask: bottom.mask(),
    }
}

fn compose_titlebar(
    theme: &Theme,
    state: &FrameState,
    canvas: &mut Canvas,
    strip_x: i32,
    strip_width: i32,
    left_cap: i32,
    right_cap: i32,
) {
    let active = state.active;
    let height = canvas.height;
    if strip_width <= 0 {
        return;
    }

    if let Some(stretch) = theme.title_stretch(active) {
        canvas.blit_stretched(stretch, strip_x, 0, strip_width, height, true);
        return;
    }

    let width_of = |part: &str| {
        theme
            .state_part(part, active)
            .map(|image| image.width)
            .unwrap_or(0)
    };
    let w2 = width_of("title-2");
    let w4 = width_of("title-4");
    // `full_width_title` runs the caps from the frame edges, otherwise they hug
    // the title text area, which for now means the button groups.
    let (w1, w5) = (left_cap.min(strip_width), right_cap.min(strip_width));
    let w3 = (strip_width - w1 - w2 - w4 - w5).max(0);

    let mut x = strip_x;
    if let Some(image) = theme.state_part("title-1", active) {
        canvas.blit_tiled(image, x, 0, w1, height);
    }
    x += w1;

    if let Some(image) = theme.state_part("title-2", active) {
        canvas.blit_tiled(image, x, 0, w2, height);
    }
    x += w2;

    if let Some(image) = theme.state_part("title-3", active) {
        canvas.blit_tiled(image, x, 0, w3, height);
    }
    x += w3;

    if let Some(image) = theme.state_part("title-4", active) {
        x = x.min(strip_x + strip_width - w4).max(strip_x);
        canvas.blit_tiled(image, x, 0, w4, height);
        x += w4;
    }

    if let Some(image) = theme.state_part("title-5", active) {
        let remaining = (strip_x + strip_width - x).max(0);
        if remaining > 0 {
            canvas.blit_tiled(image, x, 0, remaining, height);
        }
    }
}

fn compose_side(
    theme: &Theme,
    base: &str,
    width: i32,
    height: i32,
    active: bool,
    horizontally: bool,
) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    if width <= 0 || height <= 0 {
        return canvas;
    }

    if let Some(image) = theme.stretch_part(base, active) {
        canvas.blit_stretched(image, 0, 0, width, height, horizontally);
    } else if let Some(image) = theme.state_part(base, active) {
        canvas.blit_stretched(image, 0, 0, width, height, horizontally);
    }
    canvas
}

/// Button picture, falling back from the current state to plain active/inactive
/// like `clientGetButtonState` does.
fn button_image<'a>(
    theme: &'a Theme,
    state: &FrameState,
    button: FrameButton,
) -> Option<&'a ThemeImage> {
    let current = super::button_state(state, button);
    let plain = if state.active { "active" } else { "inactive" };

    let mut candidates = Vec::with_capacity(4);
    if state.maximized {
        candidates.push(format!("{}-toggled-{current}", button.name()));
        candidates.push(format!("{}-toggled-{plain}", button.name()));
    }
    candidates.push(format!("{}-{current}", button.name()));
    candidates.push(format!("{}-{plain}", button.name()));

    candidates.iter().find_map(|name| theme.part(name))
}

fn image_to_buffer(image: &ThemeImage) -> MemoryRenderBuffer {
    let mut canvas = Canvas::new(image.width, image.height);
    canvas.blit(image, 0, 0);
    canvas.to_buffer()
}

/// Mask of one corner picture placed at frame-relative `(x, y)`.
fn image_mask(image: &ThemeImage, x: i32, y: i32) -> AlphaMask {
    let mut canvas = Canvas::new(image.width, image.height);
    canvas.blit(image, 0, 0);
    let mask = canvas.mask();
    // The mask is queried with frame coordinates minus the corner position, so
    // keep the picture-local mask as is.
    let _ = (x, y);
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decor::theme::ThemeParams;
    use std::fs;

    fn fixture(name: &str) -> (std::path::PathBuf, Theme) {
        let dir = std::env::temp_dir().join(format!("mriya-frame-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let xpm = |w: i32, h: i32, color: &str| {
            format!(
                "/* XPM */\nstatic char * x[] = {{\n\"{w} {h} 1 1\",\n\". c {color}\",\n{}\n}};\n",
                std::iter::repeat_n(format!("\"{}\"", ".".repeat(w as usize)), h as usize)
                    .collect::<Vec<_>>()
                    .join(",\n")
            )
        };

        fs::write(dir.join("title-1-active.xpm"), xpm(6, 20, "#101010")).unwrap();
        fs::write(dir.join("title-2-active.xpm"), xpm(4, 20, "#202020")).unwrap();
        fs::write(dir.join("title-3-active.xpm"), xpm(8, 20, "#303030")).unwrap();
        fs::write(dir.join("title-4-active.xpm"), xpm(4, 20, "#404040")).unwrap();
        fs::write(dir.join("title-5-active.xpm"), xpm(6, 20, "#505050")).unwrap();
        fs::write(dir.join("left-active.xpm"), xpm(5, 10, "#606060")).unwrap();
        fs::write(dir.join("right-active.xpm"), xpm(5, 10, "#606060")).unwrap();
        fs::write(dir.join("bottom-active.xpm"), xpm(10, 5, "#606060")).unwrap();
        // A rounded top-left corner: only the bottom-right half is painted.
        fs::write(
            dir.join("top-left-active.xpm"),
            "/* XPM */\nstatic char * x[] = {\n\"4 4 2 1\",\n\"  c None\",\n\". c #707070\",\n\"  ..\",\n\"  ..\",\n\"....\",\n\"....\",\n};\n",
        )
        .unwrap();
        fs::write(dir.join("close-active.xpm"), xpm(9, 9, "#ff0000")).unwrap();
        fs::write(dir.join("maximize-active.xpm"), xpm(9, 9, "#0000ff")).unwrap();
        fs::write(dir.join("themerc"), "button_spacing=0\nbutton_offset=0\n").unwrap();

        let theme = Theme::load_from_dir(&dir, "fixture", ThemeParams::default());
        (dir, theme)
    }

    fn state() -> FrameState {
        FrameState {
            active: true,
            ..FrameState::default()
        }
    }

    #[test]
    fn titlebar_has_the_strip_size() {
        let (dir, theme) = fixture("strip");
        let geometry = FrameGeometry {
            left: 5,
            top: 20,
            right: 5,
            bottom: 5,
        };
        let textures = build(&theme, &state(), (200, 100), geometry);
        // The texture covers the whole frame width; corners sit on top of it.
        assert_eq!(textures.titlebar_mask.width, 200);
        assert_eq!(textures.titlebar_mask.height, 20);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rounded_corner_is_transparent_in_the_mask() {
        let (dir, theme) = fixture("rounded");
        let geometry = FrameGeometry {
            left: 5,
            top: 20,
            right: 5,
            bottom: 5,
        };
        let textures = build(&theme, &state(), (200, 100), geometry);

        assert!(!frame_opaque(&textures, geometry, (200, 100), 0, 0));
        assert!(frame_opaque(&textures, geometry, (200, 100), 3, 3));
        assert!(frame_opaque(&textures, geometry, (200, 100), 50, 10));
        // Client content is always hit.
        assert!(frame_opaque(&textures, geometry, (200, 100), 100, 50));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn buttons_follow_the_layout_string() {
        let (dir, theme) = fixture("buttons");
        let geometry = FrameGeometry {
            left: 5,
            top: 20,
            right: 5,
            bottom: 5,
        };
        let strip = super::super::button_layout(&theme, &state(), (200, 100), geometry);
        let letters: Vec<char> = strip
            .buttons
            .iter()
            .map(|(button, _)| button.letter())
            .collect();
        // Default layout "O|SHMC": only C and M are implemented, close is
        // rightmost.
        assert_eq!(letters, vec!['C', 'M']);
        let (_, close) = strip.buttons[0];
        assert_eq!(close.loc.x + close.size.w, 200 - 5);
        let _ = fs::remove_dir_all(&dir);
    }
}
