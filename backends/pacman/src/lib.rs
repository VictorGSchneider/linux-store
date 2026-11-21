//! Read-only pacman backend: reads the sync and local databases through libalpm.
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use alpm::{Alpm, SigLevel, vercmp};
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
    /// Directories holding `xml/*.xml[.gz]` and `icons/<origin>/<WxH>/`.
    catalog_dirs: Vec<PathBuf>,
}

/// What the AppStream catalog says about one package name.
struct CatalogEntry {
    app_id: String,
    title: Option<String>,
    categories: Vec<String>,
    icon: Option<String>,
}

/// True when `value` is exactly one ordinary path component, so joining it onto a directory
/// cannot leave that directory (no `..`, no absolute path, no subdirectory, not empty).
/// Duplicated from the Flatpak backend on purpose.
fn is_plain_file_name(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

/// The cached 64 px icon, else the narrowest cached one, when its file is really on disk.
fn catalog_icon(c: &store_appstream::Component, icons_root: &Path) -> Option<String> {
    let icon = c
        .icons
        .iter()
        .filter(|i| i.kind == "cached")
        .find(|i| i.width == Some(64))
        .or_else(|| {
            c.icons
                .iter()
                .filter(|i| i.kind == "cached" && i.width.is_some())
                .min_by_key(|i| i.width)
        })?;
    if !is_plain_file_name(&icon.value) {
        return None;
    }
    let width = icon.width?;
    let path = icons_root
        .join(format!("{width}x{width}"))
        .join(&icon.value);
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

/// Maps package names to their first application component. Unreadable catalogs are skipped:
/// enrichment is optional.
fn read_catalog(dirs: &[PathBuf]) -> HashMap<String, CatalogEntry> {
    let mut map = HashMap::new();
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(dir.join("xml")) else {
            continue;
        };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.ends_with(".xml") || name.ends_with(".xml.gz")
            })
            .collect();
        files.sort();
        for file in files {
            // `origin` is only known once parsing ends, and it picks the icon directory, so
            // application components are held until then. A file that fails to parse is skipped.
            let mut apps = Vec::new();
            let origin = match store_appstream::parse_file(&file, &mut |c| {
                let is_app = matches!(
                    c.kind.as_str(),
                    "desktop-application" | "console-application" | "desktop"
                );
                if is_app && !c.id.is_empty() && !c.pkgnames.is_empty() {
                    apps.push(c);
                }
            }) {
                Ok(origin) => origin,
                Err(_) => continue,
            };
            // The origin comes from the same untrusted file as the icon values.
            let icons_root = origin
                .filter(|o| is_plain_file_name(o))
                .map(|o| dir.join("icons").join(o));
            for c in apps {
                let icon = icons_root
                    .as_deref()
                    .and_then(|root| catalog_icon(&c, root));
                for name in &c.pkgnames {
                    map.entry(name.clone()).or_insert_with(|| CatalogEntry {
                        app_id: c.id.clone(),
                        title: c.name.clone().filter(|n| !n.is_empty()),
                        categories: c.categories.clone(),
                        icon: icon.clone(),
                    });
                }
            }
        }
    }
    map
}

impl PacmanBackend {
    /// Reads repositories, root and database path from the system's `pacman.conf`.
    pub fn from_system() -> PacmanBackend {
        PacmanBackend {
            config: Config::System,
            catalog_dirs: vec![
                PathBuf::from("/usr/share/swcatalog"),
                PathBuf::from("/usr/share/app-info"),
            ],
        }
    }

    /// Reads the AppStream catalogs under `dirs` (each with `xml/*.xml[.gz]` and
    /// `icons/<origin>/<WxH>/`) to enrich packages with an app id, title, categories and icon.
    pub fn with_catalog_dirs(mut self, dirs: &[PathBuf]) -> PacmanBackend {
        self.catalog_dirs = dirs.to_vec();
        self
    }

    /// Opens explicit paths and registers the named sync repos. Used by tests.
    pub fn with_paths(root: &Path, db_path: &Path, repos: &[&str]) -> PacmanBackend {
        PacmanBackend {
            config: Config::Paths {
                root: root.to_path_buf(),
                db_path: db_path.to_path_buf(),
                repos: repos.iter().map(|r| r.to_string()).collect(),
            },
            catalog_dirs: Vec::new(),
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
        let catalog = read_catalog(&self.catalog_dirs);

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
                let installed = local.pkg(p.name()).ok().map(|l| l.version().to_string());
                let upgradable = installed
                    .as_deref()
                    .is_some_and(|i| vercmp(p.version().as_str(), i) == Ordering::Greater);
                let mut package = Package {
                    source: self.source(),
                    name: p.name().to_string(),
                    version: p.version().to_string(),
                    installed_version: installed,
                    title: None,
                    description: p.desc().map(str::to_string),
                    repo: Some(db.name().to_string()),
                    url: p.url().map(str::to_string),
                    download_size: clamp(p.size()),
                    installed_size: clamp(p.isize()),
                    app_id: None,
                    categories: Vec::new(),
                    icon: None,
                    popularity: None,
                    upgradable,
                };
                if let Some(entry) = catalog.get(p.name()) {
                    package.app_id = Some(entry.app_id.clone());
                    package.title = entry.title.clone();
                    package.categories = entry.categories.clone();
                    package.icon = entry.icon.clone();
                }
                packages.push(package);
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
                app_id: None,
                categories: Vec::new(),
                icon: None,
                popularity: None,
                upgradable: false,
            });
        }
        Ok(packages)
    }
}
