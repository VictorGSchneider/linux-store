use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use store_core::{CancelToken, InstallError, Installer};
use store_installer_pkexec::{HELPER_PATH, PkexecInstaller};
use store_protocol::{ErrorCode, Event, Operation, Request, Stage};

/// Writes an executable `sh` script that plays the helper.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Runs the script through `sh` instead of executing it: a freshly written file that another
/// test thread's `fork` still holds open for writing cannot be exec'd ("Text file busy").
fn installer(path: &Path) -> PkexecInstaller {
    PkexecInstaller::with_command(vec![OsString::from("/bin/sh"), OsString::from(path)])
}

fn upgrade() -> Request {
    Request::Upgrade
}

fn run(installer: &PkexecInstaller, token: &CancelToken) -> (Result<(), InstallError>, Vec<Event>) {
    let mut events = Vec::new();
    let result = installer.run(&upgrade(), token, &mut |e| events.push(e));
    (result, events)
}

#[test]
fn system_command_is_pkexec_with_the_helper_path() {
    let system = PkexecInstaller::system();
    assert_eq!(
        system.command(),
        [OsString::from("pkexec"), OsString::from(HELPER_PATH)]
    );
    assert_eq!(HELPER_PATH, "/usr/lib/linux-store/store-helper");
}

#[test]
fn delivers_events_in_order_and_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let helper = script(
        dir.path(),
        "ok",
        r#"read line
echo '{"event":"started","op":"upgrade"}'
echo '{"event":"progress","stage":"download","package":"tree","percent":10}'
echo '{"event":"done","changes":[]}'"#,
    );
    let (result, events) = run(&installer(&helper), &CancelToken::default());
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        events,
        [
            Event::Started {
                op: Operation::Upgrade
            },
            Event::Progress {
                stage: Stage::Download,
                package: Some("tree".into()),
                percent: Some(10)
            },
            Event::Done { changes: vec![] },
        ]
    );
}

#[test]
fn writes_the_request_as_one_json_line() {
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("request.txt");
    let helper = script(
        dir.path(),
        "echo",
        &format!(
            "read line\nprintf '%s\\n' \"$line\" > {}\necho '{{\"event\":\"done\",\"changes\":[]}}'",
            saved.display()
        ),
    );
    let request = Request::Install {
        packages: vec!["tree".into(), "sl".into()],
    };
    let result = installer(&helper).run(&request, &CancelToken::default(), &mut |_| {});
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(std::fs::read_to_string(saved).unwrap(), request.to_line());
}

#[test]
fn error_event_becomes_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let helper = script(
        dir.path(),
        "err",
        "read line\necho '{\"event\":\"error\",\"code\":\"not_found\",\"message\":\"ghost\"}'\nexit 1",
    );
    let (result, events) = run(&installer(&helper), &CancelToken::default());
    match result {
        Err(InstallError::Rejected { code, message }) => {
            assert_eq!((code, message.as_str()), (ErrorCode::NotFound, "ghost"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        events.len(),
        1,
        "the error event is still delivered: {events:?}"
    );
}

#[test]
fn exit_126_is_dismissed_and_127_is_not_authorized() {
    let dir = tempfile::tempdir().unwrap();
    let dismissed = script(dir.path(), "d", "exit 126");
    assert!(matches!(
        run(&installer(&dismissed), &CancelToken::default()).0,
        Err(InstallError::Dismissed)
    ));
    let denied = script(dir.path(), "n", "echo denied >&2\nexit 127");
    match run(&installer(&denied), &CancelToken::default()).0 {
        Err(InstallError::NotAuthorized(text)) => assert!(text.contains("denied"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn garbage_output_is_a_protocol_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let helper = script(
        dir.path(),
        "garbage",
        "read line\necho 'not json'\nsleep 30",
    );
    let started = Instant::now();
    let (result, _) = run(&installer(&helper), &CancelToken::default());
    assert!(
        matches!(result, Err(InstallError::Protocol(_))),
        "{result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn silent_exit_is_a_failure_not_a_hang() {
    let dir = tempfile::tempdir().unwrap();
    let helper = script(dir.path(), "silent", "exit 3");
    match run(&installer(&helper), &CancelToken::default()).0 {
        Err(InstallError::Failed { exit_code, .. }) => assert_eq!(exit_code, Some(3)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn exit_zero_without_a_result_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let helper = script(dir.path(), "empty", "read line\nexit 0");
    assert!(matches!(
        run(&installer(&helper), &CancelToken::default()).0,
        Err(InstallError::Protocol(_))
    ));
}

#[test]
fn missing_program_is_a_spawn_error() {
    let missing =
        PkexecInstaller::with_command(vec![OsString::from("/nonexistent/linux-store-helper")]);
    assert!(matches!(
        run(&missing, &CancelToken::default()).0,
        Err(InstallError::Spawn(_))
    ));
    let empty = PkexecInstaller::with_command(vec![]);
    assert!(matches!(
        run(&empty, &CancelToken::default()).0,
        Err(InstallError::Spawn(_))
    ));
}

#[test]
fn cancel_token_sends_a_cancel_line() {
    let dir = tempfile::tempdir().unwrap();
    let helper = script(
        dir.path(),
        "cancellable",
        // The first line guards against a regression: without it a missing cancel would block
        // this test forever instead of failing it.
        r#"( sleep 10; kill $$ ) </dev/null >/dev/null 2>&1 &
read request
read second
case "$second" in
  *cancel*) echo '{"event":"error","code":"cancelled","message":"stopped"}'; exit 3;;
esac
echo '{"event":"done","changes":[]}'"#,
    );
    let token = CancelToken::default();
    token.cancel();
    match run(&installer(&helper), &token).0 {
        Err(InstallError::Rejected { code, .. }) => assert_eq!(code, ErrorCode::Cancelled),
        other => panic!("{other:?}"),
    }
}

#[test]
fn oversized_output_line_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let helper = script(
        dir.path(),
        "huge",
        "read line\nhead -c 204800 /dev/zero | tr '\\0' 'a'\necho\nsleep 30",
    );
    let started = Instant::now();
    assert!(matches!(
        run(&installer(&helper), &CancelToken::default()).0,
        Err(InstallError::Protocol(_))
    ));
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "took {:?}",
        started.elapsed()
    );
}
