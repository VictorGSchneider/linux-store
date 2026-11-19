//! Builds a tiny fake pacman root: sync repo `core` with `hello` and `world`, and a local db
//! with `hello` (older) and `foreign` (in no repo).
//!
//! libalpm derives name/version of a sync entry from its directory name `name-version`, the
//! local db needs `ALPM_DB_VERSION`, and the local `desc` uses `%SIZE%` for the installed size.
use std::fs::{self, File};
use std::path::Path;

use flate2::{Compression, write::GzEncoder};
use tempfile::TempDir;

pub struct Fixture {
    pub dir: TempDir,
}

impl Fixture {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn db_path(&self) -> std::path::PathBuf {
        self.dir.path().join("db")
    }
}

fn sync_entries() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "hello-2.0-1",
            "%NAME%\nhello\n\n%VERSION%\n2.0-1\n\n%DESC%\nHello tool\n\n%URL%\nhttps://hello.example\n\n%CSIZE%\n1234\n\n%ISIZE%\n5678\n",
        ),
        (
            "world-2.0-1",
            "%NAME%\nworld\n\n%VERSION%\n2.0-1\n\n%CSIZE%\n10\n\n%ISIZE%\n20\n",
        ),
    ]
}

pub fn build() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    fs::create_dir_all(db.join("sync")).unwrap();
    fs::create_dir_all(dir.path().join("root")).unwrap();

    let gz = GzEncoder::new(
        File::create(db.join("sync/core.db")).unwrap(),
        Compression::default(),
    );
    let mut tar = tar::Builder::new(gz);
    for (folder, desc) in sync_entries() {
        tar.append_dir(folder, ".").unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(desc.len() as u64);
        header.set_mode(0o644);
        tar.append_data(&mut header, format!("{folder}/desc"), desc.as_bytes())
            .unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();

    let local = db.join("local");
    fs::create_dir_all(&local).unwrap();
    fs::write(local.join("ALPM_DB_VERSION"), "9\n").unwrap();
    for (folder, desc) in [
        (
            "hello-1.0-1",
            "%NAME%\nhello\n\n%VERSION%\n1.0-1\n\n%SIZE%\n5000\n",
        ),
        (
            "foreign-3-1",
            "%NAME%\nforeign\n\n%VERSION%\n3-1\n\n%DESC%\nNot in any repo\n\n%SIZE%\n42\n",
        ),
    ] {
        fs::create_dir_all(local.join(folder)).unwrap();
        fs::write(local.join(folder).join("desc"), desc).unwrap();
    }
    Fixture { dir }
}
