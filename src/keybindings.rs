//! Window manager keybindings.
//!
//! `Mod` is the Super/Logo key. Bindings are hardcoded for now; they will be
//! configurable from the kdl config later.

use smithay::input::keyboard::{keysyms, Keysym, ModifiersState};

/// What a shortcut asks the compositor to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Consumed, but nothing to do.
    None,
    /// Ask the focused window to close.
    CloseWindow,
    /// End the compositor session.
    Quit,
}

/// Action bound to `modifiers` + `keysym`, if any.
pub fn action_for(modifiers: ModifiersState, keysym: Keysym) -> Option<KeyAction> {
    if !modifiers.logo {
        return None;
    }
    // Shift+Super+q must not be swallowed by Super+q handling.
    if modifiers.shift {
        return None;
    }

    match keysym.raw() {
        keysyms::KEY_q => Some(KeyAction::CloseWindow),
        keysyms::KEY_Escape => Some(KeyAction::Quit),
        _ => None,
    }
}
