//! Read-only Flatpak backend. It reads the AppStream catalog that Flatpak already keeps on
//! disk for every remote, streaming the XML so the 48 MiB Flathub file is never held whole.
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use store_core::{Backend, Error, Package, Result, SourceId};

const SOURCE: &str = "flatpak";

pub struct FlatpakBackend {
    installations: Vec<PathBuf>,
    arch: String,
}

impl FlatpakBackend {
    /// The system installation (`/var/lib/flatpak`) and the user one (`~/.local/share/flatpak`).
    pub fn from_system() -> FlatpakBackend {
        let mut installations = vec![PathBuf::from("/var/lib/flatpak")];
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            installations.push(PathBuf::from(home).join(".local/share/flatpak"));
        }
        FlatpakBackend {
            installations,
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

    fn is_installed(&self, id: &str, branch: &str) -> bool {
        self.installations.iter().any(|installation| {
            installation
                .join("app")
                .join(id)
                .join(&self.arch)
                .join(branch)
                .join("active")
                .symlink_metadata()
                .is_ok()
        })
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
            parse_catalog(file, &catalog.remote, &mut collector).map_err(backend_error)?;
        }
        let mut packages = Vec::with_capacity(collector.entries.len());
        for entry in collector.entries {
            let mut package = entry.package;
            if let Some(branch) = &entry.branch
                && self.is_installed(&package.name, branch)
            {
                // The deployed version is unknown until update detection exists.
                package.installed_version = Some(package.version.clone());
            }
            packages.push(package);
        }
        Ok(packages)
    }
}

struct Entry {
    package: Package,
    branch: Option<String>,
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

#[derive(Default)]
struct Component {
    kind: String,
    id: String,
    name: Option<String>,
    summary: Option<String>,
    homepage: Option<String>,
    bundle: Option<String>,
    /// (timestamp, version), in document order.
    releases: Vec<(u64, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Id,
    Name,
    Summary,
    Homepage,
    Bundle,
}

fn attribute(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn open(path: &Path) -> std::result::Result<Box<dyn Read>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.extension().is_some_and(|ext| ext == "gz") {
        Ok(Box::new(GzDecoder::new(file)))
    } else {
        Ok(Box::new(file))
    }
}

fn parse_catalog(
    path: &Path,
    remote: &str,
    collector: &mut Collector,
) -> std::result::Result<(), String> {
    let mut xml = Reader::from_reader(BufReader::new(open(path)?));
    let mut buf = Vec::new();
    let mut path_stack: Vec<Vec<u8>> = Vec::new();
    let mut component: Option<Component> = None;
    let mut field: Option<Field> = None;
    let mut text = String::new();

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|e| format!("invalid XML near byte {}: {e}", xml.buffer_position()))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(event, Event::Empty(_));
                let name = e.name().as_ref().to_vec();
                let depth = path_stack.len() + 1;
                if depth == 1 && name != b"components" {
                    return Err("not an AppStream catalog".to_string());
                }
                if depth == 2 && name == b"component" {
                    component = Some(Component {
                        kind: attribute(e, b"type").unwrap_or_default(),
                        ..Component::default()
                    });
                } else if depth == 3 && component.is_some() && !is_empty {
                    let tagged = attribute(e, b"xml:lang").is_some();
                    field = match name.as_slice() {
                        b"id" => Some(Field::Id),
                        b"name" if !tagged => Some(Field::Name),
                        b"summary" if !tagged => Some(Field::Summary),
                        b"url" if attribute(e, b"type").as_deref() == Some("homepage") => {
                            Some(Field::Homepage)
                        }
                        b"bundle" if attribute(e, b"type").as_deref() != Some("appdata") => {
                            Some(Field::Bundle)
                        }
                        _ => None,
                    };
                    text.clear();
                } else if depth == 4
                    && name == b"release"
                    && path_stack.get(2).is_some_and(|p| p == b"releases")
                    && let Some(c) = component.as_mut()
                    && let Some(version) = attribute(e, b"version")
                {
                    let timestamp = attribute(e, b"timestamp")
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(0);
                    c.releases.push((timestamp, version));
                }
                if !is_empty {
                    path_stack.push(name);
                }
            }
            Event::Text(ref t) => {
                if field.is_some() && path_stack.len() == 3 {
                    text.push_str(&t.decode().map_err(|e| e.to_string())?);
                }
            }
            Event::GeneralRef(ref r) => {
                if field.is_some() && path_stack.len() == 3 {
                    if let Some(c) = r.resolve_char_ref().map_err(|e| e.to_string())? {
                        text.push(c);
                    } else {
                        match r.decode().map_err(|e| e.to_string())?.as_ref() {
                            "amp" => text.push('&'),
                            "lt" => text.push('<'),
                            "gt" => text.push('>'),
                            "quot" => text.push('"'),
                            "apos" => text.push('\''),
                            _ => {}
                        }
                    }
                }
            }
            Event::End(ref e) => {
                let depth = path_stack.len();
                if depth == 3
                    && let (Some(f), Some(c)) = (field.take(), component.as_mut())
                {
                    let value = text.trim().to_string();
                    match f {
                        Field::Id => c.id = value,
                        Field::Name => c.name = Some(value),
                        Field::Summary => c.summary = Some(value),
                        Field::Homepage => c.homepage = Some(value),
                        Field::Bundle => c.bundle = Some(value),
                    }
                } else if depth == 2
                    && e.name().as_ref() == b"component"
                    && let Some(c) = component.take()
                    && let Some(entry) = finish(c, remote)
                {
                    collector.insert(entry);
                }
                path_stack.pop();
            }
            Event::Eof => {
                if !path_stack.is_empty() {
                    return Err("unexpected end of file".to_string());
                }
                return Ok(());
            }
            _ => {}
        }
        buf.clear();
    }
}

/// Builds a package from a finished `<component>`, or `None` if it is not an application.
fn finish(c: Component, remote: &str) -> Option<Entry> {
    if (c.kind != "desktop-application" && c.kind != "console-application") || c.id.is_empty() {
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
    Some(Entry {
        package: Package {
            source: SourceId::new(SOURCE),
            name: c.id,
            version,
            installed_version: None,
            title: c.name.filter(|n| !n.is_empty()),
            description: c.summary.filter(|s| !s.is_empty()),
            repo: Some(remote.to_string()),
            url: c.homepage,
            download_size: 0,
            installed_size: 0,
        },
        branch,
        stable,
    })
}
