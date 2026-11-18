//! XPM decoder.
//!
//! xfwm4 reads the XPM of every theme part first, with a private parser, because
//! XPM allows symbolic colours (`"c #FFFFFF s active_hilight_2"`) that get
//! replaced by the palette. The PNG/SVG of the same part is then composited on
//! top of that base (see [`super::image`]).

use tracing::warn;

use super::palette::{Palette, Rgba};

/// Decoded XPM picture with straight (non-premultiplied) RGBA pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XpmImage {
    pub width: i32,
    pub height: i32,
    /// Row-major RGBA8.
    pub pixels: Vec<u8>,
}

/// Reasons an XPM cannot be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XpmError {
    /// No `"width height colors chars"` header was found.
    NotXpm,
    /// The header or the colour table does not match the pixel data.
    Malformed(&'static str),
}

impl std::fmt::Display for XpmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XpmError::NotXpm => write!(f, "not an XPM file"),
            XpmError::Malformed(what) => write!(f, "malformed XPM: {what}"),
        }
    }
}

/// Decode an XPM picture, substituting symbolic colours from `palette`.
pub fn parse(input: &[u8], palette: &Palette) -> Result<XpmImage, XpmError> {
    let literals = string_literals(input);
    let mut literals = literals.into_iter();

    let header = literals.next().ok_or(XpmError::NotXpm)?;
    let mut values = header.split_whitespace();
    let width: i32 = values
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or(XpmError::NotXpm)?;
    let height: i32 = values
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or(XpmError::NotXpm)?;
    let color_count: usize = values
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or(XpmError::NotXpm)?;
    let chars_per_pixel: usize = values
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or(XpmError::NotXpm)?;

    if width <= 0 || height <= 0 || chars_per_pixel == 0 || chars_per_pixel > 4 {
        return Err(XpmError::Malformed("unsupported dimensions"));
    }

    let mut colors: Vec<(String, Rgba)> = Vec::with_capacity(color_count);
    for _ in 0..color_count {
        let spec = literals
            .next()
            .ok_or(XpmError::Malformed("colour table is short"))?;
        colors.push(parse_color_spec(&spec, chars_per_pixel, palette));
    }

    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height {
        let row = literals
            .next()
            .ok_or(XpmError::Malformed("not enough pixel rows"))?;
        if row.len() < width as usize * chars_per_pixel {
            return Err(XpmError::Malformed("pixel row is short"));
        }

        for x in 0..width {
            let start = x as usize * chars_per_pixel;
            let code = &row[start..start + chars_per_pixel];
            let color = colors
                .iter()
                .find(|(name, _)| name == code)
                .map(|(_, color)| *color)
                .unwrap_or(Rgba::TRANSPARENT);

            let offset = (y as usize * width as usize + x as usize) * 4;
            pixels[offset..offset + 4].copy_from_slice(&color.to_array());
        }
    }

    Ok(XpmImage {
        width,
        height,
        pixels,
    })
}

/// C string literals of an XPM file, in order.
fn string_literals(input: &[u8]) -> Vec<String> {
    let mut literals = Vec::new();
    let mut current = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for &byte in input {
        if !in_string {
            if byte == b'"' {
                in_string = true;
                current.clear();
            }
            continue;
        }

        if escaped {
            // C escapes, at least the ones XPM files actually use.
            current.push(match byte {
                b't' => b'\t',
                b'n' => b'\n',
                b'r' => b'\r',
                other => other,
            });
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'"' => {
                in_string = false;
                if let Ok(literal) = String::from_utf8(current.clone()) {
                    literals.push(literal);
                }
            }
            _ => current.push(byte),
        }
    }

    literals
}

/// `"code c <colour> [s <symbol>]"` (the code part may contain spaces).
fn parse_color_spec(spec: &str, chars_per_pixel: usize, palette: &Palette) -> (String, Rgba) {
    if spec.len() < chars_per_pixel {
        return (spec.to_string(), Rgba::TRANSPARENT);
    }
    let code = spec[..chars_per_pixel].to_string();
    let rest = &spec[chars_per_pixel..];
    let tokens: Vec<&str> = rest.split_whitespace().collect();

    // Keys in an XPM colour line: c (colour), s (symbolic name), m/g (mono/grey).
    let value_after = |key: &str| {
        tokens
            .iter()
            .position(|token| *token == key)
            .and_then(|index| tokens.get(index + 1))
            .copied()
    };

    if let Some(symbol) = value_after("s") {
        if let Some(color) = palette.get(symbol) {
            return (code, color);
        }
    }

    match value_after("c") {
        Some(value) => match super::palette::parse_color(value) {
            Some(color) => (code, color),
            None => {
                warn!(value, "unparsable XPM colour, using black");
                (code, Rgba::rgb(0, 0, 0))
            }
        },
        None => (code, Rgba::TRANSPARENT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xpm(body: &str) -> Vec<u8> {
        body.as_bytes().to_vec()
    }

    #[test]
    fn decodes_pixels_and_transparency() {
        let source = xpm(r##"/* XPM */
static char * test_xpm[] = {
"2 2 3 1",
"  c None",
". c #ff0000",
"# c #0000ff",
".#",
"#.",
"};"##);
        let image = parse(&source, &Palette::default()).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(&image.pixels[4..8], &[0, 0, 255, 255]);
        assert_eq!(&image.pixels[8..12], &[0, 0, 255, 255]);
        assert_eq!(&image.pixels[12..16], &[255, 0, 0, 255]);
    }

    #[test]
    fn symbolic_colours_come_from_the_palette() {
        let source = xpm(r##"/* XPM */
static char * test_xpm[] = {
"1 1 1 1",
"+ c #FFFFFF s active_hilight_2",
"+",
"};"##);
        let palette = Palette::default();
        let expected = palette.get("active_hilight_2").unwrap();
        let image = parse(&source, &palette).unwrap();
        assert_eq!(&image.pixels[0..4], &expected.to_array());
    }

    #[test]
    fn works_with_two_characters_per_pixel() {
        let source = xpm(r##"/* XPM */
static char * test_xpm[] = {
"2 1 2 2",
". \tc #00ff00",
"  \tc None",
".   ",
"};"##);
        let image = parse(&source, &Palette::default()).unwrap();
        assert_eq!(&image.pixels[0..4], &[0, 255, 0, 255]);
        assert_eq!(&image.pixels[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn named_colours_are_understood() {
        let source = xpm(r##"static char * test_xpm[] = {
"1 1 1 1",
". c gray50",
".",
"};"##);
        let image = parse(&source, &Palette::default()).unwrap();
        assert_eq!(&image.pixels[0..4], &[127, 127, 127, 255]);
    }

    #[test]
    fn rejects_non_xpm_data() {
        assert_eq!(
            parse(b"not an xpm", &Palette::default()),
            Err(XpmError::NotXpm)
        );
    }
}
