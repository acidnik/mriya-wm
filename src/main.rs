//! Mriya WM - a stacking Wayland window manager.
//!
//! Current scope: run as a nested compositor inside of the current Wayland
//! session (a window, like `niri --nested` does), serve `wl_compositor`,
//! `xdg_wm_base`, `wl_shm` and input, and configure itself from `config.kdl`.

mod backend;
mod config;
mod cursor;
mod grabs;
mod handlers;
mod input;
mod keybindings;
mod shell;
mod spawn;
mod state;

use std::{env, path::PathBuf};

use anyhow::{bail, Result};
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing::info;

use crate::{config::Config, keybindings::ModAlias, state::Mriya};

fn main() -> Result<()> {
    init_tracing();

    let args = parse_args()?;
    let config = load_config(&args);

    // niri-compatible: `environment { ... }` is exported to the session and to
    // every client we start.
    for (name, value) in &config.environment {
        env::set_var(name, value);
    }

    let mut event_loop: EventLoop<Mriya> = EventLoop::try_new()?;
    let display: Display<Mriya> = Display::new()?;
    let mut state = Mriya::new(&mut event_loop, display, config);

    info!(socket = %state.socket_name.to_string_lossy(), "Mriya WM is ready");

    if !args.command.is_empty() {
        state.spawner.spawn(&args.command);
    }
    for command in state.config.spawn_at_startup.clone() {
        state.spawner.spawn_config(&command);
    }

    backend::nested::run(event_loop, state)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mriya_wm=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn load_config(args: &Args) -> Config {
    // `Mod` is Alt while nested: the session we run in usually binds Super.
    let mod_alias = ModAlias::Alt;
    let path = args.config.clone().or_else(config::default_path);

    match &args.config {
        Some(explicit) => {
            let config = config::load_or_default(Some(explicit), mod_alias);
            if !explicit.exists() {
                tracing::warn!(path = %explicit.display(), "--config points at a missing file");
            }
            config
        }
        None => config::load_or_default(path.as_deref(), mod_alias),
    }
}

struct Args {
    command: Vec<String>,
    config: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut command = Vec::new();
    let mut config = None;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--command" => command.extend(args.by_ref()),
            "--config" => match args.next() {
                Some(path) => config = Some(PathBuf::from(path)),
                None => bail!("--config requires a path"),
            },
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other} (see --help)"),
        }
    }

    Ok(Args { command, config })
}

fn print_help() {
    println!(
        "mriya-wm {version}

A stacking Wayland window manager.

USAGE:
    mriya-wm [OPTIONS]

OPTIONS:
    --config <PATH>                     Config file (default: $XDG_CONFIG_HOME/mriya/config.kdl)
    -c, --command <PROGRAM> [ARGS...]   Start a client after the compositor is up
    -h, --help                          Print this help

The nested backend needs a running Wayland session (WAYLAND_DISPLAY)
and opens its own window in it.
",
        version = env!("CARGO_PKG_VERSION")
    );
}
