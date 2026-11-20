//! Runs the privileged helper through `pkexec` and turns its events and exit code into results.
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use store_core::{CancelToken, InstallError, Installer};
use store_protocol::{ErrorCode, Event, MAX_REQUEST_BYTES, Request};

/// Where the packaged helper lives. The polkit policy's `exec.path` must be exactly this.
pub const HELPER_PATH: &str = "/usr/lib/linux-store/store-helper";

const STDERR_LIMIT: usize = 8 * 1024;
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// `pkexec` is looked up by absolute path: a directory that comes first in `PATH` must not be
/// able to stand in for it.
pub const PKEXEC_PATH: &str = "/usr/bin/pkexec";

pub struct PkexecInstaller {
    command: Vec<OsString>,
    exit_wait: Duration,
}

impl PkexecInstaller {
    /// `pkexec /usr/lib/linux-store/store-helper`.
    pub fn system() -> PkexecInstaller {
        PkexecInstaller::with_command(vec![
            OsString::from(PKEXEC_PATH),
            OsString::from(HELPER_PATH),
        ])
    }

    /// Any command that speaks the helper protocol. The first element is the program.
    pub fn with_command(command: Vec<OsString>) -> PkexecInstaller {
        PkexecInstaller {
            command,
            exit_wait: EXIT_WAIT,
        }
    }

    /// How long to wait for the helper to exit once its output has ended (default 5 s).
    pub fn with_exit_wait(mut self, wait: Duration) -> PkexecInstaller {
        self.exit_wait = wait;
        self
    }

    pub fn command(&self) -> &[OsString] {
        &self.command
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct Seen {
    done: bool,
    error: Option<(ErrorCode, String)>,
}

/// Reads at most 8 KiB of the child's stderr and drains the rest so it never blocks on a full pipe.
fn capture_stderr(mut stderr: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut text = Vec::new();
        let _ = (&mut stderr)
            .take(STDERR_LIMIT as u64)
            .read_to_end(&mut text);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        String::from_utf8_lossy(&text).trim().to_string()
    })
}

fn read_events(stdout: ChildStdout, on_event: &mut dyn FnMut(Event)) -> Result<Seen, InstallError> {
    let mut reader = BufReader::new(stdout);
    let mut seen = Seen::default();
    loop {
        let mut line = Vec::new();
        let read = (&mut reader)
            .take(MAX_REQUEST_BYTES as u64 + 2)
            .read_until(b'\n', &mut line)
            .map_err(|e| InstallError::Protocol(format!("cannot read the helper's output: {e}")))?;
        if read == 0 {
            return Ok(seen);
        }
        if line.len() > MAX_REQUEST_BYTES {
            return Err(InstallError::Protocol(
                "an output line is longer than 64 KiB".to_string(),
            ));
        }
        let event = Event::parse(&line).map_err(|e| InstallError::Protocol(e.to_string()))?;
        match &event {
            Event::Done { .. } => seen.done = true,
            Event::Error { code, message } => seen.error = Some((*code, message.clone())),
            _ => {}
        }
        on_event(event);
    }
}

/// Waits for the child, but not forever. A helper running as root cannot be killed by the user,
/// so after the deadline the attempt is made and the child is left behind, handed to a thread
/// that reaps it whenever it does exit, so a long-lived GUI does not collect zombies.
fn wait_bounded(mut child: Child, wait: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                thread::spawn(move || {
                    let _ = child.wait();
                });
                return None;
            }
        }
    }
}

/// The stderr reader ends when every holder of the pipe is gone. A grandchild of the helper can
/// keep it open long after the helper itself was killed, so wait a moment and move on.
fn join_bounded(handle: thread::JoinHandle<String>) -> String {
    let deadline = Instant::now() + Duration::from_millis(500);
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if handle.is_finished() {
        handle.join().unwrap_or_default()
    } else {
        String::new()
    }
}

fn send(stdin: &Mutex<Option<ChildStdin>>, line: &str) {
    if let Some(writer) = lock(stdin).as_mut() {
        // The helper may already be gone; its exit status will say what happened.
        let _ = writer.write_all(line.as_bytes());
        let _ = writer.flush();
    }
}

impl Installer for PkexecInstaller {
    fn run(
        &self,
        request: &Request,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<(), InstallError> {
        let (program, args) = self.command.split_first().ok_or_else(|| {
            InstallError::Spawn(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no command configured",
            ))
        })?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(InstallError::Spawn)?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = capture_stderr(child.stderr.take().expect("stderr is piped"));

        send(&stdin, &request.to_line());

        let finished = Arc::new(AtomicBool::new(false));
        let watcher = {
            let (stdin, cancel, finished) = (stdin.clone(), cancel.clone(), finished.clone());
            thread::spawn(move || {
                while !finished.load(Ordering::SeqCst) {
                    if cancel.is_cancelled() {
                        send(&stdin, &Request::Cancel.to_line());
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            })
        };

        let outcome = read_events(stdout, on_event);

        // Closing the helper's input tells it, too, that nobody is listening any more.
        finished.store(true, Ordering::SeqCst);
        lock(&stdin).take();
        let _ = watcher.join();
        let status = wait_bounded(child, self.exit_wait);
        let stderr = join_bounded(stderr);

        let seen = outcome?;
        if let Some((code, message)) = seen.error {
            return Err(InstallError::Rejected { code, message });
        }
        match status.and_then(|s| s.code()) {
            Some(126) => Err(InstallError::Dismissed),
            Some(127) => Err(InstallError::NotAuthorized(stderr)),
            Some(0) if seen.done => Ok(()),
            Some(0) => Err(InstallError::Protocol(
                "the helper ended without reporting a result".to_string(),
            )),
            exit_code => Err(InstallError::Failed { exit_code, stderr }),
        }
    }
}
