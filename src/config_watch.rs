//! Reloading `config.kdl` when it changes.
//!
//! The directory holding the config is watched instead of the file itself:
//! editors write a temporary file and rename it over the original, which breaks
//! a watch on the inode.

use std::{
    cell::RefCell,
    ffi::OsString,
    os::fd::{AsFd, BorrowedFd},
    path::Path,
};

use anyhow::{Context, Result};
use inotify::{EventMask, Inotify, WatchMask};
use smithay::reexports::calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction};
use tracing::{debug, info, warn};

use crate::state::Mriya;

/// Events that mean "the file was written": a normal save, or a save through a
/// temporary file and rename.
const WATCH_MASK: WatchMask = WatchMask::CLOSE_WRITE
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::CREATE)
    .union(WatchMask::DELETE);

/// Watch `path` and reload the config in `Mriya` when it changes.
pub fn watch(event_loop: &mut EventLoop<Mriya>, path: &Path) -> Result<()> {
    let directory = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    let directory = directory.unwrap_or_else(|| Path::new(".")).to_path_buf();
    let file_name: Option<OsString> = path.file_name().map(|name| name.to_os_string());

    let inotify = Inotify::init().context("failed to initialize inotify")?;
    // Non-blocking, so draining the queue inside the event source never stalls
    // the compositor.
    rustix_ioctl_nonblocking(&inotify)?;

    inotify
        .watches()
        .add(&directory, WATCH_MASK)
        .with_context(|| format!("failed to watch {}", directory.display()))?;

    let fd = inotify
        .as_fd()
        .try_clone_to_owned()
        .context("failed to duplicate the inotify descriptor")?;
    let watcher = Watcher {
        inotify: RefCell::new(inotify),
        fd,
        file_name,
    };

    event_loop
        .handle()
        .insert_source(
            Generic::new(watcher, Interest::READ, Mode::Level),
            move |_, watcher, state| {
                let mut buffer = [0u8; 4096];
                let mut inotify = watcher.inotify.borrow_mut();
                let Ok(events) = inotify.read_events(&mut buffer) else {
                    return Ok(PostAction::Continue);
                };

                let relevant = events.into_iter().any(|event| {
                    if !event.mask.intersects(
                        EventMask::CLOSE_WRITE | EventMask::MOVED_TO | EventMask::CREATE,
                    ) {
                        return false;
                    }
                    match (&watcher.file_name, event.name) {
                        (Some(watched), Some(name)) => name == watched.as_os_str(),
                        // An event without a name is about the directory itself.
                        _ => false,
                    }
                });

                if relevant {
                    state.reload_config();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| anyhow::anyhow!("failed to insert the config watcher: {err}"))?;

    info!(path = %path.display(), "watching the config file for changes");
    Ok(())
}

/// The inotify handle inside its calloop source.
///
/// calloop hands out `&NoIoDrop<T>` (it does not allow moving the handle out of
/// the source), so the handle is behind a `RefCell`: the source owns it, the
/// callback borrows it mutably.
struct Watcher {
    inotify: RefCell<Inotify>,
    /// Duplicate of the inotify descriptor, so `AsFd` can hand out a borrow.
    fd: std::os::fd::OwnedFd,
    file_name: Option<OsString>,
}

impl std::os::fd::AsFd for Watcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// `O_NONBLOCK` on the inotify descriptor.
fn rustix_ioctl_nonblocking(inotify: &Inotify) -> Result<()> {
    use smithay::reexports::rustix;
    rustix::io::ioctl_fionbio(inotify, true).context("failed to make inotify non-blocking")?;
    debug!("inotify is watching in non-blocking mode");
    Ok(())
}

/// Warn once, when the config directory could not be watched.
pub fn warn_unwatched(path: &Path, error: &anyhow::Error) {
    warn!(path = %path.display(), %error, "config changes will not be picked up");
}
