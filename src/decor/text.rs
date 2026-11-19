//! Title text: font selection, layout and rasterisation into a canvas.
//!
//! xfwm4 draws titles with Pango using the `title_font` setting (an XFCE
//! setting, not part of the theme), e.g. `Sans Bold 9` in points. Mriya keeps the
//! same config syntax and renders with cosmic-text.

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight,
};
use smithay::utils::{Logical, Rectangle};

use super::{
    canvas::Canvas,
    theme::{palette::Rgba, Alignment, TitleShadow},
};

/// Width given to the layout; wide enough for any title, small enough to be
/// finite for cosmic-text's line breaking.
const TEXT_LAYOUT_WIDTH: f32 = 4096.0;

/// DPI used to convert the point size from the config to pixels.
const POINTS_TO_PIXELS: f32 = 96.0 / 72.0;

/// A parsed `title-font` value.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleFont {
    /// Font family; `None` means the system sans-serif face.
    pub family: Option<String>,
    pub bold: bool,
    pub italic: bool,
    /// Size in pixels.
    pub size: f32,
}

impl Default for TitleFont {
    fn default() -> Self {
        Self {
            family: None,
            bold: true,
            italic: false,
            size: 9.0 * POINTS_TO_PIXELS,
        }
    }
}

impl TitleFont {
    /// Parse a Pango-like specification: `Sans Bold 9`, `DejaVu Sans 10`, ...
    pub fn parse(spec: &str) -> Self {
        if spec.trim().is_empty() {
            return Self::default();
        }

        // A specification names exactly the style it wants: `DejaVu Sans 10` is
        // regular weight, only `Bold` makes it bold.
        let mut font = Self {
            bold: false,
            ..Self::default()
        };
        let mut family: Vec<&str> = Vec::new();

        for token in spec.split_whitespace() {
            match token.to_ascii_lowercase().as_str() {
                "bold" | "semibold" | "heavy" => font.bold = true,
                "normal" | "regular" | "book" => font.bold = false,
                "italic" | "oblique" => font.italic = true,
                _ => match token.parse::<f32>() {
                    Ok(points) if points > 0.0 => font.size = points * POINTS_TO_PIXELS,
                    _ => family.push(token),
                },
            }
        }

        if !family.is_empty() {
            font.family = Some(family.join(" "));
        }
        font
    }
}

/// Font system plus glyph cache; kept in the compositor state.
pub struct TextRenderer {
    font_system: FontSystem,
    swash: SwashCache,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
        }
    }

    /// Width of the rendered text in pixels.
    #[allow(dead_code)] // used by tests and handy when tuning the titlebar
    pub fn measure(&mut self, font: &TitleFont, text: &str) -> f32 {
        let buffer = self.layout(font, text);
        line_width(&buffer)
    }

    /// Draw `text` centred vertically in `area`, aligned horizontally, clipped to
    /// `area`, with the theme's shadow mode.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        font: &TitleFont,
        text: &str,
        area: Rectangle<i32, Logical>,
        vertical_offset: i32,
        alignment: Alignment,
        color: Rgba,
        shadow: TitleShadow,
        shadow_color: Rgba,
    ) {
        if text.is_empty() || area.size.w <= 0 || area.size.h <= 0 {
            return;
        }

        let buffer = self.layout(font, text);
        let width_of_text = line_width(&buffer);
        let mut line_height = font.size;
        for run in buffer.layout_runs() {
            line_height = line_height.max(run.line_height);
        }

        let start_x = match alignment {
            Alignment::Left => area.loc.x as f32,
            Alignment::Center => area.loc.x as f32 + (area.size.w as f32 - width_of_text) / 2.0,
            Alignment::Right => area.loc.x as f32 + area.size.w as f32 - width_of_text,
        }
        .max(area.loc.x as f32);
        let start_y =
            area.loc.y as f32 + vertical_offset as f32 + (area.size.h as f32 - line_height) / 2.0;
        let start_y = start_y.max(area.loc.y as f32);

        tracing::debug!(
            text,
            ?area,
            width_of_text,
            start_x,
            start_y,
            alignment = ?alignment,
            color = ?color,
            "drawing title text"
        );

        let shadow_offsets: &[(i32, i32)] = match shadow {
            TitleShadow::None => &[],
            TitleShadow::Under => &[(1, 1)],
            TitleShadow::Frame => &[(-1, 0), (0, -1), (1, 0), (0, 1)],
        };

        let clip = area;
        let mut draw_pass = |text_renderer: &mut Self, offset: (i32, i32), color: Rgba| {
            for run in buffer.layout_runs() {
                for glyph in run.glyphs {
                    let physical = glyph.physical(
                        (
                            start_x + offset.0 as f32,
                            start_y + run.line_y + offset.1 as f32,
                        ),
                        1.0,
                    );

                    text_renderer.swash.with_pixels(
                        &mut text_renderer.font_system,
                        physical.cache_key,
                        Color::rgba(color.r, color.g, color.b, color.a),
                        |x, y, pixel| {
                            let x = physical.x + x;
                            let y = physical.y + y;
                            if !clip.to_f64().contains((f64::from(x), f64::from(y))) {
                                return;
                            }
                            let alpha = (pixel.0 >> 24) as u8;
                            if alpha == 0 {
                                return;
                            }
                            let rgba = [
                                (pixel.0 & 0xFF) as u8,
                                ((pixel.0 >> 8) & 0xFF) as u8,
                                ((pixel.0 >> 16) & 0xFF) as u8,
                                alpha,
                            ];
                            canvas.blend(x, y, rgba);
                        },
                    );
                }
            }
        };

        for offset in shadow_offsets {
            let color = if shadow_color.a == 0 {
                continue;
            } else {
                shadow_color
            };
            draw_pass(self, *offset, color);
        }
        draw_pass(self, (0, 0), color);
    }

    fn layout(&mut self, font: &TitleFont, text: &str) -> Buffer {
        let metrics = Metrics::new(font.size, font.size * 1.2);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        // No wrapping: the titlebar clips instead. The width has to be a finite
        // value, `f32::MAX` makes cosmic-text compute a nonsense line width (and
        // the text then clamps to the left edge of its area).
        buffer.set_size(
            &mut self.font_system,
            Some(TEXT_LAYOUT_WIDTH),
            Some(f32::MAX),
        );

        let mut attrs = Attrs::new();
        if let Some(family) = font.family.as_deref() {
            attrs = attrs.family(Family::Name(family));
        }
        if font.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if font.italic {
            attrs = attrs.style(Style::Italic);
        }

        buffer.set_text(&mut self.font_system, text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }
}

/// Width of the laid out text: `LayoutRun::line_w` reports the buffer width for
/// unwrapped text in this version, so the glyph extents are used instead.
fn line_width(buffer: &Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| {
            run.glyphs
                .iter()
                .map(|glyph| glyph.x + glyph.w)
                .fold(0.0_f32, f32::max)
        })
        .fold(0.0_f32, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pango_like_specs() {
        let font = TitleFont::parse("Sans Bold 9");
        assert_eq!(font.family.as_deref(), Some("Sans"));
        assert!(font.bold);
        assert!((font.size - 12.0).abs() < 0.01);

        let font = TitleFont::parse("DejaVu Sans 10");
        assert_eq!(font.family.as_deref(), Some("DejaVu Sans"));
        assert!(!font.bold);
        assert!((font.size - 13.333).abs() < 0.01);

        let font = TitleFont::parse("Noto Sans Italic 8");
        assert_eq!(font.family.as_deref(), Some("Noto Sans"));
        assert!(font.italic);
    }

    #[test]
    fn empty_spec_keeps_defaults() {
        let default = TitleFont::default();
        let parsed = TitleFont::parse("");
        assert_eq!(parsed.family, default.family);
        assert_eq!(parsed.bold, default.bold);
        assert_eq!(parsed.size, default.size);
    }

    #[test]
    fn measures_and_draws_text() {
        let mut renderer = TextRenderer::new();
        let font = TitleFont::default();
        let width = renderer.measure(&font, "Mriya");
        assert!(width > 0.0, "text should have a width");

        let mut canvas = Canvas::new(200, 30);
        renderer.draw(
            &mut canvas,
            &font,
            "Mriya",
            Rectangle::new((0, 0).into(), (200, 30).into()),
            0,
            Alignment::Center,
            Rgba::rgb(255, 255, 255),
            TitleShadow::None,
            Rgba::TRANSPARENT,
        );
        // Text is centred vertically in the 30px tall area.
        assert!((0..30).any(|y| (0..200).any(|x| canvas.alpha(x, y) > 0)));
    }

    #[test]
    fn clips_outside_of_the_area() {
        let mut renderer = TextRenderer::new();
        let font = TitleFont::default();
        let mut canvas = Canvas::new(200, 30);
        renderer.draw(
            &mut canvas,
            &font,
            "Mriya",
            Rectangle::new((0, 0).into(), (4, 30).into()),
            0,
            Alignment::Left,
            Rgba::rgb(255, 255, 255),
            TitleShadow::None,
            Rgba::TRANSPARENT,
        );
        // Nothing beyond the clipped area.
        assert!((10..200).all(|x| canvas.alpha(x, 15) == 0));
    }
}
