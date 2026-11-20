//! Read-only AUR backend. It reads the official metadata dump, one HTTP request for the whole
//! AUR, and streams it into the index without ever holding the whole file in memory.
use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::Deserialize;
use serde::de::{Deserializer as _, SeqAccess, Visitor};
use store_core::{Backend, Error, Package, Result, SourceId};

pub const DEFAULT_DUMP_URL: &str = "https://aur.archlinux.org/packages-meta-ext-v1.json.gz";

const SOURCE: &str = "aur";
const USER_AGENT: &str = concat!("linux-store/", env!("CARGO_PKG_VERSION"));
/// A real dump is about 15 MiB compressed and 100 MiB expanded; these only stop runaway input.
const MAX_COMPRESSED_BYTES: u64 = 200 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;

/// Where the dump comes from.
pub enum DumpSource {
    Url(String),
    File(PathBuf),
}

pub struct AurBackend {
    source: DumpSource,
}

impl AurBackend {
    pub fn new() -> AurBackend {
        AurBackend::with_source(DumpSource::Url(DEFAULT_DUMP_URL.to_string()))
    }

    pub fn with_source(source: DumpSource) -> AurBackend {
        AurBackend { source }
    }

    fn open(&self) -> std::result::Result<Box<dyn Read>, String> {
        match &self.source {
            DumpSource::File(path) => {
                let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(Box::new(file.take(MAX_COMPRESSED_BYTES)))
            }
            DumpSource::Url(url) => {
                let config = ureq::Agent::config_builder()
                    .timeout_connect(Some(Duration::from_secs(10)))
                    .timeout_global(Some(Duration::from_secs(120)))
                    .user_agent(USER_AGENT)
                    .build();
                let response = ureq::Agent::new_with_config(config)
                    .get(url)
                    .call()
                    .map_err(|e| format!("download failed: {e}"))?;
                let reader = response
                    .into_body()
                    .into_with_config()
                    .limit(MAX_COMPRESSED_BYTES)
                    .reader();
                Ok(Box::new(reader))
            }
        }
    }
}

impl Default for AurBackend {
    fn default() -> Self {
        AurBackend::new()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Entry {
    name: String,
    version: String,
    description: Option<String>,
    #[serde(rename = "URL")]
    url: Option<String>,
}

/// Turns each array element into a `Package` as it is parsed, so memory stays flat.
struct Collect {
    packages: Vec<Package>,
    seen: HashSet<String>,
}

impl<'de> Visitor<'de> for &mut Collect {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON array of AUR packages")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        while let Some(entry) = seq.next_element::<Entry>()? {
            // The first entry wins when a name repeats, so one bad dump cannot fail the index.
            if !self.seen.insert(entry.name.clone()) {
                continue;
            }
            self.packages.push(Package {
                source: SourceId::new(SOURCE),
                name: entry.name,
                version: entry.version,
                installed_version: None,
                title: None,
                description: entry.description,
                repo: Some(SOURCE.to_string()),
                url: entry.url,
                download_size: 0,
                installed_size: 0,
            });
        }
        Ok(())
    }
}

impl Backend for AurBackend {
    fn source(&self) -> SourceId {
        SourceId::new(SOURCE)
    }

    fn is_available(&self) -> bool {
        match &self.source {
            DumpSource::Url(_) => true,
            DumpSource::File(path) => path.exists(),
        }
    }

    fn packages(&self) -> Result<Vec<Package>> {
        let backend_error = |message: String| Error::Backend {
            source_id: SourceId::new(SOURCE),
            message,
        };
        let raw = self.open().map_err(backend_error)?;
        let expanded = GzDecoder::new(BufReader::new(raw)).take(MAX_EXPANDED_BYTES);
        let mut json = serde_json::Deserializer::from_reader(BufReader::new(expanded));
        let mut collect = Collect {
            packages: Vec::new(),
            seen: HashSet::new(),
        };
        json.deserialize_seq(&mut collect)
            .map_err(|e| backend_error(format!("invalid dump: {e}")))?;
        // An empty list is never a real AUR; refusing it keeps a bad download from wiping the index.
        if collect.packages.is_empty() {
            return Err(backend_error("the dump contains no packages".to_string()));
        }
        Ok(collect.packages)
    }
}
