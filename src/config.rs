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
use tracing::{debug, info, warn};

use crate::keybindings::{self, Binding, ModAlias};

/// Everything Mriya reads from `config.kdl`.
#[derive(Debug, Clone)]
pub struct Config {
    pub cursor: CursorConfig,
    pub keyboard: KeyboardConfig,
    /// Bindings in the order they were written; the first match wins.
    pub binds: Vec<Binding>,
    /// Commands started once the compositor is up.
    pub spawn_at_startup: Vec<Vec<String>>,
    /// Environment variables exported to the session and to spawned clients.
    pub environment: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct CursorConfig {
    pub xcursor_theme: Option<String>,
    pub xcursor_size: Option<u32>,
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

impl Default for Config {
    fn default() -> Self {
        Self {
            cursor: CursorConfig::default(),
            keyboard: KeyboardConfig::default(),
            binds: keybindings::default_binds(),
            spawn_at_startup: Vec::new(),
            environment: Vec::new(),
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
    let Some(path) = path else {
        warn!("no config path available, using built-in defaults");
        return Config::default();
    };

    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            info!(path = %path.display(), "no config file, using defaults");
            return Config::default();
        }
        Err(err) => {
            warn!(path = %path.display(), %err, "cannot read config, using defaults");
            return Config::default();
        }
    };

    match parse(&text, mod_alias) {
        Ok(config) => {
            info!(path = %path.display(), binds = config.binds.len(), "config loaded");
            config
        }
        Err(err) => {
            warn!(path = %path.display(), %err, "config is invalid, using defaults");
            Config::default()
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
            "input" => parse_input(node, &mut config.keyboard),
            "binds" => parse_binds(node, &mut config.binds, mod_alias),
            "spawn-at-startup" => {
                let argv = string_args(node);
                if argv.is_empty() {
                    warn!("spawn-at-startup without a command is ignored");
                } else {
                    config.spawn_at_startup.push(argv);
                }
            }
            "environment" => parse_environment(node, &mut config.environment),
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
        config.binds = keybindings::default_binds();
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

fn parse_input(node: &KdlNode, keyboard: &mut KeyboardConfig) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children.nodes() {
        match child.name().value() {
            "keyboard" => parse_keyboard(child, keyboard),
            // touchpad/mouse/trackpoint settings are applied by libinput, which
            // only the udev backend will have; the nested backend inherits the
            // host's device configuration.
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
        let text = r#"
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

environment {
    XDG_MENU_PREFIX "arch-"
}

spawn-at-startup "waybar"
spawn-at-startup "fish" "-c" "echo hi"

binds {
    Mod+q { close-window; }
    Mod+Escape { quit; }
    Mod+T { spawn "foot"; }
    Mod+Shift+T { spawn-sh "foot -e htop"; }
    Mod+F { fullscreen-window; }
    XF86AudioPlay { spawn "playerctl" "play-pause"; }
}
"#;
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
    }

    #[test]
    fn unknown_sections_do_not_break_the_parse() {
        let text = r#"
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
"#;
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
        assert_eq!(config.binds, keybindings::default_binds());
    }
}
