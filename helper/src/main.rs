//! `store-helper`: the one privileged program of linux-store. Run it through `pkexec`.
use std::io::{BufReader, Write};
use std::sync::Arc;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use store_helper::{CancelGate, Environment, ProcessAborter, ensure_root, harden_process, serve};
use store_protocol::{EXIT_FAILED, ErrorCode, Event};

/// Setup failed before a request was read: say so as an event, like every other answer.
fn refuse(message: &str) -> i32 {
    let line = Event::Error {
        code: ErrorCode::Failed,
        message: message.to_string(),
    }
    .to_line();
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    EXIT_FAILED
}

fn run() -> i32 {
    // First: pkexec hands over the caller's umask, terminal and resource limits as they are.
    harden_process();

    // SAFETY: this is the first thing the process does, before any other thread exists.
    // A fixed PATH keeps `pacman-conf` (run by the configuration reader) out of reach of
    // whatever the caller's environment says.
    unsafe { std::env::set_var("PATH", "/usr/sbin:/usr/bin") };

    // SAFETY: geteuid has no preconditions.
    if let Err(e) = ensure_root(unsafe { libc::geteuid() }) {
        return refuse(&e.message);
    }
    let env = match Environment::system() {
        Ok(env) => env,
        Err(e) => return refuse(&e.message),
    };
    let gate = CancelGate::new(Arc::new(ProcessAborter::new(env.lock_path())));

    // The user cannot signal a root process, but the system can (shutdown): treat it as a cancel.
    match Signals::new([SIGTERM, SIGINT, SIGHUP]) {
        Ok(mut signals) => {
            let gate = gate.clone();
            std::thread::spawn(move || {
                for _ in signals.forever() {
                    gate.request_cancel();
                }
            });
        }
        Err(e) => return refuse(&format!("cannot install signal handlers: {e}")),
    }

    serve(
        &env,
        BufReader::new(std::io::stdin()),
        std::io::stdout(),
        gate,
    )
}

fn main() {
    std::process::exit(run());
}
