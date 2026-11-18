//! xfwm4 themes: `themerc` parameters plus the image parts of the frame.
//!
//! A theme is a directory (`<theme>/xfwm4`) with `themerc` and images named
//! after frame parts: `left-active`, `title-3-inactive`, `close-prelight`, ...
//! Sizes of those images define the frame geometry, exactly like in xfwm4:
//! the titlebar height comes from `title-3-active` (or `title-active-stretch`),
//! border widths from `left-active`/`right-active`/`bottom-active`.

pub mod image;
pub mod palette;
pub mod themerc;
pub mod xpm;

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

use tracing::{debug, info, warn};

pub use image::ThemeImage;
pub use themerc::{Alignment, ThemeParams, TitleShadow};

/// Borders along the sides of the frame.
const SIDES: [&str; 3] = ["left", "right", "bottom"];
/// Corner images; the order matches the naming in themes.
pub const CORNERS: [&str; 4] = ["bottom-left", "bottom-right", "top-left", "top-right"];
/// Stretchable titlebar pieces.
const TITLES: [&str; 5] = ["title-1", "title-2", "title-3", "title-4", "title-5"];
/// Optional strip above the titlebar (used by very old themes).
const TOPS: [&str; 5] = ["top-1", "top-2", "top-3", "top-4", "top-5"];
/// Titlebar buttons.
const BUTTONS: [&str; 6] = ["menu", "stick", "shade", "hide", "maximize", "close"];
/// Button states, in xfwm4's order.
const BUTTON_STATES: [&str; 8] = [
    "active",
    "inactive",
    "prelight",
    "pressed",
    "toggled-active",
    "toggled-inactive",
    "toggled-prelight",
    "toggled-pressed",
];

/// A loaded theme: parameters, geometry sources and every present image part.
// Some of this API is consumed by the themed renderer in the next stage.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Theme {
    pub name: String,
    pub params: ThemeParams,
    parts: HashMap<String, ThemeImage>,
}

/// Why a theme could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    /// No directory with that name in any of the theme search paths.
    NotFound { name: String },
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::NotFound { name } => write!(f, "theme `{name}` was not found"),
        }
    }
}

impl std::error::Error for ThemeError {}

#[allow(dead_code)]
impl Theme {
    /// Load a theme by name from the search paths.
    pub fn load(name: &str, params: ThemeParams) -> Result<Self, ThemeError> {
        let dir = find_theme_dir(name).ok_or_else(|| ThemeError::NotFound {
            name: name.to_string(),
        })?;
        Ok(Self::load_from_dir(&dir, name, params))
    }

    /// Load a theme from an explicit directory (used by tests and by
    /// `--theme-dir`, should we add one).
    pub fn load_from_dir(dir: &Path, name: &str, mut params: ThemeParams) -> Self {
        match std::fs::read_to_string(dir.join("themerc")) {
            Ok(text) => params.apply(&text),
            Err(err) => debug!(path = %dir.display(), %err, "no themerc, using defaults"),
        }

        let palette = params.palette.clone();
        let mut parts = HashMap::new();
        let mut missing = 0usize;

        let mut load = |part: String| {
            match image::load(dir, &part, &palette) {
                Some(loaded) => {
                    parts.insert(part, loaded);
                }
                None => missing += 1,
            };
        };

        for state in ["active", "inactive"] {
            for side in SIDES {
                load(format!("{side}-{state}"));
                load(format!("{side}-{state}-stretch"));
            }
            for corner in CORNERS {
                load(format!("{corner}-{state}"));
                load(format!("{corner}-{state}-stretch"));
            }
            for title in TITLES {
                load(format!("{title}-{state}"));
            }
            for top in TOPS {
                load(format!("{top}-{state}"));
            }
            load(format!("title-{state}-stretch"));
        }
        for button in BUTTONS {
            for state in BUTTON_STATES {
                load(format!("{button}-{state}"));
            }
        }

        let theme = Self {
            name: name.to_string(),
            params,
            parts,
        };

        info!(
            theme = name,
            parts = theme.parts.len(),
            missing,
            titlebar = theme.title_height(true),
            borders = format!(
                "{}/{}/{}",
                theme.border_left(true),
                theme.border_right(true),
                theme.border_bottom(true)
            ),
            "theme loaded"
        );

        theme
    }

    /// Image of a part by its full name (`title-3-active`, `close-prelight`).
    pub fn part(&self, name: &str) -> Option<&ThemeImage> {
        self.parts.get(name)
    }

    /// Number of image parts that were found.
    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    /// State-dependent part, e.g. `state_part("left", true)` → `left-active`.
    pub fn state_part(&self, base: &str, active: bool) -> Option<&ThemeImage> {
        self.part(&format!("{base}-{}", state_name(active)))
    }

    /// Stretch variant of a side or corner part, if the theme has one.
    pub fn stretch_part(&self, base: &str, active: bool) -> Option<&ThemeImage> {
        self.part(&format!("{base}-{}-stretch", state_name(active)))
    }

    /// Stretch image for the whole titlebar (`title-active-stretch`).
    pub fn title_stretch(&self, active: bool) -> Option<&ThemeImage> {
        self.part(&format!("title-{}-stretch", state_name(active)))
    }

    /// Corner image (`top-left` and friends).
    pub fn corner(&self, corner: &str, active: bool) -> Option<&ThemeImage> {
        self.state_part(corner, active)
    }

    /// A side or corner taking the stretch variant into account, like xfwm4's
    /// `frameDecoration*` helpers.
    fn decorated_part(&self, base: &str, active: bool) -> Option<&ThemeImage> {
        self.stretch_part(base, active)
            .or_else(|| self.state_part(base, active))
    }

    /// Height of the titlebar.
    pub fn title_height(&self, active: bool) -> i32 {
        self.title_stretch(active)
            .map(|image| image.height)
            .or_else(|| self.state_part("title-3", active).map(|image| image.height))
            .unwrap_or(0)
    }

    pub fn border_left(&self, active: bool) -> i32 {
        self.decorated_part("left", active)
            .map(|image| image.width)
            .unwrap_or(0)
    }

    pub fn border_right(&self, active: bool) -> i32 {
        self.decorated_part("right", active)
            .map(|image| image.width)
            .unwrap_or(0)
    }

    pub fn border_bottom(&self, active: bool) -> i32 {
        self.decorated_part("bottom", active)
            .map(|image| image.height)
            .unwrap_or(0)
    }

    /// Button image for a state (`active`, `prelight`, `toggled-active`, ...).
    pub fn button(&self, button: &str, state: &str) -> Option<&ThemeImage> {
        self.part(&format!("{button}-{state}"))
    }
}

fn state_name(active: bool) -> &'static str {
    if active {
        "active"
    } else {
        "inactive"
    }
}

/// Directories that may contain themes, most specific first.
pub fn theme_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".themes"));
        paths.push(home.join(".local/share/themes"));
    }
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(data_home).join("themes"));
    }

    let data_dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        paths.push(PathBuf::from(dir).join("themes"));
    }

    paths
}

/// Directory with the theme's `xfwm4` files.
pub fn find_theme_dir(name: &str) -> Option<PathBuf> {
    for base in theme_search_paths() {
        let with_subdir = base.join(name).join("xfwm4");
        if with_subdir.is_dir() {
            return Some(with_subdir);
        }
        // Some themes put the files directly into the theme directory.
        let flat = base.join(name);
        if flat.join("themerc").is_file() || flat.join("top-left-active.xpm").is_file() {
            return Some(flat);
        }
    }
    None
}

/// Names of all themes visible in the search paths, sorted and deduplicated.
pub fn available_themes() -> Vec<String> {
    let mut names = Vec::new();
    for base in theme_search_paths() {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_theme = path.join("xfwm4").is_dir()
                || path.join("themerc").is_file()
                || path.join("top-left-active.xpm").is_file();
            if is_theme {
                if let Some(name) = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
        }
    }
    names.sort();
    names
}

/// Warn once when the configured theme is not available, and list a few
/// alternatives to help the user fix the config.
pub fn warn_missing_theme(name: &str) {
    let alternatives = available_themes();
    let sample: Vec<&str> = alternatives.iter().take(8).map(String::as_str).collect();
    warn!(
        theme = name,
        available = alternatives.len(),
        ?sample,
        "theme not found, keeping the flat frame colours"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write a small synthetic theme and return its directory.
    fn fixture_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mriya-theme-{name}-{}", std::process::id()));
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

        fs::write(dir.join("title-3-active.xpm"), xpm(8, 22, "#808080")).unwrap();
        fs::write(dir.join("title-3-inactive.xpm"), xpm(8, 20, "#606060")).unwrap();
        fs::write(
            dir.join("left-active.xpm"),
            xpm(3, 10, "s active_color_2").replace(
                "\". c s active_color_2\"",
                "\". c #000000 s active_color_2\"",
            ),
        )
        .unwrap();
        fs::write(dir.join("right-active.xpm"), xpm(4, 10, "#000000")).unwrap();
        fs::write(dir.join("bottom-active.xpm"), xpm(10, 5, "#000000")).unwrap();
        fs::write(dir.join("top-left-active.xpm"), xpm(6, 22, "None")).unwrap();
        fs::write(dir.join("close-active.xpm"), xpm(9, 9, "#ff0000")).unwrap();
        fs::write(dir.join("close-prelight.xpm"), xpm(9, 9, "#ff8000")).unwrap();
        fs::write(
            dir.join("themerc"),
            "frame_border_top=2\nbutton_spacing=0\ntitle_vertical_offset_active=3\n\
             active_text_color=active_color_2\nactive_color_2=#336699\n",
        )
        .unwrap();

        dir
    }

    #[test]
    fn loads_geometry_from_the_images() {
        let dir = fixture_dir("geometry");
        let theme = Theme::load_from_dir(&dir, "fixture", ThemeParams::default());

        assert_eq!(theme.title_height(true), 22);
        assert_eq!(theme.title_height(false), 20);
        assert_eq!(theme.border_left(true), 3);
        assert_eq!(theme.border_right(true), 4);
        assert_eq!(theme.border_bottom(true), 5);
        assert_eq!(theme.border_right(false), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn applies_themerc_parameters_and_palette() {
        let dir = fixture_dir("themerc");
        let theme = Theme::load_from_dir(&dir, "fixture", ThemeParams::default());

        assert_eq!(theme.params.frame_border_top, 2);
        assert_eq!(theme.params.button_spacing, 0);
        assert_eq!(theme.params.title_vertical_offset_active, 3);
        let text = theme.params.palette.get("active_text_color").unwrap();
        assert_eq!((text.r, text.g, text.b), (0x33, 0x66, 0x99));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn symbolic_colours_reach_the_images() {
        let dir = fixture_dir("symbolic");
        let mut params = ThemeParams::default();
        params.palette.set("active_color_2", "#112233");
        let theme = Theme::load_from_dir(&dir, "fixture", params);

        // Themeless folder overrides the config palette, the image uses it.
        let left = theme.state_part("left", true).expect("left border");
        assert_eq!(left.pixel(0, 0).unwrap().r, 0x33);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn buttons_and_missing_parts() {
        let dir = fixture_dir("buttons");
        let theme = Theme::load_from_dir(&dir, "fixture", ThemeParams::default());

        assert!(theme.button("close", "active").is_some());
        assert!(theme.button("close", "prelight").is_some());
        assert!(theme.button("close", "pressed").is_none());
        assert!(theme.button("stick", "active").is_none());
        assert!(theme.part_count() > 6);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_theme_reports_not_found() {
        assert!(matches!(
            Theme::load("definitely-not-a-theme", ThemeParams::default()),
            Err(ThemeError::NotFound { .. })
        ));
    }

    /// Loads every theme installed on the machine. Run with
    /// `cargo test -- --ignored` on a system with `/usr/share/themes`.
    #[test]
    #[ignore]
    fn loads_all_system_themes() {
        let themes = available_themes();
        assert!(!themes.is_empty(), "no themes installed");

        let mut broken = Vec::new();
        for name in &themes {
            match Theme::load(name, ThemeParams::default()) {
                Ok(theme) => {
                    if theme.title_height(true) == 0 {
                        broken.push(format!("{name}: no titlebar height"));
                    }
                }
                Err(err) => broken.push(format!("{name}: {err}")),
            }
        }
        assert!(broken.is_empty(), "broken themes: {broken:?}");
    }
}
