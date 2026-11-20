//! Process hygiene for a root helper started by `pkexec`, which hands over the caller's umask,
//! controlling terminal and resource limits as they are. None of that is trustworthy.

/// libalpm creates downloaded packages as `~umask & 0666`, owned by root. A caller who started
/// the store with umask 0 would leave package files anyone can rewrite between the signature
/// check and the extraction. `sudo` forces at least 022; `pkexec` does not.
pub(crate) fn set_umask() {
    // SAFETY: umask has no preconditions; it only changes this process's creation mask.
    unsafe { libc::umask(0o022) };
}

fn set_limit(resource: libc::__rlimit_resource_t, soft: libc::rlim_t, hard: libc::rlim_t) {
    let limit = libc::rlimit {
        rlim_cur: soft,
        rlim_max: hard,
    };
    // SAFETY: `limit` is a valid rlimit and outlives the call. Failure is ignored on purpose:
    // the process then simply keeps the limit it inherited.
    unsafe { libc::setrlimit(resource, &limit) };
}

/// A caller-chosen file size, CPU time or descriptor limit must not be able to kill or starve
/// the helper in the middle of a transaction.
fn reset_limits() {
    for resource in [
        libc::RLIMIT_FSIZE,
        libc::RLIMIT_CPU,
        libc::RLIMIT_AS,
        libc::RLIMIT_DATA,
    ] {
        set_limit(resource, libc::RLIM_INFINITY, libc::RLIM_INFINITY);
    }
    set_limit(libc::RLIMIT_NOFILE, 4096, 1_048_576);
    set_limit(libc::RLIMIT_CORE, 0, 0);
}

/// Signals a terminal (or whoever owns it) can send to the foreground process group: they must
/// not stop or kill the helper while it holds the database lock.
fn ignore_job_control_signals() {
    for signal in [
        libc::SIGQUIT,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGXFSZ,
    ] {
        // SAFETY: setting a signal's disposition to "ignore" has no preconditions.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
}

/// Leaves the caller's terminal session, so terminal-generated signals no longer reach us.
/// Fails harmlessly if the process already leads its own session.
fn detach_session() {
    // SAFETY: setsid has no preconditions.
    unsafe { libc::setsid() };
}

/// Everything above, first thing in `main`.
pub fn harden_process() {
    set_umask();
    reset_limits();
    ignore_job_control_signals();
    detach_session();
}
