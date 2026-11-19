//! Configuration in KDL.
//!
//! Section and node names mirror niri's configuration one to one, so an
//! existing niri config can be reused: sections that Mriya does not implement
//! yet are ignored (with a debug log) instead of failing the parse.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use kdl::{KdlDocument, KdlNode};
use regex::Regex;
use tracing::{debug, info, warn};

use crate::keybindings::{self, Binding, ModAlias};

/// Everything Mriya reads from `config.kdl`.
///
/// `Default` is what the parser starts from: no binds, so `from_document` can
/// install the built-in ones with the right `Mod` alias.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub cursor: CursorConfig,
    pub decorations: DecorationsConfig,
    pub keyboard: KeyboardConfig,
    /// Touchpad settings applied by libinput (udev backend only).
    pub touchpad: TouchpadConfig,
    /// Mouse settings applied by libinput (udev backend only).
    pub mouse: MouseConfig,
    /// Per-output overrides; an entry without a name is the default for all.
    pub outputs: Vec<OutputConfig>,
    /// Bindings in the order they were written; the first match wins.
    pub binds: Vec<Binding>,
    /// Commands started once the compositor is up.
    pub spawn_at_startup: Vec<Vec<String>>,
    /// Environment variables exported to the session and to spawned clients.
    pub environment: Vec<(String, String)>,
    /// Per-window rules, later rules win.
    pub window_rules: Vec<WindowRule>,
}

/// Server-side decoration settings (`decorations { ... }`).
#[derive(Debug, Clone, Default)]
pub struct DecorationsConfig {
    /// Theme name; `None` means the built-in `Default` theme.
    pub theme: Option<String>,
    /// Font for window titles, e.g. `Sans Bold 9` (xfwm4 takes it from its own
    /// settings, not from the theme).
    pub title_font: Option<String>,
    /// Overrides the theme's `button_layout`.
    pub button_layout: Option<String>,
    /// Palette overrides for the 24 xfwm4 colour symbols.
    pub palette: Vec<(String, String)>,
    /// Where a symbolic XPM colour gets its value from when the theme also
    /// draws a literal colour: the XPM itself (the theme's own look) or Mriya's
    /// built-in palette preset.
    pub palette_mode: PaletteMode,
    /// Draw drop shadows; `None` means "whatever the theme says".
    pub shadows: Option<bool>,
    /// Shadow opacity in percent, overriding the theme.
    pub shadow_opacity: Option<u8>,
}

/// Source of the symbolic XPM colours (xfwm4 takes them from the GTK theme,
/// which Mriya does not have).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaletteMode {
    /// The literal colour drawn in the XPM: what the theme author intended.
    #[default]
    Theme,
    /// Mriya's built-in palette preset, ignoring the literal colour.
    Preset,
}

/// What to do with decorations for a matching window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationPolicy {
    ServerSide,
    ClientSide,
}

/// One `match` node of a window rule: the conditions are "and"-ed, several
/// `match` nodes inside one rule are "or"-ed, like in niri.
#[derive(Debug, Clone, Default)]
pub struct RuleMatch {
    pub app_id: Option<Regex>,
    pub title: Option<Regex>,
}

impl RuleMatch {
    fn matches(&self, app_id: &str, title: &str) -> bool {
        self.app_id
            .as_ref()
            .map(|re| re.is_match(app_id))
            .unwrap_or(true)
            && self
                .title
                .as_ref()
                .map(|re| re.is_match(title))
                .unwrap_or(true)
    }
}

/// `window-rule { match app-id=...; decorations "client-side"; }`.
#[derive(Debug, Clone, Default)]
pub struct WindowRule {
    pub matches: Vec<RuleMatch>,
    pub decorations: Option<DecorationPolicy>,
}

impl WindowRule {
    pub fn matches_window(&self, app_id: &str, title: &str) -> bool {
        self.matches.is_empty() || self.matches.iter().any(|rule| rule.matches(app_id, title))
    }
}

#[derive(Debug, Clone, Default)]
pub struct CursorConfig {
    pub xcursor_theme: Option<String>,
    pub xcursor_size: Option<u32>,
}

/// `input { touchpad { ... } }`. Every option is optional: what is not written
/// keeps libinput's (or the desktop's) default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TouchpadConfig {
    /// Tap to click.
    pub tap: Option<bool>,
    pub natural_scroll: Option<bool>,
    /// Disable the touchpad while typing.
    pub dwt: Option<bool>,
    pub left_handed: Option<bool>,
    /// Acceleration curve speed, -1.0 .. 1.0.
    pub accel_speed: Option<f64>,
    pub scroll_method: Option<ScrollMethod>,
}

/// `input { mouse { ... } }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MouseConfig {
    pub natural_scroll: Option<bool>,
    pub left_handed: Option<bool>,
    pub accel_speed: Option<f64>,
}

/// Scroll method of a touchpad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMethod {
    TwoFinger,
    Edge,
    Button,
}

/// `output "<connector>" { scale 1.5 }`, or `output { scale 1.5 }` for all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputConfig {
    /// Connector name such as `eDP-1`; `None` means every output.
    pub name: Option<String>,
    /// Scale override; `None` lets the compositor derive it from the DPI.
    pub scale: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct KeyboardConfig {
    pub xkb: XkbSettings,
    pub repeat_delay: Option<u32>,
    pub repeat_rate: Option<u32>,
}

/// `input { keyboard { xkb { ... } } }`, kept as owned strings so it can be
/// turned into smithay's borrowed `XkbConfig` where it is needed.
#[derive(Debug, Clone, Default)]
pub struct XkbSettings {
    pub rules: Option<String>,
    pub model: Option<String>,
    pub layout: Option<String>,
    pub variant: Option<String>,
    pub options: Option<String>,
}

impl XkbSettings {
    /// Borrow these settings as smithay's keyboard configuration.
    pub fn to_smithay(&self) -> smithay::input::keyboard::XkbConfig<'_> {
        smithay::input::keyboard::XkbConfig {
            rules: self.rules.as_deref().unwrap_or(""),
            model: self.model.as_deref().unwrap_or(""),
            layout: self.layout.as_deref().unwrap_or(""),
            variant: self.variant.as_deref().unwrap_or(""),
            options: self.options.clone(),
        }
    }
}

/// `$XDG_CONFIG_HOME/mriya/config.kdl`, falling back to `~/.config/...`.
pub fn default_path() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("mriya").join("config.kdl"))
}

/// Load the config, falling back to defaults when the file is missing or
/// broken. The compositor must keep running in both cases.
pub fn load_or_default(path: Option<&Path>, mod_alias: ModAlias) -> Config {
    // Built-in defaults still need the `Mod` alias, so go through the parser.
    let defaults = || from_document(&kdl::KdlDocument::new(), mod_alias);
    let Some(path) = path else {
        warn!("no config path available, using built-in defaults");
        return defaults();
    };

    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            info!(path = %path.display(), "no config file, using defaults");
            return defaults();
        }
        Err(err) => {
            warn!(path = %path.display(), %err, "cannot read config, using defaults");
            return defaults();
        }
    };

    match parse(&text, mod_alias) {
        Ok(config) => {
            info!(path = %path.display(), binds = config.binds.len(), "config loaded");
            config
        }
        Err(err) => {
            warn!(path = %path.display(), %err, "config is invalid, using defaults");
            defaults()
        }
    }
}

/// Load a config for a *reload*: unlike [`load_or_default`] a broken file keeps
/// the running configuration instead of silently falling back to defaults.
pub fn reload(path: &Path, mod_alias: ModAlias) -> Option<Config> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warn!(path = %path.display(), %err, "cannot read the config, keeping the current one");
            return None;
        }
    };

    match parse(&text, mod_alias) {
        Ok(config) => {
            info!(path = %path.display(), "config reloaded");
            Some(config)
        }
        Err(err) => {
            warn!(path = %path.display(), %err, "config is invalid, keeping the current one");
            None
        }
    }
}

/// Parse config text. niri configs are KDL v1, so parse that dialect.
pub fn parse(text: &str, mod_alias: ModAlias) -> Result<Config, kdl::KdlError> {
    let document = KdlDocument::parse_v1(text)?;
    Ok(from_document(&document, mod_alias))
}

fn from_document(document: &KdlDocument, mod_alias: ModAlias) -> Config {
    let mut config = Config {
        binds: Vec::new(),
        ..Config::default()
    };

    for node in document.nodes() {
        match node.name().value() {
            "cursor" => parse_cursor(node, &mut config.cursor),
            "decorations" => parse_decorations(node, &mut config.decorations),
            "input" => parse_input(node, &mut config),
            "binds" => parse_binds(node, &mut config.binds, mod_alias),
            "output" => parse_output(node, &mut config.outputs),
            "spawn-at-startup" => {
                let argv = string_args(node);
                if argv.is_empty() {
                    warn!("spawn-at-startup without a command is ignored");
                } else {
                    config.spawn_at_startup.push(argv);
                }
            }
            "environment" => parse_environment(node, &mut config.environment),
            "window-rule" => match parse_window_rule(node) {
                Some(rule) => config.window_rules.push(rule),
                None => warn!("window-rule without a usable condition is ignored"),
            },
            // Sections of niri's config that Mriya does not implement yet:
            // layout, output, window-rule, gestures, animations, screenshot-path,
            // prefer-no-csd, hotkey-overlay, xwayland-satellite, ...
            other => debug!(
                section = other,
                "config section is not supported yet, ignoring"
            ),
        }
    }

    if config.binds.is_empty() {
        debug!("no binds in the config, installing the default ones");
        config.binds = keybindings::default_binds(mod_alias);
    }

    config
}

fn parse_cursor(node: &KdlNode, cursor: &mut CursorConfig) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        match child.name().value() {
            "xcursor-theme" => cursor.xcursor_theme = first_string(child),
            "xcursor-size" => {
                cursor.xcursor_size = first_integer(child).and_then(|size| u32::try_from(size).ok())
            }
            other => debug!(node = other, "cursor option is not supported yet, ignoring"),
        }
    }
}

fn parse_touchpad(node: &KdlNode, touchpad: &mut TouchpadConfig) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        match child.name().value() {
            "tap" => touchpad.tap = first_bool(child),
            "natural-scroll" => touchpad.natural_scroll = first_bool(child),
            "dwt" => touchpad.dwt = first_bool(child),
            "left-handed" => touchpad.left_handed = first_bool(child),
            "accel-speed" => touchpad.accel_speed = first_float(child),
            "scroll-method" => {
                touchpad.scroll_method = match first_string(child).as_deref() {
                    Some("two-finger") => Some(ScrollMethod::TwoFinger),
                    Some("edge") => Some(ScrollMethod::Edge),
                    Some("button") => Some(ScrollMethod::Button),
                    other => {
                        warn!(value = ?other, "scroll-method must be two-finger, edge or button");
                        None
                    }
                }
            }
            other => debug!(
                option = other,
                "touchpad option is not supported yet, ignoring"
            ),
        }
    }
}

fn parse_mouse(node: &KdlNode, mouse: &mut MouseConfig) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        match child.name().value() {
            "natural-scroll" => mouse.natural_scroll = first_bool(child),
            "left-handed" => mouse.left_handed = first_bool(child),
            "accel-speed" => mouse.accel_speed = first_float(child),
            other => debug!(
                option = other,
                "mouse option is not supported yet, ignoring"
            ),
        }
    }
}

fn parse_output(node: &KdlNode, outputs: &mut Vec<OutputConfig>) {
    let mut output = OutputConfig {
        name: first_string(node),
        ..OutputConfig::default()
    };
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "scale" => output.scale = first_float(child).map(|scale| scale.clamp(0.5, 4.0)),
                other => debug!(
                    option = other,
                    "output option is not supported yet, ignoring"
                ),
            }
        }
    }
    outputs.push(output);
}

fn parse_decorations(node: &KdlNode, decorations: &mut DecorationsConfig) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        match child.name().value() {
            "theme" => decorations.theme = first_string(child),
            "shadows" => {
                decorations.shadows = first_bool(child);
            }
            "shadow-opacity" => {
                decorations.shadow_opacity =
                    first_integer(child).and_then(|value| u8::try_from(value.clamp(0, 100)).ok());
            }
            "title-font" => decorations.title_font = first_string(child),
            "palette-mode" => {
                match first_string(child)
                    .as_deref()
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("theme") => decorations.palette_mode = PaletteMode::Theme,
                    Some("preset") => decorations.palette_mode = PaletteMode::Preset,
                    other => warn!(
                        value = ?other,
                        "palette-mode is \"theme\" or \"preset\", keeping the default"
                    ),
                }
            }
            "button-layout" => decorations.button_layout = first_string(child),
            "palette" => {
                let Some(palette_children) = child.children() else {
                    continue;
                };
                for symbol in palette_children.nodes() {
                    match first_string(symbol) {
                        Some(value) => decorations
                            .palette
                            .push((symbol.name().value().to_string(), value)),
                        None => warn!(
                            symbol = symbol.name().value(),
                            "palette symbol without a value, ignoring"
                        ),
                    }
                }
            }
            other => debug!(
                option = other,
                "decoration option is not supported yet, ignoring"
            ),
        }
    }
}

fn parse_input(node: &KdlNode, config: &mut Config) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        match child.name().value() {
            "keyboard" => parse_keyboard(child, &mut config.keyboard),
            // Touchpad and mouse settings go to libinput, which only the udev
            // backend drives; the nested backend inherits the host's devices.
            "touchpad" => parse_touchpad(child, &mut config.touchpad),
            "mouse" => parse_mouse(child, &mut config.mouse),
            other => debug!(
                section = other,
                "input section is not supported yet, ignoring"
            ),
        }
    }
}

fn parse_keyboard(node: &KdlNode, keyboard: &mut KeyboardConfig) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        match child.name().value() {
            "xkb" => {
                let Some(xkb_children) = child.children() else {
                    continue;
                };
                for setting in xkb_children.nodes() {
                    let Some(value) = first_string(setting) else {
                        continue;
                    };
                    match setting.name().value() {
                        "rules" => keyboard.xkb.rules = Some(value),
                        "model" => keyboard.xkb.model = Some(value),
                        "layout" => keyboard.xkb.layout = Some(value),
                        "variant" => keyboard.xkb.variant = Some(value),
                        "options" => keyboard.xkb.options = Some(value),
                        other => debug!(setting = other, "xkb option is not supported, ignoring"),
                    }
                }
            }
            "repeat-delay" => keyboard.repeat_delay = first_integer(child).and_then(to_u32),
            "repeat-rate" => keyboard.repeat_rate = first_integer(child).and_then(to_u32),
            other => debug!(
                option = other,
                "keyboard option is not supported yet, ignoring"
            ),
        }
    }
}

fn parse_binds(node: &KdlNode, binds: &mut Vec<Binding>, mod_alias: ModAlias) {
    let Some(children) = node.children() else {
        return;
    };

    for child in children.nodes() {
        let chord = child.name().value();
        // niri allows properties on the binding node itself, e.g. `repeat=false`;
        // they are not implemented yet.
        for entry in child.entries() {
            if let Some(name) = entry.name() {
                debug!(chord, property = %name.value(), "bind property is not supported yet, ignoring");
            }
        }

        let (mods, keysym) = match keybindings::parse_chord(chord, mod_alias) {
            Ok(parsed) => parsed,
            Err(err) => {
                warn!(chord, %err, "invalid key binding, ignoring");
                continue;
            }
        };

        let Some(actions_document) = child.children() else {
            warn!(chord, "key binding without actions, ignoring");
            continue;
        };

        let mut actions = Vec::new();
        for action_node in actions_document.nodes() {
            match keybindings::parse_action(action_node) {
                Ok(action) => actions.push(action),
                Err(err) => warn!(chord, %err, "unsupported action, ignoring"),
            }
        }
        if actions.is_empty() {
            continue;
        }

        binds.push(Binding {
            mods,
            keysym,
            actions,
        });
    }
}

fn parse_window_rule(node: &KdlNode) -> Option<WindowRule> {
    let children = node.children()?;
    let mut rule = WindowRule::default();

    for child in children.nodes() {
        match child.name().value() {
            "match" => {
                let mut condition = RuleMatch::default();
                for entry in child.entries() {
                    let Some(name) = entry.name() else {
                        continue;
                    };
                    let Some(pattern) = entry.value().as_string() else {
                        continue;
                    };
                    let compiled = match Regex::new(pattern) {
                        Ok(regex) => regex,
                        Err(err) => {
                            warn!(pattern, %err, "invalid regular expression in window-rule");
                            continue;
                        }
                    };
                    match name.value() {
                        "app-id" => condition.app_id = Some(compiled),
                        "title" => condition.title = Some(compiled),
                        other => debug!(other, "unsupported match property, ignoring"),
                    }
                }
                if condition.app_id.is_some() || condition.title.is_some() {
                    rule.matches.push(condition);
                }
            }
            "decorations" => {
                rule.decorations = match first_string(child).as_deref() {
                    Some("client-side") => Some(DecorationPolicy::ClientSide),
                    Some("server-side") => Some(DecorationPolicy::ServerSide),
                    Some(other) => {
                        warn!(value = other, "unknown decorations value in window-rule");
                        None
                    }
                    None => None,
                }
            }
            other => debug!(
                option = other,
                "window-rule option is not supported yet, ignoring"
            ),
        }
    }

    if rule.matches.is_empty() && rule.decorations.is_none() {
        None
    } else {
        Some(rule)
    }
}

fn parse_environment(node: &KdlNode, environment: &mut Vec<(String, String)>) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        let name = child.name().value().to_string();
        match first_string(child) {
            Some(value) => environment.push((name, value)),
            None => warn!(name, "environment variable without a value, ignoring"),
        }
    }
}

/// All positional string arguments of a node.
fn string_args(node: &KdlNode) -> Vec<String> {
    node.entries()
        .iter()
        .filter(|entry| entry.name().is_none())
        .filter_map(|entry| entry.value().as_string().map(str::to_string))
        .collect()
}

fn first_string(node: &KdlNode) -> Option<String> {
    node.entries()
        .iter()
        .find(|entry| entry.name().is_none())
        .and_then(|entry| entry.value().as_string())
        .map(str::to_string)
}

fn first_float(node: &KdlNode) -> Option<f64> {
    node.entries()
        .iter()
        .find(|entry| entry.name().is_none())
        .and_then(|entry| entry.value().as_float())
}

fn first_bool(node: &KdlNode) -> Option<bool> {
    node.entries()
        .iter()
        .find(|entry| entry.name().is_none())
        .and_then(|entry| entry.value().as_bool())
}

fn first_integer(node: &KdlNode) -> Option<i128> {
    node.entries()
        .iter()
        .find(|entry| entry.name().is_none())
        .and_then(|entry| entry.value().as_integer())
}

fn to_u32(value: i128) -> Option<u32> {
    u32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_niri_style_config() {
        let text = r##"
cursor {
    xcursor-theme "Horse-Red"
    xcursor-size 38
    hide-when-typing
}

input {
    keyboard {
        xkb {
            layout "us,ru"
            options "grp:caps_toggle"
        }
        track-layout "window"
        repeat-delay 350
        repeat-rate 17
    }
    touchpad {
        tap
    }
}

layout {
    gaps 4
}

decorations {
    theme "Default"
    title-font "Sans Bold 9"
    shadows true
    shadow-opacity 30
    palette {
        active_color_2 "#336699"
        active_text_color "#ffffff"
    }
}

environment {
    XDG_MENU_PREFIX "arch-"
}

spawn-at-startup "waybar"
spawn-at-startup "fish" "-c" "echo hi"

window-rule {
    match app-id=r#"^org\.gnome\."#
    decorations "client-side"
}

window-rule {
    match app-id="firefox"
    match title="Picture-in-Picture"
    decorations "server-side"
}

binds {
    Mod+q { close-window; }
    Mod+Escape { quit; }
    Mod+T { spawn "foot"; }
    Mod+Shift+T { spawn-sh "foot -e htop"; }
    Mod+F { fullscreen-window; }
    XF86AudioPlay { spawn "playerctl" "play-pause"; }
}
"##;
        let config = parse(text, ModAlias::Alt).expect("config parses");

        assert_eq!(config.cursor.xcursor_theme.as_deref(), Some("Horse-Red"));
        assert_eq!(config.cursor.xcursor_size, Some(38));
        assert_eq!(config.keyboard.xkb.layout.as_deref(), Some("us,ru"));
        assert_eq!(
            config.keyboard.xkb.options.as_deref(),
            Some("grp:caps_toggle")
        );
        assert_eq!(config.keyboard.repeat_delay, Some(350));
        assert_eq!(config.keyboard.repeat_rate, Some(17));
        assert_eq!(
            config.environment,
            vec![("XDG_MENU_PREFIX".to_string(), "arch-".to_string())]
        );
        assert_eq!(
            config.spawn_at_startup,
            vec![
                vec!["waybar".to_string()],
                vec!["fish".to_string(), "-c".to_string(), "echo hi".to_string()],
            ]
        );
        assert_eq!(config.binds.len(), 6);
        assert_eq!(config.decorations.theme.as_deref(), Some("Default"));
        assert_eq!(config.decorations.shadows, Some(true));
        assert_eq!(config.decorations.shadow_opacity, Some(30));
        assert_eq!(
            config.decorations.title_font.as_deref(),
            Some("Sans Bold 9")
        );
        assert_eq!(config.window_rules.len(), 2);
        assert!(config.window_rules[0].matches_window("org.gnome.Nautilus", "Files"));
        assert!(!config.window_rules[0].matches_window("foot", "foot"));
        assert_eq!(
            config.window_rules[0].decorations,
            Some(DecorationPolicy::ClientSide)
        );
        // Several `match` nodes in one rule are alternatives (OR).
        assert!(config.window_rules[1].matches_window("firefox", "Mozilla"));
        assert!(config.window_rules[1].matches_window("foot", "Picture-in-Picture"));
        assert!(!config.window_rules[1].matches_window("foot", "foot"));
        assert_eq!(
            config.window_rules[1].decorations,
            Some(DecorationPolicy::ServerSide)
        );
        assert_eq!(
            config.decorations.palette,
            vec![
                ("active_color_2".to_string(), "#336699".to_string()),
                ("active_text_color".to_string(), "#ffffff".to_string()),
            ]
        );
    }

    #[test]
    fn unknown_sections_do_not_break_the_parse() {
        let text = r##"
overview {
    zoom 0.5
}
window-rule {
    match app-id="foot"
    open-floating true
}
binds {
    Mod+q { close-window; }
}
"##;
        let config = parse(text, ModAlias::Alt).expect("unknown sections are ignored");
        assert_eq!(config.binds.len(), 1);
    }

    #[test]
    fn broken_config_is_an_error() {
        assert!(parse("binds { Mod+q { close-window; ", ModAlias::Alt).is_err());
    }

    #[test]
    fn empty_config_gets_default_binds() {
        let config = parse("", ModAlias::Alt).expect("empty config parses");
        assert_eq!(
            config.binds,
            keybindings::default_binds(crate::keybindings::ModAlias::Alt)
        );
    }
}
