//! Mriya WM - a stacking Wayland window manager.
//!
//! Iteration 1 scope: run as a nested compositor inside of the current
//! Wayland session (a window, like `niri --nested` does), serve `wl_compositor`,
//! `xdg_wm_base` and `wl_shm`, and display client windows without decorations.

mod backend;
mod grabs;
mod handlers;
mod input;
mod keybindings;
mod shell;
mod state;

use std::{env, process::Command};

use anyhow::{anyhow, bail, Result};
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing::{info, warn};

use crate::state::Mriya;

fn main() -> Result<()> {
    init_tracing();

    let args = parse_args()?;

    let mut event_loop: EventLoop<Mriya> = EventLoop::try_new()?;
    let display: Display<Mriya> = Display::new()?;
    let state = Mriya::new(&mut event_loop, display);

    let socket_name = state.socket_name.to_string_lossy().to_string();
    info!(socket = %socket_name, "Mriya WM is ready");

    if !args.command.is_empty() {
        spawn_client(&args.command, &socket_name)?;
    }

    backend::nested::run(event_loop, state)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mriya_wm=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

struct Args {
    command: Vec<String>,
}

fn parse_args() -> Result<Args> {
    let mut command = Vec::new();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--command" => command.extend(args.by_ref()),
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other} (see --help)"),
        }
    }

    Ok(Args { command })
}

fn print_help() {
    println!(
        "mriya-wm {version}

A stacking Wayland window manager.

USAGE:
    mriya-wm [OPTIONS]

OPTIONS:
    -c, --command <PROGRAM> [ARGS...]   Start a client after the compositor is up
    -h, --help                          Print this help

The nested backend needs a running Wayland session (WAYLAND_DISPLAY)
and opens its own window in it.
",
        version = env!("CARGO_PKG_VERSION")
    );
}

fn spawn_client(argv: &[String], socket_name: &str) -> Result<()> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| anyhow!("--command requires a program name"))?;

    let mut cmd = Command::new(program);
    cmd.args(rest);
    // The client connects to us, not to the parent session compositor.
    cmd.env("WAYLAND_DISPLAY", socket_name);
    match cmd.spawn() {
        Ok(child) => {
            info!(program, pid = child.id(), "started client");
            Ok(())
        }
        Err(err) => {
            warn!(program, %err, "failed to start client");
            Err(err.into())
        }
    }
}
