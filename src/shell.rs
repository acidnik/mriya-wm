//! Window placement policy and window state changes.
//!
//! Placement: every window is put in the middle of the only output with a small
//! cascade offset, i.e. classic stacking behaviour. Tiling, snapping and
//! per-app rules will live here later.

use std::sync::Mutex;

use smithay::{
    desktop::{Space, Window},
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle},
};

/// Offset of each subsequent window in the cascade, in logical pixels.
const CASCADE_STEP: i32 = 32;

/// How many steps the cascade wraps around before starting over.
const CASCADE_WRAP: i32 = 8;

/// Geometry a window had before it was maximized or made fullscreen, so the
/// state can be undone.
#[derive(Default)]
struct SavedGeometry(Mutex<Option<Rectangle<i32, Logical>>>);

/// Location for a newly mapped window.
pub fn location_for_new_window(space: &Space<Window>) -> Point<i32, Logical> {
    let Some(output) = space.outputs().next() else {
        return (0, 0).into();
    };
    let Some(output_geo) = space.output_geometry(output) else {
        return (0, 0).into();
    };

    let step = (space.elements().count() as i32 % CASCADE_WRAP) * CASCADE_STEP;
    // A third of the way in looks better than dead centre for the first window.
    let base = Point::<i32, Logical>::from((
        output_geo.loc.x + output_geo.size.w / 6,
        output_geo.loc.y + output_geo.size.h / 6,
    ));

    base + Point::from((step, step))
}

/// Toggle fullscreen for `window`.
pub fn toggle_fullscreen(space: &mut Space<Window>, window: &Window) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };
    let fullscreen =
        toplevel.with_pending_state(|state| state.states.contains(xdg_toplevel::State::Fullscreen));

    if fullscreen {
        restore_geometry(space, window);
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = None;
        });
    } else {
        let Some(geometry) = output_geometry(space) else {
            return;
        };
        remember_geometry(space, window);
        space.map_element(window.clone(), geometry.loc, true);
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(geometry.size);
        });
    }

    toplevel.send_pending_configure();
}

/// Toggle maximization for `window`.
pub fn toggle_maximized(space: &mut Space<Window>, window: &Window) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };
    let maximized =
        toplevel.with_pending_state(|state| state.states.contains(xdg_toplevel::State::Maximized));

    if maximized {
        restore_geometry(space, window);
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = None;
        });
    } else {
        let Some(geometry) = output_geometry(space) else {
            return;
        };
        remember_geometry(space, window);
        space.map_element(window.clone(), geometry.loc, true);
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some(geometry.size);
        });
    }

    toplevel.send_pending_configure();
}

/// Maximize / fullscreen a window on a client request.
pub fn set_fullscreen(space: &mut Space<Window>, window: &Window, on: bool) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };
    if on {
        let Some(geometry) = output_geometry(space) else {
            toplevel.send_configure();
            return;
        };
        remember_geometry(space, window);
        space.map_element(window.clone(), geometry.loc, true);
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(geometry.size);
        });
    } else {
        restore_geometry(space, window);
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = None;
        });
    }
    toplevel.send_configure();
}

/// Maximize / unmaximize a window on a client request.
pub fn set_maximized(space: &mut Space<Window>, window: &Window, on: bool) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };
    if on {
        let Some(geometry) = output_geometry(space) else {
            toplevel.send_configure();
            return;
        };
        remember_geometry(space, window);
        space.map_element(window.clone(), geometry.loc, true);
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some(geometry.size);
        });
    } else {
        restore_geometry(space, window);
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = None;
        });
    }
    toplevel.send_configure();
}

fn output_geometry(space: &Space<Window>) -> Option<Rectangle<i32, Logical>> {
    let output = space.outputs().next()?;
    space.output_geometry(output)
}

fn remember_geometry(space: &Space<Window>, window: &Window) {
    let Some(location) = space.element_location(window) else {
        return;
    };
    let size = window.geometry().size;

    window.user_data().insert_if_missing(SavedGeometry::default);
    if let Some(saved) = window.user_data().get::<SavedGeometry>() {
        *saved.0.lock().unwrap() = Some(Rectangle::new(location, size));
    }
}

fn restore_geometry(space: &mut Space<Window>, window: &Window) {
    let saved = window
        .user_data()
        .get::<SavedGeometry>()
        .and_then(|saved| *saved.0.lock().unwrap());
    if let Some(geometry) = saved {
        space.map_element(window.clone(), geometry.loc, true);
    }
}
