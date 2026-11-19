//! Composing a window frame out of theme images.
//!
//! The frame is built the way xfwm4's `frameCreateTitlePixmap` and friends do it:
//! the titlebar strip is tiled from `title-1..title-5` (or stretched when the
//! theme ships `title-*-stretch`), the sides are stretched along their length,
//! corners and buttons are composited on top, and buttons follow the
//! `button_layout` string. The result is a few small textures plus per-pixel
//! alpha masks, which make transparent parts (rounded corners, empty button
//! areas) click-through.

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use tracing::{debug, trace};

use super::{
    canvas::{AlphaMask, Canvas},
    icons::IconCache,
    text::TextRenderer,
    theme::{palette::Rgba, Theme, ThemeImage},
    FrameButton, FrameGeometry, FrameState,
};

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
    /// Titlebar strip (background tiling only), in frame coordinates.
    pub titlebar: MemoryRenderBuffer,
    /// Text, icon and buttons drawn on top of the titlebar, as separate
    /// elements: keeping them out of the tiled texture makes every part
    /// independently visible and damage-tracked.
    pub overlays: Vec<(i32, i32, MemoryRenderBuffer)>,
    pub left: MemoryRenderBuffer,
    pub right: MemoryRenderBuffer,
    pub bottom: MemoryRenderBuffer,
    pub corners: Vec<CornerTexture>,
    pub titlebar_mask: AlphaMask,
    pub left_mask: AlphaMask,
    pub right_mask: AlphaMask,
    pub bottom_mask: AlphaMask,
    /// Drop shadow drawn behind the frame, already downscaled for the GPU.
    pub shadow: Option<ShadowTexture>,
}

/// A blurred silhouette of the frame, positioned relative to the frame origin.
#[derive(Debug)]
pub struct ShadowTexture {
    pub offset: (i32, i32),
    /// Size of the texture itself, in its own pixels.
    pub source_size: (i32, i32),
    /// Size the shadow should be drawn at (it is built at a lower resolution).
    pub draw_size: (i32, i32),
    pub buffer: MemoryRenderBuffer,
}

/// Shadows are built at 1/N resolution: they are blurry anyway and this keeps
/// rebuilding them during interactive resizes cheap.
const SHADOW_SCALE: i32 = 4;
/// Box blur radius in shadow pixels; three passes approximate a gaussian.
const SHADOW_RADIUS: i32 = 3;
/// Padding around the silhouette: three box passes spread the blur by
/// `3 * SHADOW_RADIUS`, and anything less clips the kernel into a hard edge.
const SHADOW_PADDING: i32 = 3 * SHADOW_RADIUS + 1;
/// Global opacity multiplier: blurring spreads the same energy over more pixels.
const SHADOW_STRENGTH: f32 = 1.6;

/// Opaque silhouette of the frame, used as the source for the drop shadow.
fn frame_silhouette(
    titlebar: &Canvas,
    left: &Canvas,
    right: &Canvas,
    bottom: &Canvas,
    corners: &[(i32, i32, &ThemeImage)],
    size: (i32, i32),
    geometry: FrameGeometry,
) -> Canvas {
    let mut silhouette = Canvas::new(size.0, size.1);
    let black = [0, 0, 0, 255];

    let mut paint = |canvas: &Canvas, x: i32, y: i32| {
        for cy in 0..canvas.height {
            for cx in 0..canvas.width {
                if canvas.alpha(cx, cy) > 8 {
                    silhouette.blend(x + cx, y + cy, black);
                }
            }
        }
    };

    paint(titlebar, 0, 0);
    paint(left, 0, geometry.top);
    paint(right, size.0 - geometry.right, geometry.top);
    paint(bottom, 0, size.1 - geometry.bottom);
    for (x, y, image) in corners {
        for dy in 0..image.height {
            for dx in 0..image.width {
                if image.alpha(dx, dy) > 8 {
                    silhouette.blend(x + dx, y + dy, black);
                }
            }
        }
    }

    silhouette
}

/// Downscale, blur and fade the silhouette into a shadow texture.
///
/// `shadow_delta_*` from the theme are not applied: xfwm4 uses them together
/// with its own gaussian offsets, and in this simpler model they only shift the
/// shadow up and to the left, which reads as a misplaced rectangle rather than a
/// drop shadow.
fn build_shadow(silhouette: &Canvas, opacity: u8) -> Option<ShadowTexture> {
    if opacity == 0 || silhouette.width <= 0 || silhouette.height <= 0 {
        return None;
    }

    let scale = SHADOW_SCALE;
    let width = (silhouette.width + scale - 1) / scale;
    let height = (silhouette.height + scale - 1) / scale;
    let padding = SHADOW_PADDING;
    let stride = width + 2 * padding;
    let padded_height = height + 2 * padding;

    // Sampling the silhouette down into the padded buffer.
    let mut alpha = vec![0u8; (stride * padded_height) as usize];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0u32;
            let mut count = 0u32;
            for sy in 0..scale {
                for sx in 0..scale {
                    sum += u32::from(silhouette.alpha(x * scale + sx, y * scale + sy));
                    count += 1;
                }
            }
            alpha[((y + padding) * stride + x + padding) as usize] = (sum / count) as u8;
        }
    }

    blur_alpha(&mut alpha, stride, padded_height);

    let opacity = f32::from(opacity) / 100.0 * SHADOW_STRENGTH;
    let mut canvas = Canvas::new(stride, padded_height);
    for y in 0..padded_height {
        for x in 0..stride {
            let value = (f32::from(alpha[(y * stride + x) as usize]) * opacity).min(255.0);
            if value >= 1.0 {
                canvas.blend(x, y, [0, 0, 0, value as u8]);
            }
        }
    }

    let max_alpha = alpha.iter().copied().max().unwrap_or(0);
    trace!(
        ?opacity,
        max_alpha,
        width = stride,
        height = padded_height,
        "shadow composed"
    );

    Some(ShadowTexture {
        offset: (-padding * scale, -padding * scale),
        source_size: (stride, padded_height),
        draw_size: (stride * scale, padded_height * scale),
        buffer: canvas.to_buffer(),
    })
}

/// Separable box blur, three passes approximate a gaussian well enough.
///
/// Outside of the buffer counts as zero and the divisor stays constant: skipping
/// the out-of-bounds samples (and shrinking the divisor) is what produced bright
/// rims and a hard rectangular edge around the shadow.
fn blur_alpha(alpha: &mut [u8], stride: i32, height: i32) {
    let radius = SHADOW_RADIUS;
    if radius <= 0 {
        return;
    }
    let width = stride;
    let window = (2 * radius + 1) as u32;
    let mut scratch = vec![0u8; alpha.len()];

    for _ in 0..3 {
        // Horizontal.
        for y in 0..height {
            let row = y * width;
            for x in 0..width {
                let mut sum = 0u32;
                for dx in -radius..=radius {
                    let sx = x + dx;
                    if sx < 0 || sx >= width {
                        continue;
                    }
                    sum += u32::from(alpha[(row + sx) as usize]);
                }
                scratch[(row + x) as usize] = (sum / window) as u8;
            }
        }
        // Vertical.
        for x in 0..width {
            for y in 0..height {
                let mut sum = 0u32;
                for dy in -radius..=radius {
                    let sy = y + dy;
                    if sy < 0 || sy >= height {
                        continue;
                    }
                    sum += u32::from(scratch[(sy * width + x) as usize]);
                }
                alpha[(y * width + x) as usize] = (sum / window) as u8;
            }
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameKey {
    pub width: i32,
    pub height: i32,
    pub active: bool,
    pub maximized: bool,
    pub hover: Option<FrameButton>,
    pub pressed: Option<FrameButton>,
    /// Title text is rendered into the titlebar texture.
    pub title: String,
    /// The application icon depends on it.
    pub app_id: String,
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
    text: &mut TextRenderer,
    icons: &mut IconCache,
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
        title: state.title.clone(),
        app_id: state.app_id.clone(),
    };

    // Rebuild outside of the borrow of `cell` to keep `state` readable while
    // composing (callers hold a borrow of the frame state).
    let needs_build = cell
        .borrow()
        .as_ref()
        .map(|cached| cached.key != key)
        .unwrap_or(true);
    if needs_build {
        let textures = build(
            theme,
            text,
            icons,
            state,
            (outer_size.w, outer_size.h),
            geometry,
        );
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
    text: &mut TextRenderer,
    icons: &mut IconCache,
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

    // Text, icon and buttons go on top as separate textures.
    let mut overlays: Vec<(i32, i32, MemoryRenderBuffer)> = Vec::new();

    if let Some((x, y, text_canvas)) = compose_title_text(
        theme,
        text,
        state,
        strip_x,
        strip_w,
        (buttons.left_end - strip_x).max(0),
        title_cap_width(theme, active),
        title_cap_width(theme, active),
        (width - buttons.right_start + strip_x).max(0),
        geometry.top,
    ) {
        if text_canvas.has_content() {
            overlays.push((x, y, text_canvas.to_buffer()));
        }
    }

    if theme.params.show_app_icon && theme.params.button_layout.contains('O') {
        if let Some(icon) = icons.get(&state.app_id) {
            let icon_height = (geometry.top - 6).max(8);
            let side = icon_height.min(geometry.top - 2);
            let x = geometry.left + theme.params.button_offset + 2;
            let y = ((geometry.top - side) / 2).max(0);
            let mut icon_canvas = Canvas::new(side, side);
            icon_canvas.blit_scaled(&icon, 0, 0, side, side);
            overlays.push((x, y, icon_canvas.to_buffer()));
        }
    }

    for (button, rect) in &buttons.buttons {
        if let Some(image) = button_image(theme, state, *button) {
            let mut button_canvas = Canvas::new(image.width, image.height);
            button_canvas.blit(image, 0, 0);
            overlays.push((rect.loc.x, rect.loc.y, button_canvas.to_buffer()));
        }
    }

    let side_height = (height - geometry.top - geometry.bottom).max(0);
    let left = compose_side(theme, "left", geometry.left, side_height, active, false);
    let right = compose_side(theme, "right", geometry.right, side_height, active, false);
    let bottom = compose_side(theme, "bottom", width, geometry.bottom, active, true);

    let mut corners = Vec::new();
    let mut corner_images: Vec<(i32, i32, &ThemeImage)> = Vec::new();
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
        corner_images.push((x, y, image));
        corners.push(CornerTexture {
            x,
            y,
            buffer: image_to_buffer(image),
            mask: image_mask(image, x, y),
        });
    }

    trace!(
        app_id = %state.app_id,
        title = %state.title,
        active,
        mode = ?state.mode,
        client_decorations = state.client_decorations,
        requested = ?state.requested_mode,
        "frame text and icon"
    );
    let placed: Vec<char> = buttons
        .buttons
        .iter()
        .map(|(button, _)| button.letter())
        .collect();
    debug!(
        theme = %theme.name,
        app_id = %state.app_id,
        active,
        width,
        height,
        titlebar = geometry.top,
        strip = strip_w,
        left_end = buttons.left_end,
        right_start = buttons.right_start,
        alignment = ?theme.params.title_alignment,
        ?placed,
        "frame composed"
    );

    let silhouette = frame_silhouette(
        &titlebar_canvas,
        &left,
        &right,
        &bottom,
        &corner_images,
        (width, height),
        geometry,
    );
    let shadow = build_shadow(&silhouette, theme.params.shadow_opacity);

    FrameTextures {
        shadow,
        titlebar: titlebar_canvas.to_buffer(),
        overlays,
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

#[allow(clippy::too_many_arguments)]
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

    if let Some(stretch) = theme.title_stretch(active) {
        canvas.blit_stretched(stretch, strip_x, 0, strip_width, height, true);
    } else {
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
}

/// Width of the `title-2`/`title-4` caps the text sits between.
fn title_cap_width(theme: &Theme, active: bool) -> i32 {
    theme
        .state_part("title-2", active)
        .map(|image| image.width)
        .unwrap_or(0)
}

/// Title text rasterised into a canvas that is exactly as large as the text, so
/// it can be placed at the computed position in the titlebar.
#[allow(clippy::too_many_arguments)]
fn compose_title_text(
    theme: &Theme,
    text: &mut TextRenderer,
    state: &FrameState,
    strip_x: i32,
    strip_width: i32,
    w1: i32,
    w2: i32,
    w4: i32,
    w5: i32,
    height: i32,
) -> Option<(i32, i32, Canvas)> {
    if state.title.is_empty() {
        return None;
    }

    let params = &theme.params;
    let active = state.active;
    let x0 = (strip_x + w1 + w2).max(strip_x);
    let x1 = (strip_x + strip_width - w5 - w4).min(strip_x + strip_width);
    if x1 <= x0 {
        return None;
    }
    let vertical_offset = if active {
        params.title_vertical_offset_active
    } else {
        params.title_vertical_offset_inactive
    };
    let color = theme_text_color(params, active);
    let shadow = if active {
        params.title_shadow_active
    } else {
        params.title_shadow_inactive
    };
    let shadow_color = theme_text_shadow_color(params, active);

    let size = text.measure(&params.title_font, &state.title);
    let text_width = (size.ceil() as i32).max(1);
    let mut canvas = Canvas::new(text_width, height);

    let start_x = match params.title_alignment {
        super::theme::Alignment::Left => x0,
        super::theme::Alignment::Center => x0 + (x1 - x0 - text_width) / 2,
        super::theme::Alignment::Right => x1 - text_width,
    }
    .clamp(x0, x1.saturating_sub(text_width).max(x0));

    text.draw(
        &mut canvas,
        &params.title_font,
        &state.title,
        smithay::utils::Rectangle::new((0, 0).into(), (text_width, height).into()),
        vertical_offset,
        super::theme::Alignment::Left,
        color,
        shadow,
        shadow_color,
    );

    Some((start_x, 0, canvas))
}

fn theme_text_color(params: &super::theme::ThemeParams, active: bool) -> Rgba {
    let symbol = if active {
        "active_text_color"
    } else {
        "inactive_text_color"
    };
    params
        .palette
        .get(symbol)
        .unwrap_or(Rgba::rgb(255, 255, 255))
}

fn theme_text_shadow_color(params: &super::theme::ThemeParams, active: bool) -> Rgba {
    let symbol = if active {
        "active_text_shadow_color"
    } else {
        "inactive_text_shadow_color"
    };
    params.palette.get(symbol).unwrap_or(Rgba::TRANSPARENT)
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
        let mut text = TextRenderer::new();
        let mut icons = IconCache::new();
        let textures = build(
            &theme,
            &mut text,
            &mut icons,
            &state(),
            (200, 100),
            geometry,
        );
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
        let mut text = TextRenderer::new();
        let mut icons = IconCache::new();
        let textures = build(
            &theme,
            &mut text,
            &mut icons,
            &state(),
            (200, 100),
            geometry,
        );

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
