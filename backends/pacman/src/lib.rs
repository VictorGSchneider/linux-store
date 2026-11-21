//! Read-only pacman backend: reads the sync and local databases through libalpm.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use alpm::{Alpm, SigLevel};
use store_core::{Backend, Error, Package, Result, SourceId};

const SOURCE: &str = "pacman";

enum Config {
    /// Use the system's `/etc/pacman.conf`.
    System,
    Paths {
        root: PathBuf,
        db_path: PathBuf,
        repos: Vec<String>,
    },
}

pub struct PacmanBackend {
    config: Config,
}

impl PacmanBackend {
    /// Reads repositories, root and database path from the system's `pacman.conf`.
    pub fn from_system() -> PacmanBackend {
        PacmanBackend {
            config: Config::System,
        }
    }

    /// Opens explicit paths and registers the named sync repos. Used by tests.
    pub fn with_paths(root: &Path, db_path: &Path, repos: &[&str]) -> PacmanBackend {
        PacmanBackend {
            config: Config::Paths {
                root: root.to_path_buf(),
                db_path: db_path.to_path_buf(),
                repos: repos.iter().map(|r| r.to_string()).collect(),
            },
        }
    }

    /// Opens a fresh libalpm handle. Handles are not `Send`/`Sync`, so one is made per call.
    fn open(&self) -> std::result::Result<Alpm, String> {
        match &self.config {
            Config::System => {
                let conf = pacmanconf::Config::new().map_err(|e| e.to_string())?;
                alpm_utils::alpm_with_conf(&conf).map_err(|e| e.to_string())
            }
            Config::Paths {
                root,
                db_path,
                repos,
            } => {
                let root = root.to_str().ok_or("root path is not valid UTF-8")?;
                let db_path = db_path.to_str().ok_or("db path is not valid UTF-8")?;
                let alpm = Alpm::new(root, db_path).map_err(|e| e.to_string())?;
                for repo in repos {
                    alpm.register_syncdb(repo.as_str(), SigLevel::NONE)
                        .map_err(|e| e.to_string())?;
                }
                Ok(alpm)
            }
        }
    }
}

fn clamp(n: i64) -> u64 {
    n.max(0) as u64
}

impl Backend for PacmanBackend {
    fn source(&self) -> SourceId {
        SourceId::new(SOURCE)
    }

    fn is_available(&self) -> bool {
        self.unavailable_reason().is_none()
    }

    fn unavailable_reason(&self) -> Option<String> {
        self.open().err()
    }

    fn packages(&self) -> Result<Vec<Package>> {
        let backend_error = |message: String| Error::Backend {
            source_id: self.source(),
            message,
        };
        let alpm = self.open().map_err(backend_error)?;
        let local = alpm.localdb();
        let mut seen = HashSet::new();
        let mut packages = Vec::new();

        for db in alpm.syncdbs() {
            let pkgs = db.pkgs();
            // libalpm hands back an empty list for a missing or corrupt sync database and only
            // records the reason in the handle's errno (`Db::is_valid` stays Ok). Treat that as
            // a failure, otherwise a refresh would silently wipe the repo's catalog.
            if pkgs.is_empty() && !alpm.last_error().ok() {
                return Err(backend_error(format!(
                    "repository {}: {}",
                    db.name(),
                    alpm.last_error()
                )));
            }
            for p in pkgs {
                // The first repo wins when a name appears in several, like pacman itself.
                if !seen.insert(p.name().to_string()) {
                    continue;
                }
                packages.push(Package {
                    source: self.source(),
                    name: p.name().to_string(),
                    version: p.version().to_string(),
                    installed_version: local.pkg(p.name()).ok().map(|l| l.version().to_string()),
                    title: None,
                    description: p.desc().map(str::to_string),
                    repo: Some(db.name().to_string()),
                    url: p.url().map(str::to_string),
                    download_size: clamp(p.size()),
                    installed_size: clamp(p.isize()),
                });
            }
        }

        for p in local.pkgs() {
            if seen.contains(p.name()) {
                continue;
            }
            packages.push(Package {
                source: self.source(),
                name: p.name().to_string(),
                version: p.version().to_string(),
                installed_version: Some(p.version().to_string()),
                title: None,
                description: p.desc().map(str::to_string),
                repo: None,
                url: p.url().map(str::to_string),
                download_size: 0,
                installed_size: clamp(p.isize()),
            });
        }
        Ok(packages)
    }
}
