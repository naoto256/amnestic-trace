//! Self-daemonization by double fork. macOS ships no `setsid(1)`, so the
//! detach is done in-process; it completes before any heavy initialization so
//! the window in which the host can kill the worker stays narrow.

/// Returns `true` in the process that should do the work.
///
/// The caller returns immediately in the original process, so the hook exits
/// while extraction runs in parallel with compaction itself. If forking fails
/// we return `true` in the original process: doing the work inline is worse
/// than detached but still better than losing the snapshot.
pub fn detach() -> bool {
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
            libc::dup2(devnull, 2);
            if devnull > 2 {
                libc::close(devnull);
            }
        }
    }
    true
}
