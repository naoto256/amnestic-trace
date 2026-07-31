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

/// Returns `true` in the process that should do the work.
///
/// The caller returns immediately in the original process, so the hook exits
/// while extraction runs in parallel with compaction itself. If forking fails
/// we return `true` in the original process: doing the work inline is worse
/// than detached but still better than losing the snapshot.
///
/// `log_dir` receives the worker's stderr. Everything after this point is
/// invisible by construction — no terminal, no exit status anyone reads, and a
/// design that swallows failures on purpose — so without somewhere for the
/// diagnostics to land, "the memory silently stopped working" has no evidence
/// behind it at all.
pub fn detach(log_dir: &Path) -> bool {
    let log_path = log_dir.join("amtr.log");
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::remove_file(&log_path);
    }

    unsafe {
        match libc::fork() {
            -1 => return true,
            0 => {}
            _ => return false, // original process: hook returns now
        }
        // New session: we are no longer in the host's process group, so a
        // group-wide kill on hook timeout does not reach us.
        libc::setsid();
        match libc::fork() {
            -1 => {}
            0 => {}
            _ => libc::_exit(0), // reparented to init, so nobody waits on us
        }
        // Close the hook's stdio pipes; a host that reads them to EOF would
        // otherwise block on a worker that outlives it.
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if devnull >= 0 {
            libc::dup2(devnull, 0);
            libc::dup2(devnull, 1);
            if devnull > 2 {
                libc::close(devnull);
            }
        }

        // stderr goes to the log instead. Opened owner-only, appending, so
        // concurrent workers interleave whole writes rather than overwrite.
        let mut buf = log_path.as_os_str().as_encoded_bytes().to_vec();
        buf.push(0);
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
        } else if devnull >= 0 {
            libc::dup2(devnull, 2);
        }
    }
    true
}
