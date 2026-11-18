//! Cursor images: client-provided surfaces and XCursor theme icons.

use std::{collections::HashMap, fs::File, io::Read, sync::Mutex};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::render_elements_from_surface_tree,
                Kind,
            },
            gles::GlesRenderer,
        },
    },
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    utils::{Logical, Physical, Point, Scale, Transform},
    wayland::compositor::with_states,
};
use tracing::{debug, warn};
use xcursor::{
    parser::{parse_xcursor, Image},
    CursorTheme,
};

use crate::backend::element::NestedRenderElement;

/// Nominal cursor size in logical pixels when the environment says nothing.
const DEFAULT_SIZE: u32 = 24;
/// Icon names to fall back on when the theme has no icon for a requested name.
const FALLBACK_ICONS: [&str; 2] = ["default", "left_ptr"];

/// Cursor renderer with a cache of cursors uploaded to the GPU.
pub struct Cursor {
    theme: Option<CursorTheme>,
    theme_name: String,
    /// Nominal cursor size in logical pixels (`XCURSOR_SIZE`).
    size: u32,
    /// Uploaded cursors, keyed by icon name and pixel size.
    cache: HashMap<(String, u32), Option<LoadedCursor>>,
}

struct LoadedCursor {
    buffer: MemoryRenderBuffer,
    /// Hotspot in pixels of the already scaled buffer.
    hotspot: Point<i32, Logical>,
}

impl Cursor {
    pub fn new() -> Self {
        let theme_name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|size: &u32| *size > 0)
            .unwrap_or(DEFAULT_SIZE);

        let theme = CursorTheme::load(&theme_name);
        let theme = match Cursor::read_icon(&theme, "default") {
            Some(_) => {
                debug!(theme = %theme_name, size, "loaded cursor theme");
                Some(theme)
            }
            None => {
                warn!(theme = %theme_name, "no default cursor in the theme, falling back to the host cursor");
                None
            }
        };

        Self {
            theme,
            theme_name,
            size,
            cache: HashMap::new(),
        }
    }

    /// Cursor elements for `status`, with the top-left corner placed so that the
    /// hotspot lands on `position` (in logical output coordinates).
    pub fn render_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        position: Point<f64, Logical>,
        scale: f64,
        status: &CursorImageStatus,
    ) -> Vec<NestedRenderElement<GlesRenderer>> {
        match status {
            CursorImageStatus::Hidden => Vec::new(),
            CursorImageStatus::Named(icon) => {
                let pixel_size = ((f64::from(self.size) * scale).round() as u32).max(1);
                let Some(cursor) = self.loaded(icon.name(), pixel_size) else {
                    return Vec::new();
                };

                // The hotspot belongs to the already scaled buffer.
                let hotspot = Point::<f64, Physical>::from((
                    f64::from(cursor.hotspot.x),
                    f64::from(cursor.hotspot.y),
                ));
                let location: Point<i32, Physical> =
                    (position.to_physical(scale) - hotspot).to_i32_round();
                match MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    location.to_f64(),
                    &cursor.buffer,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                ) {
                    Ok(element) => vec![NestedRenderElement::Memory(element)],
                    Err(err) => {
                        warn!(%err, "failed to import cursor texture");
                        Vec::new()
                    }
                }
            }
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .map(|attributes| attributes.lock().unwrap().hotspot)
                })
                .unwrap_or_default();

                let location = (position - hotspot.to_f64())
                    .to_physical(scale)
                    .to_i32_round();
                render_elements_from_surface_tree(
                    renderer,
                    surface,
                    location,
                    Scale::from(scale),
                    1.0,
                    Kind::Cursor,
                )
                .into_iter()
                .map(NestedRenderElement::Surface)
                .collect()
            }
        }
    }

    fn loaded(&mut self, icon: &str, pixel_size: u32) -> Option<&LoadedCursor> {
        let key = (icon.to_string(), pixel_size);
        if !self.cache.contains_key(&key) {
            let cursor = self.load(icon, pixel_size);
            self.cache.insert(key.clone(), cursor);
        }
        self.cache.get(&key).and_then(|cursor| cursor.as_ref())
    }

    fn load(&self, icon: &str, pixel_size: u32) -> Option<LoadedCursor> {
        let theme = self.theme.as_ref()?;

        // Themes are not consistent about names, so try the requested one and
        // then the usual suspects.
        let mut candidates = vec![icon];
        candidates.extend(FALLBACK_ICONS);
        let images = candidates
            .iter()
            .find_map(|name| Cursor::read_icon(theme, name))?;

        let image = nearest_image(&images, pixel_size)?;
        // XCursor stores pixels as ARGB, byte order matches `Fourcc::Argb8888`.
        let buffer = MemoryRenderBuffer::from_slice(
            &image.pixels_rgba,
            Fourcc::Argb8888,
            (image.width as i32, image.height as i32),
            1,
            Transform::Normal,
            None,
        );
        tracing::debug!(
            icon,
            pixel_size,
            nominal = image.size,
            width = image.width,
            height = image.height,
            "loaded cursor"
        );
        Some(LoadedCursor {
            buffer,
            hotspot: (image.xhot as i32, image.yhot as i32).into(),
        })
    }

    fn read_icon(theme: &CursorTheme, name: &str) -> Option<Vec<Image>> {
        let path = theme.load_icon(name)?;
        let mut data = Vec::new();
        File::open(path).ok()?.read_to_end(&mut data).ok()?;
        let images = parse_xcursor(&data)?;
        if images.is_empty() {
            None
        } else {
            Some(images)
        }
    }

    /// Name of the theme in use, for logging.
    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// Whether named cursors can be drawn at all.
    pub fn has_theme(&self) -> bool {
        self.theme.is_some()
    }
}

/// Image of the size group closest to `pixel_size`; animated icons yield their
/// first frame.
fn nearest_image(images: &[Image], pixel_size: u32) -> Option<&Image> {
    let nearest = images
        .iter()
        .min_by_key(|image| (i64::from(image.size) - i64::from(pixel_size)).abs())?;
    images
        .iter()
        .find(|image| image.width == nearest.width && image.height == nearest.height)
}
