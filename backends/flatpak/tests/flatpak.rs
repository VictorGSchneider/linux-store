use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, write::GzEncoder};
use store_backend_flatpak::{FlatpakBackend, installation_dirs, user_installation_dir};
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

fn app_with_icon(id: &str) -> String {
    format!(
        r#"<component type="desktop-application">
  <id>{id}</id>
  <name>Iconic</name>
  <categories><category>Graphics</category><category>Utility</category></categories>
  <icon type="cached" width="64" height="64">{id}.png</icon>
  <icon type="cached" width="128" height="128">{id}.png</icon>
  <bundle type="flatpak">app/{id}/{ARCH}/stable</bundle>
</component>"#
    )
}

#[test]
fn package_has_app_id_categories_and_icon() {
    let dir = tempfile::tempdir().unwrap();
    write_catalog(
        dir.path(),
        "flathub",
        &catalog(&[app_with_icon("org.example.Icon")]),
    );
    let icons = dir
        .path()
        .join("appstream/flathub")
        .join(ARCH)
        .join("active/icons/128x128");
    fs::create_dir_all(&icons).unwrap();
    fs::write(icons.join("org.example.Icon.png"), b"png").unwrap();
    let packages = backend(&[dir.path()]).packages().unwrap();
    let p = find(&packages, "org.example.Icon");
    assert_eq!(p.app_id.as_deref(), Some("org.example.Icon"));
    assert_eq!(p.categories, ["Graphics", "Utility"]);
    assert_eq!(
        p.icon.as_deref(),
        Some(icons.join("org.example.Icon.png").to_str().unwrap())
    );
}

#[test]
fn missing_icon_file_gives_no_icon() {
    let (_dir, packages) = load(&catalog(&[app_with_icon("org.example.NoFile")]));
    let p = find(&packages, "org.example.NoFile");
    assert_eq!(p.app_id.as_deref(), Some("org.example.NoFile"));
    assert_eq!(p.icon, None);
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
    // Flathub ships Firefox with the legacy type="desktop"; it must not be dropped.
    assert!(
        packages.iter().any(|p| p.name == "org.mozilla.firefox"),
        "org.mozilla.firefox missing"
    );
}

#[test]
fn legacy_desktop_type_is_an_application() {
    // Real Flathub ships org.mozilla.firefox with the legacy type="desktop".
    let firefox = app("org.mozilla.firefox", "Firefox", "", "stable")
        .replace("desktop-application", "desktop");
    let (_dir, packages) = load(&catalog(&[firefox]));
    assert_eq!(
        find(&packages, "org.mozilla.firefox").title.as_deref(),
        Some("Firefox")
    );
}

#[test]
fn empty_or_non_xml_catalog_is_an_error() {
    for (label, content) in [
        ("zero bytes", ""),
        ("xml declaration only", "<?xml version=\"1.0\"?>"),
        ("plain text", "hello world"),
        ("wrong root", "<other/>"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write_catalog(dir.path(), "flathub", content);
        let message = backend_error(backend(&[dir.path()]).packages());
        assert!(message.contains("flathub"), "{label}: {message}");
    }
}

#[test]
fn empty_catalog_does_not_hide_a_working_remote() {
    let dir = tempfile::tempdir().unwrap();
    write_catalog(
        dir.path(),
        "good",
        &catalog(&[app("org.example.App", "App", "", "stable")]),
    );
    write_catalog(dir.path(), "truncated-to-nothing", "");
    // The whole refresh fails, so the previously indexed Flatpak data is kept.
    backend_error(backend(&[dir.path()]).packages());
}

#[test]
fn gzip_of_nothing_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let catalog_dir = dir
        .path()
        .join("appstream/flathub")
        .join(ARCH)
        .join("active");
    fs::create_dir_all(&catalog_dir).unwrap();
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    fs::write(catalog_dir.join("appstream.xml.gz"), enc.finish().unwrap()).unwrap();
    backend_error(backend(&[dir.path()]).packages());
}

#[test]
fn installed_on_another_branch_counts() {
    let dir = tempfile::tempdir().unwrap();
    write_catalog(
        dir.path(),
        "flathub",
        &catalog(&[app("org.example.App", "App", two_releases(), "stable")]),
    );
    install(dir.path(), "org.example.App", "beta");
    let packages = backend(&[dir.path()]).packages().unwrap();
    assert!(
        find(&packages, "org.example.App")
            .installed_version
            .is_some()
    );
}

#[test]
fn installed_apps_missing_from_the_catalog_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    write_catalog(
        dir.path(),
        "flathub",
        &catalog(&[app("org.example.App", "App", "", "stable")]),
    );
    install(dir.path(), "org.gone.Legacy", "stable");
    let packages = backend(&[dir.path()]).packages().unwrap();
    let gone = find(&packages, "org.gone.Legacy");
    assert_eq!(gone.installed_version.as_deref(), Some("stable"));
    assert_eq!(gone.version, "stable");
    assert_eq!((gone.title.as_deref(), gone.repo.as_deref()), (None, None));
}

#[test]
fn user_installation_honours_xdg_data_home() {
    use std::ffi::OsString;
    let path = |x: Option<&str>, h: Option<&str>| {
        user_installation_dir(x.map(OsString::from), h.map(OsString::from))
    };
    assert_eq!(
        path(Some("/data"), Some("/home/u")),
        Some(PathBuf::from("/data/flatpak"))
    );
    assert_eq!(
        path(None, Some("/home/u")),
        Some(PathBuf::from("/home/u/.local/share/flatpak"))
    );
    assert_eq!(
        path(Some(""), Some("/home/u")),
        Some(PathBuf::from("/home/u/.local/share/flatpak"))
    );
    assert_eq!(path(None, None), None);
}

#[test]
fn development_releases_are_not_the_version() {
    let releases = r#"<releases>
        <release version="48.beta" timestamp="900" type="development"/>
        <release version="47.1" timestamp="800" type="stable"/>
        <release version="47.0" timestamp="700"/></releases>"#;
    let (_dir, packages) = load(&catalog(&[app(
        "org.example.App",
        "App",
        releases,
        "stable",
    )]));
    assert_eq!(find(&packages, "org.example.App").version, "47.1");
}

#[test]
fn an_app_with_only_development_releases_falls_back_to_the_branch() {
    let releases =
        r#"<releases><release version="1.0-rc1" timestamp="9" type="development"/></releases>"#;
    let (_dir, packages) = load(&catalog(&[app(
        "org.example.App",
        "App",
        releases,
        "stable",
    )]));
    assert_eq!(find(&packages, "org.example.App").version, "stable");
}

#[test]
fn cdata_text_is_kept_as_written() {
    let xml = catalog(&[
        "<component type=\"desktop-application\"><id>org.example.C</id>\
        <name><![CDATA[Tom & Jerry]]></name>\
        <summary>Fun <![CDATA[<b>and</b>]]> games</summary></component>"
            .to_string(),
    ]);
    let (_dir, packages) = load(&xml);
    let package = find(&packages, "org.example.C");
    assert_eq!(package.title.as_deref(), Some("Tom & Jerry"));
    assert_eq!(package.description.as_deref(), Some("Fun <b>and</b> games"));
}

#[test]
fn entities_in_attributes_are_decoded() {
    let releases = r#"<releases><release version="1&amp;2" timestamp="1"/></releases>"#;
    let (_dir, packages) = load(&catalog(&[app(
        "org.example.App",
        "App",
        releases,
        "stable",
    )]));
    assert_eq!(find(&packages, "org.example.App").version, "1&2");
}

#[test]
fn environment_and_config_files_decide_the_installations() {
    use std::ffi::OsString;
    let config = tempfile::tempdir().unwrap();
    let conf = config.path().join("installations.d");
    fs::create_dir_all(&conf).unwrap();
    fs::write(
        conf.join("games.conf"),
        "# an extra installation\n[Installation \"games\"]\nPath=/mnt/games/flatpak\nDisplayName=Games\n",
    )
    .unwrap();
    fs::write(conf.join("notes.txt"), "Path=/not/a/conf/file\n").unwrap();
    let dirs = |system: Option<&str>, user: Option<&str>| {
        installation_dirs(
            system.map(OsString::from),
            user.map(OsString::from),
            None,
            Some(OsString::from("/home/u")),
            config.path(),
        )
    };
    assert_eq!(
        dirs(None, None),
        [
            PathBuf::from("/var/lib/flatpak"),
            PathBuf::from("/home/u/.local/share/flatpak"),
            PathBuf::from("/mnt/games/flatpak"),
        ]
    );
    assert_eq!(
        dirs(Some("/srv/system"), Some("/srv/user")),
        [
            PathBuf::from("/srv/system"),
            PathBuf::from("/srv/user"),
            PathBuf::from("/mnt/games/flatpak"),
        ]
    );
}
