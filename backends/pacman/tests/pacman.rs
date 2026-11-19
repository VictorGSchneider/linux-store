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
    packages.iter().find(|p| p.name == name).unwrap_or_else(|| panic!("{name} missing"))
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
    assert_eq!(find(&packages, "hello").installed_version.as_deref(), Some("1.0-1"));
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
#[ignore = "reads the real system pacman databases; run manually"]
fn real_system_smoke() {
    let packages = PacmanBackend::from_system().packages().unwrap();
    assert!(packages.len() > 1000, "only {} packages", packages.len());
}
