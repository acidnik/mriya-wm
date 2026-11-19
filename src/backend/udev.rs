//! udev/drm backend: run Mriya on a TTY, from a display manager.
//!
//! The session (and with it access to `/dev/dri/*`) comes from logind through
//! libseat, input from libinput, and every connected connector gets a
//! `DrmOutput` that scans out the same scene the nested backend draws.
//!
//! Test from a VT:
//!
//! ```sh
//! Ctrl+Alt+F2
//! RUST_LOG=info,mriya_wm=debug mriya-wm --drm
//! Ctrl+Alt+F1     # back to the previous session
//! ```

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use smithay::{
    backend::{
        allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        drm::{
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager},
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode,
        },
        egl::{context::ContextPriority, EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{gles::GlesRenderer, ImportMemWl},
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{primary_gpu, UdevBackend as UdevDeviceMonitor, UdevEvent},
    },
    input::pointer::CursorImageStatus,
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, LoopHandle, RegistrationToken},
        drm::control::{
            connector::{self, State as ConnectorState},
            crtc, Device as ControlDevice, ModeTypeFlags,
        },
        input::{DeviceCapability, Libinput},
    },
    utils::{DeviceFd, Logical},
};

use crate::{backend::Backend, cursor::Cursor, layers, state::Mriya};

/// Colour the output is cleared with; matches the nested backend.
const BACKGROUND: [f32; 4] = [0.11, 0.11, 0.13, 1.0];

type OutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;
type DrmOut =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// One scanning-out connector.
struct OutputSurface {
    output: Output,
    global: Option<smithay::reexports::wayland_server::backend::GlobalId>,
    drm: DrmOut,
}

struct DeviceState {
    manager: OutputManager,
    renderer: GlesRenderer,
    surfaces: HashMap<crtc::Handle, OutputSurface>,
    notifier: RegistrationToken,
}

pub struct UdevBackend {
    session: LibSeatSession,
    libinput: Libinput,
    devices: HashMap<DrmNode, DeviceState>,
    cursor: Cursor,
    /// Diagnostics: how many frames we actually queued and how many were empty.
    frames_drawn: u32,
    frames_empty: u32,
    loop_handle: LoopHandle<'static, Mriya>,
    shm_formats: Vec<smithay::reexports::wayland_server::protocol::wl_shm::Format>,
}

impl UdevBackend {
    pub fn reload_cursor(&mut self, config: &crate::config::CursorConfig) {
        self.cursor = Cursor::new(config);
    }

    /// Draw every output. Missing frames are simply skipped.
    pub fn draw(&mut self, state: &mut Mriya) -> Result<()> {
        let nodes: Vec<DrmNode> = self.devices.keys().copied().collect();
        for node in nodes {
            let crtcs: Vec<crtc::Handle> = match self.devices.get(&node) {
                Some(device) => device.surfaces.keys().copied().collect(),
                None => continue,
            };
            for crtc in crtcs {
                if let Err(err) = self.render_surface(state, node, crtc) {
                    // A failing output should not take the whole compositor down.
                    tracing::warn!(?node, ?crtc, %err, "failed to render an output");
                }
            }
        }
        Ok(())
    }

    fn render_surface(
        &mut self,
        state: &mut Mriya,
        node: DrmNode,
        crtc: crtc::Handle,
    ) -> Result<()> {
        let cursor_status = match state.cursor_override {
            Some(icon) => CursorImageStatus::Named(icon),
            None => state.cursor_status.clone(),
        };

        let Some(device) = self.devices.get_mut(&node) else {
            return Ok(());
        };
        let Some(surface) = device.surfaces.get_mut(&crtc) else {
            return Ok(());
        };
        let output = surface.output.clone();
        let scale = output.current_scale().fractional_scale();

        let mut elements = crate::backend::scene_elements(
            &mut device.renderer,
            state,
            &output,
            scale,
            &mut self.cursor,
            &cursor_status,
        );
        // `DrmOutput` takes the list front to back, like the damage tracker.
        elements.reverse();

        let result = surface
            .drm
            .render_frame(
                &mut device.renderer,
                &elements,
                BACKGROUND,
                FrameFlags::DEFAULT,
            )
            .map_err(|err| anyhow!("render_frame failed: {err}"))?;

        if result.is_empty {
            // Nothing damaged: nothing to scan out. Log the first few so a black
            // screen can be told apart from "we never even started drawing".
            if self.frames_drawn < 3 && self.frames_empty < 3 {
                self.frames_empty += 1;
                tracing::warn!("frame has no damage, skipping it");
            }
            return Ok(());
        }
        self.frames_drawn += 1;
        if self.frames_drawn <= 3 {
            tracing::debug!(frames = self.frames_drawn, "output frame queued");
        }
        surface
            .drm
            .queue_frame(())
            .map_err(|err| anyhow!("queue_frame failed: {err}"))?;
        Ok(())
    }

    /// vblank: the frame is on screen, so hand out frame callbacks.
    fn frame_finished(&mut self, state: &mut Mriya, node: DrmNode, crtc: crtc::Handle) {
        let Some(device) = self.devices.get_mut(&node) else {
            return;
        };
        let Some(surface) = device.surfaces.get_mut(&crtc) else {
            return;
        };
        let output = surface.output.clone();
        if let Err(err) = surface.drm.frame_submitted() {
            tracing::warn!(%err, "frame_submitted failed");
        }

        let now = state.start_time.elapsed();
        for window in state.space.elements() {
            window.send_frame(&output, now, None, |_, _| Some(output.clone()));
        }
        for (layer, _) in layers::surfaces(&state.space) {
            layer.send_frame(&output, now, None, |_, _| Some(output.clone()));
        }
    }

    fn add_device(&mut self, state: &mut Mriya, node: DrmNode, path: &Path) -> Result<()> {
        let fd = self
            .session
            .open(
                path,
                smithay::reexports::rustix::fs::OFlags::RDWR
                    | smithay::reexports::rustix::fs::OFlags::CLOEXEC
                    | smithay::reexports::rustix::fs::OFlags::NOCTTY
                    | smithay::reexports::rustix::fs::OFlags::NONBLOCK,
            )
            .map_err(|err| anyhow!("cannot open {}: {err}", path.display()))?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm_device, notifier) = DrmDevice::new(fd.clone(), true)
            .map_err(|err| anyhow!("cannot open the DRM device: {err}"))?;
        let gbm = GbmDevice::new(fd).map_err(|err| anyhow!("cannot create a GBM device: {err}"))?;

        // One GL renderer per device; the laptop's discrete GPU can render for
        // the integrated one, but a single renderer per card is enough here.
        let display = unsafe { EGLDisplay::new(gbm.clone()) }
            .map_err(|err| anyhow!("cannot get an EGL display: {err}"))?;
        let context = EGLContext::new_with_priority(&display, ContextPriority::High)
            .map_err(|err| anyhow!("cannot create an EGL context: {err}"))?;
        let renderer = unsafe { GlesRenderer::new(context) }
            .map_err(|err| anyhow!("cannot create the GL renderer: {err}"))?;
        let shm_formats: Vec<_> = renderer.shm_formats().collect();

        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );
        let exporter = GbmFramebufferExporter::new(gbm.clone(), None);
        let render_formats = renderer
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let color_formats = [smithay::backend::allocator::Fourcc::Xrgb8888];

        let manager = OutputManager::new(
            drm_device,
            allocator,
            exporter,
            Some(gbm.clone()),
            color_formats.iter().copied(),
            render_formats,
        );

        let notifier_token = self
            .loop_handle
            .insert_source(
                notifier,
                move |event, _metadata, state: &mut Mriya| match event {
                    DrmEvent::VBlank(crtc) => {
                        let _ = state.with_udev_backend(|backend, state| {
                            backend.frame_finished(state, node, crtc)
                        });
                    }
                    DrmEvent::Error(err) => tracing::warn!(%err, "DRM error"),
                },
            )
            .map_err(|err| anyhow!("cannot watch the DRM device: {err}"))?;

        let mut device = DeviceState {
            manager,
            renderer,
            surfaces: HashMap::new(),
            notifier: notifier_token,
        };
        Self::scan_connectors(state, node, &mut device)?;
        self.devices.insert(node, device);

        if self.shm_formats.is_empty() {
            self.shm_formats = shm_formats;
        }
        let _ = state;
        Ok(())
    }

    /// Connect every connector that is plugged in and has a free crtc.
    fn scan_connectors(state: &mut Mriya, node: DrmNode, device: &mut DeviceState) -> Result<()> {
        let drm = device.manager.device();
        let handles = drm
            .resource_handles()
            .map_err(|err| anyhow!("cannot list DRM resources: {err}"))?;

        let mut connected = Vec::new();
        for handle in handles.connectors() {
            let info = drm
                .get_connector(*handle, false)
                .map_err(|err| anyhow!("cannot read connector: {err}"))?;
            if info.state() == ConnectorState::Connected {
                connected.push(info);
            }
        }

        for info in connected {
            let handle = info.handle();
            if device
                .surfaces
                .values()
                .any(|surface| surface.output.name().ends_with(&connector_name(&info)))
            {
                continue;
            }
            let Some(crtc) = handles
                .crtcs()
                .iter()
                .copied()
                .find(|crtc| !device.surfaces.contains_key(crtc))
            else {
                tracing::warn!("no free CRTC for {}", connector_name(&info));
                continue;
            };
            if let Err(err) = Self::enable_connector(state, node, device, &info, crtc) {
                tracing::warn!(connector = %connector_name(&info), %err, "cannot enable connector");
                let _ = handle;
            }
        }
        Ok(())
    }

    fn enable_connector(
        state: &mut Mriya,
        node: DrmNode,
        device: &mut DeviceState,
        info: &connector::Info,
        crtc: crtc::Handle,
    ) -> Result<()> {
        let mode_index = info
            .modes()
            .iter()
            .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(0);
        let mode = *info
            .modes()
            .get(mode_index)
            .ok_or_else(|| anyhow!("connector has no modes"))?;
        let wl_mode = OutputMode::from(mode);

        let name = connector_name(info);
        let (phys_w, phys_h) = info.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "Unknown".into(),
                model: "Unknown".into(),
            },
        );
        let global = output.create_global::<Mriya>(&state.display_handle);
        let position = state
            .space
            .outputs()
            .filter_map(|other| state.space.output_geometry(other).map(|geo| geo.size.w))
            .sum::<i32>();
        let position = smithay::utils::Point::<i32, Logical>::from((position, 0));
        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), None, None, Some(position));
        state.map_output_everywhere(&output, position);
        crate::layers::arrange(&state.space);

        let planes = device.manager.device().planes(&crtc).ok();
        let drm_output = device
            .manager
            .initialize_output::<_, crate::backend::NestedRenderElement<GlesRenderer>>(
                crtc,
                mode,
                &[info.handle()],
                &output,
                planes,
                &mut device.renderer,
                &Default::default(),
            )
            .map_err(|err| anyhow!("cannot initialize the output: {err}"))?;

        device.surfaces.insert(
            crtc,
            OutputSurface {
                output: output.clone(),
                global: Some(global),
                drm: drm_output,
            },
        );

        tracing::info!(
            connector = %name,
            mode = ?mode.size(),
            "output configured"
        );
        let _ = node;
        // First frame for this output.
        state.request_redraw();
        Ok(())
    }

    /// Re-read the connectors of a device (hotplug).
    fn rescan(&mut self, state: &mut Mriya, node: DrmNode) {
        let Some(mut device) = self.devices.remove(&node) else {
            return;
        };
        let result = Self::scan_connectors(state, node, &mut device);
        self.devices.insert(node, device);
        if let Err(err) = result {
            tracing::warn!(?node, %err, "cannot rescan connectors");
        }
    }

    fn remove_device(&mut self, state: &mut Mriya, node: DrmNode) {
        let Some(device) = self.devices.remove(&node) else {
            return;
        };
        self.loop_handle.remove(device.notifier);
        for (_, mut surface) in device.surfaces {
            if let Some(global) = surface.global.take() {
                state.display_handle.remove_global::<Mriya>(global);
            }
        }
        tracing::info!(?node, "device removed");
        state.request_redraw();
    }
}

fn connector_name(info: &connector::Info) -> String {
    format!("{}-{}", info.interface().as_str(), info.interface_id())
}

/// Start the compositor on a TTY.
pub fn run(mut event_loop: EventLoop<'static, Mriya>, mut state: Mriya) -> Result<()> {
    // Without DRM master the compositor happily configures the connector and
    // renders into buffers nobody scans out: the screen goes black and nothing
    // can bring it back. That happens when Mriya is started inside another
    // compositor, which already owns the GPU.
    if let Some(display) = std::env::var_os("WAYLAND_DISPLAY") {
        if std::env::var_os("MRIYA_ALLOW_NESTED_DRM").is_none() {
            return Err(anyhow!(
                "WAYLAND_DISPLAY={} is set, so this is a session inside another compositor that \
                 already holds the GPU (drm master). Nothing would be drawn.\n\
                 Start Mriya from a free VT instead:\n    chvt 2        # or Ctrl+Alt+F2 (Fn+F2 on \
                 laptops with media keys)\n    log in on the text console\n    mriya-wm --drm\n\
                 (MRIYA_ALLOW_NESTED_DRM=1 tries anyway.)",
                display.to_string_lossy()
            ));
        }
    }

    let (session, notifier) = LibSeatSession::new()
        .map_err(|err| anyhow!("cannot open a session through logind/seatd: {err}"))?;
    let seat_name = session.seat();

    let primary = match std::env::var("MRIYA_DRM_DEVICE") {
        Ok(path) => DrmNode::from_path(&path)
            .map_err(|err| anyhow!("MRIYA_DRM_DEVICE={path} is not a DRM node: {err}"))?,
        Err(_) => primary_gpu(&seat_name)
            .ok()
            .flatten()
            .and_then(|path| DrmNode::from_path(path).ok())
            .ok_or_else(|| anyhow!("no DRM device found on seat {seat_name}"))?,
    };
    tracing::info!(?primary, seat = %seat_name, "using this GPU as primary");

    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat_name)
        .map_err(|_| anyhow!("cannot assign the seat to libinput"))?;
    let libinput_backend = LibinputInputBackend::new(libinput.clone());

    let monitor = UdevDeviceMonitor::new(&seat_name)
        .map_err(|err| anyhow!("cannot watch udev devices: {err}"))?;

    let loop_handle = event_loop.handle();
    state.loop_handle = Some(loop_handle.clone());
    let backend = UdevBackend {
        session: session.clone(),
        libinput,
        devices: HashMap::new(),
        cursor: Cursor::new(&state.config.cursor),
        frames_drawn: 0,
        frames_empty: 0,
        loop_handle: loop_handle.clone(),
        shm_formats: Vec::new(),
    };

    // Open the devices that are already there, primary GPU first so the
    // renderer of the boot GPU exists before the others ask for it.
    let mut devices: Vec<(u64, PathBuf)> = monitor
        .device_list()
        .map(|(id, path)| (id, path.to_path_buf()))
        .collect();
    devices.sort_by_key(|(_, path)| {
        DrmNode::from_path(path)
            .map(|node| node != primary)
            .unwrap_or(true)
    });

    state.renderer = Some(Backend::Udev(backend));
    for (_, path) in devices {
        let Ok(node) = DrmNode::from_path(&path) else {
            continue;
        };
        if let Some(Err(err)) =
            state.with_udev_backend(|backend, state| backend.add_device(state, node, &path))
        {
            tracing::warn!(path = %path.display(), %err, "skipping device");
        }
    }
    let _ = state.with_udev_backend(|backend, state| {
        let formats = backend.shm_formats.clone();
        state.shm_state.update_formats(formats);
    });
    state.map_outputs_into_workspaces();

    event_loop
        .handle()
        .insert_source(libinput_backend, |mut event, _, state: &mut Mriya| {
            if let smithay::backend::input::InputEvent::DeviceAdded { device } = &mut event {
                if device.has_capability(DeviceCapability::Keyboard) {
                    if let Some(keyboard) = state.seat.get_keyboard() {
                        device.led_update(keyboard.led_state().into());
                    }
                }
            }
            state.process_input_event(event);
            state.request_redraw();
        })
        .map_err(|err| anyhow!("cannot watch libinput: {err}"))?;

    event_loop
        .handle()
        .insert_source(monitor, |event, _, state: &mut Mriya| match event {
            UdevEvent::Added { device_id, path } => {
                let Ok(node) = DrmNode::from_dev_id(device_id) else {
                    return;
                };
                if let Some(Err(err)) =
                    state.with_udev_backend(|backend, state| backend.add_device(state, node, &path))
                {
                    tracing::warn!(path = %path.display(), %err, "cannot add device");
                }
                state.request_redraw();
            }
            UdevEvent::Changed { device_id } => {
                let Ok(node) = DrmNode::from_dev_id(device_id) else {
                    return;
                };
                let _ = state.with_udev_backend(|backend, state| backend.rescan(state, node));
                state.request_redraw();
            }
            UdevEvent::Removed { device_id } => {
                let Ok(node) = DrmNode::from_dev_id(device_id) else {
                    return;
                };
                let _ =
                    state.with_udev_backend(|backend, state| backend.remove_device(state, node));
                state.request_redraw();
            }
        })
        .map_err(|err| anyhow!("cannot watch udev: {err}"))?;

    event_loop
        .handle()
        .insert_source(notifier, |event, _, state: &mut Mriya| match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused, dropping the DRM state");
                state.with_udev_backend(|backend, _| {
                    backend.libinput.suspend();
                    for device in backend.devices.values_mut() {
                        device.manager.pause();
                    }
                });
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session active again");
                state.with_udev_backend(|backend, _| {
                    let _ = backend.libinput.resume();
                    for device in backend.devices.values_mut() {
                        if let Err(err) = device.manager.activate(false) {
                            tracing::warn!(%err, "cannot reactivate the DRM device");
                        }
                    }
                });
                state.request_redraw();
            }
        })
        .map_err(|err| anyhow!("cannot watch the session: {err}"))?;

    state.request_redraw();

    event_loop
        .run(None, &mut state, |state| {
            state.flush_clients();
            state.spawner.reap();
        })
        .context("event loop failed")
}
