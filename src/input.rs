//! Input event processing.
//!
//! The nested backend forwards whatever the host session gives us; for the
//! pointer that means absolute positions inside our window.

use std::time::{Duration, Instant};

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, Keycode, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, Focus, MotionEvent},
    },
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point, Rectangle, Serial, SERIAL_COUNTER},
};

use crate::{
    decor::{self, FrameButton, FramePart},
    grabs::{self, MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
    keybindings::Action,
    shell,
    state::Mriya,
};

/// Two clicks closer than this in time and space count as a double click.
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_DISTANCE: f64 = 6.0;

/// Left mouse button (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
/// Right mouse button (linux/input-event-codes.h).
const BTN_RIGHT: u32 = 0x111;

impl Mriya {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let time = Event::time_msec(&event);
                if let Some(actions) = self.keyboard_actions(event.key_code(), event.state(), time)
                {
                    self.apply_actions(actions);
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next().cloned() else {
                    return;
                };
                let Some(output_geo) = self.space.output_geometry(&output) else {
                    return;
                };

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                self.pointer_motion_to(pos, event.time_msec());
                self.update_frame_hover();
            }
            // Relative motion only matters for the winit backend when the host
            // sends it; the pointer position comes from absolute events there.
            InputEvent::PointerMotion { .. } => {}
            InputEvent::PointerButton { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();
                let button = event.button_code();
                let button_state = event.state();
                let location = pointer.current_location();

                if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
                    let modifiers = keyboard.modifier_state();
                    if modifiers.logo && self.start_wm_grab(button, serial) {
                        pointer.frame(self);
                        return;
                    }

                    if self.frame_button_pressed(button, serial, location) {
                        pointer.frame(self);
                        return;
                    }

                    if let Some((window, _)) = self.window_under(location) {
                        self.focus_window(&window);
                    } else {
                        self.clear_focus(serial);
                    }
                }

                if button_state == ButtonState::Released && self.frame_button_released(button) {
                    pointer.frame(self);
                    return;
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }
                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }

    fn pointer_motion_to(&mut self, pos: Point<f64, Logical>, time: u32) {
        tracing::trace!(?pos, "pointer motion");
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.seat.get_pointer().unwrap();

        self.pointer_location = pos;
        // Re-resolve the surface under the pointer: the pointer may have a grab,
        // in which case the grab decides how focus changes.
        let under = self.surface_under(pos);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial,
                time,
            },
        );
        pointer.frame(self);
    }

    /// Start a `Mod`-initiated move or resize grab. Returns `true` when a grab
    /// was started (and the button must not reach the client).
    fn start_wm_grab(&mut self, button: u32, serial: Serial) -> bool {
        let location = self.pointer_location;
        let Some((window, window_location)) = self.window_under(location) else {
            return false;
        };

        self.focus_window(&window);
        let start_data = grabs::start_data_for(self, button);

        match button {
            BTN_LEFT => {
                let grab = MoveSurfaceGrab {
                    start_data,
                    window,
                    initial_window_location: window_location,
                };
                self.seat
                    .get_pointer()
                    .unwrap()
                    .set_grab(self, grab, serial, Focus::Clear);
                true
            }
            BTN_RIGHT => {
                let geometry = window.geometry();
                let grab = ResizeSurfaceGrab::start(
                    start_data,
                    window,
                    ResizeEdge::BOTTOM_RIGHT,
                    smithay::utils::Rectangle::new(window_location, geometry.size),
                );
                self.seat
                    .get_pointer()
                    .unwrap()
                    .set_grab(self, grab, serial, Focus::Clear);
                true
            }
            _ => false,
        }
    }

    /// Handle a button press that lands on a window frame. Returns `true` when
    /// the frame consumed the event.
    fn frame_button_pressed(
        &mut self,
        button: u32,
        serial: Serial,
        location: Point<f64, Logical>,
    ) -> bool {
        let Some(hit) = decor::hit_test(
            self.theme.as_ref(),
            &mut self.text,
            &mut self.icons,
            &self.space,
            location,
        ) else {
            return false;
        };
        if hit.part == FramePart::Content {
            return false;
        }

        self.focus_window(&hit.window);

        match hit.part {
            FramePart::Content => false,
            FramePart::Button(frame_button) => {
                if button == BTN_LEFT {
                    decor::state_mut(&hit.window).pressed = Some(frame_button);
                    self.frame_press = Some((hit.window.clone(), frame_button));
                    self.request_redraw();
                }
                true
            }
            FramePart::Titlebar => {
                if button != BTN_LEFT {
                    return true;
                }

                let now = Instant::now();
                let double_click = self
                    .last_titlebar_click
                    .map(|(time, position)| {
                        now.duration_since(time) < DOUBLE_CLICK_TIME
                            && (position.x - location.x).abs() < DOUBLE_CLICK_DISTANCE
                            && (position.y - location.y).abs() < DOUBLE_CLICK_DISTANCE
                    })
                    .unwrap_or(false);
                self.last_titlebar_click = Some((now, location));

                if double_click {
                    self.last_titlebar_click = None;
                    shell::toggle_maximized(self.theme.as_ref(), &mut self.space, &hit.window);
                    self.request_redraw();
                    return true;
                }

                let grab = MoveSurfaceGrab {
                    start_data: grabs::start_data_at(location, button),
                    window: hit.window.clone(),
                    initial_window_location: hit.content_location,
                };
                self.seat
                    .get_pointer()
                    .unwrap()
                    .set_grab(self, grab, serial, Focus::Clear);
                true
            }
            FramePart::Edge(edge) => {
                if button != BTN_LEFT {
                    return true;
                }

                let grab = ResizeSurfaceGrab::start(
                    grabs::start_data_at(location, button),
                    hit.window.clone(),
                    edge,
                    Rectangle::new(hit.content_location, hit.content_size),
                );
                self.seat
                    .get_pointer()
                    .unwrap()
                    .set_grab(self, grab, serial, Focus::Clear);
                true
            }
        }
    }

    /// Resolve a titlebar button press. Returns `true` when the frame consumed
    /// the release.
    fn frame_button_released(&mut self, button: u32) -> bool {
        let Some((window, pressed)) = self.frame_press.take() else {
            return false;
        };

        let hovered = decor::state(&window).hover;
        decor::state_mut(&window).pressed = None;
        self.request_redraw();

        if button == BTN_LEFT && hovered == Some(pressed) {
            match pressed {
                FrameButton::Close => {
                    if let Some(toplevel) = window.toplevel() {
                        toplevel.send_close();
                    }
                }
                FrameButton::Maximize => {
                    shell::toggle_maximized(self.theme.as_ref(), &mut self.space, &window)
                }
                // No minimisation yet: the button is drawn but does nothing.
                FrameButton::Hide => {}
            }
        }
        true
    }

    /// Track which titlebar button the pointer is over, for hover states.
    fn update_frame_hover(&mut self) {
        let hit = decor::hit_test(
            self.theme.as_ref(),
            &mut self.text,
            &mut self.icons,
            &self.space,
            self.pointer_location,
        );
        let (hovered_window, hovered_button) = match &hit {
            Some(hit) => match hit.part {
                FramePart::Button(button) => (Some(hit.window.clone()), Some(button)),
                _ => (None, None),
            },
            None => (None, None),
        };

        let mut changed = false;
        for window in self.space.elements() {
            let want = if Some(window) == hovered_window.as_ref() {
                hovered_button
            } else {
                None
            };
            let mut frame = decor::state_mut(window);
            if frame.hover != want {
                frame.hover = want;
                changed = true;
            }
        }

        if changed {
            self.request_redraw();
        }
    }

    fn clear_focus(&mut self, serial: Serial) {
        for window in self.space.elements() {
            decor::state_mut(window).active = false;
            let Some(toplevel) = window.toplevel() else {
                continue;
            };
            toplevel.with_pending_state(|state| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
                state.states.unset(xdg_toplevel::State::Activated);
            });
            toplevel.send_pending_configure();
        }

        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, Option::<WlSurface>::None, serial);
        self.request_redraw();
    }

    /// Feed a key event to the focused client, intercepting our shortcuts.
    fn keyboard_actions(
        &mut self,
        keycode: Keycode,
        state: KeyState,
        time: u32,
    ) -> Option<Vec<Action>> {
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().unwrap();
        tracing::trace!(
            ?state,
            keycode = u32::from(keycode),
            focus = ?keyboard.current_focus().map(|surface| format!("{:?}", surface.id())),
            "key event"
        );
        let mut suppressed: Vec<Keycode> = std::mem::take(&mut self.suppressed_keys);
        // Bindings are matched against the US layout, so `Mod+t` keeps working
        // when the Russian layout is selected.
        let binding_keysym = self.binding_keysym(keycode);
        // The filter closure gets `&mut Mriya`, so the table is moved out for
        // the duration of the call instead of being borrowed from it.
        let keybindings = std::mem::take(&mut self.keybindings);

        let action = keyboard.input(
            self,
            keycode,
            state,
            serial,
            time,
            |_, modifiers, handle| {
                tracing::trace!(
                    keysym = ?handle.modified_sym(),
                    ?binding_keysym,
                    ?modifiers,
                    "input filter"
                );
                match state {
                    KeyState::Pressed => {
                        match binding_keysym
                            .and_then(|keysym| keybindings.action_for(modifiers, keysym))
                        {
                            Some(actions) => {
                                suppressed.push(keycode);
                                FilterResult::Intercept(actions)
                            }
                            None => FilterResult::Forward,
                        }
                    }
                    KeyState::Released => {
                        if suppressed.contains(&keycode) {
                            suppressed.retain(|key| *key != keycode);
                            FilterResult::Intercept(Vec::new())
                        } else {
                            FilterResult::Forward
                        }
                    }
                }
            },
        );

        self.suppressed_keys = suppressed;
        self.keybindings = keybindings;
        action
    }

    fn apply_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            self.run_action(action);
        }
    }
}
