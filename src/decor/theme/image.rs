//! Loading one theme part: XPM base with palette colours, PNG composited on top.

use std::path::Path;

use tracing::{debug, warn};

use super::palette::{Palette, Rgba};
use super::xpm;

/// Decoded picture of one theme part, straight (non-premultiplied) RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeImage {
    pub width: i32,
    pub height: i32,
    /// Row-major RGBA8.
    pub pixels: Vec<u8>,
}

#[allow(dead_code)]
impl ThemeImage {
    pub fn new(width: i32, height: i32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    /// Colour of one pixel; `None` outside of the image.
    pub fn pixel(&self, x: i32, y: i32) -> Option<Rgba> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        Some(Rgba {
            r: self.pixels[offset],
            g: self.pixels[offset + 1],
            b: self.pixels[offset + 2],
            a: self.pixels[offset + 3],
        })
    }

    /// Alpha of one pixel, zero outside of the image.
    pub fn alpha(&self, x: i32, y: i32) -> u8 {
        self.pixel(x, y).map(|color| color.a).unwrap_or(0)
    }

    /// Composite `top` over `self`, cropping to the smaller of the two, which is
    /// what `xfwmPixmapCompose` does in xfwm4.
    pub fn over(&mut self, top: &ThemeImage) {
        let width = self.width.min(top.width);
        let height = self.height.min(top.height);
        if width <= 0 || height <= 0 {
            return;
        }

        for y in 0..height {
            for x in 0..width {
                let offset = (y as usize * self.width as usize + x as usize) * 4;
                let source = (y as usize * top.width as usize + x as usize) * 4;
                blend_over(
                    &mut self.pixels[offset..offset + 4],
                    &top.pixels[source..source + 4],
                );
            }
        }
    }
}

/// Source-over blend for straight alpha, written back into `dst`.
fn blend_over(dst: &mut [u8], src: &[u8]) {
    let src_alpha = u16::from(src[3]);
    if src_alpha == 0 {
        return;
    }
    if src_alpha == 255 {
        dst.copy_from_slice(src);
        return;
    }

    let dst_alpha = u16::from(dst[3]);
    let inverse = 255 - src_alpha;
    let out_alpha = src_alpha + dst_alpha * inverse / 255;
    if out_alpha == 0 {
        dst.fill(0);
        return;
    }

    for channel in 0..3 {
        let numerator = u32::from(src[channel]) * u32::from(src_alpha)
            + u32::from(dst[channel]) * u32::from(dst_alpha) * u32::from(inverse) / 255;
        dst[channel] = (numerator / u32::from(out_alpha)) as u8;
    }
    dst[3] = out_alpha as u8;
}

/// Load `<dir>/<name>` as xfwm4 does: XPM first (with palette colours), then a
/// PNG composited over it.
pub fn load(dir: &Path, name: &str, palette: &Palette) -> Option<ThemeImage> {
    let mut image = match std::fs::read(dir.join(format!("{name}.xpm"))) {
        Ok(data) => match xpm::parse(&data, palette) {
            Ok(xpm) => Some(ThemeImage::new(xpm.width, xpm.height, xpm.pixels)),
            Err(err) => {
                warn!(part = name, %err, "failed to decode theme part");
                None
            }
        },
        Err(_) => None,
    };

    match std::fs::read(dir.join(format!("{name}.png"))) {
        Ok(data) => match decode_png(&data) {
            Ok(overlay) => match image.as_mut() {
                Some(base) => base.over(&overlay),
                None => image = Some(overlay),
            },
            Err(err) => warn!(part = name, %err, "failed to decode PNG overlay"),
        },
        Err(_) => {
            if image.is_none() {
                debug!(part = name, "theme part is missing");
            }
        }
    }

    image.filter(|image| !image.is_empty())
}

fn decode_png(data: &[u8]) -> Result<ThemeImage, String> {
    let image = image::load_from_memory(data).map_err(|err| err.to_string())?;
    let image = image.to_rgba8();
    Ok(ThemeImage::new(
        image.width() as i32,
        image.height() as i32,
        image.into_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_composites_transparent_pixels() {
        let mut base = ThemeImage::new(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
        let top = ThemeImage::new(2, 1, vec![0, 0, 0, 0, 255, 255, 255, 128]);
        base.over(&top);

        // First pixel keeps the base colour, second one gets a lighter blend.
        assert_eq!(&base.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(base.pixels[7], 255);
        assert!(base.pixels[4] > 0 && base.pixels[5] == 255);
    }

    #[test]
    fn over_crops_to_the_smaller_image() {
        let mut base = ThemeImage::new(2, 2, vec![255; 16]);
        let top = ThemeImage::new(1, 1, vec![0, 0, 0, 255]);
        base.over(&top);
        assert_eq!(&base.pixels[0..4], &[0, 0, 0, 255]);
        assert_eq!(&base.pixels[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn alpha_lookup_outside_is_zero() {
        let image = ThemeImage::new(1, 1, vec![10, 20, 30, 40]);
        assert_eq!(image.alpha(0, 0), 40);
        assert_eq!(image.alpha(-1, 0), 0);
        assert_eq!(image.alpha(1, 0), 0);
        assert_eq!(
            image.pixel(0, 0),
            Some(Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 40
            })
        );
    }
}
