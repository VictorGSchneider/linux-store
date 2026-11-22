use std::collections::HashSet;
use std::path::Path;

use crate::{App, Host, Index, Package, Result, SourceId, assemble};

/// A landing-page row: some apps of one source and how many packages the source has.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub source: SourceId,
    pub apps: Vec<App>,
    pub total: usize,
}

/// Read API over the index for the UI. Every [`App`] it returns is complete: it carries the
/// variants from all sources, even when only one of them matched the query.
pub struct Catalog {
    index: Index,
    host: Host,
}

impl Catalog {
    pub fn new(index: Index, host: Host) -> Catalog {
        Catalog { index, host }
    }

    /// Opens its own connection to the index at `path`.
    pub fn open(path: &Path, host: Host) -> Result<Catalog> {
        Ok(Catalog::new(Index::open(path)?, host))
    }

    pub fn search(&self, query: &str, source: Option<&SourceId>, limit: usize) -> Result<Vec<App>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let found = self
            .index
            .search_in(query, limit.saturating_mul(3), source)?;
        let mut apps = self.complete(found)?;
        apps.truncate(limit);
        Ok(apps)
    }

    pub fn app(&self, key: &str) -> Result<Option<App>> {
        let members = self.index.members(key)?;
        let linked = members.into_iter().map(|p| (key.to_string(), p)).collect();
        Ok(assemble(linked, &self.host).pop())
    }

    /// Sources with packages and their package counts, in host order.
    pub fn chips(&self) -> Result<Vec<(SourceId, usize)>> {
        let mut counts = self.index.source_counts()?;
        counts.sort_by_key(|(source, _)| self.host.rank(source));
        Ok(counts)
    }

    pub fn home(&self, per_row: usize, seed: u64) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for (source, total) in self.chips()? {
            let featured = self.index.featured(&source, per_row, seed)?;
            let apps = self.complete(featured)?;
            if !apps.is_empty() {
                rows.push(Row {
                    source,
                    apps,
                    total,
                });
            }
        }
        Ok(rows)
    }

    pub fn installed(&self) -> Result<Vec<App>> {
        let packages = self.index.installed()?;
        self.complete_sorted(packages)
    }

    pub fn updates(&self) -> Result<Vec<App>> {
        let packages = self.index.upgradable()?;
        self.complete_sorted(packages)
    }

    fn complete_sorted(&self, packages: Vec<Package>) -> Result<Vec<App>> {
        let mut apps = self.complete(packages)?;
        apps.sort_by_cached_key(|a| (a.title.to_lowercase(), a.key.clone()));
        Ok(apps)
    }

    /// Turns found packages into complete apps, one per key, in first-seen order.
    fn complete(&self, packages: Vec<Package>) -> Result<Vec<App>> {
        let mut seen = HashSet::new();
        let mut linked = Vec::new();
        for key in self.index.keys_of(&packages)? {
            if seen.contains(&key) {
                continue;
            }
            for member in self.index.members(&key)? {
                linked.push((key.clone(), member));
            }
            seen.insert(key);
        }
        Ok(assemble(linked, &self.host))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::pkg;

    fn catalog(sets: &[(&str, Vec<Package>)]) -> Catalog {
        let mut index = Index::open_in_memory().unwrap();
        for (source, packages) in sets {
            index
                .replace_source(&SourceId::new(*source), packages)
                .unwrap();
        }
        Catalog::new(index, Host::with_native("pacman"))
    }

    fn sources(app: &App) -> Vec<&str> {
        app.variants.iter().map(|p| p.source.as_str()).collect()
    }

    fn firefox_both() -> Catalog {
        catalog(&[
            ("pacman", vec![pkg("pacman", "firefox", "Web browser")]),
            (
                "flatpak",
                vec![pkg("flatpak", "firefox", "Sandboxed thing")],
            ),
        ])
    }

    #[test]
    fn search_returns_one_app_with_variants_from_every_source() {
        let apps = firefox_both().search("browser", None, 10).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(sources(&apps[0]), ["pacman", "flatpak"]);
    }

    #[test]
    fn search_with_source_filter_still_returns_complete_apps() {
        let apps = firefox_both()
            .search("sandboxed", Some(&SourceId::new("flatpak")), 10)
            .unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(sources(&apps[0]), ["pacman", "flatpak"]);
    }

    #[test]
    fn search_of_punctuation_or_empty_returns_nothing() {
        let c = firefox_both();
        assert!(c.search("", None, 10).unwrap().is_empty());
        assert!(c.search("?!-", None, 10).unwrap().is_empty());
        assert!(c.search("firefox", None, 0).unwrap().is_empty());
    }

    #[test]
    fn search_cuts_to_limit() {
        let c = catalog(&[(
            "pacman",
            vec![
                pkg("pacman", "tool-a", "tool"),
                pkg("pacman", "tool-b", "tool"),
                pkg("pacman", "tool-c", "tool"),
            ],
        )]);
        assert_eq!(c.search("tool", None, 2).unwrap().len(), 2);
    }

    #[test]
    fn app_by_key_returns_none_for_unknown_key() {
        let c = firefox_both();
        assert!(c.app("nope").unwrap().is_none());
        let app = c.app("firefox").unwrap().unwrap();
        assert_eq!(sources(&app), ["pacman", "flatpak"]);
    }

    #[test]
    fn chips_follow_host_order_and_skip_empty_sources() {
        let c = catalog(&[
            ("aur", vec![pkg("aur", "x", "x")]),
            (
                "flatpak",
                vec![pkg("flatpak", "a", "a"), pkg("flatpak", "b", "b")],
            ),
            ("pacman", vec![pkg("pacman", "p", "p")]),
            ("snap", vec![]),
        ]);
        let chips: Vec<_> = c
            .chips()
            .unwrap()
            .into_iter()
            .map(|(s, n)| (s.to_string(), n))
            .collect();
        assert_eq!(
            chips,
            [
                ("pacman".to_string(), 1),
                ("flatpak".to_string(), 2),
                ("aur".to_string(), 1)
            ]
        );
    }

    #[test]
    fn home_rows_follow_host_order() {
        let c = catalog(&[
            ("aur", vec![pkg("aur", "x", "x")]),
            (
                "pacman",
                vec![pkg("pacman", "p", "p"), pkg("pacman", "q", "q")],
            ),
            ("flatpak", vec![]),
        ]);
        let rows = c.home(1, 7).unwrap();
        let order: Vec<_> = rows.iter().map(|r| r.source.as_str()).collect();
        assert_eq!(order, ["pacman", "aur"]);
        assert_eq!(rows[0].total, 2);
        assert_eq!(rows[0].apps.len(), 1);
    }

    fn installed_pkg(source: &str, name: &str, upgradable: bool) -> Package {
        let mut p = pkg(source, name, "d");
        p.installed_version = Some("1.0".into());
        p.upgradable = upgradable;
        p
    }

    #[test]
    fn installed_groups_an_app_installed_in_two_sources() {
        let c = catalog(&[
            (
                "pacman",
                vec![
                    installed_pkg("pacman", "firefox", false),
                    pkg("pacman", "zed", "d"),
                ],
            ),
            (
                "flatpak",
                vec![
                    installed_pkg("flatpak", "firefox", false),
                    installed_pkg("flatpak", "Alpha", false),
                ],
            ),
        ]);
        let apps = c.installed().unwrap();
        let keys: Vec<_> = apps.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, ["alpha", "firefox"]);
        assert_eq!(sources(&apps[1]), ["pacman", "flatpak"]);
    }

    #[test]
    fn updates_lists_only_upgradable_apps() {
        let c = catalog(&[
            (
                "pacman",
                vec![
                    installed_pkg("pacman", "old", true),
                    installed_pkg("pacman", "fresh", false),
                ],
            ),
            ("flatpak", vec![pkg("flatpak", "old", "d")]),
        ]);
        let apps = c.updates().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].key, "old");
        assert_eq!(sources(&apps[0]), ["pacman", "flatpak"]);
    }
}
