use std::collections::HashSet;

use crate::{Backend, Error, Host, Index, Package, Result, SourceId};

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

/// Progress of one source during [`Store::refresh_with`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshStage {
    Started,
    Finished(Outcome),
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
    host: Host,
}

impl Store {
    pub fn new(index: Index, backends: Vec<Box<dyn Backend>>) -> Store {
        Store {
            index,
            backends,
            host: Host::none(),
        }
    }

    /// Orders the sources of [`Store::search_by_source`] for this host.
    pub fn with_host(self, host: Host) -> Store {
        Store { host, ..self }
    }

    /// Reloads the index from every available backend. One source's outcome never affects
    /// another's, and a source that is unavailable or fails keeps its previous data.
    pub fn refresh(&mut self) -> RefreshReport {
        self.refresh_with(None, &|_, _| {})
    }

    /// Like [`Store::refresh`], but limited to the sources in `only` (when given; names that match
    /// no backend are ignored) and reporting progress. The slow `Backend::packages()` calls run
    /// in parallel, one thread per source. Each source is written to the index and reported as
    /// finished as soon as its own collection is done, whatever the order of the backends; the
    /// writes happen one at a time on the calling thread. `RefreshReport.sources` is in backend
    /// order. `progress` is only ever called on the calling thread, and it
    /// must not panic.
    pub fn refresh_with(
        &mut self,
        only: Option<&[SourceId]>,
        progress: &(dyn Fn(&SourceId, &RefreshStage) + Sync),
    ) -> RefreshReport {
        let Store {
            index, backends, ..
        } = self;
        let in_scope: Vec<(SourceId, &dyn Backend)> = backends
            .iter()
            .map(|b| (b.source(), b.as_ref()))
            .filter(|(source, _)| only.is_none_or(|only| only.contains(source)))
            .collect();
        for (source, _) in &in_scope {
            progress(source, &RefreshStage::Started);
        }
        let mut outcomes: Vec<Option<Outcome>> = vec![None; in_scope.len()];
        let mut pending = 0;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            for (i, (source, backend)) in in_scope.iter().enumerate() {
                if backend.is_available() {
                    pending += 1;
                    let tx = tx.clone();
                    scope.spawn(move || {
                        let collected =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                backend.packages().map(first_of_each_name)
                            }));
                        // The receiver outlives every sender, so this cannot fail.
                        let _ = tx.send((i, collected));
                    });
                } else {
                    let outcome = Outcome::Unavailable {
                        reason: backend.unavailable_reason(),
                    };
                    progress(source, &RefreshStage::Finished(outcome.clone()));
                    outcomes[i] = Some(outcome);
                }
            }
            drop(tx);
            for _ in 0..pending {
                let Ok((i, collected)) = rx.recv() else { break };
                let source = &in_scope[i].0;
                let outcome = match collected {
                    Err(_) => Outcome::Failed("backend panicked".to_string()),
                    Ok(collected) => match collected.and_then(|packages| {
                        index
                            .replace_source(source, &packages)
                            .map(|()| packages.len())
                    }) {
                        Ok(packages) => Outcome::Updated { packages },
                        Err(e) => Outcome::Failed(failure_message(&e)),
                    },
                };
                progress(source, &RefreshStage::Finished(outcome.clone()));
                outcomes[i] = Some(outcome);
            }
        });
        RefreshReport {
            sources: in_scope
                .into_iter()
                .zip(outcomes)
                .map(|((source, _), outcome)| SourceReport {
                    source,
                    outcome: outcome.expect("every source in scope gets an outcome"),
                })
                .collect(),
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Package>> {
        self.index.search(query, limit)
    }

    /// The best `limit` matches of every source that has any, grouped by source in the host's
    /// order (see [`Host::rank`]).
    pub fn search_by_source(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(SourceId, Vec<Package>)>> {
        let mut sources = self.index.sources()?;
        self.host.sort(&mut sources);
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
    fn search_by_source_groups_in_host_order_and_limits_each_source() {
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
        )
        .with_host(Host::with_native("pacman"));
        store.refresh();
        let groups = store.search_by_source("tool", 1).unwrap();
        let order: Vec<String> = groups.iter().map(|(s, _)| s.to_string()).collect();
        assert_eq!(order, ["pacman", "flatpak", "aur", "zzz"]);
        assert!(groups.iter().all(|(_, found)| found.len() == 1));
    }

    /// A backend whose `packages()` waits on a shared barrier (so it only returns when its peer
    /// runs at the same time) or panics.
    struct TestBackend {
        source: SourceId,
        barrier: Option<std::sync::Arc<std::sync::Barrier>>,
        panics: bool,
        gate: Option<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
    }

    impl Backend for TestBackend {
        fn source(&self) -> SourceId {
            self.source.clone()
        }
        fn is_available(&self) -> bool {
            true
        }
        fn unavailable_reason(&self) -> Option<String> {
            None
        }
        fn packages(&self) -> Result<Vec<Package>> {
            if self.panics {
                panic!("test backend panic");
            }
            if let Some(gate) = &self.gate {
                gate.lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("a later source never finished while this one was still running");
            }
            if let Some(barrier) = &self.barrier {
                // A sequential refresh would block here forever; fail loudly instead.
                let (tx, rx) = std::sync::mpsc::channel();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let _ = tx.send(());
                });
                rx.recv_timeout(std::time::Duration::from_secs(5))
                    .expect("the sources did not run in parallel (barrier timed out)");
            }
            let name = format!("{}-pkg", self.source);
            Ok(vec![pkg(&self.source.to_string(), &name, "x")])
        }
    }

    #[test]
    fn progress_reports_started_then_finished_per_source() {
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![
                boxed(FakeBackend::new("a", vec![pkg("a", "one", "x")])),
                boxed(FakeBackend::new("b", vec![]).unavailable()),
                boxed(FakeBackend::new("c", vec![]).failing("boom")),
            ],
        );
        let events = std::sync::Mutex::new(Vec::new());
        let report = store.refresh_with(None, &|source, stage| {
            events
                .lock()
                .unwrap()
                .push((source.to_string(), stage.clone()));
        });
        let events = events.into_inner().unwrap();
        for (name, outcome) in [
            ("a", Outcome::Updated { packages: 1 }),
            ("b", Outcome::Unavailable { reason: None }),
            ("c", Outcome::Failed("boom".into())),
        ] {
            let mine: Vec<_> = events.iter().filter(|(s, _)| s == name).collect();
            assert_eq!(
                mine,
                [
                    &(name.to_string(), RefreshStage::Started),
                    &(name.to_string(), RefreshStage::Finished(outcome))
                ]
            );
        }
        assert_eq!(report.sources.len(), 3);
    }

    #[test]
    fn refresh_only_touches_the_named_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let mut store = Store::new(
            Index::open(&path).unwrap(),
            vec![
                boxed(FakeBackend::new("a", vec![pkg("a", "old-a", "x")])),
                boxed(FakeBackend::new("b", vec![pkg("b", "old-b", "x")])),
            ],
        );
        store.refresh();
        let mut store = Store::new(
            Index::open(&path).unwrap(),
            vec![
                boxed(FakeBackend::new("a", vec![pkg("a", "new-a", "x")])),
                boxed(FakeBackend::new("b", vec![pkg("b", "new-b", "x")])),
            ],
        );
        let only = [SourceId::new("a"), SourceId::new("nonexistent")];
        let report = store.refresh_with(Some(&only), &|_, _| {});
        assert_eq!(
            report.sources,
            [SourceReport {
                source: SourceId::new("a"),
                outcome: Outcome::Updated { packages: 1 }
            }]
        );
        assert_eq!(store.search("new-a", 10).unwrap().len(), 1);
        assert_eq!(store.search("old-a", 10).unwrap().len(), 0);
        assert_eq!(store.search("old-b", 10).unwrap().len(), 1);
        assert_eq!(store.search("new-b", 10).unwrap().len(), 0);
    }

    #[test]
    fn one_failing_source_does_not_stop_the_others() {
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![
                boxed(FakeBackend::new("a", vec![pkg("a", "tool-a", "x")])),
                boxed(FakeBackend::new("b", vec![]).failing("boom")),
                boxed(FakeBackend::new("c", vec![pkg("c", "tool-c", "x")])),
            ],
        );
        let report = store.refresh();
        let outcomes: Vec<_> = report.sources.iter().map(|r| r.outcome.clone()).collect();
        assert_eq!(
            outcomes,
            [
                Outcome::Updated { packages: 1 },
                Outcome::Failed("boom".into()),
                Outcome::Updated { packages: 1 }
            ]
        );
        assert_eq!(store.search("tool", 10).unwrap().len(), 2);
    }

    #[test]
    fn sources_really_run_in_parallel() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let backend = |name: &str| -> Box<dyn Backend> {
            Box::new(TestBackend {
                source: SourceId::new(name),
                barrier: Some(barrier.clone()),
                panics: false,
                gate: None,
            })
        };
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![backend("a"), backend("b")],
        );
        let report = store.refresh();
        assert!(
            report
                .sources
                .iter()
                .all(|r| r.outcome == Outcome::Updated { packages: 1 }),
            "{report:?}"
        );
    }

    #[test]
    fn a_source_that_is_ready_is_reported_and_written_before_a_slower_earlier_one() {
        let (release, gate) = std::sync::mpsc::channel();
        let mut store = Store::new(
            Index::open_in_memory().unwrap(),
            vec![
                Box::new(TestBackend {
                    source: SourceId::new("slow"),
                    barrier: None,
                    panics: false,
                    gate: Some(std::sync::Mutex::new(gate)),
                }),
                boxed(FakeBackend::new("fast", vec![pkg("fast", "quick", "x")])),
                boxed(FakeBackend::new("off", vec![]).unavailable()),
            ],
        );
        let finished = std::sync::Mutex::new(Vec::new());
        let report = store.refresh_with(None, &|source, stage| {
            if let RefreshStage::Finished(_) = stage {
                finished.lock().unwrap().push(source.to_string());
                // The slow source may only complete once "fast" has been reported.
                if source.to_string() == "fast" {
                    release.send(()).unwrap();
                }
            }
        });
        assert_eq!(finished.into_inner().unwrap(), ["off", "fast", "slow"]);
        let names: Vec<_> = report
            .sources
            .iter()
            .map(|r| r.source.to_string())
            .collect();
        assert_eq!(names, ["slow", "fast", "off"]);
        assert_eq!(report.sources[0].outcome, Outcome::Updated { packages: 1 });
    }

    #[test]
    fn a_panicking_backend_is_reported_as_failed_and_others_still_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        Store::new(
            Index::open(&path).unwrap(),
            vec![boxed(FakeBackend::new("p", vec![pkg("p", "keep", "x")]))],
        )
        .refresh();
        let mut store = Store::new(
            Index::open(&path).unwrap(),
            vec![
                boxed(FakeBackend::new("a", vec![pkg("a", "tool-a", "x")])),
                Box::new(TestBackend {
                    source: SourceId::new("p"),
                    barrier: None,
                    panics: true,
                    gate: None,
                }),
                boxed(FakeBackend::new("c", vec![pkg("c", "tool-c", "x")])),
            ],
        );
        let events = std::sync::Mutex::new(Vec::new());
        let report = store.refresh_with(None, &|source, stage| {
            events
                .lock()
                .unwrap()
                .push((source.to_string(), stage.clone()));
        });
        let outcomes: Vec<_> = report.sources.iter().map(|r| r.outcome.clone()).collect();
        assert_eq!(
            outcomes,
            [
                Outcome::Updated { packages: 1 },
                Outcome::Failed("backend panicked".into()),
                Outcome::Updated { packages: 1 }
            ]
        );
        assert!(events.into_inner().unwrap().contains(&(
            "p".to_string(),
            RefreshStage::Finished(Outcome::Failed("backend panicked".into()))
        )));
        assert_eq!(store.search("tool", 10).unwrap().len(), 2);
        assert_eq!(store.search("keep", 10).unwrap().len(), 1);
    }
}
