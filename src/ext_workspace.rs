//! `ext-workspace-v1`: tell panels which workspaces exist and which is active.
//!
//! waybar's `ext/workspaces` module speaks this protocol, the compositor-agnostic
//! one (as opposed to `niri/*`, `hyprland/*` or `sway/*`, which talk to their own
//! IPC).
//!
//! Mriya has a flat, numbered set of workspaces on a single output, so the whole
//! tree is one group that holds every workspace handle.

use smithay::reexports::{
    wayland_protocols::ext::workspace::v1::server::{
        ext_workspace_group_handle_v1::{
            self as group, ExtWorkspaceGroupHandleV1, GroupCapabilities,
        },
        ext_workspace_handle_v1::{
            self as handle, ExtWorkspaceHandleV1, State as WorkspaceState, WorkspaceCapabilities,
        },
        ext_workspace_manager_v1::{self as manager, ExtWorkspaceManagerV1},
    },
    wayland_server::{
        backend::GlobalId, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    },
};

use crate::state::Mriya;

/// Protocol state kept in [`Mriya`].
#[derive(Default)]
pub struct ExtWorkspaceState {
    /// Kept so the global stays advertised for the lifetime of the compositor.
    #[allow(dead_code)]
    global: Option<GlobalId>,
    /// One entry per client that bound the manager.
    managers: Vec<Manager>,
}

struct Manager {
    resource: ExtWorkspaceManagerV1,
    group: Option<ExtWorkspaceGroupHandleV1>,
    /// Workspace index and its handle, in ascending order.
    workspaces: Vec<(usize, ExtWorkspaceHandleV1)>,
}

impl ExtWorkspaceState {
    /// Advertise the global.
    pub fn new(display: &DisplayHandle) -> Self {
        let global = display.create_global::<Mriya, ExtWorkspaceManagerV1, ()>(1, ());
        Self {
            global: Some(global),
            managers: Vec::new(),
        }
    }
}

impl GlobalDispatch<ExtWorkspaceManagerV1, (), Mriya> for Mriya {
    fn bind(
        state: &mut Mriya,
        _handle: &DisplayHandle,
        _client: &smithay::reexports::wayland_server::Client,
        resource: New<ExtWorkspaceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Mriya>,
    ) {
        let resource = data_init.init(resource, ());
        state.ext_workspace_state.managers.push(Manager {
            resource,
            group: None,
            workspaces: Vec::new(),
        });
        state.publish_workspaces();
    }
}

impl Dispatch<ExtWorkspaceManagerV1, (), Mriya> for Mriya {
    fn request(
        state: &mut Mriya,
        _client: &smithay::reexports::wayland_server::Client,
        resource: &ExtWorkspaceManagerV1,
        request: <ExtWorkspaceManagerV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Mriya>,
    ) {
        match request {
            // The client acknowledged the queued state; nothing to answer for a
            // compositor-side list.
            manager::Request::Commit => {}
            manager::Request::Stop => {
                state
                    .ext_workspace_state
                    .managers
                    .retain(|entry| entry.resource != *resource);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, (), Mriya> for Mriya {
    fn request(
        _state: &mut Mriya,
        _client: &smithay::reexports::wayland_server::Client,
        _resource: &ExtWorkspaceGroupHandleV1,
        request: <ExtWorkspaceGroupHandleV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Mriya>,
    ) {
        if let group::Request::CreateWorkspace { .. } = request {
            // Workspaces are numbered and grown by the key bindings; the
            // capability is not advertised, so this should not happen.
            tracing::debug!("ext-workspace: create_workspace is not supported");
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, u32, Mriya> for Mriya {
    fn request(
        state: &mut Mriya,
        _client: &smithay::reexports::wayland_server::Client,
        _resource: &ExtWorkspaceHandleV1,
        request: <ExtWorkspaceHandleV1 as Resource>::Request,
        data: &u32,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Mriya>,
    ) {
        match request {
            handle::Request::Activate => {
                state.switch_workspace(*data as usize);
                state.publish_workspaces();
            }
            handle::Request::Deactivate
            | handle::Request::Remove
            | handle::Request::Assign { .. } => {
                // Not advertised in our capabilities.
                tracing::debug!("ext-workspace: ignoring an unsupported request");
            }
            _ => {}
        }
    }
}

impl Mriya {
    /// Create missing objects and send the current workspace state.
    ///
    /// Called when a client binds the global and after every workspace change.
    pub fn publish_workspaces(&mut self) {
        let output = self.space.outputs().next().cloned();
        let count = self.workspace_count();
        let active = self.active_workspace;

        // Take the list out so the borrow checker lets us touch the rest of the
        // state (display handle, space) while the managers are processed.
        let managers = std::mem::take(&mut self.ext_workspace_state.managers);
        let mut alive = Vec::with_capacity(managers.len());

        for mut entry in managers {
            if !entry.resource.is_alive() {
                continue;
            }
            let Some(client) = entry.resource.client() else {
                continue;
            };

            if entry.group.is_none() {
                let group = match client.create_resource::<ExtWorkspaceGroupHandleV1, (), Mriya>(
                    &self.display_handle,
                    1,
                    (),
                ) {
                    Ok(group) => group,
                    Err(err) => {
                        tracing::debug!(%err, "ext-workspace: cannot create a group handle");
                        continue;
                    }
                };
                entry.resource.workspace_group(&group);
                group.capabilities(GroupCapabilities::empty());
                if let Some(output) = &output {
                    for wl_output in output.client_outputs(&client) {
                        group.output_enter(&wl_output);
                    }
                }
                entry.group = Some(group);
            }

            entry.workspaces.retain(|(_, handle)| handle.is_alive());

            for index in 0..count {
                if entry
                    .workspaces
                    .iter()
                    .any(|(existing, _)| *existing == index)
                {
                    continue;
                }
                let handle = match client.create_resource::<ExtWorkspaceHandleV1, u32, Mriya>(
                    &self.display_handle,
                    1,
                    index as u32,
                ) {
                    Ok(handle) => handle,
                    Err(err) => {
                        tracing::debug!(%err, "ext-workspace: cannot create a workspace handle");
                        continue;
                    }
                };
                entry.resource.workspace(&handle);
                handle.id(format!("{}", index + 1));
                handle.name(format!("{}", index + 1));
                handle.coordinates(coordinates(index));
                handle.capabilities(
                    WorkspaceCapabilities::Activate | WorkspaceCapabilities::Deactivate,
                );
                if let Some(group) = &entry.group {
                    group.workspace_enter(&handle);
                }
                entry.workspaces.push((index, handle));
            }

            for (index, handle) in &entry.workspaces {
                handle.state(if *index == active {
                    WorkspaceState::Active
                } else {
                    WorkspaceState::empty()
                });
            }
            entry.resource.done();
            alive.push(entry);
        }

        self.ext_workspace_state.managers = alive;
    }
}

/// `coordinates` is a raw byte array of u32 values, little endian on the wire.
fn coordinates(index: usize) -> Vec<u8> {
    (index as u32).to_ne_bytes().to_vec()
}
