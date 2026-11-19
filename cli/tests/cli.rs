use std::ffi::OsString;
use std::path::PathBuf;

use store_cli::{default_db_path_from, run};
use store_core::testing::{FakeBackend, pkg};
use store_core::{Index, Package, Store};

/// Runs the CLI against `db` with one fake backend; returns (exit code, stdout, stderr).
fn run_cli(args: &[&str], backend: FakeBackend) -> (i32, String, String) {
    let argv: Vec<String> = std::iter::once("store")
        .chain(args.iter().copied())
        .map(String::from)
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(
        &argv,
        move |path| Ok(Store::new(Index::open(path)?, vec![Box::new(backend)])),
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
    let (code, out, _) = run_cli(&["--db", &db, "search", "alpha"], fake());
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
