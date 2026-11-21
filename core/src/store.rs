use std::collections::HashSet;

use crate::{Backend, Error, Index, Package, Result, SourceId};

/// What happened to one source during [`Store::refresh`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Updated {
        packages: usize,
    },
    /// The backend cannot be used on this system; its indexed data is untouched.
    Unavailable {
        reason: Option<String>,
    },
    /// The backend or the index failed; the source's indexed data is untouched.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    pub source: SourceId,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    pub sources: Vec<SourceReport>,
}

/// The local index plus the backends that feed it. Searches are answered from the index only.
pub struct Store {
    index: Index,
    backends: Vec<Box<dyn Backend>>,
}

impl Store {
    pub fn new(index: Index, backends: Vec<Box<dyn Backend>>) -> Store {
        Store { index, backends }
    }

    /// Reloads the index from every available backend. One source's outcome never affects
    /// another's, and a source that is unavailable or fails keeps its previous data.
    pub fn refresh(&mut self) -> RefreshReport {
        let mut sources = Vec::with_capacity(self.backends.len());
        for backend in &self.backends {
            let source = backend.source();
            let outcome = if !backend.is_available() {
                Outcome::Unavailable {
                    reason: backend.unavailable_reason(),
                }
            } else {
                match backend
                    .packages()
                    .map(first_of_each_name)
                    .and_then(|packages| {
                        self.index
                            .replace_source(&source, &packages)
                            .map(|()| packages.len())
                    }) {
                    Ok(packages) => Outcome::Updated { packages },
                    Err(e) => Outcome::Failed(failure_message(&e)),
                }
            };
            sources.push(SourceReport { source, outcome });
        }
        RefreshReport { sources }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Package>> {
        self.index.search(query, limit)
    }

    /// The best `limit` matches of every source that has any, grouped by source: pacman first,
    /// then Flatpak, then the AUR (least trusted last), then any other source by name.
    pub fn search_by_source(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(SourceId, Vec<Package>)>> {
        let mut sources = self.index.sources()?;
        let rank = |s: &SourceId| match s.to_string().as_str() {
            "pacman" => 0,
            "flatpak" => 1,
            "aur" => 2,
            _ => 3,
        };
        sources.sort_by_key(|s| (rank(s), s.to_string()));
        let mut groups = Vec::new();
        for source in sources {
            let found = self.index.search_in(query, limit, Some(&source))?;
            if !found.is_empty() {
                groups.push((source, found));
            }
        }
        Ok(groups)
    }

    pub fn installed(&self) -> Result<Vec<Package>> {
        self.index.installed()
    }
}

/// A repeated name would break the index's uniqueness and fail the whole source, so the first
/// entry wins, like pacman's first repository does.
fn first_of_each_name(packages: Vec<Package>) -> Vec<Package> {
    let mut seen = HashSet::new();
    packages
        .into_iter()
        .filter(|p| seen.insert(p.name.clone()))
        .collect()
}

/// The message for a failed source. A backend error already says which source it came from in
/// `Display`, and the report names the source itself, so only the message is kept.
fn failure_message(error: &Error) -> String {
    match error {
        Error::Backend { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeBackend, pkg};
    use crate::{Backend, Package, SourceId};

    fn boxed(b: FakeBackend) -> Box<dyn Backend> {
        Box::new(b)
    }

    fn installed_pkg(source: &str, name: &str) -> Package {
        Package {
            installed_version: Some("1.0".into()),
            ..pkg(source, name, "x")
        }
    }

    #[test]
    fn refresh_indexes_available_backends() {
        let fake = FakeBackend::new(
            "fake",
            vec![pkg("fake", "alpha", "first"), pkg("fake", "beta", "second")],
        );
        let mut store = Store::new(Index::open_in_memory().unwrap(), vec![boxed(fake)]);
        let report = store.refresh();
        assert_eq!(
            report.sources,
            [SourceReport {
                source: SourceId::new("fake"),
                outcome: Outcome::Updated { packages: 2 }
            }]
        );
        assert_eq!(store.search("alpha", 10).unwrap().len(), 1);
    }

    #[test]
    fn failing_backend_does_not_block_others() {
        let bad = FakeBackend::new("bad", vec![]).failing("boom");
        let good = FakeBackend::new("good", vec![pkg("good", "tool", "x")]);
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![boxed(bad), boxed(good)],
        );
        let report = store.refresh();
        assert!(matches!(report.sources[0].outcome, Outcome::Failed(_)));
        assert_eq!(report.sources[1].outcome, Outcome::Updated { packages: 1 });
        assert_eq!(store.search("tool", 10).unwrap().len(), 1);
    }

    fn seeded_index(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("index.db");
        let mut store = Store::new(
            Index::open(&path).unwrap(),
            vec![boxed(FakeBackend::new("s", vec![pkg("s", "keep", "x")]))],
        );
        store.refresh();
        path
    }

    #[test]
    fn unavailable_backend_keeps_old_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = seeded_index(&dir);
        let mut store = Store::new(
            Index::open(&path).unwrap(),
            vec![boxed(FakeBackend::new("s", vec![]).unavailable())],
        );
        let report = store.refresh();
        assert_eq!(
            report.sources[0].outcome,
            Outcome::Unavailable { reason: None }
        );
        assert_eq!(store.search("keep", 10).unwrap().len(), 1);
    }

    #[test]
    fn failed_refresh_keeps_old_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = seeded_index(&dir);
        let mut store = Store::new(
            Index::open(&path).unwrap(),
            vec![boxed(FakeBackend::new("s", vec![]).failing("net down"))],
        );
        let report = store.refresh();
        match &report.sources[0].outcome {
            Outcome::Failed(message) => assert!(message.contains("net down"), "{message}"),
            other => panic!("unexpected: {other:?}"),
        }
        assert_eq!(store.search("keep", 10).unwrap().len(), 1);
    }

    #[test]
    fn installed_comes_from_the_index() {
        let fake = FakeBackend::new(
            "fake",
            vec![installed_pkg("fake", "yes"), pkg("fake", "no", "x")],
        );
        let mut store = Store::new(Index::open_in_memory().unwrap(), vec![boxed(fake)]);
        store.refresh();
        let names: Vec<String> = store
            .installed()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["yes"]);
    }

    #[test]
    fn a_failed_source_reports_its_message_once() {
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![boxed(FakeBackend::new("s", vec![]).failing("boom"))],
        );
        let report = store.refresh();
        assert_eq!(
            report.sources[0].outcome,
            Outcome::Failed("boom".to_string())
        );
    }

    #[test]
    fn an_unavailable_source_reports_its_reason() {
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![
                boxed(FakeBackend::new("a", vec![]).unavailable_because("no catalog found")),
                boxed(FakeBackend::new("b", vec![]).unavailable()),
            ],
        );
        let report = store.refresh();
        assert_eq!(
            report.sources[0].outcome,
            Outcome::Unavailable {
                reason: Some("no catalog found".to_string())
            }
        );
        assert_eq!(
            report.sources[1].outcome,
            Outcome::Unavailable { reason: None }
        );
    }

    #[test]
    fn duplicate_names_from_a_backend_do_not_fail_the_source() {
        // The first of a repeated name wins; one bad entry must not stop the whole source.
        let first = Package {
            version: "1.0".into(),
            ..pkg("s", "dup", "first")
        };
        let second = Package {
            version: "2.0".into(),
            ..pkg("s", "dup", "second")
        };
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![boxed(FakeBackend::new(
                "s",
                vec![first, second, pkg("s", "other", "x")],
            ))],
        );
        let report = store.refresh();
        assert_eq!(report.sources[0].outcome, Outcome::Updated { packages: 2 });
        let found = store.search("dup", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version, "1.0");
    }

    #[test]
    fn search_by_source_groups_in_a_fixed_order_and_limits_each_source() {
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![
                boxed(FakeBackend::new(
                    "aur",
                    vec![pkg("aur", "tool-a", "x"), pkg("aur", "tool-b", "x")],
                )),
                boxed(FakeBackend::new("zzz", vec![pkg("zzz", "tool-z", "x")])),
                boxed(FakeBackend::new(
                    "flatpak",
                    vec![pkg("flatpak", "tool-f", "x")],
                )),
                boxed(FakeBackend::new(
                    "pacman",
                    vec![pkg("pacman", "tool-p", "x")],
                )),
                boxed(FakeBackend::new("empty", vec![pkg("empty", "other", "y")])),
            ],
        );
        store.refresh();
        let groups = store.search_by_source("tool", 1).unwrap();
        let order: Vec<String> = groups.iter().map(|(s, _)| s.to_string()).collect();
        assert_eq!(order, ["pacman", "flatpak", "aur", "zzz"]);
        assert!(groups.iter().all(|(_, found)| found.len() == 1));
    }
}
