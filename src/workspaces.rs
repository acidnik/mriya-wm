//! Workspaces: several independent window stacks, one visible at a time.
//!
//! The active workspace *is* `Mriya::space`, the others wait in
//! `Mriya::workspaces`. Switching swaps the two, so every other part of the
//! compositor keeps working on `self.space` without knowing about workspaces at
//! all.

use smithay::{
    desktop::{Space, Window},
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{decor, shell, state::Mriya};

/// Workspaces created up front; more are added on demand by `focus-workspace N`.
pub const DEFAULT_WORKSPACES: usize = 9;

/// Upper bound, so a bad binding cannot grow the list forever.
pub const MAX_WORKSPACES: usize = 32;

impl Mriya {
    /// Make sure workspace `index` exists and has the output mapped.
    pub fn ensure_workspace(&mut self, index: usize) {
        if index >= MAX_WORKSPACES {
            return;
        }
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let location = self
            .space
            .output_geometry(&output)
            .map(|geometry| geometry.loc)
            .unwrap_or_default();

        while self.workspaces.len() <= index {
            let mut space = Space::default();
            space.map_output(&output, location);
            self.workspaces.push(space);
        }
    }

    /// Map the output in every workspace; called when the backend created it.
    pub fn map_output_everywhere(
        &mut self,
        output: &smithay::output::Output,
        location: Point<i32, Logical>,
    ) {
        self.space.map_output(output, location);
        for space in &mut self.workspaces {
            space.map_output(output, location);
        }
    }

    /// Map every output in every workspace; the DRM backend creates outputs
    /// after the workspaces already exist.
    pub fn map_outputs_into_workspaces(&mut self) {
        let outputs: Vec<(smithay::output::Output, Point<i32, Logical>)> = self
            .space
            .outputs()
            .filter_map(|output| {
                let geometry = self.space.output_geometry(output)?;
                Some((output.clone(), geometry.loc))
            })
            .collect();
        for (output, location) in outputs {
            self.map_output_everywhere(&output, location);
        }
    }

    /// Number of workspaces the user has ever reached, at least the default.
    pub fn workspace_count(&self) -> usize {
        self.workspaces.len().max(DEFAULT_WORKSPACES)
    }

    /// Show workspace `index` (zero based).
    pub fn switch_workspace(&mut self, index: usize) {
        if index >= MAX_WORKSPACES {
            return;
        }
        self.ensure_workspace(index);
        if index == self.active_workspace {
            return;
        }

        // The windows left behind are no longer active.
        for window in self.space.elements() {
            self.set_window_activated(window, false);
        }

        // `workspaces[i]` is the content of workspace i; the slot of the active
        // workspace is stale because its content lives in `self.space`. Hand the
        // content back first, then pick up the target.
        let previous = self.active_workspace;
        std::mem::swap(&mut self.space, &mut self.workspaces[previous]);
        std::mem::swap(&mut self.space, &mut self.workspaces[index]);
        self.active_workspace = index;

        match self.space.elements().last().cloned() {
            Some(window) => self.focus_window(&window),
            None => self.clear_focus(SERIAL_COUNTER.next_serial()),
        }
        // The pointer may now be over a different surface.
        self.refresh_pointer_focus();
        self.publish_workspaces();
        tracing::debug!(workspace = index + 1, "workspace focused");
        self.request_redraw();
    }

    /// Switch relative to the active workspace, wrapping around at the ends.
    pub fn switch_workspace_relative(&mut self, delta: i32) {
        let count = self.workspace_count() as i32;
        let current = self.active_workspace as i32;
        let next = (current + delta).rem_euclid(count);
        self.switch_workspace(next as usize);
    }

    /// Move the focused window to workspace `index` and stay where we are.
    pub fn move_window_to_workspace(&mut self, index: usize) {
        if index >= MAX_WORKSPACES || index == self.active_workspace {
            return;
        }
        let Some(window) = self
            .focused_window()
            .or_else(|| self.space.elements().last().cloned())
        else {
            return;
        };

        self.ensure_workspace(index);
        self.space.unmap_elem(&window);
        let location = shell::location_for_new_window(&self.workspaces[index]);
        self.workspaces[index].map_element(window.clone(), location, false);
        self.set_window_activated(&window, false);

        match self.space.elements().last().cloned() {
            Some(next) => self.focus_window(&next),
            None => self.clear_focus(SERIAL_COUNTER.next_serial()),
        }
        self.refresh_pointer_focus();
        self.publish_workspaces();
        tracing::debug!(workspace = index + 1, "window moved");
        self.request_redraw();
    }

    /// Move the focused window one workspace up or down.
    pub fn move_window_to_workspace_relative(&mut self, delta: i32) {
        let count = self.workspace_count() as i32;
        let next = (self.active_workspace as i32 + delta).rem_euclid(count);
        self.move_window_to_workspace(next as usize);
    }

    fn set_window_activated(&self, window: &Window, active: bool) {
        decor::state_mut(window).active = active;
        if window.toplevel().is_none() {
            return;
        }
        self.send_configure(window, |state| {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
            if active {
                state.states.set(xdg_toplevel::State::Activated);
            } else {
                state.states.unset(xdg_toplevel::State::Activated);
            }
        });
    }
}
