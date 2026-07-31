//! Detaching from the host by double fork. macOS ships no `setsid(1)`, so it
//! is done in-process; it completes before any heavy initialization so the
//! window in which the host can kill the worker stays narrow.
//!
//! The worker this produces is not a daemon: it does one extraction and exits.
//! Cutting it loose from the host's process group is the whole point.

use std::path::Path;

/// Past this the log is truncated. It exists to explain the last failure, not
/// to accumulate history — this tool keeps no history of anything else either.
const MAX_LOG_BYTES: u64 = 256 * 1024;

/// Which side of the fork the caller is on.
pub enum Role {
    /// The hook's own process. Return immediately.
    Caller,
    /// Detached, reparented, and free to take as long as it needs.
    Worker,
    /// Forking failed. There is no safe way to continue here — see `synthesize`.
    CannotDetach,
}

/// Points stderr at the log, creating the directory if it does not exist.
///
/// Called *before* anything that can fail, not after the fork. The store's own
/// setup is exactly what fails when the directory is unwritable, and a hook
/// discards this process's output, so a failure before the redirect leaves no
/// evidence anywhere on disk: the memory is dead and nothing says so.
///
/// Best-effort by nature. If the log itself cannot be opened there is nowhere
/// left to complain to, and failing the synthesize over it would trade a
/// missing diagnostic for a missing snapshot.
pub fn log_stderr_to(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let log_path = dir.join("amtr.log");
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::remove_file(&log_path);
    }

    let mut buf = log_path.as_os_str().as_encoded_bytes().to_vec();
    buf.push(0);
    unsafe {
        let fd = libc::open(
            buf.as_ptr() as *const libc::c_char,
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
            0o600 as libc::c_uint,
        );
        if fd >= 0 {
            libc::dup2(fd, 2);
            if fd > 2 {
                libc::close(fd);
            }
        }
    }
}

/// Detaches the worker from the host's process group.
///
/// The caller returns immediately in the original process, so the hook exits
/// while extraction runs in parallel with compaction itself.
///
/// # Assumes a single-threaded process
///
/// `fork` carries over only the calling thread. A lock held by any other thread
/// at that instant stays locked forever in the child, and the allocator's is
/// enough to hang it on the next allocation. Nothing before this point starts a
/// thread today — the work that does (the extraction subprocess's reader and
/// writer) happens after — and anything that changes must keep it that way, or
/// move the fork ahead of itself.
pub fn detach() -> Role {
    unsafe {
        match libc::fork() {
            -1 => return Role::CannotDetach,
            0 => {}
            _ => return Role::Caller, // original process: hook returns now
        }
        // New session: we are no longer in the host's process group, so a
        // group-wide kill on hook timeout does not reach us.
        libc::setsid();
        match libc::fork() {
            -1 => {}
            0 => {}
            _ => libc::_exit(0), // reparented to init, so nobody waits on us
        }
        // Close the hook's stdin and stdout; a host that reads them to EOF
        // would otherwise block on a worker that outlives it. stderr is left
        // alone: it already points at the log, opened before any of this.
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if devnull >= 0 {
            libc::dup2(devnull, 0);
            libc::dup2(devnull, 1);
            if devnull > 2 {
                libc::close(devnull);
            }
        }
    }
    Role::Worker
}
