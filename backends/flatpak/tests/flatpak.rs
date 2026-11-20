use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, write::GzEncoder};
use store_backend_flatpak::FlatpakBackend;
use store_core::{Backend, Error, Package, SourceId};

const ARCH: &str = "x86_64";

fn app(id: &str, name: &str, releases: &str, branch: &str) -> String {
    format!(
        r#"<component type="desktop-application">
  <id>{id}</id>
  <name>{name}</name>
  <name xml:lang="pl">Polish {name}</name>
  <summary>Summary of {name}</summary>
  <summary xml:lang="pl">Polish summary</summary>
  <url type="homepage">https://{id}.example</url>
  <url type="bugtracker">https://bugs.example</url>
  <developer><name>Someone</name></developer>
  {releases}
  <bundle type="flatpak" runtime="org.gnome.Platform/x86_64/51">app/{id}/{ARCH}/{branch}</bundle>
</component>"#
    )
}

fn catalog(components: &[String]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<components version=\"0.8\" origin=\"flatpak\">\n{}\n</components>\n",
        components.join("\n")
    )
}

/// `<root>/appstream/<remote>/x86_64/active/appstream.xml`
fn write_catalog(root: &Path, remote: &str, xml: &str) {
    let dir = root
        .join("appstream")
        .join(remote)
        .join(ARCH)
        .join("active");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("appstream.xml"), xml).unwrap();
}

fn install(root: &Path, id: &str, branch: &str) {
    fs::create_dir_all(
        root.join("app")
            .join(id)
            .join(ARCH)
            .join(branch)
            .join("active"),
    )
    .unwrap();
}

fn backend(roots: &[&Path]) -> FlatpakBackend {
    let dirs: Vec<PathBuf> = roots.iter().map(|p| p.to_path_buf()).collect();
    FlatpakBackend::with_installations(&dirs, ARCH)
}

fn load(xml: &str) -> (tempfile::TempDir, Vec<Package>) {
    let dir = tempfile::tempdir().unwrap();
    write_catalog(dir.path(), "flathub", xml);
    let packages = backend(&[dir.path()]).packages().unwrap();
    (dir, packages)
}

fn find<'a>(packages: &'a [Package], id: &str) -> &'a Package {
    packages
        .iter()
        .find(|p| p.name == id)
        .unwrap_or_else(|| panic!("{id} missing"))
}

fn two_releases() -> &'static str {
    r#"<releases><release version="1.0" timestamp="100"/><release version="1.1" timestamp="200"/></releases>"#
}

#[test]
fn keeps_only_applications() {
    let console = app("org.example.Cli", "Cli", "", "stable")
        .replace("desktop-application", "console-application");
    let runtime = app("org.example.Platform", "Platform", "", "stable")
        .replace("desktop-application", "runtime");
    let addon =
        app("org.example.Addon", "Addon", "", "stable").replace("desktop-application", "addon");
    let (_dir, packages) = load(&catalog(&[
        app("org.example.App", "App", "", "stable"),
        console,
        runtime,
        addon,
    ]));
    let mut names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["org.example.App", "org.example.Cli"]);
}

#[test]
fn maps_fields() {
    let (_dir, packages) = load(&catalog(&[app(
        "org.example.App",
        "Example",
        two_releases(),
        "stable",
    )]));
    let p = find(&packages, "org.example.App");
    assert_eq!(p.source, SourceId::new("flatpak"));
    assert_eq!(p.title.as_deref(), Some("Example"));
    assert_eq!(p.description.as_deref(), Some("Summary of Example"));
    assert_eq!(p.version, "1.1");
    assert_eq!(p.repo.as_deref(), Some("flathub"));
    assert_eq!(p.url.as_deref(), Some("https://org.example.App.example"));
    assert_eq!((p.download_size, p.installed_size), (0, 0));
    assert_eq!(p.installed_version, None);
}

#[test]
fn decodes_entities() {
    let xml = catalog(&[app(
        "org.example.App",
        "Tom &amp; Jerry &#8212; &lt;1&gt;",
        "",
        "stable",
    )]);
    let (_dir, packages) = load(&xml);
    assert_eq!(
        find(&packages, "org.example.App").title.as_deref(),
        Some("Tom & Jerry — <1>")
    );
}

#[test]
fn branches_collapse_to_one_entry_preferring_stable() {
    let beta = app(
        "org.example.App",
        "App",
        r#"<releases><release version="2.0-beta" timestamp="900"/></releases>"#,
        "beta",
    );
    let stable = app(
        "org.example.App",
        "App",
        r#"<releases><release version="1.0" timestamp="100"/></releases>"#,
        "stable",
    );
    let (_dir, packages) = load(&catalog(&[beta, stable]));
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].version, "1.0");
}

#[test]
fn no_releases_falls_back_to_branch() {
    let (_dir, packages) = load(&catalog(&[app("org.example.App", "App", "", "stable")]));
    assert_eq!(packages[0].version, "stable");
}

#[test]
fn marks_installed_apps() {
    let dir = tempfile::tempdir().unwrap();
    write_catalog(
        dir.path(),
        "flathub",
        &catalog(&[
            app("org.example.One", "One", two_releases(), "stable"),
            app("org.example.Two", "Two", two_releases(), "stable"),
        ]),
    );
    install(dir.path(), "org.example.One", "stable");
    let packages = backend(&[dir.path()]).packages().unwrap();
    assert!(
        find(&packages, "org.example.One")
            .installed_version
            .is_some()
    );
    assert_eq!(find(&packages, "org.example.Two").installed_version, None);
}

#[test]
fn reads_gzipped_catalogs() {
    let dir = tempfile::tempdir().unwrap();
    let xml = catalog(&[app("org.example.App", "App", "", "stable")]);
    let catalog_dir = dir
        .path()
        .join("appstream/flathub")
        .join(ARCH)
        .join("active");
    fs::create_dir_all(&catalog_dir).unwrap();
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(xml.as_bytes()).unwrap();
    fs::write(catalog_dir.join("appstream.xml.gz"), enc.finish().unwrap()).unwrap();
    let packages = backend(&[dir.path()]).packages().unwrap();
    assert_eq!(packages.len(), 1);
}

#[test]
fn reads_user_and_system_installations() {
    let system = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let xml = catalog(&[app("org.example.App", "App", "", "stable")]);
    write_catalog(system.path(), "flathub", &xml);
    write_catalog(user.path(), "flathub", &xml);
    install(user.path(), "org.example.App", "stable");
    let packages = backend(&[system.path(), user.path()]).packages().unwrap();
    assert_eq!(packages.len(), 1);
    assert!(
        packages[0].installed_version.is_some(),
        "installed in the user installation"
    );
}

fn backend_error(result: store_core::Result<Vec<Package>>) -> String {
    match result {
        Err(Error::Backend { source_id, message }) => {
            assert_eq!(source_id, SourceId::new("flatpak"));
            message
        }
        other => panic!("expected a backend error, got {other:?}"),
    }
}

#[test]
fn broken_xml_is_an_error_naming_the_remote() {
    let xml = catalog(&[app("org.example.App", "App", "", "stable")]);
    let dir = tempfile::tempdir().unwrap();
    write_catalog(dir.path(), "flathub", &xml[..xml.len() / 2]);
    let message = backend_error(backend(&[dir.path()]).packages());
    assert!(message.contains("flathub"), "{message}");
}

#[test]
fn remote_with_missing_catalog_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    write_catalog(
        dir.path(),
        "good",
        &catalog(&[app("org.example.App", "App", "", "stable")]),
    );
    fs::create_dir_all(
        dir.path()
            .join("appstream/broken")
            .join(ARCH)
            .join("active"),
    )
    .unwrap();
    let message = backend_error(backend(&[dir.path()]).packages());
    assert!(message.contains("broken"), "{message}");
}

#[test]
fn no_catalog_means_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!backend(&[dir.path()]).is_available());
    write_catalog(dir.path(), "flathub", &catalog(&[]));
    assert!(backend(&[dir.path()]).is_available());
}

#[test]
#[ignore = "reads the real Flatpak catalog on this machine; run manually"]
fn real_system_smoke() {
    let packages = FlatpakBackend::from_system().packages().unwrap();
    assert!(packages.len() > 1000, "only {} apps", packages.len());
}
