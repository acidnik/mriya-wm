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

Current state (iteration 1): nested compositor, runs as a window inside the
current Wayland session and shows client windows without decorations.

```sh
cargo run -- -c foot
```

See [docs/architecture.md](docs/architecture.md) for the code layout, keybindings
and next steps (the doc is in Russian).

