use std::fmt;

/// Identifies a package source, e.g. `pacman`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(id: impl Into<String>) -> SourceId {
        SourceId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub source: SourceId,
    pub name: String,
    /// Newest version the source knows (the installed version for installed-only packages).
    pub version: String,
    pub installed_version: Option<String>,
    /// Human display name when the source's `name` is an id (e.g. Flatpak `org.mozilla.firefox`).
    pub title: Option<String>,
    pub description: Option<String>,
    pub repo: Option<String>,
    pub url: Option<String>,
    pub download_size: u64,
    pub installed_size: u64,
    /// AppStream component id (e.g. `org.mozilla.firefox`), when the source knows one.
    pub app_id: Option<String>,
    /// AppStream categories as written, e.g. `Network`.
    pub categories: Vec<String>,
    /// Absolute path of a local icon file.
    pub icon: Option<String>,
    pub popularity: Option<f64>,
    /// An installed package that has a newer version available.
    pub upgradable: bool,
}

impl Package {
    pub fn is_installed(&self) -> bool {
        self.installed_version.is_some()
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::pkg;

    #[test]
    fn package_is_installed_follows_installed_version() {
        let mut p = pkg("fake", "a", "x");
        assert!(!p.is_installed());
        p.installed_version = Some("1.0".into());
        assert!(p.is_installed());
    }
}
