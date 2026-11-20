//! A throw-away pacman world for unprivileged tests: a temporary root, databases and cache, and
//! a `file://` repository built with the same tools pacman uses (`bsdtar`, `repo-add`).
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use crate::{Aborter, Environment};

pub(crate) struct Fixture {
    dir: TempDir,
}

/// `repo-add` on this system is a Rust program whose parallel instances race on a shared
/// temporary directory ("Failed to change pwd"), so the fixture runs the tools one at a time.
static TOOL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_tool(tool: &str, cwd: &Path, args: &[&std::ffi::OsStr]) {
    let _one_at_a_time = TOOL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let status = Command::new(tool)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| {
            panic!("the test fixture needs `{tool}` (Arch's base-devel provides it): {e}")
        });
    assert!(status.success(), "`{tool}` failed: {status}");
}

impl Fixture {
    pub(crate) fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        for sub in [
            "root", "db/local", "db/sync", "cache", "hooks", "repo", "stage",
        ] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        fs::write(dir.path().join("db/local/ALPM_DB_VERSION"), "9\n").unwrap();
        Fixture { dir }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    /// Adds `name-version` to the repository (not yet to the system's sync database).
    pub(crate) fn publish(&self, name: &str, version: &str, depends: &[&str]) {
        self.publish_full(name, version, depends, &[]);
    }

    pub(crate) fn publish_full(
        &self,
        name: &str,
        version: &str,
        depends: &[&str],
        conflicts: &[&str],
    ) {
        let stage = self.path("stage").join(format!("{name}-{version}"));
        fs::create_dir_all(stage.join("usr/bin")).unwrap();
        let binary = stage.join("usr/bin").join(name);
        fs::write(&binary, format!("#!/bin/sh\necho {name} {version}\n")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let mut info = format!(
            "pkgname = {name}\npkgver = {version}\npkgdesc = Fixture {name}\nurl = https://x.example\n\
             builddate = 1700000000\npackager = Test <t@x>\nsize = 64\narch = any\nlicense = GPL\n"
        );
        for dep in depends {
            info.push_str(&format!("depend = {dep}\n"));
        }
        for conflict in conflicts {
            info.push_str(&format!("conflict = {conflict}\n"));
        }
        fs::write(stage.join(".PKGINFO"), info).unwrap();

        let file_name = format!("{name}-{version}-any.pkg.tar.zst");
        let package = self.path("repo").join(&file_name);
        run_tool(
            "bsdtar",
            &self.path("repo"),
            &[
                "--zstd".as_ref(),
                "-cf".as_ref(),
                package.as_os_str(),
                "-C".as_ref(),
                stage.as_os_str(),
                ".PKGINFO".as_ref(),
                "usr".as_ref(),
            ],
        );
        run_tool(
            "repo-add",
            &self.path("repo"),
            &[
                "-q".as_ref(),
                self.path("repo/test.db.tar.gz").as_os_str(),
                package.as_os_str(),
            ],
        );
        // repo-add replaced the database entry; drop older files of the same package.
        let prefix = format!("{name}-");
        for entry in fs::read_dir(self.path("repo")).unwrap().flatten() {
            let other = entry.file_name().to_string_lossy().into_owned();
            let is_old = other != file_name
                && other.ends_with("-any.pkg.tar.zst")
                && other
                    .strip_prefix(&prefix)
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
            if is_old {
                fs::remove_file(entry.path()).unwrap();
            }
        }
    }

    /// Makes the system's sync database match the repository, like a finished `pacman -Sy`.
    pub(crate) fn sync(&self) {
        let target = self.path("db/sync/test.db");
        fs::copy(self.path("repo/test.db.tar.gz"), &target).unwrap();
        // libalpm refreshes a database only if the server's copy is newer, with one-second
        // resolution; backdate ours so a database published right afterwards always is.
        let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        fs::File::options()
            .write(true)
            .open(&target)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
    }

    /// The raw bytes of the system's sync database, to check that it was (not) refreshed.
    pub(crate) fn sync_db_bytes(&self) -> Vec<u8> {
        fs::read(self.path("db/sync/test.db")).unwrap()
    }

    pub(crate) fn env(&self) -> Environment {
        let repo_url = format!("file://{}", self.path("repo").display());
        Environment::with_paths(
            &self.path("root"),
            &self.path("db"),
            &self.path("cache"),
            &self.path("hooks"),
            &[("test", repo_url.as_str())],
            &["x86_64"],
        )
    }

    /// `name-version` of every package in the local database, sorted.
    pub(crate) fn installed(&self) -> Vec<String> {
        let alpm = self.env().open().unwrap();
        let mut found: Vec<String> = alpm
            .localdb()
            .pkgs()
            .iter()
            .map(|p| format!("{}-{}", p.name(), p.version()))
            .collect();
        found.sort();
        found
    }

    pub(crate) fn lock_path(&self) -> PathBuf {
        self.path("db/db.lck")
    }

    pub(crate) fn lock_exists(&self) -> bool {
        self.lock_path().exists()
    }

    pub(crate) fn file_exists(&self, relative_to_root: &str) -> bool {
        Path::new(&self.path("root"))
            .join(relative_to_root)
            .exists()
    }
}

/// Counts how many times the cancel gate asked to abort. The real aborter exits the process.
#[derive(Default)]
pub(crate) struct RecordingAborter(pub std::sync::atomic::AtomicUsize);

impl Aborter for RecordingAborter {
    fn abort(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
