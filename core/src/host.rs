use crate::SourceId;

/// The system the store runs on: which source is the distribution's own package manager.
/// It decides the order sources are shown in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Host {
    native: Option<SourceId>,
}

impl Host {
    /// Reads `/etc/os-release`. Any failure gives [`Host::none`].
    pub fn detect() -> Host {
        match std::fs::read_to_string("/etc/os-release") {
            Ok(text) => Host::from_os_release(&text),
            Err(_) => Host::none(),
        }
    }

    /// The native source of the distribution `text` (an os-release file) describes, looking at
    /// `ID` and then every `ID_LIKE`. An unknown distribution has none.
    pub fn from_os_release(text: &str) -> Host {
        let mut ids: Vec<String> = Vec::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            match key.trim() {
                "ID" => ids.insert(0, value.to_lowercase()),
                "ID_LIKE" => ids.extend(value.split_whitespace().map(str::to_lowercase)),
                _ => {}
            }
        }
        let native = ids.iter().find_map(|id| match id.as_str() {
            "arch" => Some("pacman"),
            "debian" | "ubuntu" => Some("apt"),
            "fedora" | "rhel" => Some("dnf"),
            _ => None,
        });
        native.map_or_else(Host::none, Host::with_native)
    }

    pub fn with_native(source: &str) -> Host {
        Host {
            native: Some(SourceId::new(source)),
        }
    }

    pub fn none() -> Host {
        Host { native: None }
    }

    pub fn native(&self) -> Option<&SourceId> {
        self.native.as_ref()
    }

    /// Sort key: the native source first, then Flatpak, then `pacman`, `aur`, `snap` and
    /// `appimage`, then any other source by name.
    pub fn rank(&self, source: &SourceId) -> (u8, String) {
        let rank = if self.native.as_ref() == Some(source) {
            0
        } else {
            match source.as_str() {
                "flatpak" => 1,
                "pacman" => 2,
                "aur" => 3,
                "snap" => 4,
                "appimage" => 5,
                _ => 6,
            }
        };
        (rank, source.to_string())
    }

    pub fn sort(&self, sources: &mut [SourceId]) {
        sources.sort_by_key(|s| self.rank(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_of(text: &str) -> Option<String> {
        Host::from_os_release(text).native().map(|s| s.to_string())
    }

    fn ids(names: &[&str]) -> Vec<SourceId> {
        names.iter().map(|n| SourceId::new(*n)).collect()
    }

    fn names(sources: &[SourceId]) -> Vec<String> {
        sources.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arch_and_derivatives_map_to_pacman() {
        assert_eq!(
            native_of("ID=cachyos\nID_LIKE=arch\n").as_deref(),
            Some("pacman")
        );
        assert_eq!(native_of("ID=arch\n").as_deref(), Some("pacman"));
    }

    #[test]
    fn debian_family_maps_to_apt() {
        assert_eq!(
            native_of("ID=ubuntu\nID_LIKE=\"debian\"\n").as_deref(),
            Some("apt")
        );
        assert_eq!(native_of("ID=\"debian\"\n").as_deref(), Some("apt"));
    }

    #[test]
    fn fedora_family_maps_to_dnf() {
        assert_eq!(native_of("ID=fedora\n").as_deref(), Some("dnf"));
        assert_eq!(
            native_of("ID=nobara\nID_LIKE=\"rhel fedora\"\n").as_deref(),
            Some("dnf")
        );
    }

    #[test]
    fn unknown_or_empty_os_release_has_no_native() {
        assert_eq!(native_of(""), None);
        assert_eq!(native_of("this is not\n=an os-release\n\u{0}"), None);
        assert_eq!(native_of("ID=nixos\n"), None);
    }

    #[test]
    fn native_first_then_flatpak_then_fixed_list() {
        let host = Host::with_native("pacman");
        let mut sources = ids(&["aur", "flatpak", "snap", "pacman", "zzz", "appimage"]);
        host.sort(&mut sources);
        assert_eq!(
            names(&sources),
            ["pacman", "flatpak", "aur", "snap", "appimage", "zzz"]
        );
    }

    #[test]
    fn without_native_flatpak_comes_first() {
        let mut sources = ids(&["aur", "pacman", "flatpak"]);
        Host::none().sort(&mut sources);
        assert_eq!(names(&sources), ["flatpak", "pacman", "aur"]);
    }
}
