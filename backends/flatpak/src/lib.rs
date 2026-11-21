//! Read-only Flatpak backend. It reads the AppStream catalog that Flatpak already keeps on
//! disk for every remote, streaming the XML so the 48 MiB Flathub file is never held whole.
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use store_appstream::Component;
use store_core::{Backend, Error, Package, Result, SourceId};

const SOURCE: &str = "flatpak";

pub struct FlatpakBackend {
    installations: Vec<PathBuf>,
    arch: String,
}

impl FlatpakBackend {
    /// The system installation (`/var/lib/flatpak`) and the user one
    /// (`$XDG_DATA_HOME/flatpak`, else `~/.local/share/flatpak`).
    pub fn from_system() -> FlatpakBackend {
        let config = std::env::var_os("FLATPAK_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from("/etc/flatpak"), PathBuf::from);
        FlatpakBackend {
            installations: installation_dirs(
                std::env::var_os("FLATPAK_SYSTEM_DIR"),
                std::env::var_os("FLATPAK_USER_DIR"),
                std::env::var_os("XDG_DATA_HOME"),
                std::env::var_os("HOME"),
                &config,
            ),
            arch: flatpak_arch(),
        }
    }

    pub fn with_installations(dirs: &[PathBuf], arch: &str) -> FlatpakBackend {
        FlatpakBackend {
            installations: dirs.to_vec(),
            arch: arch.to_string(),
        }
    }

    /// Every remote that has a catalog directory for this architecture.
    fn catalogs(&self) -> Vec<Catalog> {
        let mut found = Vec::new();
        for installation in &self.installations {
            let base = installation.join("appstream");
            let Ok(entries) = std::fs::read_dir(&base) else {
                continue;
            };
            let mut remotes: Vec<String> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            remotes.sort();
            for remote in remotes {
                let arch_dir = base.join(&remote).join(&self.arch);
                if !arch_dir.exists() {
                    continue; // this remote has nothing for our architecture
                }
                let active = arch_dir.join("active");
                let file = ["appstream.xml", "appstream.xml.gz"]
                    .iter()
                    .map(|name| active.join(name))
                    .find(|p| p.exists());
                found.push(Catalog {
                    remote,
                    file,
                    active,
                });
            }
        }
        found
    }

    /// Installed apps across every installation: id -> branch (`stable` preferred). An app is
    /// installed when `app/<id>/<arch>/<branch>/active` exists, whichever branch that is.
    fn installed_apps(&self) -> HashMap<String, String> {
        let mut installed = HashMap::new();
        for installation in &self.installations {
            let Ok(ids) = std::fs::read_dir(installation.join("app")) else {
                continue;
            };
            for id_entry in ids.flatten() {
                let Ok(id) = id_entry.file_name().into_string() else {
                    continue;
                };
                let Ok(branches) = std::fs::read_dir(id_entry.path().join(&self.arch)) else {
                    continue;
                };
                let mut branches: Vec<String> = branches
                    .flatten()
                    .filter(|b| b.path().join("active").symlink_metadata().is_ok())
                    .filter_map(|b| b.file_name().into_string().ok())
                    .collect();
                branches.sort();
                let chosen = branches
                    .iter()
                    .find(|b| b.as_str() == "stable")
                    .or(branches.first());
                if let Some(branch) = chosen {
                    installed.entry(id).or_insert_with(|| branch.clone());
                }
            }
        }
        installed
    }
}

/// Every installation Flatpak would use: the system one (`/var/lib/flatpak`, or
/// `FLATPAK_SYSTEM_DIR`), the user one (see [`user_installation_dir`], or `FLATPAK_USER_DIR`),
/// and the extra system-wide ones declared in `<config_dir>/installations.d/*.conf`
/// (`/etc/flatpak/installations.d`), each by its `Path=`.
pub fn installation_dirs(
    flatpak_system_dir: Option<OsString>,
    flatpak_user_dir: Option<OsString>,
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
    config_dir: &Path,
) -> Vec<PathBuf> {
    let mut dirs = vec![
        flatpak_system_dir
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from("/var/lib/flatpak"), PathBuf::from),
    ];
    dirs.extend(
        flatpak_user_dir
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| user_installation_dir(xdg_data_home, home)),
    );
    let mut confs: Vec<PathBuf> = std::fs::read_dir(config_dir.join("installations.d"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "conf"))
        .collect();
    confs.sort();
    for conf in confs {
        let Ok(text) = std::fs::read_to_string(&conf) else {
            continue;
        };
        for line in text.lines() {
            if let Some(path) = line.trim().strip_prefix("Path=") {
                let path = PathBuf::from(path.trim());
                if path.is_absolute() && !dirs.contains(&path) {
                    dirs.push(path);
                }
            }
        }
    }
    dirs
}

/// Where Flatpak keeps the per-user installation.
pub fn user_installation_dir(
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    match xdg_data_home.filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join("flatpak")),
        None => home
            .filter(|v| !v.is_empty())
            .map(|home| PathBuf::from(home).join(".local/share/flatpak")),
    }
}

struct Catalog {
    remote: String,
    file: Option<PathBuf>,
    active: PathBuf,
}

/// Flatpak spells some architectures differently from Rust.
fn flatpak_arch() -> String {
    match std::env::consts::ARCH {
        "x86" => "i386".to_string(),
        other => other.to_string(),
    }
}

impl Backend for FlatpakBackend {
    fn source(&self) -> SourceId {
        SourceId::new(SOURCE)
    }

    fn is_available(&self) -> bool {
        self.catalogs().iter().any(|c| c.file.is_some())
    }

    fn packages(&self) -> Result<Vec<Package>> {
        let mut collector = Collector::default();
        for catalog in self.catalogs() {
            let backend_error = |message: String| Error::Backend {
                source_id: SourceId::new(SOURCE),
                message: format!("remote {}: {message}", catalog.remote),
            };
            let Some(file) = &catalog.file else {
                return Err(backend_error(format!(
                    "no appstream.xml in {}",
                    catalog.active.display()
                )));
            };
            store_appstream::parse_file(file, &mut |c| {
                if let Some(entry) = finish(c, &catalog.remote, &catalog.active) {
                    collector.insert(entry);
                }
            })
            .map_err(backend_error)?;
        }
        let mut installed = self.installed_apps();
        let mut packages = Vec::with_capacity(collector.entries.len() + installed.len());
        for entry in collector.entries {
            let mut package = entry.package;
            if installed.remove(&package.name).is_some() {
                // The deployed version is unknown until update detection exists.
                package.installed_version = Some(package.version.clone());
            }
            packages.push(package);
        }
        // Installed apps no catalog lists (end-of-life, or from a remote without AppStream).
        let mut leftover: Vec<(String, String)> = installed.into_iter().collect();
        leftover.sort();
        for (id, branch) in leftover {
            packages.push(Package {
                source: SourceId::new(SOURCE),
                name: id,
                version: branch.clone(),
                installed_version: Some(branch),
                title: None,
                description: None,
                repo: None,
                url: None,
                download_size: 0,
                installed_size: 0,
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

struct Entry {
    package: Package,
    stable: bool,
}

/// One entry per app id across every catalog: the `stable` branch wins, else the first seen.
#[derive(Default)]
struct Collector {
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
}

impl Collector {
    fn insert(&mut self, entry: Entry) {
        match self.index.get(&entry.package.name) {
            Some(&i) => {
                if !self.entries[i].stable && entry.stable {
                    self.entries[i] = entry;
                }
            }
            None => {
                self.index
                    .insert(entry.package.name.clone(), self.entries.len());
                self.entries.push(entry);
            }
        }
    }
}

/// Builds a package from a finished `<component>`, or `None` if it is not an application.
fn finish(c: Component, remote: &str, active: &Path) -> Option<Entry> {
    // `desktop` is the legacy AppStream spelling of `desktop-application`; Flathub still
    // ships org.mozilla.firefox and 18 other apps with it.
    let is_app = matches!(
        c.kind.as_str(),
        "desktop-application" | "console-application" | "desktop"
    );
    if !is_app || c.id.is_empty() {
        return None;
    }
    // `app/<id>/<arch>/<branch>`
    let branch = c
        .bundle
        .as_deref()
        .and_then(|b| b.split('/').nth(3))
        .map(str::to_string);
    // The newest release wins; with equal or missing timestamps the first one listed does.
    let newest = c
        .releases
        .iter()
        .fold(None::<&(u64, String)>, |best, r| match best {
            Some(b) if b.0 >= r.0 => Some(b),
            _ => Some(r),
        })
        .map(|(_, version)| version.clone());
    let version = newest
        .or_else(|| branch.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let stable = branch.as_deref() == Some("stable");
    // The cached 128 px icon, when its file is really on disk.
    let icon = c
        .icons
        .iter()
        .find(|i| i.kind == "cached" && i.width == Some(128))
        .map(|i| active.join("icons/128x128").join(&i.value))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned());
    Some(Entry {
        package: Package {
            source: SourceId::new(SOURCE),
            app_id: Some(c.id.clone()),
            name: c.id,
            version,
            installed_version: None,
            title: c.name.filter(|n| !n.is_empty()),
            description: c.summary.filter(|s| !s.is_empty()),
            repo: Some(remote.to_string()),
            url: c.homepage,
            download_size: 0,
            installed_size: 0,
            categories: c.categories,
            icon,
            popularity: None,
            upgradable: false,
        },
        stable,
    })
}
