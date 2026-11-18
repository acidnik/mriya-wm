//! Application icons for the titlebar.
//!
//! Wayland toplevels carry no icon, so the icon is looked up the freedesktop way:
//! `app_id` → `<app_id>.desktop` → `Icon=` → the theme icon (PNG or SVG), scaled
//! to the titlebar. xfwm4 shows it in the menu button, which is where Mriya puts
//! it as well.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use tracing::debug;

use super::theme::ThemeImage;

/// Icon size we rasterise to; the titlebar scales it down if needed.
const ICON_SIZE: u32 = 48;

/// Looked-up icons, keyed by app-id (including misses, which are common for
/// windows that only set a title).
pub struct IconCache {
    data_dirs: Vec<PathBuf>,
    icons: HashMap<String, Option<Arc<ThemeImage>>>,
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new()
    }
}

impl IconCache {
    pub fn new() -> Self {
        Self {
            data_dirs: data_dirs(),
            icons: HashMap::new(),
        }
    }

    /// Icon for `app_id`, loaded on first use.
    pub fn get(&mut self, app_id: &str) -> Option<Arc<ThemeImage>> {
        if app_id.is_empty() {
            return None;
        }
        if let Some(cached) = self.icons.get(app_id) {
            return cached.clone();
        }

        let icon = self.load(app_id).map(Arc::new);
        if icon.is_none() {
            debug!(app_id, "no icon found");
        }
        self.icons.insert(app_id.to_string(), icon.clone());
        icon
    }

    fn load(&self, app_id: &str) -> Option<ThemeImage> {
        let icon_name = self.icon_name(app_id)?;
        self.icon_image(&icon_name)
    }

    /// Icon name from the application's desktop file.
    fn icon_name(&self, app_id: &str) -> Option<String> {
        let candidate = format!("{app_id}.desktop");
        for dir in &self.data_dirs {
            let path = dir.join("applications").join(&candidate);
            if let Some(name) = read_icon_name(&path) {
                return Some(name);
            }
        }

        // Fall back to a `StartupWMClass` match, which is how applications with a
        // different desktop file name are found.
        for dir in &self.data_dirs {
            let Ok(entries) = std::fs::read_dir(dir.join("applications")) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                    continue;
                }
                if read_wm_class(&path).as_deref() == Some(app_id) {
                    return read_icon_name(&path);
                }
            }
        }

        None
    }

    /// Rasterised icon by name, PNG or SVG.
    fn icon_image(&self, name: &str) -> Option<ThemeImage> {
        for dir in &self.data_dirs {
            let bases = [dir.join("icons"), dir.join("pixmaps")];
            for base in bases {
                // `pixmaps` keeps flat `<name>.png`, icon themes nest by size.
                for extension in ["png", "svg", "xpm"] {
                    let flat = base.join(format!("{name}.{extension}"));
                    if let Some(image) = load_icon_file(&flat) {
                        return Some(image);
                    }
                }
                if let Some(found) = search_theme_dir(&base, name) {
                    return Some(found);
                }
            }
        }
        None
    }
}

/// `Icon=` value from a `.desktop` file.
fn read_icon_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Icon=") {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            // Absolute paths are allowed by the spec.
            return Some(value.to_string());
        }
    }
    None
}

fn read_wm_class(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("StartupWMClass=") {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Walk `<theme>/<size>/apps/<name>.<ext>`, preferring a size close to
/// [`ICON_SIZE`].
fn search_theme_dir(base: &Path, name: &str) -> Option<ThemeImage> {
    let Ok(themes) = std::fs::read_dir(base) else {
        return None;
    };

    let mut best: Option<(i32, PathBuf)> = None;
    for theme in themes.flatten() {
        let theme_path = theme.path();
        if !theme_path.is_dir() {
            continue;
        }
        let Ok(sizes) = std::fs::read_dir(&theme_path) else {
            continue;
        };
        for size_dir in sizes.flatten() {
            let size_name = size_dir.file_name().to_string_lossy().into_owned();
            let numeric: String = size_name
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let size = match numeric.parse::<i32>() {
                Ok(size) => size,
                // `scalable/` has no size: usable, but a crisp bitmap of the
                // right size is preferred.
                Err(_) if size_name.starts_with("scalable") => ICON_SIZE as i32 * 2,
                Err(_) => continue,
            };

            for category in ["apps", "categories", "devices", "places"] {
                for extension in ["png", "svg"] {
                    let path = size_dir
                        .path()
                        .join(category)
                        .join(format!("{name}.{extension}"));
                    if !path.is_file() {
                        continue;
                    }
                    // Prefer the smallest size that is still large enough.
                    let distance = if size >= ICON_SIZE as i32 {
                        size - ICON_SIZE as i32
                    } else {
                        (ICON_SIZE as i32 - size) * 4
                    };
                    if best.as_ref().map(|(b, _)| distance < *b).unwrap_or(true) {
                        best = Some((distance, path));
                    }
                }
            }
        }
    }

    best.and_then(|(_, path)| load_icon_file(&path))
}

fn load_icon_file(path: &Path) -> Option<ThemeImage> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_lowercase();
    match extension.as_str() {
        "png" => {
            let data = std::fs::read(path).ok()?;
            let image = image::load_from_memory(&data).ok()?.to_rgba8();
            Some(ThemeImage::new(
                image.width() as i32,
                image.height() as i32,
                image.into_raw(),
            ))
        }
        "svg" => {
            let data = std::fs::read(path).ok()?;
            rasterise_svg(&data, ICON_SIZE)
                .map(|(width, height, pixels)| ThemeImage::new(width, height, pixels))
        }
        // XPM icons are rare and would need the theme palette; skip them.
        _ => None,
    }
}

/// Rasterise an SVG at roughly `size` pixels.
fn rasterise_svg(data: &[u8], size: u32) -> Option<(i32, i32, Vec<u8>)> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(data, &options).ok()?;

    let svg_size = tree.size();
    let scale = size as f32 / svg_size.width().max(svg_size.height()).max(1.0);
    let width = (svg_size.width() * scale).round().max(1.0) as u32;
    let height = (svg_size.height() * scale).round().max(1.0) as u32;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia gives premultiplied RGBA, the theme images are straight alpha.
    let mut pixels = pixmap.take();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in pixel[..3].iter_mut() {
            *channel = ((u16::from(*channel) * 255) / u16::from(alpha)).min(255) as u8;
        }
    }

    Some((width as i32, height as i32, pixels))
}

/// XDG data directories that may contain applications and icons.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share"));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home));
    }
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mriya-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 8x8 opaque PNG encoded at runtime.
    fn write_png(path: &Path, size: u32) {
        let pixels = image::RgbaImage::from_pixel(size, size, image::Rgba([200, 30, 40, 255]));
        pixels.save(path).unwrap();
    }

    #[test]
    fn reads_icon_name_from_desktop_file() {
        let dir = temp_dir("desktop");
        std::fs::create_dir_all(dir.join("applications")).unwrap();
        std::fs::write(
            dir.join("applications/foot.desktop"),
            "[Desktop Entry]\nName=foot\nIcon=foot\n",
        )
        .unwrap();

        let cache = IconCache {
            data_dirs: vec![dir.clone()],
            icons: HashMap::new(),
        };
        assert_eq!(cache.icon_name("foot").as_deref(), Some("foot"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_png_icons_in_theme_sizes() {
        let dir = temp_dir("theme");
        let apps = dir.join("icons/hicolor/48x48/apps");
        std::fs::create_dir_all(&apps).unwrap();
        write_png(&apps.join("foot.png"), 48);
        std::fs::create_dir_all(dir.join("applications")).unwrap();
        std::fs::write(
            dir.join("applications/foot.desktop"),
            "[Desktop Entry]\nIcon=foot\n",
        )
        .unwrap();

        let mut cache = IconCache {
            data_dirs: vec![dir.clone()],
            icons: HashMap::new(),
        };
        let icon = cache.get("foot").expect("icon is found");
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(icon.pixel(0, 0).unwrap().r, 200);

        // Misses are cached too.
        assert!(cache.get("no-such-app").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn falls_back_to_startup_wm_class() {
        let dir = temp_dir("wmclass");
        std::fs::create_dir_all(dir.join("applications")).unwrap();
        std::fs::write(
            dir.join("applications/org.example.Terminal.desktop"),
            "[Desktop Entry]\nIcon=org.example.Terminal\nStartupWMClass=example-term\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("icons/hicolor/32x32/apps")).unwrap();
        write_png(
            &dir.join("icons/hicolor/32x32/apps/org.example.Terminal.png"),
            32,
        );

        let mut cache = IconCache {
            data_dirs: vec![dir.clone()],
            icons: HashMap::new(),
        };
        let icon = cache.get("example-term").expect("icon via StartupWMClass");
        assert_eq!(icon.width, 32);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
