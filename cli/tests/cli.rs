use std::ffi::OsString;
use std::path::PathBuf;

use store_cli::{default_db_path_from, run};
use store_core::testing::{FakeBackend, FakeInstaller, pkg};
use store_core::{Index, InstallError, Package, Store};
use store_protocol::{Change, ChangeKind, ErrorCode, Event, Operation, Request, Stage};

/// Runs the CLI against `db` with one fake backend; returns (exit code, stdout, stderr).
fn run_cli(args: &[&str], backend: FakeBackend) -> (i32, String, String) {
    let argv: Vec<OsString> = std::iter::once("store")
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let installer = FakeInstaller::new(vec![], Ok(()));
    let code = run(
        &argv,
        move |path| Ok(Store::new(Index::open(path)?, vec![Box::new(backend)])),
        &installer,
        &mut out,
        &mut err,
    );
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn fake() -> FakeBackend {
    FakeBackend::new(
        "fake",
        vec![pkg("fake", "alpha", "first"), pkg("fake", "beta", "second")],
    )
}

fn db(dir: &tempfile::TempDir) -> String {
    dir.path().join("index.db").to_str().unwrap().to_string()
}

#[test]
fn search_prints_matching_lines() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    run_cli(&["--db", &db, "refresh"], fake());
    let (code, out, _) = run_cli(&["--db", &db, "search", "--plain", "alpha"], fake());
    assert_eq!(code, 0);
    assert!(out.contains("fake/- alpha 1.0 — first"), "{out}");
    assert!(!out.contains("beta"), "{out}");
}

#[test]
fn search_with_no_match_prints_nothing_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    run_cli(&["--db", &db, "refresh"], fake());
    let (code, out, _) = run_cli(&["--db", &db, "search", "nonexistent"], fake());
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn refresh_reports_package_count() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out, _) = run_cli(&["--db", &db(&dir), "refresh"], fake());
    assert_eq!(code, 0);
    assert!(out.contains("fake: 2 packages"), "{out}");
}

#[test]
fn refresh_reports_unavailable_source() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out, _) = run_cli(&["--db", &db(&dir), "refresh"], fake().unavailable());
    assert_eq!(code, 0);
    assert!(out.contains("fake: unavailable"), "{out}");
}

#[test]
fn refresh_exits_nonzero_when_every_source_failed() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out, _) = run_cli(&["--db", &db(&dir), "refresh"], fake().failing("boom"));
    assert_eq!(code, 1);
    assert!(
        out.contains("fake: failed:") && out.contains("boom"),
        "{out}"
    );
}

#[test]
fn installed_marks_installed_packages() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    let installed = Package {
        installed_version: Some("1.0".into()),
        ..pkg("fake", "yes", "x")
    };
    let backend = || FakeBackend::new("fake", vec![installed.clone(), pkg("fake", "no", "x")]);
    run_cli(&["--db", &db, "refresh"], backend());
    let (code, out, _) = run_cli(&["--db", &db, "installed"], backend());
    assert_eq!(code, 0);
    assert!(out.contains("yes 1.0 [installed]"), "{out}");
    assert!(!out.contains("no 1.0"), "{out}");
}

#[test]
fn unwritable_db_path_is_a_clean_error() {
    let (code, out, err) = run_cli(&["--db", "/proc/nope/index.db", "refresh"], fake());
    assert_eq!(code, 1);
    assert_eq!(out, "");
    assert!(err.contains("cannot open index"), "{err}");
}

#[test]
fn usage_errors_exit_nonzero() {
    let (code, _, err) = run_cli(&["frobnicate"], fake());
    assert_ne!(code, 0);
    assert!(!err.is_empty());
}

#[test]
fn default_db_path_prefers_xdg_cache_home() {
    let path = default_db_path_from(
        Some(OsString::from("/x/cache")),
        Some(OsString::from("/home/u")),
    );
    assert_eq!(path, Some(PathBuf::from("/x/cache/linux-store/index.db")));
}

#[test]
fn default_db_path_falls_back_to_home_cache() {
    let path = default_db_path_from(None, Some(OsString::from("/home/u")));
    assert_eq!(
        path,
        Some(PathBuf::from("/home/u/.cache/linux-store/index.db"))
    );
    assert_eq!(default_db_path_from(None, None), None);
}

#[test]
fn search_shows_title_before_description() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    let titled = || {
        FakeBackend::new(
            "fake",
            vec![Package {
                title: Some("Alpha App".into()),
                ..pkg("fake", "alpha", "first")
            }],
        )
    };
    run_cli(&["--db", &db, "refresh"], titled());
    let (code, out, _) = run_cli(&["--db", &db, "search", "--plain", "alpha"], titled());
    assert_eq!(code, 0);
    assert!(out.contains("— Alpha App: first"), "{out}");
}

#[test]
fn refresh_reports_every_source() {
    let dir = tempfile::tempdir().unwrap();
    let argv: Vec<OsString> = ["store", "--db", &db(&dir), "refresh"]
        .iter()
        .map(OsString::from)
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let installer = FakeInstaller::new(vec![], Ok(()));
    let code = run(
        &argv,
        |path| {
            Ok(Store::new(
                Index::open(path)?,
                vec![
                    Box::new(FakeBackend::new("one", vec![pkg("one", "a", "x")])),
                    Box::new(FakeBackend::new("two", vec![]).unavailable()),
                ],
            ))
        },
        &installer,
        &mut out,
        &mut err,
    );
    let out = String::from_utf8(out).unwrap();
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("one: 1 packages"), "{out}");
    assert!(out.contains("two: unavailable"), "{out}");
}

/// Runs an install/remove/upgrade command against a fake installer; no index is involved.
fn run_installer(args: &[&str], installer: &FakeInstaller) -> (i32, String, String) {
    let argv: Vec<OsString> = std::iter::once("store")
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(
        &argv,
        |_| panic!("installing must not open the index"),
        installer,
        &mut out,
        &mut err,
    );
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn progress(stage: Stage, package: &str, percent: u8) -> Event {
    Event::Progress {
        stage,
        package: Some(package.into()),
        percent: Some(percent),
    }
}

#[test]
fn install_sends_the_request_and_prints_progress() {
    let fake = FakeInstaller::new(
        vec![
            Event::Started {
                op: Operation::Install,
            },
            progress(Stage::Download, "tree", 42),
            progress(Stage::Install, "tree", 100),
            Event::Done {
                changes: vec![Change {
                    package: "tree".into(),
                    kind: ChangeKind::Installed,
                    version: Some("2.3.2-1".into()),
                }],
            },
        ],
        Ok(()),
    );
    let (code, out, err) = run_installer(&["install", "tree", "sl"], &fake);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        fake.requests(),
        [Request::Install {
            packages: vec!["tree".into(), "sl".into()]
        }]
    );
    for expected in [
        "started install",
        "[download] tree 42%",
        "[install] tree 100%",
        "done: installed tree 2.3.2-1",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in {out}");
    }
}

#[test]
fn remove_and_upgrade_send_the_matching_requests() {
    let fake = FakeInstaller::new(vec![Event::Done { changes: vec![] }], Ok(()));
    let (code, out, _) = run_installer(&["remove", "tree"], &fake);
    assert_eq!(code, 0);
    assert!(out.contains("done: nothing to do"), "{out}");
    let fake_again = FakeInstaller::new(vec![Event::Done { changes: vec![] }], Ok(()));
    assert_eq!(run_installer(&["upgrade"], &fake_again).0, 0);
    assert_eq!(
        fake.requests(),
        [Request::Remove {
            packages: vec!["tree".into()]
        }]
    );
    assert_eq!(fake_again.requests(), [Request::Upgrade]);
}

#[test]
fn invalid_name_is_rejected_before_the_helper_runs() {
    for args in [
        &["install", "Tree"][..],
        &["install", "a/b"],
        &["remove", "--", "-x"],
        &["install", "ok", "a b"],
    ] {
        let fake = FakeInstaller::new(vec![], Ok(()));
        let (code, out, err) = run_installer(args, &fake);
        assert_eq!(code, 1, "{args:?}");
        assert_eq!(out, "");
        assert!(err.contains("invalid package name"), "{err}");
        assert!(fake.requests().is_empty(), "{args:?} reached the helper");
    }
}

#[test]
fn dismissed_exits_126_and_not_authorized_exits_127_with_a_hint() {
    let dismissed = FakeInstaller::new(vec![], Err(InstallError::Dismissed));
    let (code, _, err) = run_installer(&["upgrade"], &dismissed);
    assert_eq!(code, 126);
    assert!(err.contains("dismissed"), "{err}");
    let denied = FakeInstaller::new(
        vec![],
        Err(InstallError::NotAuthorized("Not authorized".into())),
    );
    let (code, _, err) = run_installer(&["upgrade"], &denied);
    assert_eq!(code, 127);
    assert!(
        err.contains("Not authorized") && err.contains("authentication agent"),
        "{err}"
    );
}

#[test]
fn rejected_error_prints_the_message_and_exits_1() {
    let fake = FakeInstaller::new(
        vec![Event::Error {
            code: ErrorCode::NotFound,
            message: "not found: ghost".into(),
        }],
        Err(InstallError::Rejected {
            code: ErrorCode::NotFound,
            message: "not found: ghost".into(),
        }),
    );
    let (code, out, err) = run_installer(&["install", "ghost"], &fake);
    assert_eq!(code, 1);
    assert!(err.contains("not found: ghost"), "{err}");
    assert!(
        !out.contains("ghost"),
        "the error belongs on stderr only: {out}"
    );
}

#[test]
fn other_failures_exit_1() {
    let fake = FakeInstaller::new(vec![], Err(InstallError::Protocol("garbage".into())));
    let (code, _, err) = run_installer(&["upgrade"], &fake);
    assert_eq!(code, 1);
    assert!(err.contains("garbage"), "{err}");
}

#[test]
fn progress_lines_are_throttled() {
    let events: Vec<Event> = (0..100)
        .map(|_| progress(Stage::Download, "tree", 42))
        .collect();
    let fake = FakeInstaller::new(events, Ok(()));
    let (_, out, _) = run_installer(&["upgrade"], &fake);
    assert_eq!(out.matches("[download] tree 42%").count(), 1, "{out}");
}

#[test]
fn a_late_cancel_is_reported() {
    let fake = FakeInstaller::new(
        vec![Event::CancelIgnored, Event::Done { changes: vec![] }],
        Ok(()),
    );
    let (_, out, _) = run_installer(&["upgrade"], &fake);
    assert!(out.contains("too late to cancel"), "{out}");
}

#[test]
fn helper_text_cannot_inject_terminal_escapes() {
    let evil = "evil\u{1b}[31m\u{1b}]0;pwned\u{7}";
    let fake = FakeInstaller::new(
        vec![
            Event::Started {
                op: Operation::Upgrade,
            },
            Event::Progress {
                stage: Stage::Download,
                package: Some(evil.into()),
                percent: Some(10),
            },
            Event::Done {
                changes: vec![Change {
                    package: evil.into(),
                    kind: ChangeKind::Installed,
                    version: Some(evil.into()),
                }],
            },
        ],
        Err(InstallError::Rejected {
            code: ErrorCode::Failed,
            message: evil.into(),
        }),
    );
    let (_, out, err) = run_installer(&["upgrade"], &fake);
    for text in [&out, &err] {
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{7}'),
            "raw control characters in {text:?}"
        );
    }
    assert!(
        out.contains("evil"),
        "the readable part must survive: {out}"
    );
}

#[test]
fn search_output_cannot_inject_terminal_escapes() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    let backend = || {
        FakeBackend::new(
            "fake",
            vec![pkg("fake", "alpha", "evil\u{1b}[2J description")],
        )
    };
    run_cli(&["--db", &db, "refresh"], backend());
    let (code, out, _) = run_cli(&["--db", &db, "search", "--plain", "alpha"], backend());
    assert_eq!(code, 0);
    assert!(!out.contains('\u{1b}'), "{out:?}");
    assert!(out.contains("description"), "{out}");
}

#[test]
fn remove_progress_is_summarised() {
    let events: Vec<Event> = (0..=100)
        .map(|p| progress(Stage::Remove, "cowsay", p))
        .collect();
    let fake = FakeInstaller::new(events, Ok(()));
    let (_, out, _) = run_installer(&["remove", "cowsay"], &fake);
    let lines = out.lines().filter(|l| l.starts_with("[remove]")).count();
    assert!(lines <= 12, "{lines} lines:\n{out}");
    assert!(out.contains("[remove] cowsay 100%"), "{out}");
    assert!(out.contains("[remove] cowsay 0%"), "{out}");
}

#[test]
fn a_non_utf8_argument_is_a_usage_error_not_a_panic() {
    use std::os::unix::ffi::OsStringExt;
    let argv = vec![
        OsString::from("store"),
        OsString::from("search"),
        OsString::from_vec(vec![0xff, 0xfe, b'x']),
    ];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let installer = FakeInstaller::new(vec![], Ok(()));
    let code = run(
        &argv,
        |_| panic!("no index needed"),
        &installer,
        &mut out,
        &mut err,
    );
    assert_ne!(code, 0);
    assert!(
        !err.is_empty() || !out.is_empty(),
        "the user must be told what was wrong"
    );
}

#[test]
fn an_unavailable_source_shows_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out, _) = run_cli(
        &["--db", &db(&dir), "refresh"],
        fake().unavailable_because("no catalog found"),
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("fake: unavailable (no catalog found)"),
        "{out}"
    );
}

fn two_sources() -> Vec<Box<dyn store_core::Backend>> {
    vec![
        Box::new(FakeBackend::new(
            "aur",
            vec![pkg("aur", "tool-aur", "from the aur")],
        )),
        Box::new(FakeBackend::new(
            "pacman",
            vec![pkg(
                "pacman",
                "tool-repo-with-a-rather-long-name",
                "from a repo",
            )],
        )),
    ]
}

fn run_two(args: &[&str]) -> (i32, String) {
    let argv: Vec<OsString> = std::iter::once("store")
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let installer = FakeInstaller::new(vec![], Ok(()));
    let code = run(
        &argv,
        |path| Ok(Store::new(Index::open(path)?, two_sources())),
        &installer,
        &mut out,
        &mut err,
    );
    (code, String::from_utf8(out).unwrap())
}

#[test]
fn search_puts_each_source_in_its_own_column() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    run_two(&["--db", &db, "refresh"]);
    let (code, out) = run_two(&["--db", &db, "search", "--width", "60", "tool"]);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    // pacman comes before aur, side by side on the same lines
    assert!(
        lines[0].starts_with("pacman") && lines[0].contains("aur"),
        "{out}"
    );
    // positions in characters: the shortened name contains a multi-byte `…`
    let chars_before = |line: &str, needle: &str| {
        line.find(needle)
            .map(|at| line[..at].chars().count())
            .unwrap()
    };
    let header = chars_before(lines[0], "aur");
    let row = lines
        .iter()
        .find(|l| l.contains("tool-aur"))
        .expect("aur row");
    assert_eq!(
        chars_before(row, "tool-aur"),
        header,
        "columns must line up:\n{out}"
    );
    assert!(
        lines.iter().all(|l| l.chars().count() <= 60),
        "no line wider than --width:\n{out}"
    );
    assert!(out.contains('…'), "the long name is shortened:\n{out}");
}

#[test]
fn search_plain_keeps_one_package_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    run_two(&["--db", &db, "refresh"]);
    let (_, out) = run_two(&["--db", &db, "search", "--plain", "tool"]);
    assert!(out.contains("aur/- tool-aur 1.0 — from the aur"), "{out}");
    assert!(
        out.contains("pacman/- tool-repo-with-a-rather-long-name 1.0 — from a repo"),
        "{out}"
    );
}

#[test]
fn search_with_one_source_hit_is_a_single_column() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(&dir);
    run_two(&["--db", &db, "refresh"]);
    let (code, out) = run_two(&["--db", &db, "search", "--width", "40", "tool-aur"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("aur"), "{out}");
    assert!(!out.contains("pacman"), "{out}");
}
