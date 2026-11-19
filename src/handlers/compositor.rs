//! `wl_compositor` / `wl_shm` surface handling.

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        shm::{ShmHandler, ShmState},
    },
};

use crate::{grabs::resize_grab, handlers::xdg_shell, state::ClientState, Mriya};

impl CompositorHandler for Mriya {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let window = self
                .space
                .elements()
                .find(|window| {
                    window
                        .toplevel()
                        .map(|toplevel| toplevel.wl_surface() == &root)
                        .unwrap_or(false)
                })
                .cloned();
            match window {
                Some(window) => {
                    window.on_commit();
                    // The client's window geometry is what tells whether it
                    // draws its own decorations, and it is only known after a
                    // commit.
                    self.update_decoration_mode(&window);
                }
                None => {
                    // Panels are not in the space; their size and scale come from
                    // the layer map on every commit of the root surface.
                    if crate::layers::is_layer_surface(&self.space, &root) {
                        self.configure_layer_surface(&root);
                    }
                }
            }
        }

        xdg_shell::handle_commit(&mut self.popups, &self.space, surface);
        resize_grab::handle_commit(&mut self.space, surface);

        self.request_redraw();
    }
}

impl BufferHandler for Mriya {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Mriya {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Mriya);
delegate_shm!(Mriya);
