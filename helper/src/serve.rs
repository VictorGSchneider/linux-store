//! The request loop: read one request, run it, report events, return the exit code.
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use store_protocol::{
    EXIT_CANCELLED, EXIT_FAILED, EXIT_INVALID, EXIT_OK, ErrorCode, Event, MAX_REQUEST_BYTES,
    Operation, Request,
};

use crate::{
    AuditOutcome, CancelGate, CancelOutcome, Emitter, Environment, HelperError, audit, audit_line,
    audit_outcome_line, install, remove, upgrade,
};

/// More progress events than this waiting to be written means the client stopped reading.
const MAX_PENDING_PROGRESS: usize = 256;
/// How long to wait, at the end, for the last events to reach a client.
const DEFAULT_FLUSH_WAIT: Duration = Duration::from_secs(5);

/// The helper only makes sense as root (through `pkexec`); anything else is refused up front.
pub fn ensure_root(euid: u32) -> Result<(), HelperError> {
    if euid == 0 {
        Ok(())
    } else {
        Err(HelperError::new(
            ErrorCode::Failed,
            "store-helper must run as root; run it through pkexec",
        ))
    }
}

enum Msg {
    Event(Event),
    Stop,
}

/// Where events go. A separate thread does the writing, so a client that stops reading its pipe
/// can never block the transaction (which holds the database lock).
#[derive(Clone)]
struct Sink {
    tx: mpsc::Sender<Msg>,
    pending: Arc<AtomicUsize>,
}

impl Sink {
    fn channel() -> (Sink, mpsc::Receiver<Msg>) {
        let (tx, rx) = mpsc::channel();
        (
            Sink {
                tx,
                pending: Arc::new(AtomicUsize::new(0)),
            },
            rx,
        )
    }

    /// Never blocks. Progress is dropped when the writer is far behind; every other event
    /// (there are only a few per request) is always kept.
    fn send(&self, event: Event) {
        if matches!(event, Event::Progress { .. })
            && self.pending.load(Ordering::Relaxed) >= MAX_PENDING_PROGRESS
        {
            return;
        }
        self.pending.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(Msg::Event(event));
    }

    fn stop(&self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

fn spawn_writer<W: Write + Send + 'static>(
    mut output: W,
    receiver: mpsc::Receiver<Msg>,
    pending: Arc<AtomicUsize>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut alive = true;
        for msg in receiver {
            match msg {
                Msg::Stop => break,
                Msg::Event(event) => {
                    pending.fetch_sub(1, Ordering::Relaxed);
                    // A client that went away is not an error; keep draining.
                    if alive
                        && (output.write_all(event.to_line().as_bytes()).is_err()
                            || output.flush().is_err())
                    {
                        alive = false;
                    }
                }
            }
        }
    })
}

/// Waits for the writer to flush, but not forever: a client that never reads must not keep the
/// process alive after the work is done.
fn finish_writer(sink: &Sink, writer: thread::JoinHandle<()>, wait: Duration) {
    sink.stop();
    let deadline = Instant::now() + wait;
    while !writer.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if writer.is_finished() {
        let _ = writer.join();
    }
}

/// Reads one line, at most `MAX_REQUEST_BYTES + 1` bytes of it, so a hostile peer cannot make
/// the helper buffer more than that.
fn read_line(input: &mut impl BufRead) -> std::io::Result<Vec<u8>> {
    let mut line = Vec::new();
    (&mut *input)
        .take(MAX_REQUEST_BYTES as u64 + 2)
        .read_until(b'\n', &mut line)?;
    Ok(line)
}

/// Asks the gate to cancel; if it is too late, says so.
fn handle_cancel(gate: &CancelGate, sink: &Sink) {
    if gate.request_cancel() == CancelOutcome::TooLate {
        sink.send(Event::CancelIgnored);
    }
}

/// After the request, a `cancel` line or the end of the input asks to cancel; any other line is
/// ignored.
fn watch_for_cancel(mut input: impl BufRead, gate: CancelGate, sink: Sink) {
    loop {
        match read_line(&mut input) {
            Ok(line) if line.is_empty() => {
                handle_cancel(&gate, &sink);
                break;
            }
            Ok(line) => {
                if matches!(Request::parse(&line), Ok(Request::Cancel)) {
                    handle_cancel(&gate, &sink);
                }
            }
            Err(_) => {
                handle_cancel(&gate, &sink);
                break;
            }
        }
    }
}

/// Serves exactly one request and returns the process exit code.
///
/// A request that does not parse never reaches libalpm: it is answered with `invalid_request`
/// and `EXIT_INVALID` before `env` is touched.
pub fn serve<W>(
    env: &Environment,
    input: impl BufRead + Send + 'static,
    output: W,
    gate: CancelGate,
) -> i32
where
    W: Write + Send + 'static,
{
    serve_with(env, input, output, gate, DEFAULT_FLUSH_WAIT)
}

/// Like [`serve`], with an explicit bound on how long to wait for the last events to be written.
pub fn serve_with<W>(
    env: &Environment,
    input: impl BufRead + Send + 'static,
    output: W,
    gate: CancelGate,
    flush_wait: Duration,
) -> i32
where
    W: Write + Send + 'static,
{
    let (sink, receiver) = Sink::channel();
    let writer = spawn_writer(output, receiver, sink.pending.clone());
    let code = handle(env, input, &sink, gate);
    finish_writer(&sink, writer, flush_wait);
    code
}

fn handle(
    env: &Environment,
    mut input: impl BufRead + Send + 'static,
    sink: &Sink,
    gate: CancelGate,
) -> i32 {
    let invalid = |message: String| {
        sink.send(Event::Error {
            code: ErrorCode::InvalidRequest,
            message,
        });
        EXIT_INVALID
    };
    let line = match read_line(&mut input) {
        Ok(line) => line,
        Err(e) => return invalid(format!("cannot read the request: {e}")),
    };
    let request = match Request::parse(&line) {
        Ok(Request::Cancel) => {
            return invalid("the first request must be an operation".to_string());
        }
        Ok(request) => request,
        Err(e) => return invalid(e.to_string()),
    };
    let caller_uid = std::env::var("PKEXEC_UID").ok();
    audit(&audit_line(caller_uid.as_deref(), &request));
    let op = match &request {
        Request::Install { .. } => Operation::Install,
        Request::Remove { .. } => Operation::Remove,
        Request::Upgrade => Operation::Upgrade,
        Request::Cancel => unreachable!("rejected above"),
    };
    sink.send(Event::Started { op });

    {
        let (gate, sink) = (gate.clone(), sink.clone());
        thread::spawn(move || watch_for_cancel(input, gate, sink));
    }
    let emitter = {
        let sink = sink.clone();
        Emitter::new(move |event| sink.send(event))
    };
    let result = match &request {
        Request::Install { packages } => install(env, packages, &emitter, &gate),
        Request::Remove { packages } => remove(env, packages, &emitter, &gate),
        Request::Upgrade => upgrade(env, &emitter, &gate),
        Request::Cancel => unreachable!("rejected above"),
    };
    match result {
        Ok(changes) => {
            let outcome = AuditOutcome::Ok {
                changes: changes.len(),
            };
            audit(&audit_outcome_line(
                caller_uid.as_deref(),
                &request,
                &outcome,
            ));
            sink.send(Event::Done { changes });
            EXIT_OK
        }
        Err(e) => {
            audit(&audit_outcome_line(
                caller_uid.as_deref(),
                &request,
                &AuditOutcome::Error(e.code),
            ));
            let code = match e.code {
                ErrorCode::Cancelled => EXIT_CANCELLED,
                ErrorCode::InvalidRequest => EXIT_INVALID,
                _ => EXIT_FAILED,
            };
            sink.send(Event::Error {
                code: e.code,
                message: e.message,
            });
            code
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::sync::{Arc, Mutex};

    use store_protocol::{
        ChangeKind, EXIT_CANCELLED, EXIT_FAILED, EXIT_INVALID, EXIT_OK, ErrorCode, Event,
        MAX_REQUEST_BYTES,
    };

    use super::*;
    use crate::fixture::{Fixture, RecordingAborter};

    /// A writer the test can read back after `serve` returns.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn events(&self) -> Vec<Event> {
            let bytes = self.0.lock().unwrap().clone();
            bytes
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .map(|l| {
                    Event::parse(l).unwrap_or_else(|e| {
                        panic!("bad event line {:?}: {e}", String::from_utf8_lossy(l))
                    })
                })
                .collect()
        }
    }

    fn gate() -> CancelGate {
        CancelGate::new(Arc::new(RecordingAborter::default()))
    }

    /// Runs `serve` with an input that stays open until the returned writer is dropped.
    fn run_held_open(env: &Environment, request: &str, gate: CancelGate) -> (i32, Vec<Event>) {
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(request.as_bytes()).unwrap();
        let out = Shared::default();
        let code = serve(env, std::io::BufReader::new(reader), out.clone(), gate);
        let events = out.events();
        drop(writer);
        (code, events)
    }

    fn world() -> Fixture {
        let fx = Fixture::new();
        fx.publish("libfoo", "1.0-1", &[]);
        fx.publish("app", "1.0-1", &["libfoo"]);
        fx.sync();
        fx
    }

    const INSTALL_APP: &str = "{\"op\":\"install\",\"packages\":[\"app\"]}\n";

    fn hostile_lines() -> Vec<Vec<u8>> {
        let many: Vec<String> = (0..=200).map(|i| format!("\"p{i}\"")).collect();
        let mut lines: Vec<Vec<u8>> = [
            r#"{"op":"install","packages":["-x"]}"#,
            r#"{"op":"install","packages":["--root=/x"]}"#,
            r#"{"op":"install","packages":["../x"]}"#,
            r#"{"op":"install","packages":["a/b"]}"#,
            r#"{"op":"install","packages":["a b"]}"#,
            r#"{"op":"install","packages":["a;b"]}"#,
            r#"{"op":"install","packages":["a\nb"]}"#,
            r#"{"op":"install","packages":["a\u0000b"]}"#,
            r#"{"op":"install","packages":[""]}"#,
            r#"{"op":"install","packages":["a","a"]}"#,
            r#"{"op":"remove","packages":["Tree"]}"#,
            r#"{"op":"upgrade","packages":[]}"#,
            r#"{"op":"cancel"}"#,
            r#"[]"#,
            r#"not json at all"#,
            "",
            "   ",
        ]
        .iter()
        .map(|l| format!("{l}\n").into_bytes())
        .collect();
        lines.push(
            format!("{{\"op\":\"install\",\"packages\":[{}]}}\n", many.join(",")).into_bytes(),
        );
        lines.push(vec![0xff, 0xfe, b'\n']);
        let mut huge = br#"{"op":"upgrade"}"#.to_vec();
        huge.resize(MAX_REQUEST_BYTES + 10, b' ');
        huge.push(b'\n');
        lines.push(huge);
        lines
    }

    #[test]
    fn hostile_requests_never_open_libalpm() {
        for line in hostile_lines() {
            let out = Shared::default();
            let code = serve(
                &Environment::unreachable(),
                Cursor::new(line.clone()),
                out.clone(),
                gate(),
            );
            assert_eq!(
                code,
                EXIT_INVALID,
                "{:?}",
                String::from_utf8_lossy(&line[..line.len().min(80)])
            );
            let events = out.events();
            assert_eq!(events.len(), 1, "{events:?}");
            assert!(
                matches!(
                    events[0],
                    Event::Error {
                        code: ErrorCode::InvalidRequest,
                        ..
                    }
                ),
                "{events:?}"
            );
        }
    }

    #[test]
    fn install_end_to_end() {
        let fx = world();
        let (code, events) = run_held_open(&fx.env(), INSTALL_APP, gate());
        assert_eq!(code, EXIT_OK, "{events:?}");
        assert!(
            matches!(events.first(), Some(Event::Started { .. })),
            "{events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, Event::Progress { .. })),
            "{events:?}"
        );
        match events.last() {
            Some(Event::Done { changes }) => {
                let mut names: Vec<&str> = changes.iter().map(|c| c.package.as_str()).collect();
                names.sort();
                assert_eq!(names, ["app", "libfoo"]);
                assert!(changes.iter().all(|c| c.kind == ChangeKind::Installed));
            }
            other => panic!("last event was {other:?}"),
        }
        assert_eq!(fx.installed(), ["app-1.0-1", "libfoo-1.0-1"]);
        assert!(!fx.lock_exists());
    }

    #[test]
    fn unknown_package_is_a_failed_exit_with_a_not_found_error() {
        let fx = world();
        let (code, events) = run_held_open(
            &fx.env(),
            "{\"op\":\"install\",\"packages\":[\"ghost\"]}\n",
            gate(),
        );
        assert_eq!(code, EXIT_FAILED);
        assert!(
            matches!(
                events.last(),
                Some(Event::Error {
                    code: ErrorCode::NotFound,
                    ..
                })
            ),
            "{events:?}"
        );
        assert!(fx.installed().is_empty());
    }

    #[test]
    fn extra_garbage_lines_after_the_request_are_ignored() {
        let fx = world();
        let request = format!(
            "{INSTALL_APP}this is not json\n{{\"op\":\"install\",\"packages\":[\"ghost\"]}}\n"
        );
        let (code, events) = run_held_open(&fx.env(), &request, gate());
        assert_eq!(code, EXIT_OK, "{events:?}");
        assert_eq!(fx.installed(), ["app-1.0-1", "libfoo-1.0-1"]);
    }

    #[test]
    fn cancel_before_work_is_exit_cancelled_and_changes_nothing() {
        let fx = world();
        let gate = gate();
        gate.request_cancel();
        let (code, events) = run_held_open(&fx.env(), INSTALL_APP, gate);
        assert_eq!(code, EXIT_CANCELLED, "{events:?}");
        assert!(
            matches!(
                events.last(),
                Some(Event::Error {
                    code: ErrorCode::Cancelled,
                    ..
                })
            ),
            "{events:?}"
        );
        assert!(fx.installed().is_empty());
        assert!(!fx.lock_exists());
    }

    #[test]
    fn end_of_input_after_the_request_is_a_cancel_request() {
        // Nothing keeps the input open, so the reader sees end of input at once: the operation
        // either completes or is cancelled, and either way the system is consistent.
        let fx = world();
        let out = Shared::default();
        let code = serve(
            &fx.env(),
            Cursor::new(INSTALL_APP.as_bytes().to_vec()),
            out.clone(),
            gate(),
        );
        match code {
            EXIT_OK => assert_eq!(fx.installed(), ["app-1.0-1", "libfoo-1.0-1"]),
            EXIT_CANCELLED => assert!(fx.installed().is_empty(), "{:?}", fx.installed()),
            other => panic!("unexpected exit code {other}: {:?}", out.events()),
        }
        assert!(!fx.lock_exists());
    }

    #[test]
    fn a_late_cancel_is_reported_as_cancel_ignored() {
        let gate = gate();
        gate.open_window().unwrap();
        gate.commit_started();
        let (sink, receiver) = Sink::channel();
        handle_cancel(&gate, &sink);
        let queued: Vec<Msg> = receiver.try_iter().collect();
        assert!(
            matches!(queued.as_slice(), [Msg::Event(Event::CancelIgnored)]),
            "a late cancel must be answered with cancel_ignored"
        );
    }

    #[test]
    fn ensure_root_rejects_non_root() {
        assert!(ensure_root(1000).is_err());
        assert_eq!(ensure_root(1000).unwrap_err().code, ErrorCode::Failed);
        assert!(ensure_root(0).is_ok());
    }

    #[test]
    fn progress_events_are_dropped_not_queued_forever() {
        let (sink, receiver) = Sink::channel();
        for _ in 0..(MAX_PENDING_PROGRESS + 100) {
            sink.send(Event::Progress {
                stage: store_protocol::Stage::Download,
                package: None,
                percent: None,
            });
        }
        sink.send(Event::Done { changes: vec![] });
        sink.send(Event::CancelIgnored);
        let queued: Vec<Msg> = receiver.try_iter().collect();
        let progress = queued
            .iter()
            .filter(|m| matches!(m, Msg::Event(Event::Progress { .. })))
            .count();
        assert_eq!(progress, MAX_PENDING_PROGRESS, "progress must be capped");
        assert!(
            queued
                .iter()
                .any(|m| matches!(m, Msg::Event(Event::Done { .. }))),
            "Done must never be dropped"
        );
        assert!(
            queued
                .iter()
                .any(|m| matches!(m, Msg::Event(Event::CancelIgnored)))
        );
    }

    /// A writer that never accepts anything, like a client that stopped reading its pipe.
    struct Stalled;

    impl Write for Stalled {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            loop {
                std::thread::park();
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stalled_reader_never_blocks_the_transaction() {
        let fx = world();
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(INSTALL_APP.as_bytes()).unwrap();
        let env = fx.env();
        // Run it in a thread with a deadline: if this ever regresses it must fail, not hang CI.
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let code = serve_with(
                &env,
                std::io::BufReader::new(reader),
                Stalled,
                gate(),
                std::time::Duration::from_millis(200),
            );
            let _ = done.send(code);
        });
        let code = result
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("serve must not block on a client that stopped reading");
        drop(writer);
        assert_eq!(code, EXIT_OK);
        assert_eq!(
            fx.installed(),
            ["app-1.0-1", "libfoo-1.0-1"],
            "the transaction must complete"
        );
        assert!(!fx.lock_exists(), "the lock must be released");
    }
}
