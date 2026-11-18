//! Starting and reaping client processes.

use std::{
    env,
    path::PathBuf,
    process::{Child, Command},
};

use tracing::{debug, info, warn};

/// Starts clients and makes sure their exit is reaped, so the compositor does
/// not collect zombies over a long session.
pub struct Spawner {
    socket_name: String,
    children: Vec<Child>,
}

impl Spawner {
    pub fn new(socket_name: String) -> Self {
        Self {
            socket_name,
            children: Vec::new(),
        }
    }

    /// Run a program with its arguments.
    pub fn spawn(&mut self, argv: &[String]) {
        let Some((program, args)) = argv.split_first() else {
            return;
        };
        let mut command = Command::new(expand_home(program));
        command.args(args);
        self.launch(command, program);
    }

    /// Run a command line through `sh -c`.
    pub fn spawn_shell(&mut self, command_line: &str) {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(command_line);
        self.launch(command, command_line);
    }

    /// `spawn-at-startup` semantics: a single string is a command line, several
    /// arguments are a program with its own arguments.
    pub fn spawn_config(&mut self, argv: &[String]) {
        match argv {
            [command_line] if command_line.contains(' ') => self.spawn_shell(command_line),
            _ => self.spawn(argv),
        }
    }

    /// Collect exit statuses of finished children.
    pub fn reap(&mut self) {
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(status)) => {
                debug!(pid = child.id(), ?status, "client exited");
                false
            }
            Ok(None) => true,
            Err(err) => {
                warn!(pid = child.id(), %err, "failed to query client status");
                false
            }
        });
    }

    fn launch(&mut self, mut command: Command, description: &str) {
        // Clients have to talk to us, not to the session we are running in.
        command.env("WAYLAND_DISPLAY", &self.socket_name);

        match command.spawn() {
            Ok(child) => {
                info!(pid = child.id(), command = description, "started client");
                self.children.push(child);
            }
            Err(err) => warn!(command = description, %err, "failed to start client"),
        }
    }
}

/// Terminals tried by `spawn-terminal`, in order of preference.
const TERMINALS: [&str; 7] = [
    "foot",
    "ghostty",
    "kitty",
    "wezterm",
    "alacritty",
    "xfce4-terminal",
    "xterm",
];

/// Command for `spawn-terminal`: `$TERMINAL` when set, otherwise the first known
/// terminal found in `PATH`.
pub fn default_terminal() -> Option<Vec<String>> {
    if let Ok(terminal) = env::var("TERMINAL") {
        let mut words = terminal.split_whitespace().map(str::to_string);
        if let Some(program) = words.next() {
            return Some(std::iter::once(program).chain(words).collect());
        }
    }

    let search_path: Vec<PathBuf> = env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).collect())
        .unwrap_or_default();
    find_terminal(&search_path).map(|program| vec![program])
}

/// First terminal from [`TERMINALS`] present in one of `search_path`.
fn find_terminal(search_path: &[PathBuf]) -> Option<String> {
    TERMINALS
        .iter()
        .find(|program| {
            search_path
                .iter()
                .any(|directory| directory.join(program).is_file())
        })
        .map(|program| (*program).to_string())
}

/// `~` at the start of a program path means the home directory, like in niri.
fn expand_home(program: &str) -> String {
    if let Some(rest) = program.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            let mut path = home;
            path.push("/");
            path.push(rest);
            return path.to_string_lossy().into_owned();
        }
    }
    program.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_terminal_in_the_search_path() {
        let dir = env::temp_dir().join(format!("mriya-tty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kitty"), b"#!/bin/sh\n").unwrap();

        assert_eq!(find_terminal(&[dir.clone()]), Some("kitty".to_string()));
        assert_eq!(find_terminal(&[]), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
