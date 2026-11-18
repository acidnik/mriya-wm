//! `themerc` parsing: layout parameters and palette references.

use std::collections::HashMap;

use super::palette::{self, Palette, SYMBOLS};
use crate::decor::text::TitleFont;

/// Title alignment inside the titlebar, from `title_alignment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    Left,
    #[default]
    Center,
    Right,
}

/// Where the title text shadow is drawn, from `title_shadow_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleShadow {
    #[default]
    None,
    /// One pixel down and right, like `title_shadow_active=true`.
    Under,
    /// Around the text in all four directions, like `title_shadow_active=frame`.
    Frame,
}

/// Everything `themerc` may change in a theme.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeParams {
    pub palette: Palette,
    /// Extra border between the titlebar and the client content.
    pub frame_border_top: i32,
    pub button_offset: i32,
    pub button_spacing: i32,
    /// Button layout, e.g. `O|SHMC` (`|` separates the left and right groups).
    pub button_layout: String,
    pub full_width_title: bool,
    pub title_alignment: Alignment,
    pub title_horizontal_offset: i32,
    pub title_vertical_offset_active: i32,
    pub title_vertical_offset_inactive: i32,
    pub title_shadow_active: TitleShadow,
    pub title_shadow_inactive: TitleShadow,
    /// Offset used instead of `button_offset` when maximized.
    pub maximized_offset: i32,
    pub shadow_delta_x: i32,
    pub shadow_delta_y: i32,
    pub shadow_delta_width: i32,
    pub shadow_delta_height: i32,
    pub shadow_opacity: u8,
    pub show_app_icon: bool,
    /// Title font; an xfwm4 setting rather than a theme one, but it takes part in
    /// composing the titlebar, so it lives here.
    pub title_font: TitleFont,
}

impl Default for ThemeParams {
    /// Values xfwm4 uses when neither the theme nor the settings say anything.
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            frame_border_top: 0,
            button_offset: 0,
            button_spacing: 2,
            button_layout: "O|SHMC".to_string(),
            full_width_title: true,
            title_alignment: Alignment::Center,
            title_horizontal_offset: 0,
            title_vertical_offset_active: 0,
            title_vertical_offset_inactive: 0,
            title_shadow_active: TitleShadow::None,
            title_shadow_inactive: TitleShadow::None,
            maximized_offset: 0,
            shadow_delta_x: 0,
            shadow_delta_y: 0,
            shadow_delta_width: 0,
            shadow_delta_height: 0,
            shadow_opacity: 0,
            show_app_icon: false,
            title_font: TitleFont::default(),
        }
    }
}

impl ThemeParams {
    /// Apply a `themerc` on top of the defaults (with the palette preset from
    /// the config already in place).
    pub fn apply(&mut self, text: &str) {
        let values = parse_lines(text);
        self.apply_values(&values);
    }

    fn apply_values(&mut self, values: &HashMap<&str, &str>) {
        resolve_palette(values, &mut self.palette);

        for (key, value) in values {
            match *key {
                "frame_border_top" => self.frame_border_top = int(value, self.frame_border_top),
                "button_offset" => self.button_offset = int(value, self.button_offset),
                "button_spacing" => self.button_spacing = int(value, self.button_spacing),
                "button_layout" => self.button_layout = (*value).to_string(),
                "full_width_title" => self.full_width_title = boolean(value, self.full_width_title),
                "title_alignment" => {
                    self.title_alignment = match value.to_ascii_lowercase().as_str() {
                        "left" => Alignment::Left,
                        "right" => Alignment::Right,
                        _ => Alignment::Center,
                    }
                }
                "title_horizontal_offset" => {
                    self.title_horizontal_offset = int(value, self.title_horizontal_offset)
                }
                "title_vertical_offset_active" => {
                    self.title_vertical_offset_active =
                        int(value, self.title_vertical_offset_active)
                }
                "title_vertical_offset_inactive" => {
                    self.title_vertical_offset_inactive =
                        int(value, self.title_vertical_offset_inactive)
                }
                "title_shadow_active" => self.title_shadow_active = title_shadow(value),
                "title_shadow_inactive" => self.title_shadow_inactive = title_shadow(value),
                "maximized_offset" => self.maximized_offset = int(value, self.maximized_offset),
                "shadow_delta_x" => self.shadow_delta_x = int(value, self.shadow_delta_x),
                "shadow_delta_y" => self.shadow_delta_y = int(value, self.shadow_delta_y),
                "shadow_delta_width" => {
                    self.shadow_delta_width = int(value, self.shadow_delta_width)
                }
                "shadow_delta_height" => {
                    self.shadow_delta_height = int(value, self.shadow_delta_height)
                }
                "shadow_opacity" => {
                    self.shadow_opacity =
                        int(value, i32::from(self.shadow_opacity)).clamp(0, 100) as u8
                }
                "show_app_icon" => self.show_app_icon = boolean(value, self.show_app_icon),
                // Palette symbols are handled by `resolve_palette`, everything
                // else belongs to the xfwm4 settings, not to the theme.
                _ => {}
            }
        }
    }
}

/// `key=value` lines, comments and empty lines skipped.
fn parse_lines(text: &str) -> HashMap<&str, &str> {
    let mut values = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim(), value.trim());
        }
    }
    values
}

/// Colours may point at another symbol (`active_text_color=active_text_color_2`),
/// possibly through a couple of hops, so references are followed.
fn resolve_palette(values: &HashMap<&str, &str>, palette: &mut Palette) {
    for symbol in SYMBOLS {
        let Some(initial) = values.get(symbol).copied() else {
            continue;
        };

        let mut value = initial;
        for _ in 0..8 {
            if palette::parse_color(value).is_some() {
                break;
            }
            match values.get(value).copied() {
                Some(next) if next != value => value = next,
                _ => break,
            }
        }

        palette.set(symbol, value);
    }
}

fn int(value: &str, fallback: i32) -> i32 {
    value.trim().parse().unwrap_or(fallback)
}

fn boolean(value: &str, fallback: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => true,
        "false" | "0" | "no" => false,
        _ => fallback,
    }
}

fn title_shadow(value: &str) -> TitleShadow {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "under" | "1" => TitleShadow::Under,
        "frame" => TitleShadow::Frame,
        _ => TitleShadow::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_layout_values() {
        let mut params = ThemeParams::default();
        params.apply(
            "frame_border_top=5\nbutton_spacing=2\nbutton_offset=3\ntitle_alignment=left\n\
             title_horizontal_offset=4\ntitle_vertical_offset_active=1\nfull_width_title=false\n\
             title_shadow_active=true\ntitle_shadow_inactive=frame\nmaximized_offset=6\n\
             shadow_delta_x=-6\nshadow_delta_y=-8\nshadow_delta_width=-6\n\
             shadow_delta_height=4\nshadow_opacity=30\nshow_app_icon=true\n",
        );

        assert_eq!(params.frame_border_top, 5);
        assert_eq!(params.button_offset, 3);
        assert_eq!(params.button_spacing, 2);
        assert_eq!(params.title_alignment, Alignment::Left);
        assert_eq!(params.title_horizontal_offset, 4);
        assert_eq!(params.title_vertical_offset_active, 1);
        assert!(!params.full_width_title);
        assert_eq!(params.title_shadow_active, TitleShadow::Under);
        assert_eq!(params.title_shadow_inactive, TitleShadow::Frame);
        assert_eq!(params.maximized_offset, 6);
        assert_eq!(params.shadow_delta_y, -8);
        assert_eq!(params.shadow_opacity, 30);
        assert!(params.show_app_icon);
    }

    #[test]
    fn resolves_transitive_palette_references() {
        let mut params = ThemeParams::default();
        params.apply(
            "active_text_color=active_text_color_2\n\
             active_text_color_2=#ffffff\n\
             inactive_text_color=inactive_text_color_2\n\
             inactive_text_color_2=active_color_2\n\
             active_color_2=#336699\n",
        );

        assert_eq!(params.palette.get("active_text_color").unwrap().r, 255);
        let inactive = params.palette.get("inactive_text_color").unwrap();
        assert_eq!((inactive.r, inactive.g, inactive.b), (0x33, 0x66, 0x99));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let mut params = ThemeParams::default();
        let before = params.clone();
        params.apply("theme=Default\nsomething_else=42\n");
        assert_eq!(params, before);
    }

    #[test]
    fn broken_ints_fall_back_to_defaults() {
        let mut params = ThemeParams::default();
        params.apply("button_offset=not-a-number\n");
        assert_eq!(params.button_offset, 0);
    }
}
