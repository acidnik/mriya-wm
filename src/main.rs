//! Mriya WM - a stacking Wayland window manager.
//!
//! Current scope: run as a nested compositor inside of the current Wayland
//! session (a window, like `niri --nested` does), serve `wl_compositor`,
//! `xdg_wm_base`, `wl_shm` and input, and configure itself from `config.kdl`.

mod backend;
mod config;
mod config_watch;
mod cursor;
mod decor;
mod ext_workspace;
mod grabs;
mod handlers;
mod input;
mod keybindings;
mod layers;
mod shell;
mod spawn;
mod state;
mod workspaces;

use std::{env, path::PathBuf};

use anyhow::{bail, Result};
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing::info;

use crate::{config::Config, keybindings::ModAlias, state::Mriya};

fn main() -> Result<()> {
    init_tracing();

    let args = parse_args()?;
    // A display manager starts us without a Wayland display; there we own the
    // seat and `Mod` means Super. Nested, the parent session usually owns
    // Super, so `Mod` becomes Alt.
    let use_drm = args.drm
        || (env::var_os("WAYLAND_DISPLAY").is_none() && std::path::Path::new("/dev/dri").exists());
    let mod_alias = if use_drm {
        ModAlias::Super
    } else {
        ModAlias::Alt
    };
    if args.list_themes {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        for name in decor::theme::available_themes() {
            // Ignore a closed pipe, `--list-themes | head` is a normal thing to do.
            if writeln!(stdout, "{name}").is_err() {
                break;
            }
        }
        return Ok(());
    }
    let (config, config_path) = load_config(&args, mod_alias);

    // niri-compatible: `environment { ... }` is exported to the session and to
    // every client we start.
    for (name, value) in &config.environment {
        env::set_var(name, value);
    }

    let mut event_loop: EventLoop<Mriya> = EventLoop::try_new()?;
    let display: Display<Mriya> = Display::new()?;
    let mut state = Mriya::new(&mut event_loop, display, config);
    state.mod_alias = mod_alias;
    state.config_path = config_path.clone();

    info!(socket = %state.socket_name.to_string_lossy(), "Mriya WM is ready");

    if !args.command.is_empty() {
        state.spawner.spawn(&args.command);
    }
    for command in state.config.spawn_at_startup.clone() {
        state.spawner.spawn_config(&command);
    }

    if let Some(path) = &config_path {
        if let Err(err) = config_watch::watch(&mut event_loop, path) {
            config_watch::warn_unwatched(path, &err);
        }
    }

    if use_drm {
        info!("starting the DRM backend");
        backend::udev::run(event_loop, state)
    } else {
        backend::nested::run(event_loop, state)
    }
}

fn init_tracing() {
    use std::io::Write as _;
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mriya_wm=debug"));

    // A session started by a display manager has no terminal, so errors are lost
    // unless they also go into a file. `MRIYA_LOG_FILE` overrides the path; a
    // real terminal (a manual `mriya-wm` run) keeps stderr only, unless asked.
    let on_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let path = std::env::var_os("MRIYA_LOG_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| (!on_terminal).then(session_log_path).flatten());

    let file = path.as_ref().and_then(|path| match open_log_file(path) {
        Ok(file) => Some(file),
        Err(err) => {
            eprintln!("mriya-wm: cannot write {}: {err}", path.display());
            None
        }
    });

    let stderr_layer = fmt::layer().with_writer(std::io::stderr);
    match file {
        Some(file) => {
            let path = path.expect("the file came from a path");
            let file_layer = fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file));
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();
            tracing::info!(path = %path.display(), "logging to this file as well");
        }
        None => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .init();
        }
    }
    let _ = std::io::stderr().flush();
}

/// Where a session started without a terminal writes its log.
///
/// `/tmp` is what one looks at after a session failed to start, but a fixed name
/// there is a symlink target someone else could plant before we ever run. The
/// file is therefore named after the user, opened with mode 0600 and only
/// appended to when it is a regular file that we own.
fn session_log_path() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::MetadataExt;

    let user = std::env::var("USER")
        .ok()
        .or_else(|| {
            let home = std::env::var_os("HOME")?;
            std::path::Path::new(&home)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "user".to_string());
    let user: String = user
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();

    let candidate = std::path::PathBuf::from(format!("/tmp/mriya-{user}.log"));
    match std::fs::symlink_metadata(&candidate) {
        Ok(meta) if meta.file_type().is_file() => {
            // Owned by us? Otherwise someone else's file, keep away.
            let ours = std::env::var_os("HOME")
                .and_then(|home| std::fs::metadata(home).ok())
                .map(|home| home.uid());
            if ours != Some(meta.uid()) {
                // Tracing is not initialised yet, so this goes to stderr.
                eprintln!(
                    "mriya-wm: {} belongs to another user, logging to the state directory",
                    candidate.display()
                );
                return state_log_path();
            }
            Some(candidate)
        }
        Ok(_) => {
            eprintln!(
                "mriya-wm: {} is not a regular file, logging to the state directory",
                candidate.display()
            );
            state_log_path()
        }
        Err(_) => Some(candidate),
    }
}

fn state_log_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state"))
        })?;
    let dir = base.join("mriya");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("mriya.log"))
}

/// Open the log for appending, refusing anything that is not a plain file.
fn open_log_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true).mode(0o600);
    // Keep a session log from growing without bound; a failed start is what one
    // looks for, and that is at the top of the current run anyway.
    if std::fs::metadata(path)
        .map(|meta| meta.len() > 8 * 1024 * 1024)
        .unwrap_or(false)
    {
        options.truncate(true).append(false).write(true);
    }
    options.open(path)
}

/// Config shipped with the compositor, written to the user's config directory
/// on the first start so a fresh session is not a black screen.
const DEFAULT_CONFIG: &str = include_str!("../dist/config.kdl");

/// Create `~/.config/mriya/config.kdl` when it does not exist yet.
fn seed_default_config(path: &std::path::Path) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!(path = %parent.display(), %err, "cannot create the config directory");
            return;
        }
    }
    match std::fs::write(path, DEFAULT_CONFIG) {
        Ok(()) => info!(
            path = %path.display(),
            "no config found, wrote the default one"
        ),
        Err(err) => tracing::warn!(
            path = %path.display(),
            %err,
            "cannot write the default config, continuing with built-in defaults"
        ),
    }
}

fn load_config(args: &Args, mod_alias: ModAlias) -> (Config, Option<PathBuf>) {
    let path = args.config.clone().or_else(config::default_path);
    if args.config.is_none() {
        if let Some(path) = &path {
            seed_default_config(path);
        }
    }

    match &args.config {
        Some(explicit) => {
            let config = config::load_or_default(Some(explicit), mod_alias);
            if !explicit.exists() {
                tracing::warn!(path = %explicit.display(), "--config points at a missing file");
            }
            (config, Some(explicit.clone()))
        }
        None => (config::load_or_default(path.as_deref(), mod_alias), path),
    }
}

struct Args {
    command: Vec<String>,
    config: Option<PathBuf>,
    list_themes: bool,
    /// Run on a TTY through DRM/udev instead of nested in another session.
    drm: bool,
}

fn parse_args() -> Result<Args> {
    let mut command = Vec::new();
    let mut config = None;
    let mut list_themes = false;
    let mut drm = false;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--command" => command.extend(args.by_ref()),
            "--config" => match args.next() {
                Some(path) => config = Some(PathBuf::from(path)),
                None => bail!("--config requires a path"),
            },
            "--list-themes" => list_themes = true,
            "--drm" => drm = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other} (see --help)"),
        }
    }

    Ok(Args {
        command,
        config,
        list_themes,
        drm,
    })
}

fn print_help() {
    println!(
        "mriya-wm {version}

A stacking Wayland window manager.

USAGE:
    mriya-wm [OPTIONS]

OPTIONS:
    --config <PATH>                     Config file (default: $XDG_CONFIG_HOME/mriya/config.kdl)
    --drm                               Run on a TTY through DRM/udev (a display
                                        manager starts the compositor this way)
    --list-themes                       Print the xfwm4 themes found and exit
    -c, --command <PROGRAM> [ARGS...]   Start a client after the compositor is up
    -h, --help                          Print this help

The nested backend needs a running Wayland session (WAYLAND_DISPLAY)
and opens its own window in it.
",
        version = env!("CARGO_PKG_VERSION")
    );
}
