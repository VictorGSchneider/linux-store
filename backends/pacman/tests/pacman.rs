mod fixture;

use store_backend_pacman::PacmanBackend;
use store_core::{Backend, Package, SourceId};

fn load() -> (fixture::Fixture, Vec<Package>) {
    let fx = fixture::build();
    let backend = PacmanBackend::with_paths(&fx.root().join("root"), &fx.db_path(), &["core"]);
    let packages = backend.packages().unwrap();
    (fx, packages)
}

fn find<'a>(packages: &'a [Package], name: &str) -> &'a Package {
    packages
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("{name} missing"))
}

#[test]
fn lists_sync_packages_with_metadata() {
    let (_fx, packages) = load();
    let hello = find(&packages, "hello");
    assert_eq!(hello.source, SourceId::new("pacman"));
    assert_eq!(hello.version, "2.0-1");
    assert_eq!(hello.description.as_deref(), Some("Hello tool"));
    assert_eq!(hello.repo.as_deref(), Some("core"));
    assert_eq!(hello.url.as_deref(), Some("https://hello.example"));
    assert_eq!(hello.download_size, 1234);
    assert_eq!(hello.installed_size, 5678);
}

#[test]
fn marks_installed_version() {
    let (_fx, packages) = load();
    assert_eq!(
        find(&packages, "hello").installed_version.as_deref(),
        Some("1.0-1")
    );
    assert_eq!(find(&packages, "world").installed_version, None);
}

#[test]
fn includes_foreign_installed_packages() {
    let (_fx, packages) = load();
    let foreign = find(&packages, "foreign");
    assert_eq!(foreign.repo, None);
    assert_eq!(foreign.version, "3-1");
    assert_eq!(foreign.installed_version.as_deref(), Some("3-1"));
    assert_eq!(foreign.description.as_deref(), Some("Not in any repo"));
}

#[test]
fn source_is_pacman() {
    let fx = fixture::build();
    let backend = PacmanBackend::with_paths(&fx.root().join("root"), &fx.db_path(), &["core"]);
    assert_eq!(backend.source(), SourceId::new("pacman"));
    assert!(backend.is_available());
}

#[test]
fn missing_db_is_unavailable_not_a_panic() {
    let nowhere = std::path::Path::new("/nonexistent/linux-store-test");
    let backend = PacmanBackend::with_paths(nowhere, &nowhere.join("db"), &["core"]);
    assert!(!backend.is_available());
}

#[test]
fn corrupt_sync_db_is_an_error_not_an_empty_catalog() {
    let fx = fixture::build();
    std::fs::write(fx.db_path().join("sync/core.db"), "garbage").unwrap();
    let backend = PacmanBackend::with_paths(&fx.root().join("root"), &fx.db_path(), &["core"]);
    match backend.packages() {
        Err(store_core::Error::Backend { message, .. }) => {
            assert!(message.contains("core"), "{message}")
        }
        other => panic!("expected a backend error, got {other:?}"),
    }
}

#[test]
fn missing_sync_db_is_an_error_not_an_empty_catalog() {
    let fx = fixture::build();
    std::fs::remove_file(fx.db_path().join("sync/core.db")).unwrap();
    let backend = PacmanBackend::with_paths(&fx.root().join("root"), &fx.db_path(), &["core"]);
    assert!(matches!(
        backend.packages(),
        Err(store_core::Error::Backend { .. })
    ));
}

#[test]
#[ignore = "reads the real system pacman databases; run manually"]
fn real_system_smoke() {
    let packages = PacmanBackend::from_system().packages().unwrap();
    assert!(packages.len() > 1000, "only {} packages", packages.len());
}

fn catalog_backend(fx: &fixture::Fixture, catalog: &std::path::Path) -> PacmanBackend {
    PacmanBackend::with_paths(&fx.root().join("root"), &fx.db_path(), &["core"])
        .with_catalog_dirs(&[catalog.to_path_buf()])
}

/// A catalog dir with `xml/core.xml` (origin `test-core`) and, when `icon` is given, that file
/// under `icons/test-core/64x64/`.
fn write_catalog(xml: &str, icon: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("xml")).unwrap();
    std::fs::write(dir.path().join("xml/core.xml"), xml).unwrap();
    if let Some(icon) = icon {
        let icons = dir.path().join("icons/test-core/64x64");
        std::fs::create_dir_all(&icons).unwrap();
        std::fs::write(icons.join(icon), "x").unwrap();
    }
    dir
}

fn firefox_xml(icon_value: &str) -> String {
    format!(
        r#"<components version="0.16" origin="test-core">
  <component type="desktop-application">
    <id>org.mozilla.firefox</id>
    <name>Firefox</name>
    <pkgname>firefox</pkgname>
    <categories><category>Network</category><category>WebBrowser</category></categories>
    <icon type="cached" width="64" height="64">{icon_value}</icon>
  </component>
</components>"#
    )
}

#[test]
fn catalog_enriches_packages_by_pkgname() {
    let fx = fixture::build();
    let cat = write_catalog(&firefox_xml("firefox.jxl"), Some("firefox.jxl"));
    let packages = catalog_backend(&fx, cat.path()).packages().unwrap();
    let ff = find(&packages, "firefox");
    assert_eq!(ff.app_id.as_deref(), Some("org.mozilla.firefox"));
    assert_eq!(ff.title.as_deref(), Some("Firefox"));
    assert_eq!(ff.categories, vec!["Network", "WebBrowser"]);
    let expected = cat.path().join("icons/test-core/64x64/firefox.jxl");
    assert_eq!(ff.icon.as_deref(), expected.to_str());
    // Packages the catalog does not mention stay untouched.
    assert_eq!(find(&packages, "world").app_id, None);
}

#[test]
fn catalog_ignores_non_application_components() {
    let fx = fixture::build();
    let xml = firefox_xml("firefox.jxl").replace("desktop-application", "runtime");
    let cat = write_catalog(&xml, None);
    let packages = catalog_backend(&fx, cat.path()).packages().unwrap();
    assert_eq!(find(&packages, "firefox").app_id, None);
}

#[test]
fn missing_catalog_changes_nothing() {
    let fx = fixture::build();
    let missing = std::path::Path::new("/nonexistent/linux-store-catalog");
    let with = catalog_backend(&fx, missing).packages().unwrap();
    let (_fx2, without) = load();
    assert_eq!(with, without);
}

#[test]
fn corrupt_catalog_is_ignored_not_an_error() {
    let fx = fixture::build();
    let cat = write_catalog(&firefox_xml("firefox.jxl"), Some("firefox.jxl"));
    // Sorts before core.xml, so an early return on the first failure would skip the valid file.
    std::fs::write(cat.path().join("xml/a-corrupt.xml.gz"), "not gzip").unwrap();
    std::fs::write(
        cat.path().join("xml/b-corrupt.xml"),
        "<components><component",
    )
    .unwrap();
    let packages = catalog_backend(&fx, cat.path()).packages().unwrap();
    let ff = find(&packages, "firefox");
    assert_eq!(ff.app_id.as_deref(), Some("org.mozilla.firefox"));
    assert_eq!(ff.categories, vec!["Network", "WebBrowser"]);
    assert!(ff.icon.is_some());
}

#[test]
fn icon_must_be_a_plain_file_name_inside_the_icon_dir() {
    let fx = fixture::build();
    for value in ["../x.jxl", "/etc/passwd", "", "a/b.jxl"] {
        let cat = write_catalog(&firefox_xml(value), Some("unrelated.jxl"));
        // Files at the places the value would reach if it were joined unchecked.
        let icons = cat.path().join("icons/test-core");
        std::fs::write(icons.join("x.jxl"), "x").unwrap();
        std::fs::create_dir_all(icons.join("64x64/a")).unwrap();
        std::fs::write(icons.join("64x64/a/b.jxl"), "x").unwrap();
        let packages = catalog_backend(&fx, cat.path()).packages().unwrap();
        let ff = find(&packages, "firefox");
        assert_eq!(ff.icon, None, "value {value:?}");
        assert_eq!(ff.app_id.as_deref(), Some("org.mozilla.firefox"));
    }
}

#[test]
fn icon_missing_on_disk_is_none() {
    let fx = fixture::build();
    let cat = write_catalog(&firefox_xml("firefox.jxl"), None);
    let packages = catalog_backend(&fx, cat.path()).packages().unwrap();
    assert_eq!(find(&packages, "firefox").icon, None);
}

#[test]
fn package_is_upgradable_when_repo_version_is_newer() {
    let (_fx, packages) = load();
    assert!(find(&packages, "hello").upgradable); // 2.0-1 in the repo, 1.0-1 installed
    assert!(!find(&packages, "world").upgradable); // not installed
}

#[test]
fn local_newer_than_repo_is_not_upgradable() {
    let (_fx, packages) = load();
    let ahead = find(&packages, "ahead"); // 1.0-1 in the repo, 2.0-1 installed
    assert_eq!(ahead.installed_version.as_deref(), Some("2.0-1"));
    assert!(!ahead.upgradable);
    assert!(!find(&packages, "foreign").upgradable);
}

#[test]
#[ignore = "reads the real system databases and catalog; run manually"]
fn catalog_enrichment_timing() {
    use std::time::Instant;
    let start = Instant::now();
    let with = PacmanBackend::from_system().packages().unwrap();
    let with_time = start.elapsed();
    let start = Instant::now();
    let without = PacmanBackend::from_system()
        .with_catalog_dirs(&[])
        .packages()
        .unwrap();
    let without_time = start.elapsed();
    let enriched = with.iter().filter(|p| p.app_id.is_some()).count();
    let icons = with.iter().filter(|p| p.icon.is_some()).count();
    println!(
        "with catalog: {with_time:?} ({enriched} enriched, {icons} icons); without: {without_time:?} ({} packages)",
        without.len()
    );
}

#[test]
fn an_unavailable_backend_says_why() {
    let nowhere = std::path::Path::new("/nonexistent/linux-store-test");
    let backend = PacmanBackend::with_paths(nowhere, &nowhere.join("db"), &["core"]);
    let reason = backend.unavailable_reason().expect("a reason");
    assert!(!reason.is_empty());
    let fx = fixture::build();
    let working = PacmanBackend::with_paths(&fx.root().join("root"), &fx.db_path(), &["core"]);
    assert_eq!(working.unavailable_reason(), None);
}
