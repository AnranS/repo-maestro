//! Subprocess teardown helpers.
//!
//! Adapters and the acceptance-check runner spawn a coding tool or shell that
//! in turn spawns its *own* children (the real editors, `cargo test`, dev
//! servers, …). `kill_on_drop(true)` only signals the **direct** child, so on a
//! timeout those grandchildren are orphaned and keep running — holding ports,
//! locks, and CPU long after maestro has given up on the task.
//!
//! Spawning the child in its own process group lets a timeout SIGKILL the whole
//! subtree at once. This is Unix-only; on other platforms the functions are
//! no-ops and we fall back to `kill_on_drop`.

use tokio::process::Command;

/// Put `cmd`'s child in a fresh process group so the entire descendant tree can
/// be signalled together. Call before `spawn()`.
pub fn isolate_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        // 0 => the child becomes leader of a new group whose pgid equals its pid.
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

/// Best-effort SIGKILL of the whole process group led by `pid` (the direct
/// child's pid, which equals the pgid after [`isolate_process_group`]). Use on
/// timeout to tear down the child *and* everything it spawned.
pub fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        // A negative pid targets the process group in kill(2).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}
