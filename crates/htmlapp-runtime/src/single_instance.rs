//! Keeping the launcher single-instance (docs/architecture.md).
//!
//! "The launcher is single-instance. Opening a file from it spawns a detached child; the launcher
//! stays up." Documents are the opposite — every one gets its own process — so this guard is only
//! ever applied to the launcher.
//!
//! An advisory `flock` on a file in the runtime directory. The lock is released by the kernel when
//! The process exits, however it exits, so a crashed launcher leaves nothing to clean up — which a
//! pid file would not manage.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

/// Held for the lifetime of the process. Dropping it releases the lock.
pub struct InstanceLock {
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// `$XDG_RUNTIME_DIR/htmlapp/launcher.lock`, falling back to the cache directory.
fn lock_path() -> PathBuf {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("htmlapp")
        .join("launcher.lock")
}

/// Try to become the one launcher.
///
/// Returns `None` when another launcher already holds the lock, in which case the caller should
/// exit rather than open a second window.
pub fn acquire() -> Option<InstanceLock> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let file = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .ok()?;

    // `flock` is used through the `flock` binary rather than libc so this crate stays free of
    // `unsafe`; the lock is taken on a duplicated descriptor that the child inherits... which would
    // not outlive the child. So it is done directly instead.
    if !try_lock(&file) {
        return None;
    }

    Some(InstanceLock { file, path })
}

/// Take a non-blocking exclusive lock.
#[cfg(target_os = "linux")]
fn try_lock(file: &File) -> bool {
    // `flock(2)` via the raw syscall interface `rustix` exposes, which is a safe wrapper.
    rustix::fs::flock(
        unsafe_fd(file),
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .is_ok()
}

/// `rustix` takes a borrowed descriptor; this is the safe conversion for one.
#[cfg(target_os = "linux")]
fn unsafe_fd(file: &File) -> std::os::fd::BorrowedFd<'_> {
    use std::os::fd::AsFd;
    let _ = file.as_raw_fd();
    file.as_fd()
}

#[cfg(not(target_os = "linux"))]
fn try_lock(_file: &File) -> bool {
    true
}
