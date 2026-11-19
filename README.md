Mriya WM - stacking Wayland window manager

Goals:
support xfce4 themes (xfwm4-themes /usr/share/themes/)
support xdg autostart
manage keyboard layout
manage mouse options and themes

Non-goals:
This is not a desktop environment, we implement only the window management and related features. We aim to reuse existing apps, panels, applets and so on as much as possible

Stack:
Rust with smithay
config in kdl

Current state: nested compositor, runs as a window inside the current Wayland
session and shows client windows without decorations. Configuration is KDL and
uses the same section names as niri, so an existing niri config can be copied.

```sh
cargo run                              # uses ~/.config/mriya/config.kdl
cargo run -- --config ./my.kdl         # explicit config
cargo run -- -c foot                   # also start a client
```

Start from [docs/example-config.kdl](docs/example-config.kdl). Cursor theme and
size, `input { keyboard { xkb } }`, `environment`, `spawn-at-startup` and `binds`
are implemented; other niri sections are ignored for now.
See [docs/architecture.md](docs/architecture.md) for the code layout, keybindings
and next steps (the doc is in Russian).

Panels: `zwlr_layer_shell_v1` is supported (panels, bars, wallpapers reserve
their space), so waybar works. A compositor-agnostic waybar config and style
live in [examples/waybar](examples/waybar).

## Standalone session

Right now Mriya only has the nested backend: it runs as a window inside another
session. Running it from a display manager needs the DRM/udev backend (see
[docs/architecture.md](docs/architecture.md#следующие-шаги)), which is not
written yet. The session plumbing is ready in [dist](dist):

```sh
sudo install -Dm755 dist/mriya-session /usr/local/bin/mriya-session
sudo install -Dm644 dist/mriya.desktop /usr/share/wayland-sessions/mriya.desktop
```

After that the greeter lists "Mriya" next to the other sessions. The compositor
also has to detect that it was started on a VT and drive the DRM device then
instead of opening a window.

