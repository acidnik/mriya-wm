//! Window manager key bindings.
//!
//! Bindings come from the `binds { ... }` section of the config, which is
//! written exactly like niri's (chords separated by `+`, an XKB key name last,
//! actions as child nodes). `Mod` follows niri's convention: `Super` when the
//! compositor owns the input devices, `Alt` when it runs nested inside another
//! session whose own `Super` bindings would otherwise win.

use kdl::KdlNode;
use smithay::input::keyboard::{xkb, Keysym, ModifiersState};
use tracing::debug;

/// Which key `Mod` refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModAlias {
    /// `Super` — used when the compositor owns the input devices (TTY backend).
    #[allow(dead_code)]
    Super,
    /// `Alt` — used when nested, where the host session usually binds `Super`.
    Alt,
}

/// Modifier set a binding requires. Matching is exact: `Mod+Shift+q` and
/// `Mod+q` are different bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl Mods {
    fn matches(&self, state: &ModifiersState) -> bool {
        self.ctrl == state.ctrl
            && self.alt == state.alt
            && self.shift == state.shift
            && self.logo == state.logo
    }
}

/// What a binding asks the compositor to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CloseWindow,
    Quit,
    /// Toggle fullscreen for the focused window.
    FullscreenWindow,
    /// Toggle maximization for the focused window.
    MaximizeWindow,
    /// Start a command directly, without a shell.
    Spawn(Vec<String>),
    /// Run a command line through `sh -c`.
    SpawnShell(String),
    /// Start the default terminal (`$TERMINAL` or the first one found in `PATH`).
    SpawnTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub mods: Mods,
    pub keysym: Keysym,
    pub actions: Vec<Action>,
}

/// Runtime lookup table built from the config.
#[derive(Debug, Clone)]
pub struct Keybindings {
    binds: Vec<Binding>,
}

impl Keybindings {
    pub fn new(binds: Vec<Binding>) -> Self {
        Self { binds }
    }

    /// Actions bound to this key press, if any.
    pub fn action_for(&self, mods: &ModifiersState, keysym: Keysym) -> Option<Vec<Action>> {
        self.binds
            .iter()
            .find(|bind| bind.keysym == keysym && bind.mods.matches(mods))
            .map(|bind| bind.actions.clone())
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::new(default_binds())
    }
}

/// Bindings used when the config does not define any.
pub fn default_binds() -> Vec<Binding> {
    let alias = ModAlias::Alt;
    [
        // A terminal is the way to start anything else, so it works out of the
        // box, even without a config file.
        ("Mod+Return", Action::SpawnTerminal),
        ("Mod+q", Action::CloseWindow),
        ("Mod+Escape", Action::Quit),
        ("Mod+f", Action::FullscreenWindow),
        ("Mod+m", Action::MaximizeWindow),
    ]
    .into_iter()
    .filter_map(|(chord, action)| {
        let (mods, keysym) = parse_chord(chord, alias).ok()?;
        Some(Binding {
            mods,
            keysym,
            actions: vec![action],
        })
    })
    .collect()
}

/// Parse a chord like `Mod+Shift+q` into its modifiers and key symbol.
pub fn parse_chord(chord: &str, mod_alias: ModAlias) -> Result<(Mods, Keysym), String> {
    let mut parts: Vec<&str> = chord.split('+').map(str::trim).collect();
    let Some(key_name) = parts.pop() else {
        return Err("empty key binding".to_string());
    };
    if key_name.is_empty() {
        return Err("key binding without a key".to_string());
    }

    let mut mods = Mods::default();
    for modifier in parts {
        match modifier.to_ascii_lowercase().as_str() {
            "mod" => match mod_alias {
                ModAlias::Super => mods.logo = true,
                ModAlias::Alt => mods.alt = true,
            },
            "super" | "logo" | "win" | "cmd" => mods.logo = true,
            "alt" => mods.alt = true,
            "ctrl" | "control" => mods.ctrl = true,
            "shift" => mods.shift = true,
            other => return Err(format!("unknown modifier `{other}`")),
        }
    }

    let keysym =
        keysym_by_name(key_name).ok_or_else(|| format!("unknown key name `{key_name}`"))?;
    Ok((mods, keysym))
}

/// XKB key names are usually lowercase, but the config may spell them like the
/// symbol (`Escape`) or in the case of a letter (`Mod+T`). A binding always
/// refers to the unshifted key, so an uppercase letter is folded.
fn keysym_by_name(name: &str) -> Option<Keysym> {
    for candidate in [name.to_string(), name.to_ascii_lowercase()] {
        let keysym = xkb::keysym_from_name(&candidate, xkb::KEYSYM_NO_FLAGS);
        if keysym == Keysym::NoSymbol {
            continue;
        }
        let raw = keysym.raw();
        if (u32::from(b'A')..=u32::from(b'Z')).contains(&raw) {
            return Some(Keysym::new(raw + 0x20));
        }
        return Some(keysym);
    }
    None
}

/// Parse one action node from a `binds` block.
pub fn parse_action(node: &KdlNode) -> Result<Action, String> {
    let name = node.name().value();
    // niri puts properties like `allow-when-locked=true` on the action node.
    for entry in node.entries() {
        if let Some(property) = entry.name() {
            debug!(action = name, property = %property.value(), "action property is not supported yet, ignoring");
        }
    }

    match name {
        "close-window" => Ok(Action::CloseWindow),
        "quit" => Ok(Action::Quit),
        "fullscreen-window" => Ok(Action::FullscreenWindow),
        "maximize-window" => Ok(Action::MaximizeWindow),
        "spawn" => {
            let argv = string_args(node);
            if argv.is_empty() {
                Err("spawn without a command".to_string())
            } else {
                Ok(Action::Spawn(argv))
            }
        }
        "spawn-terminal" => Ok(Action::SpawnTerminal),
        "spawn-sh" => match string_args(node).first() {
            Some(command) => Ok(Action::SpawnShell(command.clone())),
            None => Err("spawn-sh without a command".to_string()),
        },
        other => Err(format!("action `{other}` is not implemented")),
    }
}

fn string_args(node: &KdlNode) -> Vec<String> {
    node.entries()
        .iter()
        .filter(|entry| entry.name().is_none())
        .filter_map(|entry| entry.value().as_string().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::input::keyboard::keysyms;

    fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            shift,
            logo,
            ..ModifiersState::default()
        }
    }

    #[test]
    fn mod_is_alt_when_nested() {
        let (mods, keysym) = parse_chord("Mod+q", ModAlias::Alt).unwrap();
        assert!(mods.alt);
        assert_eq!(keysym, Keysym::new(keysyms::KEY_q));
    }

    #[test]
    fn uppercase_letters_bind_to_the_unshifted_key() {
        let (_, keysym) = parse_chord("Mod+T", ModAlias::Alt).unwrap();
        assert_eq!(keysym, Keysym::new(keysyms::KEY_t));
        let (_, keysym) = parse_chord("Mod+Q", ModAlias::Alt).unwrap();
        assert_eq!(keysym, Keysym::new(keysyms::KEY_q));
    }

    #[test]
    fn mod_is_super_on_a_tty() {
        let (mods, keysym) = parse_chord("Mod+q", ModAlias::Super).unwrap();
        assert!(mods.logo);
        assert_eq!(keysym, Keysym::new(keysyms::KEY_q));
    }

    #[test]
    fn explicit_modifiers_are_parsed() {
        let (mods, keysym) = parse_chord("Super+Ctrl+Shift+Escape", ModAlias::Alt).unwrap();
        assert!(mods.logo && mods.ctrl && mods.shift && !mods.alt);
        assert_eq!(keysym, Keysym::new(keysyms::KEY_Escape));
    }

    #[test]
    fn named_and_function_keys_work() {
        assert_eq!(
            parse_chord("Mod+Slash", ModAlias::Alt).unwrap().1,
            Keysym::new(keysyms::KEY_slash)
        );
        assert_eq!(
            parse_chord("XF86AudioPlay", ModAlias::Alt).unwrap().1,
            Keysym::new(keysyms::KEY_XF86AudioPlay)
        );
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert!(parse_chord("Mod+NotAKey", ModAlias::Alt).is_err());
        assert!(parse_chord("Hyper+q", ModAlias::Alt).is_err());
        assert!(parse_chord("Mod+", ModAlias::Alt).is_err());
    }

    fn table() -> Keybindings {
        let mut binds = Vec::new();
        for (chord, action) in [
            ("Mod+q", Action::CloseWindow),
            ("Mod+Shift+q", Action::Quit),
        ] {
            let (mods, keysym) = parse_chord(chord, ModAlias::Alt).unwrap();
            binds.push(Binding {
                mods,
                keysym,
                actions: vec![action],
            });
        }
        Keybindings::new(binds)
    }

    #[test]
    fn only_the_exact_modifier_set_matches() {
        let table = table();
        assert_eq!(
            table.action_for(
                &mods(false, true, false, false),
                Keysym::new(keysyms::KEY_q)
            ),
            Some(vec![Action::CloseWindow])
        );
        assert_eq!(
            table.action_for(&mods(false, true, true, false), Keysym::new(keysyms::KEY_q)),
            Some(vec![Action::Quit])
        );
    }

    #[test]
    fn unbound_keys_are_forwarded_to_the_client() {
        let table = table();
        assert_eq!(
            table.action_for(
                &mods(false, true, false, false),
                Keysym::new(keysyms::KEY_w)
            ),
            None
        );
        assert_eq!(
            table.action_for(
                &mods(false, false, false, false),
                Keysym::new(keysyms::KEY_q)
            ),
            None
        );
        assert_eq!(
            table.action_for(
                &mods(false, false, false, true),
                Keysym::new(keysyms::KEY_q)
            ),
            None
        );
    }
}
