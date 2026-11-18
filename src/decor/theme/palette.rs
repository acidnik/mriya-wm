//! The 24 colour symbols xfwm4 themes refer to.
//!
//! Theme images and `themerc` use names like `active_color_2` instead of literal
//! colours; xfwm4 fills them from the running GTK theme (widget colours in the
//! `selected` and `insensitive` states). Mriya has no GTK, so it ships a preset
//! and lets the config override individual symbols.

use tracing::debug;

/// Symbol names in xfwm4's order (`Settings rc[]` in `settings.c`).
pub const SYMBOLS: [&str; 24] = [
    "active_text_color",
    "inactive_text_color",
    "active_text_shadow_color",
    "inactive_text_shadow_color",
    "active_border_color",
    "inactive_border_color",
    "active_color_1",
    "active_hilight_1",
    "active_shadow_1",
    "active_mid_1",
    "active_text_color_2",
    "active_color_2",
    "active_hilight_2",
    "active_shadow_2",
    "active_mid_2",
    "inactive_color_1",
    "inactive_hilight_1",
    "inactive_shadow_1",
    "inactive_mid_1",
    "inactive_text_color_2",
    "inactive_color_2",
    "inactive_hilight_2",
    "inactive_shadow_2",
    "inactive_mid_2",
];

/// Preset that approximates a classic XFCE look: bluish selected colours, a
/// grey window background and white text on the active titlebar.
const PRESET: [&str; 24] = [
    "#ffffff", // active_text_color
    "#a8a8a8", // inactive_text_color
    "#000000", // active_text_shadow_color
    "#000000", // inactive_text_shadow_color
    "#2f2f2f", // active_border_color
    "#2f2f2f", // inactive_border_color
    "#4a6f9c", // active_color_1
    "#6b8cb4", // active_hilight_1
    "#33507a", // active_shadow_1
    "#3f6390", // active_mid_1
    "#ffffff", // active_text_color_2
    "#4a6f9c", // active_color_2
    "#6b8cb4", // active_hilight_2
    "#33507a", // active_shadow_2
    "#3f6390", // active_mid_2
    "#d6d6d6", // inactive_color_1
    "#e8e8e8", // inactive_hilight_1
    "#a9a9a9", // inactive_shadow_1
    "#c4c4c4", // inactive_mid_1
    "#8a8a8a", // inactive_text_color_2
    "#d6d6d6", // inactive_color_2
    "#e8e8e8", // inactive_hilight_2
    "#a9a9a9", // inactive_shadow_2
    "#c4c4c4", // inactive_mid_2
];

/// Straight (non-premultiplied) RGBA colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// Palette of the 24 symbols.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    colors: [Rgba; 24],
}

impl Default for Palette {
    fn default() -> Self {
        let mut colors = [Rgba::TRANSPARENT; 24];
        for (index, value) in PRESET.iter().enumerate() {
            colors[index] = parse_color(value).unwrap_or(Rgba::TRANSPARENT);
        }
        Self { colors }
    }
}

impl Palette {
    /// Colour bound to a symbol name.
    pub fn get(&self, symbol: &str) -> Option<Rgba> {
        SYMBOLS
            .iter()
            .position(|name| *name == symbol)
            .map(|index| self.colors[index])
    }

    /// Override a symbol from its config or themerc representation.
    pub fn set(&mut self, symbol: &str, value: &str) -> bool {
        let Some(index) = SYMBOLS.iter().position(|name| *name == symbol) else {
            return false;
        };
        match parse_color(value) {
            Some(color) => {
                self.colors[index] = color;
                true
            }
            None => {
                debug!(
                    symbol,
                    value, "unparsable palette colour, keeping the preset"
                );
                false
            }
        }
    }
}

/// Parse `#rgb`, `#rrggbb`, `#rrrrggggbbbb`, `None` and a few colour names.
pub fn parse_color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("none") {
        return Some(Rgba::TRANSPARENT);
    }
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    parse_named(value.to_ascii_lowercase().as_str())
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let digits = |value: &str| u8::from_str_radix(value, 16).ok();
    match hex.len() {
        3 => {
            let channel = |index: usize| {
                let digit = digits(&hex[index..index + 1])?;
                Some(digit * 17)
            };
            Some(Rgba::rgb(channel(0)?, channel(1)?, channel(2)?))
        }
        6 => {
            let channel = |index: usize| digits(&hex[index..index + 2]);
            Some(Rgba::rgb(channel(0)?, channel(2)?, channel(4)?))
        }
        12 => {
            // 16 bits per channel, xfwm4/gdk_pixbuf accepts this form too.
            let channel = |index: usize| {
                let value = u16::from_str_radix(&hex[index..index + 4], 16).ok()?;
                Some((value >> 8) as u8)
            };
            Some(Rgba::rgb(channel(0)?, channel(4)?, channel(8)?))
        }
        _ => None,
    }
}

fn parse_named(name: &str) -> Option<Rgba> {
    let fixed = match name {
        "black" => Some(Rgba::rgb(0, 0, 0)),
        "white" => Some(Rgba::rgb(255, 255, 255)),
        "red" => Some(Rgba::rgb(255, 0, 0)),
        "green" => Some(Rgba::rgb(0, 255, 0)),
        "blue" => Some(Rgba::rgb(0, 0, 255)),
        "yellow" => Some(Rgba::rgb(255, 255, 0)),
        "cyan" => Some(Rgba::rgb(0, 255, 255)),
        "magenta" => Some(Rgba::rgb(255, 0, 255)),
        "gray" | "grey" => Some(Rgba::rgb(190, 190, 190)),
        _ => None,
    };
    if fixed.is_some() {
        return fixed;
    }

    // `gray50`, `grey80` and friends.
    for prefix in ["gray", "grey"] {
        if let Some(percent) = name.strip_prefix(prefix) {
            if let Ok(percent) = percent.parse::<u32>() {
                if percent <= 100 {
                    let value = (percent * 255 / 100) as u8;
                    return Some(Rgba::rgb(value, value, value));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_is_complete() {
        let palette = Palette::default();
        for symbol in SYMBOLS {
            let color = palette.get(symbol).expect("every symbol has a colour");
            assert_eq!(color.a, 255);
        }
    }

    #[test]
    fn hex_forms_are_parsed() {
        assert_eq!(parse_color("#ffffff"), Some(Rgba::rgb(255, 255, 255)));
        assert_eq!(parse_color("#fff"), Some(Rgba::rgb(255, 255, 255)));
        assert_eq!(parse_color("#ffffffffffff"), Some(Rgba::rgb(255, 255, 255)));
        assert_eq!(parse_color("#102030"), Some(Rgba::rgb(0x10, 0x20, 0x30)));
    }

    #[test]
    fn named_and_transparent_colours_are_parsed() {
        assert_eq!(parse_color("None"), Some(Rgba::TRANSPARENT));
        assert_eq!(parse_color("black"), Some(Rgba::rgb(0, 0, 0)));
        assert_eq!(parse_color("gray50"), Some(Rgba::rgb(127, 127, 127)));
        assert_eq!(parse_color("not-a-colour"), None);
    }

    #[test]
    fn unknown_symbols_are_ignored() {
        let mut palette = Palette::default();
        assert!(!palette.set("nonsense", "#ffffff"));
        assert!(palette.set("active_color_2", "#123456"));
        assert_eq!(
            palette.get("active_color_2"),
            Some(Rgba::rgb(0x12, 0x34, 0x56))
        );
    }
}
