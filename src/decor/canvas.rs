//! Small RGBA canvas and alpha mask helpers shared by the frame and text
//! composition.

use smithay::backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer};
use smithay::utils::Transform;

use super::theme::{palette::Rgba, ThemeImage};

/// A composed picture with straight alpha.
#[derive(Debug, Clone)]
pub(crate) struct Canvas {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pixels: Vec<u8>,
}

impl Canvas {
    pub(crate) fn new(width: i32, height: i32) -> Self {
        let (width, height) = (width.max(0), height.max(0));
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    pub(crate) fn alpha(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        self.pixels[((y * self.width + x) * 4 + 3) as usize]
    }

    /// Source-over one pixel.
    pub(crate) fn blend(&mut self, x: i32, y: i32, src: [u8; 4]) {
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

    pub(crate) fn blit(&mut self, source: &ThemeImage, x: i32, y: i32) {
        for sy in 0..source.height {
            for sx in 0..source.width {
                if let Some(color) = source.pixel(sx, sy) {
                    self.blend(x + sx, y + sy, color.to_array());
                }
            }
        }
    }

    /// Repeat `source` to fill a rectangle, like cairo's `EXTEND_REPEAT`.
    pub(crate) fn blit_tiled(
        &mut self,
        source: &ThemeImage,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
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
    pub(crate) fn blit_stretched(
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
    pub(crate) fn to_buffer(&self) -> MemoryRenderBuffer {
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

    pub(crate) fn mask(&self) -> AlphaMask {
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

pub(crate) fn mix(from: Option<Rgba>, to: Option<Rgba>, ratio: f64) -> Option<[u8; 4]> {
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
pub(crate) struct AlphaMask {
    pub(crate) width: i32,
    pub(crate) height: i32,
    bits: Vec<u64>,
}

impl AlphaMask {
    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return false;
        }
        let index = (y * self.width + x) as usize;
        self.bits[index / 64] & (1 << (index % 64)) != 0
    }
}
