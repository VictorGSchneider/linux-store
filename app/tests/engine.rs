use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use store_app::{AppView, Command, Engine, EngineConfig, Event, OpState, RefreshState, coalesce};
use store_core::testing::{FakeBackend, FakeInstaller, pkg};
use store_core::{Backend, CancelToken, Host, InstallError, Installer, Package, SourceId};
use store_protocol::{Change, ChangeKind, ErrorCode, Event as HelperEvent, Request, Stage};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Harness {
    engine: Engine,
    events: Receiver<Event>,
    _dir: tempfile::TempDir,
}

fn source(name: &str) -> SourceId {
    SourceId::new(name)
}

fn boxed(b: FakeBackend) -> Box<dyn Backend> {
    Box::new(b)
}

fn start_in(
    dir: tempfile::TempDir,
    backends: Vec<Box<dyn Backend>>,
    installer: Arc<dyn Installer>,
) -> Harness {
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let config = EngineConfig {
        // A missing parent directory is created by the engine.
        db_path: dir.path().join("cache").join("index.db"),
        backends,
        installer,
        installable: vec![source("pacman")],
        host: Host::with_native("pacman"),
    };
    let engine = Engine::start(config, move |event| {
        let _ = tx.lock().unwrap().send(event);
    })
    .unwrap();
    Harness {
        engine,
        events,
        _dir: dir,
    }
}

fn start(backends: Vec<Box<dyn Backend>>, installer: Arc<dyn Installer>) -> Harness {
    start_in(tempfile::tempdir().unwrap(), backends, installer)
}

fn idle() -> Arc<dyn Installer> {
    Arc::new(FakeInstaller::new(vec![], Ok(())))
}

fn with_app_id(source: &str, name: &str, app_id: &str) -> Package {
    Package {
        app_id: Some(app_id.into()),
        ..pkg(source, name, "Web browser")
    }
}

/// `firefox` in pacman and flatpak, both linked to the key `org.mozilla.firefox`.
fn firefox_backends() -> Vec<Box<dyn Backend>> {
    vec![
        boxed(FakeBackend::new(
            "pacman",
            vec![
                with_app_id("pacman", "firefox", "org.mozilla.firefox"),
                pkg("pacman", "tree", "Lists directories"),
            ],
        )),
        boxed(FakeBackend::new(
            "flatpak",
            vec![with_app_id(
                "flatpak",
                "org.mozilla.firefox",
                "org.mozilla.firefox",
            )],
        )),
    ]
}

const FIREFOX: &str = "org.mozilla.firefox";

impl Harness {
    fn next(&self) -> Event {
        self.events
            .recv_timeout(TIMEOUT)
            .expect("no event from the engine within 5 s")
    }

    /// Skips events until one matches, returning it.
    fn wait_for(&self, wanted: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = self
                .events
                .recv_timeout(left)
                .expect("the expected event did not arrive within 5 s");
            if wanted(&event) {
                return event;
            }
        }
    }

    /// Every event up to and including the next `RefreshDone`.
    fn until_refresh_done(&self) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            let event = self.next();
            let done = event == Event::RefreshDone;
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    fn refreshed(&self) {
        self.engine.send(Command::Refresh);
        self.until_refresh_done();
    }

    fn finished(&self) -> Result<String, String> {
        match self.wait_for(|e| matches!(e, Event::Operation(OpState::Finished(_)))) {
            Event::Operation(OpState::Finished(result)) => result,
            _ => unreachable!(),
        }
    }

    fn install(&self, key: &str, from: &str, package: &str) {
        self.engine.send(Command::Install {
            key: key.into(),
            source: source(from),
            package: package.into(),
        });
    }
}

// --- coalesce ---

fn search(generation: u64, query: &str) -> Command {
    Command::Search {
        generation,
        query: query.into(),
        source: None,
    }
}

#[test]
fn coalesce_keeps_only_the_last_search() {
    let out = coalesce(vec![search(1, "f"), search(2, "fi"), search(3, "fir")]);
    assert_eq!(out, [search(3, "fir")]);
}

#[test]
fn coalesce_keeps_other_commands_in_order() {
    let out = coalesce(vec![
        Command::Chips,
        search(1, "f"),
        Command::Installed,
        search(2, "fi"),
        Command::Updates,
        Command::Open { key: "k".into() },
    ]);
    assert_eq!(
        out,
        [
            Command::Chips,
            Command::Installed,
            search(2, "fi"),
            Command::Updates,
            Command::Open { key: "k".into() },
        ]
    );
    assert_eq!(coalesce(vec![]), []);
    assert_eq!(coalesce(vec![Command::Chips]), [Command::Chips]);
}

// --- refresh and chips ---

#[test]
fn chips_event_lists_sources_after_refresh() {
    let h = start(firefox_backends(), idle());
    h.engine.send(Command::Refresh);
    let events = h.until_refresh_done();
    let chips = events.iter().find_map(|e| match e {
        Event::Chips(chips) => Some(chips.clone()),
        _ => None,
    });
    assert_eq!(
        chips,
        Some(vec![(source("pacman"), 2), (source("flatpak"), 1)])
    );
    // Asking again gives the same counts.
    h.engine.send(Command::Chips);
    assert_eq!(
        h.next(),
        Event::Chips(vec![(source("pacman"), 2), (source("flatpak"), 1)])
    );
}

#[test]
fn refresh_emits_per_source_state_then_done() {
    let h = start(
        vec![
            boxed(FakeBackend::new("a", vec![pkg("a", "one", "x")])),
            boxed(FakeBackend::new("b", vec![]).unavailable_because("not installed")),
            boxed(FakeBackend::new("c", vec![]).failing("boom")),
        ],
        idle(),
    );
    h.engine.send(Command::Refresh);
    let events = h.until_refresh_done();
    for (name, last) in [
        ("a", RefreshState::Updated(1)),
        ("b", RefreshState::Unavailable(Some("not installed".into()))),
        ("c", RefreshState::Failed("boom".into())),
    ] {
        let mine: Vec<&RefreshState> = events
            .iter()
            .filter_map(|e| match e {
                Event::Refresh { source: s, state } if s.as_str() == name => Some(state),
                _ => None,
            })
            .collect();
        assert_eq!(mine, [&RefreshState::Started, &last], "source {name}");
    }
    let n = events.len();
    assert_eq!(events[n - 1], Event::RefreshDone);
    assert_eq!(events[n - 2], Event::Chips(vec![(source("a"), 1)]));
}

#[test]
fn failing_source_does_not_remove_other_chips() {
    let first = start_in(
        tempfile::tempdir().unwrap(),
        vec![
            boxed(FakeBackend::new("a", vec![pkg("a", "one", "x")])),
            boxed(FakeBackend::new("b", vec![pkg("b", "two", "x")])),
        ],
        idle(),
    );
    first.refreshed();
    let Harness { engine, _dir, .. } = first;
    drop(engine);

    // Same index, now `a` fails: both sources keep their chips.
    let second = start_in(
        _dir,
        vec![
            boxed(FakeBackend::new("a", vec![]).failing("network down")),
            boxed(FakeBackend::new("b", vec![pkg("b", "two", "x")])),
        ],
        idle(),
    );
    second.engine.send(Command::Refresh);
    let events = second.until_refresh_done();
    assert!(events.contains(&Event::Refresh {
        source: source("a"),
        state: RefreshState::Failed("network down".into())
    }));
    assert!(events.contains(&Event::Chips(vec![(source("a"), 1), (source("b"), 1)])));
}

// --- queries ---

#[test]
fn search_result_carries_its_generation() {
    let h = start(firefox_backends(), idle());
    h.refreshed();
    h.engine.send(search(7, "browser"));
    match h.next() {
        Event::SearchResults { generation, apps } => {
            assert_eq!(generation, 7);
            assert_eq!(apps.len(), 1);
            assert_eq!(apps[0].key, FIREFOX);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn search_can_be_limited_to_one_source() {
    let h = start(firefox_backends(), idle());
    h.refreshed();
    h.engine.send(Command::Search {
        generation: 1,
        query: "lists".into(),
        source: Some(source("flatpak")),
    });
    assert_eq!(
        h.next(),
        Event::SearchResults {
            generation: 1,
            apps: vec![]
        }
    );
}

#[test]
fn open_unknown_key_gives_none() {
    let h = start(firefox_backends(), idle());
    h.refreshed();
    h.engine.send(Command::Open {
        key: "no-such-app".into(),
    });
    assert_eq!(h.next(), Event::AppPage(None));
}

#[test]
fn open_lists_installable_sources() {
    let h = start(firefox_backends(), idle());
    h.refreshed();
    h.engine.send(Command::Open {
        key: FIREFOX.into(),
    });
    match h.next() {
        Event::AppPage(Some(AppView { app, installable })) => {
            assert_eq!(app.key, FIREFOX);
            assert_eq!(app.variants.len(), 2);
            assert_eq!(installable, [source("pacman")]);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn home_installed_and_updates_answer_from_the_index() {
    let installed = Package {
        installed_version: Some("0.9".into()),
        upgradable: true,
        ..pkg("pacman", "old", "x")
    };
    let h = start(
        vec![boxed(FakeBackend::new(
            "pacman",
            vec![installed, pkg("pacman", "fresh", "x")],
        ))],
        idle(),
    );
    h.refreshed();
    h.engine.send(Command::Home {
        per_row: 5,
        seed: 1,
    });
    match h.next() {
        Event::Home(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].total, 2);
        }
        other => panic!("unexpected: {other:?}"),
    }
    h.engine.send(Command::Installed);
    h.engine.send(Command::Updates);
    for _ in 0..2 {
        match h.next() {
            Event::Installed(apps) | Event::Updates(apps) => {
                let keys: Vec<_> = apps.iter().map(|a| a.key.as_str()).collect();
                assert_eq!(keys, ["old"]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}

// --- operations ---

#[test]
fn install_unsupported_source_never_calls_the_installer() {
    let fake = Arc::new(FakeInstaller::new(vec![], Ok(())));
    let h = start(firefox_backends(), fake.clone());
    h.refreshed();
    h.install(FIREFOX, "flatpak", FIREFOX);
    assert!(matches!(
        h.next(),
        Event::Operation(OpState::Finished(Err(_)))
    ));
    // Nothing ran, so the operation is settled at once, without a refresh.
    assert_eq!(h.next(), Event::OperationSettled);
    // A source that is installable but has no variant of the app, and an unknown key.
    h.engine.send(Command::Remove {
        key: "no-such-app".into(),
        source: source("pacman"),
        package: "no-such-app".into(),
    });
    assert!(matches!(
        h.next(),
        Event::Operation(OpState::Finished(Err(_)))
    ));
    assert_eq!(h.next(), Event::OperationSettled);
    assert!(fake.requests().is_empty());
    // Nothing ran, so nothing is refreshed: had the first operation asked for a refresh of
    // flatpak, it would be queued before this full refresh and end first.
    h.engine.send(Command::Refresh);
    let events = h.until_refresh_done();
    assert!(events.contains(&Event::Refresh {
        source: source("pacman"),
        state: RefreshState::Started
    }));
}

#[test]
fn install_sends_the_variant_name_to_the_installer() {
    let fake = Arc::new(FakeInstaller::new(
        vec![
            HelperEvent::Progress {
                stage: Stage::Download,
                package: Some("firefox".into()),
                percent: Some(42),
            },
            HelperEvent::Done {
                changes: vec![Change {
                    package: "firefox".into(),
                    kind: ChangeKind::Installed,
                    version: Some("1.0".into()),
                }],
            },
        ],
        Ok(()),
    ));
    let h = start(firefox_backends(), fake.clone());
    h.refreshed();
    h.install(FIREFOX, "pacman", "firefox");
    assert!(matches!(h.next(), Event::Operation(OpState::Started(_))));
    match h.next() {
        Event::Operation(OpState::Progress { message, percent }) => {
            assert!(message.contains("firefox"), "{message}");
            assert!(message.to_lowercase().contains("download"), "{message}");
            assert_eq!(percent, Some(42));
        }
        other => panic!("unexpected: {other:?}"),
    }
    match h.next() {
        Event::Operation(OpState::Finished(Ok(summary))) => {
            assert!(summary.contains("firefox"), "{summary}")
        }
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(
        fake.requests(),
        [Request::Install {
            packages: vec!["firefox".into()]
        }]
    );
    // Then only the affected source is refreshed.
    let events = h.until_refresh_done();
    let refreshed: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Refresh { source, .. } => Some(source.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(refreshed, ["pacman", "pacman"]);
    // The settle follows the post-operation refresh of that source.
    let n = events.len();
    let pos = events
        .iter()
        .position(|e| *e == Event::OperationSettled)
        .expect("settled");
    let updated = events
        .iter()
        .position(|e| {
            matches!(e, Event::Refresh { source, state: RefreshState::Updated(_) }
                if source.as_str() == "pacman")
        })
        .unwrap();
    assert!(pos > updated && pos < n, "{events:?}");
}

// --- settling after an operation ---

/// The packages a source reports, changed by `MarkingInstaller` like a real install changes the
/// system. `packages()` reads them when it starts; a held call waits for `release` after that.
#[derive(Clone)]
struct System {
    packages: Arc<Mutex<Vec<Package>>>,
}

impl System {
    fn new(packages: Vec<Package>) -> System {
        System {
            packages: Arc::new(Mutex::new(packages)),
        }
    }
}

struct SystemBackend {
    name: &'static str,
    system: System,
    /// `packages()` calls to hold, each until a message arrives (or 5 s pass).
    hold: Arc<Mutex<usize>>,
    release: Mutex<Option<Receiver<()>>>,
    started: Mutex<Option<Sender<()>>>,
}

impl SystemBackend {
    fn new(name: &'static str, system: &System) -> SystemBackend {
        SystemBackend {
            name,
            system: system.clone(),
            hold: Arc::new(Mutex::new(0)),
            release: Mutex::new(None),
            started: Mutex::new(None),
        }
    }

    /// Holds the next `n` calls; returns the backend, a receiver signalled at each call, the
    /// release sender and the hold counter (to hold more calls later).
    #[allow(clippy::type_complexity)]
    fn holding(self, n: usize) -> (SystemBackend, Receiver<()>, Sender<()>, Arc<Mutex<usize>>) {
        let (started_tx, started) = channel();
        let (release, release_rx) = channel();
        *self.hold.lock().unwrap() = n;
        *self.release.lock().unwrap() = Some(release_rx);
        *self.started.lock().unwrap() = Some(started_tx);
        let hold = self.hold.clone();
        (self, started, release, hold)
    }
}

impl Backend for SystemBackend {
    fn source(&self) -> SourceId {
        source(self.name)
    }
    fn is_available(&self) -> bool {
        true
    }
    fn unavailable_reason(&self) -> Option<String> {
        None
    }
    fn packages(&self) -> store_core::Result<Vec<Package>> {
        let snapshot = self.system.packages.lock().unwrap().clone();
        let held = {
            let mut hold = self.hold.lock().unwrap();
            let held = *hold > 0;
            *hold = hold.saturating_sub(1);
            held
        };
        if let Some(started) = self.started.lock().unwrap().as_ref() {
            let _ = started.send(());
        }
        if held && let Some(release) = self.release.lock().unwrap().as_ref() {
            let _ = release.recv_timeout(TIMEOUT);
        }
        Ok(snapshot)
    }
}

/// Marks the requested package installed in `system`, then succeeds.
struct MarkingInstaller {
    system: System,
}

impl Installer for MarkingInstaller {
    fn run(
        &self,
        request: &Request,
        _cancel: &CancelToken,
        _on_event: &mut dyn FnMut(HelperEvent),
    ) -> Result<(), InstallError> {
        if let Request::Install { packages } = request {
            for p in self.system.packages.lock().unwrap().iter_mut() {
                if packages.contains(&p.name) {
                    p.installed_version = Some(p.version.clone());
                }
            }
        }
        Ok(())
    }
}

impl Harness {
    fn open_installed(&self, key: &str, from: &str) -> bool {
        self.engine.send(Command::Open { key: key.into() });
        match self.wait_for(|e| matches!(e, Event::AppPage(_))) {
            Event::AppPage(Some(view)) => view.app.variant(&source(from)).unwrap().is_installed(),
            other => panic!("unexpected: {other:?}"),
        }
    }
}

#[test]
fn an_operation_is_settled_once_its_source_shows_the_change() {
    let system = System::new(vec![pkg("pacman", "tree", "Lists directories")]);
    let h = start(
        vec![Box::new(SystemBackend::new("pacman", &system))],
        Arc::new(MarkingInstaller {
            system: system.clone(),
        }),
    );
    h.refreshed();
    assert!(!h.open_installed("tree", "pacman"));
    h.install("tree", "pacman", "tree");
    assert!(h.finished().is_ok());
    let seen = {
        let mut seen = Vec::new();
        loop {
            let e = h.next();
            let done = e == Event::OperationSettled;
            seen.push(e);
            if done {
                break seen;
            }
        }
    };
    assert!(
        seen.contains(&Event::Refresh {
            source: source("pacman"),
            state: RefreshState::Updated(1)
        }),
        "{seen:?}"
    );
    // Anything opened after the settle shows the new state.
    assert!(h.open_installed("tree", "pacman"));
}

#[test]
fn the_post_operation_refresh_does_not_wait_for_a_slow_full_refresh() {
    let system = System::new(vec![pkg("pacman", "tree", "Lists directories")]);
    let (slow, slow_started, release_slow, _) =
        SystemBackend::new("aur", &System::new(vec![pkg("aur", "yay", "x")])).holding(1);
    let h = start(
        vec![
            Box::new(SystemBackend::new("pacman", &system)),
            Box::new(slow),
        ],
        Arc::new(MarkingInstaller {
            system: system.clone(),
        }),
    );
    h.engine.send(Command::Refresh);
    slow_started.recv_timeout(TIMEOUT).unwrap();
    // The fast source of the full pass is written; the slow one is still downloading.
    h.wait_for(|e| {
        matches!(e, Event::Refresh { source, state: RefreshState::Updated(_) }
            if source.as_str() == "pacman")
    });
    h.install("tree", "pacman", "tree");
    assert!(h.finished().is_ok());
    h.wait_for(|e| *e == Event::OperationSettled);
    assert!(h.open_installed("tree", "pacman"));
    // The full pass is still running: it ends only now.
    release_slow.send(()).unwrap();
    let rest = h.until_refresh_done();
    assert!(
        rest.contains(&Event::Refresh {
            source: source("aur"),
            state: RefreshState::Updated(1)
        }),
        "{rest:?}"
    );
}

#[test]
fn a_collection_that_started_before_the_operation_does_not_settle_it() {
    let system = System::new(vec![pkg("pacman", "tree", "Lists directories")]);
    let (pacman, started, release, hold) = SystemBackend::new("pacman", &system).holding(0);
    let h = start(
        vec![Box::new(pacman)],
        Arc::new(MarkingInstaller {
            system: system.clone(),
        }),
    );
    h.refreshed();
    started.recv_timeout(TIMEOUT).unwrap();
    // A full refresh whose collection reads the system, then is held while the install runs.
    *hold.lock().unwrap() = 1;
    h.engine.send(Command::Refresh);
    started.recv_timeout(TIMEOUT).unwrap();
    h.install("tree", "pacman", "tree");
    assert!(h.finished().is_ok());
    // The held collection read the old state: it must not settle the operation.
    let deadline = Instant::now() + Duration::from_millis(300);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match h.events.recv_timeout(left) {
            Ok(e) => assert_ne!(e, Event::OperationSettled, "settled by a stale collection"),
            Err(_) => break,
        }
    }
    release.send(()).unwrap();
    let mut seen = Vec::new();
    loop {
        let e = h.next();
        let done = e == Event::OperationSettled;
        seen.push(e);
        if done {
            break;
        }
    }
    let pacman: Vec<&RefreshState> = seen
        .iter()
        .filter_map(|e| match e {
            Event::Refresh { source, state } if source.as_str() == "pacman" => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        pacman,
        [
            &RefreshState::Updated(1),
            &RefreshState::Started,
            &RefreshState::Updated(1)
        ],
        "{seen:?}"
    );
    assert!(h.open_installed("tree", "pacman"));
}

#[test]
fn a_package_that_changed_since_the_page_was_opened_is_refused() {
    let fake = Arc::new(FakeInstaller::new(vec![], Ok(())));
    let h = start(firefox_backends(), fake.clone());
    h.refreshed();
    // The page showed another package name for this source than the index now holds.
    h.install(FIREFOX, "pacman", "firefox-esr");
    assert_eq!(
        h.finished(),
        Err("the package changed since the page was opened; reopen the app page".to_string())
    );
    assert_eq!(h.next(), Event::OperationSettled);
    assert!(fake.requests().is_empty());
}

#[test]
fn remove_sends_a_remove_request() {
    let fake = Arc::new(FakeInstaller::new(vec![], Ok(())));
    let h = start(firefox_backends(), fake.clone());
    h.refreshed();
    h.engine.send(Command::Remove {
        key: "tree".into(),
        source: source("pacman"),
        package: "tree".into(),
    });
    assert!(h.finished().is_ok());
    assert_eq!(
        fake.requests(),
        [Request::Remove {
            packages: vec!["tree".into()]
        }]
    );
}

#[test]
fn dismissed_authentication_finishes_idle_with_message() {
    let h = start(
        firefox_backends(),
        Arc::new(FakeInstaller::new(vec![], Err(InstallError::Dismissed))),
    );
    h.refreshed();
    h.install(FIREFOX, "pacman", "firefox");
    assert_eq!(h.finished(), Err("authentication cancelled".to_string()));
    let events = h.until_refresh_done();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Operation(OpState::Finished(_)))),
        "exactly one Finished"
    );
}

#[test]
fn rejected_request_finishes_with_the_helper_message() {
    let h = start(
        firefox_backends(),
        Arc::new(FakeInstaller::new(
            vec![],
            Err(InstallError::Rejected {
                code: ErrorCode::Protected,
                message: "refusing to touch a protected package".into(),
            }),
        )),
    );
    h.refreshed();
    h.install(FIREFOX, "pacman", "firefox");
    assert_eq!(
        h.finished(),
        Err("refusing to touch a protected package".to_string())
    );
}

#[test]
fn an_error_event_finishes_with_its_message() {
    let h = start(
        firefox_backends(),
        Arc::new(FakeInstaller::new(
            vec![HelperEvent::Error {
                code: ErrorCode::Failed,
                message: "disk full".into(),
            }],
            Ok(()),
        )),
    );
    h.refreshed();
    h.install(FIREFOX, "pacman", "firefox");
    assert_eq!(h.finished(), Err("disk full".to_string()));
}

/// Signals when it starts, then waits until its token is cancelled (at most 5 s).
struct BlockingInstaller {
    started: Mutex<Sender<()>>,
    saw_cancel: AtomicBool,
    runs: Mutex<usize>,
}

impl BlockingInstaller {
    fn new() -> (Arc<BlockingInstaller>, Receiver<()>) {
        let (tx, rx) = channel();
        let installer = Arc::new(BlockingInstaller {
            started: Mutex::new(tx),
            saw_cancel: AtomicBool::new(false),
            runs: Mutex::new(0),
        });
        (installer, rx)
    }
}

impl Installer for BlockingInstaller {
    fn run(
        &self,
        _request: &Request,
        cancel: &CancelToken,
        _on_event: &mut dyn FnMut(HelperEvent),
    ) -> Result<(), InstallError> {
        *self.runs.lock().unwrap() += 1;
        let _ = self.started.lock().unwrap().send(());
        let deadline = Instant::now() + TIMEOUT;
        while !cancel.is_cancelled() {
            if Instant::now() > deadline {
                return Err(InstallError::Protocol("never cancelled".into()));
            }
            std::thread::yield_now();
            std::thread::sleep(Duration::from_millis(1));
        }
        self.saw_cancel.store(true, Ordering::SeqCst);
        Err(InstallError::Rejected {
            code: ErrorCode::Cancelled,
            message: "cancelled by the user".into(),
        })
    }
}

#[test]
fn cancel_during_an_operation_sets_the_token() {
    let (installer, started) = BlockingInstaller::new();
    let h = start(firefox_backends(), installer.clone());
    h.refreshed();
    // A cancel with nothing running is ignored and does not affect the next operation.
    h.engine.send(Command::CancelOperation);
    h.install(FIREFOX, "pacman", "firefox");
    started.recv_timeout(TIMEOUT).unwrap();
    h.engine.send(Command::CancelOperation);
    assert_eq!(h.finished(), Err("cancelled".to_string()));
    assert!(installer.saw_cancel.load(Ordering::SeqCst));
}

/// Drops `engine` on another thread and fails if that takes more than 5 s.
fn drop_within_timeout(engine: Engine) {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        drop(engine);
        let _ = tx.send(());
    });
    rx.recv_timeout(TIMEOUT)
        .expect("dropping the engine did not return within 5 s");
}

#[test]
fn dropping_the_engine_joins_its_threads() {
    let h = start(firefox_backends(), idle());
    h.refreshed();
    let Harness { engine, events, .. } = h;
    drop_within_timeout(engine);
    // Every thread is gone, so every clone of the callback (and its sender) was dropped.
    assert!(events.recv_timeout(TIMEOUT).is_err());

    let (installer, started) = BlockingInstaller::new();
    let h = start(firefox_backends(), installer.clone());
    h.refreshed();
    h.install(FIREFOX, "pacman", "firefox");
    // A second operation waits in the queue; shutting down must not start it.
    h.install(FIREFOX, "pacman", "firefox");
    started.recv_timeout(TIMEOUT).unwrap();
    let Harness { engine, events, .. } = h;
    drop_within_timeout(engine);
    assert!(installer.saw_cancel.load(Ordering::SeqCst));
    assert_eq!(*installer.runs.lock().unwrap(), 1);
    let finished = events
        .try_iter()
        .filter(|e| matches!(e, Event::Operation(OpState::Finished(_))))
        .count();
    assert_eq!(finished, 1, "the running operation still finishes once");
}

#[test]
fn dropping_the_engine_with_a_slow_callback_does_not_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let config = EngineConfig {
        db_path: dir.path().join("index.db"),
        backends: firefox_backends(),
        installer: idle(),
        installable: vec![source("pacman")],
        host: Host::none(),
    };
    let engine = Engine::start(config, |_| std::thread::sleep(Duration::from_millis(20))).unwrap();
    engine.send(Command::Refresh);
    for _ in 0..20 {
        engine.send(Command::Chips);
        engine.send(Command::LoadIcon {
            path: "/nonexistent.png".into(),
            side: 64,
        });
    }
    drop_within_timeout(engine);
}

struct PanickingInstaller;

impl Installer for PanickingInstaller {
    fn run(
        &self,
        _request: &Request,
        _cancel: &CancelToken,
        _on_event: &mut dyn FnMut(HelperEvent),
    ) -> Result<(), InstallError> {
        panic!("installer bug");
    }
}

#[test]
fn an_installer_panic_still_finishes_the_operation() {
    let h = start(firefox_backends(), Arc::new(PanickingInstaller));
    h.refreshed();
    h.install(FIREFOX, "pacman", "firefox");
    assert!(h.finished().is_err());
    // The operations thread survived and takes the next operation.
    h.install(FIREFOX, "pacman", "firefox");
    assert!(h.finished().is_err());
}

/// A backend whose `packages()` signals that it started, then blocks until released.
struct BlockingBackend {
    started: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
}

impl Backend for BlockingBackend {
    fn source(&self) -> SourceId {
        source("slow")
    }
    fn is_available(&self) -> bool {
        true
    }
    fn unavailable_reason(&self) -> Option<String> {
        None
    }
    fn packages(&self) -> store_core::Result<Vec<Package>> {
        let _ = self.started.lock().unwrap().send(());
        // Released by the test, or by the test ending (the sender is dropped).
        let _ = self.release.lock().unwrap().recv();
        Ok(vec![])
    }
}

#[test]
fn dropping_the_engine_does_not_wait_for_a_slow_refresh() {
    let (started_tx, started) = channel();
    let (release, release_rx) = channel();
    let backend = BlockingBackend {
        started: Mutex::new(started_tx),
        release: Mutex::new(release_rx),
    };
    let h = start(vec![Box::new(backend)], idle());
    h.engine.send(Command::Refresh);
    started.recv_timeout(TIMEOUT).unwrap();
    let Harness { engine, .. } = h;
    let begin = Instant::now();
    drop_within_timeout(engine);
    assert!(
        begin.elapsed() < Duration::from_secs(3),
        "drop took {:?}",
        begin.elapsed()
    );
    // Only now does the backend return; the detached refresh thread then ends on its own.
    let _ = release.send(());
}

// --- icons ---

#[test]
fn icon_event_is_none_for_a_missing_file() {
    let h = start(vec![], idle());
    h.engine.send(Command::LoadIcon {
        path: "/nonexistent/icon.png".into(),
        side: 64,
    });
    assert_eq!(
        h.next(),
        Event::Icon {
            path: "/nonexistent/icon.png".into(),
            image: None
        }
    );
}
