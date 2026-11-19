use crate::{Backend, Index, Package, Result, SourceId};

/// What happened to one source during [`Store::refresh`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Updated { packages: usize },
    /// The backend cannot be used on this system; its indexed data is untouched.
    Unavailable,
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
                Outcome::Unavailable
            } else {
                match backend
                    .packages()
                    .and_then(|packages| self.index.replace_source(&source, &packages).map(|()| packages.len()))
                {
                    Ok(packages) => Outcome::Updated { packages },
                    Err(e) => Outcome::Failed(e.to_string()),
                }
            };
            sources.push(SourceReport { source, outcome });
        }
        RefreshReport { sources }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Package>> {
        self.index.search(query, limit)
    }

    pub fn installed(&self) -> Result<Vec<Package>> {
        self.index.installed()
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
        Package { installed_version: Some("1.0".into()), ..pkg(source, name, "x") }
    }

    #[test]
    fn refresh_indexes_available_backends() {
        let fake = FakeBackend::new("fake", vec![pkg("fake", "alpha", "first"), pkg("fake", "beta", "second")]);
        let mut store = Store::new(Index::open_in_memory().unwrap(), vec![boxed(fake)]);
        let report = store.refresh();
        assert_eq!(
            report.sources,
            [SourceReport { source: SourceId::new("fake"), outcome: Outcome::Updated { packages: 2 } }]
        );
        assert_eq!(store.search("alpha", 10).unwrap().len(), 1);
    }

    #[test]
    fn failing_backend_does_not_block_others() {
        let bad = FakeBackend::new("bad", vec![]).failing("boom");
        let good = FakeBackend::new("good", vec![pkg("good", "tool", "x")]);
        let mut store = Store::new(Index::open_in_memory().unwrap(), vec![boxed(bad), boxed(good)]);
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
        assert_eq!(report.sources[0].outcome, Outcome::Unavailable);
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
        let fake = FakeBackend::new("fake", vec![installed_pkg("fake", "yes"), pkg("fake", "no", "x")]);
        let mut store = Store::new(Index::open_in_memory().unwrap(), vec![boxed(fake)]);
        store.refresh();
        let names: Vec<String> = store.installed().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["yes"]);
    }
}
