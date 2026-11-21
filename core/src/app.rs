use std::collections::HashMap;

use crate::{Host, Package, SourceId};

/// The same application seen in one or more sources.
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub key: String,
    pub title: String,
    /// One package per source, in [`Host::sort`] order.
    pub variants: Vec<Package>,
}

impl App {
    pub fn icon(&self) -> Option<&str> {
        self.variants.iter().find_map(|p| p.icon.as_deref())
    }

    pub fn description(&self) -> Option<&str> {
        self.variants.iter().find_map(|p| p.description.as_deref())
    }

    pub fn is_installed(&self) -> bool {
        self.variants.iter().any(Package::is_installed)
    }

    pub fn is_upgradable(&self) -> bool {
        self.variants.iter().any(|p| p.upgradable)
    }

    pub fn variant(&self, source: &SourceId) -> Option<&Package> {
        self.variants.iter().find(|p| &p.source == source)
    }
}

/// Groups `(app_key, package)` pairs by key. Apps come out in the order of each key's first
/// package (keeping the search ranking); variants are sorted by `host`.
pub fn assemble(linked: Vec<(String, Package)>, host: &Host) -> Vec<App> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut apps: Vec<App> = Vec::new();
    for (key, package) in linked {
        match index.get(&key) {
            Some(&i) => apps[i].variants.push(package),
            None => {
                index.insert(key.clone(), apps.len());
                apps.push(App {
                    key,
                    title: String::new(),
                    variants: vec![package],
                });
            }
        }
    }
    for app in &mut apps {
        app.variants.sort_by_key(|p| host.rank(&p.source));
        app.title = app
            .variants
            .iter()
            .find_map(|p| p.title.clone())
            .unwrap_or_else(|| app.variants[0].name.clone());
    }
    apps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::pkg;

    fn linked(key: &str, p: Package) -> (String, Package) {
        (key.to_string(), p)
    }

    fn host() -> Host {
        Host::with_native("pacman")
    }

    #[test]
    fn same_key_joins_variants() {
        let apps = assemble(
            vec![
                linked("firefox", pkg("flatpak", "org.mozilla.firefox", "Browser")),
                linked("firefox", pkg("pacman", "firefox", "Browser")),
            ],
            &host(),
        );
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].key, "firefox");
        assert_eq!(apps[0].variants.len(), 2);
    }

    #[test]
    fn different_keys_stay_separate() {
        let apps = assemble(
            vec![
                linked("a", pkg("pacman", "a", "A")),
                linked("b", pkg("pacman", "b", "B")),
            ],
            &host(),
        );
        assert_eq!(apps.len(), 2);
    }

    #[test]
    fn apps_follow_input_order() {
        let apps = assemble(
            vec![
                linked("b", pkg("pacman", "b", "B")),
                linked("a", pkg("pacman", "a", "A")),
                linked("b", pkg("flatpak", "b", "B")),
            ],
            &host(),
        );
        let keys: Vec<_> = apps.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, ["b", "a"]);
    }

    #[test]
    fn variants_follow_host_order() {
        let input = vec![
            linked("x", pkg("flatpak", "x", "X")),
            linked("x", pkg("aur", "x", "X")),
            linked("x", pkg("pacman", "x", "X")),
        ];
        let apps = assemble(input.clone(), &host());
        let sources: Vec<_> = apps[0].variants.iter().map(|p| p.source.as_str()).collect();
        assert_eq!(sources, ["pacman", "flatpak", "aur"]);

        let apps = assemble(input, &Host::none());
        let sources: Vec<_> = apps[0].variants.iter().map(|p| p.source.as_str()).collect();
        assert_eq!(sources, ["flatpak", "pacman", "aur"]);
    }

    #[test]
    fn title_prefers_a_variant_with_title() {
        let mut titled = pkg("flatpak", "org.mozilla.firefox", "Browser");
        titled.title = Some("Firefox".into());
        let apps = assemble(
            vec![
                linked("firefox", pkg("pacman", "firefox", "Browser")),
                linked("firefox", titled),
            ],
            &host(),
        );
        assert_eq!(apps[0].title, "Firefox");

        let apps = assemble(
            vec![
                linked("k", pkg("flatpak", "zeta", "Z")),
                linked("k", pkg("pacman", "alpha", "A")),
            ],
            &host(),
        );
        assert_eq!(apps[0].title, "alpha");
    }

    #[test]
    fn is_installed_and_upgradable_look_at_any_variant() {
        let mut installed = pkg("flatpak", "x", "X");
        installed.installed_version = Some("1.0".into());
        installed.upgradable = true;
        let apps = assemble(
            vec![
                linked("x", pkg("pacman", "x", "X")),
                linked("x", installed),
                linked("y", pkg("pacman", "y", "Y")),
            ],
            &host(),
        );
        assert!(apps[0].is_installed());
        assert!(apps[0].is_upgradable());
        assert!(!apps[1].is_installed());
        assert!(!apps[1].is_upgradable());
    }

    #[test]
    fn icon_description_and_variant_come_from_variants() {
        let mut flat = pkg("flatpak", "x", "From flatpak");
        flat.icon = Some("/icons/x.png".into());
        let mut native = pkg("pacman", "x", "From pacman");
        native.description = None;
        let apps = assemble(vec![linked("x", flat), linked("x", native)], &host());
        let app = &apps[0];
        assert_eq!(app.icon(), Some("/icons/x.png"));
        assert_eq!(app.description(), Some("From flatpak"));
        assert_eq!(
            app.variant(&SourceId::new("pacman"))
                .map(|p| p.name.as_str()),
            Some("x")
        );
        assert!(app.variant(&SourceId::new("aur")).is_none());
    }
}
