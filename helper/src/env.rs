//! How libalpm is opened: from the real system, or (tests only) from a temporary root.
use std::path::PathBuf;

use alpm::{Alpm, TransFlag};
use store_protocol::ErrorCode;

use crate::HelperError;

pub struct Environment {
    kind: Kind,
}

enum Kind {
    System {
        db_path: PathBuf,
    },
    #[cfg(test)]
    Paths(TestPaths),
    #[cfg(test)]
    Unreachable,
}

#[cfg(test)]
struct TestPaths {
    root: PathBuf,
    db_path: PathBuf,
    cache_dir: PathBuf,
    hook_dir: PathBuf,
    repos: Vec<(String, String)>,
    archs: Vec<String>,
}

fn failed(message: String) -> HelperError {
    HelperError::new(ErrorCode::Failed, message)
}

impl Environment {
    /// The real system, configured by `/etc/pacman.conf` only.
    pub fn system() -> Result<Environment, HelperError> {
        let conf = pacmanconf::Config::new()
            .map_err(|e| failed(format!("cannot read pacman.conf: {e}")))?;
        Ok(Environment {
            kind: Kind::System {
                db_path: PathBuf::from(conf.db_path),
            },
        })
    }

    /// Temporary roots for tests. Does not exist in a build that ships.
    #[cfg(test)]
    pub(crate) fn with_paths(
        root: &std::path::Path,
        db_path: &std::path::Path,
        cache_dir: &std::path::Path,
        hook_dir: &std::path::Path,
        repos: &[(&str, &str)],
        archs: &[&str],
    ) -> Environment {
        Environment {
            kind: Kind::Paths(TestPaths {
                root: root.to_path_buf(),
                db_path: db_path.to_path_buf(),
                cache_dir: cache_dir.to_path_buf(),
                hook_dir: hook_dir.to_path_buf(),
                repos: repos
                    .iter()
                    .map(|(n, u)| (n.to_string(), u.to_string()))
                    .collect(),
                archs: archs.iter().map(|a| a.to_string()).collect(),
            }),
        }
    }

    /// An environment that panics if libalpm is ever opened, to prove a request never got there.
    #[cfg(test)]
    pub(crate) fn unreachable() -> Environment {
        Environment {
            kind: Kind::Unreachable,
        }
    }

    /// Opens a fresh libalpm handle (handles are not `Send`, so one is made per operation).
    pub(crate) fn open(&self) -> Result<Alpm, HelperError> {
        match &self.kind {
            Kind::System { .. } => {
                let conf = pacmanconf::Config::new()
                    .map_err(|e| failed(format!("cannot read pacman.conf: {e}")))?;
                // Also applies `DownloadUser`, so downloads run without root.
                alpm_utils::alpm_with_conf(&conf)
                    .map_err(|e| failed(format!("cannot open the package database: {e}")))
            }
            #[cfg(test)]
            Kind::Paths(t) => {
                let text = |p: &std::path::Path| {
                    p.to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| failed("path is not UTF-8".into()))
                };
                let fail = |e: alpm::Error| failed(format!("cannot open the test database: {e}"));
                let mut alpm = Alpm::new(text(&t.root)?, text(&t.db_path)?).map_err(fail)?;
                alpm.add_cachedir(text(&t.cache_dir)?).map_err(fail)?;
                alpm.add_hookdir(text(&t.hook_dir)?).map_err(fail)?;
                alpm.set_logfile(text(&t.db_path.join("pacman.log"))?)
                    .map_err(fail)?;
                for arch in &t.archs {
                    alpm.add_architecture(arch.as_str()).map_err(fail)?;
                }
                for (name, url) in &t.repos {
                    let db = alpm
                        .register_syncdb_mut(name.as_str(), alpm::SigLevel::NONE)
                        .map_err(fail)?;
                    db.add_server(url.as_str()).map_err(fail)?;
                }
                Ok(alpm)
            }
            #[cfg(test)]
            Kind::Unreachable => panic!("libalpm must not be opened for this request"),
        }
    }

    /// The lock file libalpm takes while a transaction is open.
    pub fn lock_path(&self) -> PathBuf {
        match &self.kind {
            Kind::System { db_path } => db_path.join("db.lck"),
            #[cfg(test)]
            Kind::Paths(t) => t.db_path.join("db.lck"),
            #[cfg(test)]
            Kind::Unreachable => PathBuf::new(),
        }
    }

    /// Scriptlets run for real; tests skip them (they would need a chroot).
    pub(crate) fn trans_flags(&self) -> TransFlag {
        match &self.kind {
            Kind::System { .. } => TransFlag::NONE,
            #[cfg(test)]
            _ => TransFlag::NO_SCRIPTLET,
        }
    }
}
