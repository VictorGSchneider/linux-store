//! The engine behind the GUI: it owns the index and the installer and does every slow thing off
//! the UI thread. The UI sends [`Command`]s and gets [`Event`]s back through a callback.
//!
//! Threads, each fed by its own channel (`Engine::send` routes, never blocks):
//! - query: a `Catalog` connection; answers `Chips`, `Home`, `Search`, `Open`, `Installed`,
//!   `Updates`, coalescing searches that queued up behind one another;
//! - refresh: the `Store` (the only writer) and a `Catalog` for the chip counts afterwards;
//! - operations: one install or remove at a time, in order; it asks the refresh thread to reload
//!   the affected source when an operation ends;
//! - two icon threads decoding icon files.
//!
//! `CancelOperation` is handled in `send` itself, so it reaches the running operation instead of
//! waiting behind it. Dropping the `Engine` stops and joins every thread.
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use store_core::{
    App, Backend, CancelToken, Catalog, Host, Index, InstallError, Installer, Outcome,
    RefreshStage, Row, SourceId, Store,
};
use store_protocol::{ChangeKind, ErrorCode, Request, Stage};

use crate::icons::{self, Rgba};

/// Most apps one search returns.
const SEARCH_LIMIT: usize = 100;

const ICON_THREADS: usize = 2;

pub struct EngineConfig {
    pub db_path: PathBuf,
    pub backends: Vec<Box<dyn Backend>>,
    pub installer: Arc<dyn Installer>,
    /// Sources the installer can install from and remove from.
    pub installable: Vec<SourceId>,
    pub host: Host,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Chips,
    Home {
        per_row: usize,
        seed: u64,
    },
    Search {
        generation: u64,
        query: String,
        source: Option<SourceId>,
    },
    Open {
        key: String,
    },
    Installed,
    Updates,
    /// Reloads every source.
    Refresh,
    Install {
        key: String,
        source: SourceId,
    },
    Remove {
        key: String,
        source: SourceId,
    },
    /// Cancels the running operation, if any. Queued operations are not affected.
    CancelOperation,
    LoadIcon {
        path: String,
        /// The size the icon will be shown at; informational for now.
        side: u32,
    },
}

/// An app page: the app and which of its sources the installer can act on.
#[derive(Debug, Clone, PartialEq)]
pub struct AppView {
    pub app: App,
    pub installable: Vec<SourceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshState {
    Started,
    Updated(usize),
    Unavailable(Option<String>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpState {
    Started(String),
    Progress {
        message: String,
        percent: Option<u8>,
    },
    /// Sent exactly once for every `Install` and `Remove`, whatever happened.
    Finished(Result<String, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Chips(Vec<(SourceId, usize)>),
    Home(Vec<Row>),
    SearchResults {
        generation: u64,
        apps: Vec<App>,
    },
    AppPage(Option<AppView>),
    Installed(Vec<App>),
    Updates(Vec<App>),
    Refresh {
        source: SourceId,
        state: RefreshState,
    },
    /// Ends every refresh, including the one that follows an operation.
    RefreshDone,
    Operation(OpState),
    Icon {
        path: String,
        image: Option<Arc<Rgba>>,
    },
    Error(String),
}

/// From several `Search` commands keeps only the last one, where it was; every other command
/// keeps its place.
pub fn coalesce(commands: Vec<Command>) -> Vec<Command> {
    let last_search = commands
        .iter()
        .rposition(|c| matches!(c, Command::Search { .. }));
    commands
        .into_iter()
        .enumerate()
        .filter(|(i, c)| !matches!(c, Command::Search { .. }) || Some(*i) == last_search)
        .map(|(_, c)| c)
        .collect()
}

type Callback = Arc<dyn Fn(Event) + Send + Sync>;

/// Calls the UI's callback. A panic in it must not take an engine thread down with it.
fn emit(on_event: &Callback, event: Event) {
    let _ = catch_unwind(AssertUnwindSafe(|| on_event(event)));
}

enum RefreshJob {
    All,
    Only(Vec<SourceId>),
}

enum OpKind {
    Install,
    Remove,
}

struct OpJob {
    kind: OpKind,
    key: String,
    source: SourceId,
}

struct Channels {
    query: Sender<Command>,
    refresh: Sender<RefreshJob>,
    operations: Sender<OpJob>,
    icons: Sender<String>,
}

/// State shared by `Engine` and its threads.
struct Shared {
    /// Set once, when the engine is dropped: threads stop taking new work.
    stopping: AtomicBool,
    /// The running operation's token. `stopping` is set while holding this lock, so an operation
    /// either sees `stopping` before it starts or has its token cancelled by the drop.
    running: Mutex<Option<CancelToken>>,
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    fn running(&self) -> MutexGuard<'_, Option<CancelToken>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct Engine {
    channels: Option<Channels>,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Engine {
    /// Opens the index at `config.db_path` (creating its directory) and starts the threads.
    /// Nothing is refreshed until a `Command::Refresh` arrives.
    pub fn start(
        config: EngineConfig,
        on_event: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Engine, store_core::Error> {
        let EngineConfig {
            db_path,
            backends,
            installer,
            installable,
            host,
        } = config;
        if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        // The writer opens first, so it is the one that creates or migrates the schema.
        let store = Store::new(Index::open(&db_path)?, backends).with_host(host.clone());
        let refresh_catalog = Catalog::open(&db_path, host.clone())?;
        let query_catalog = Catalog::open(&db_path, host.clone())?;
        let ops_catalog = Catalog::open(&db_path, host)?;

        let on_event: Callback = Arc::new(on_event);
        let (query_tx, query_rx) = channel();
        let (refresh_tx, refresh_rx) = channel();
        let (ops_tx, ops_rx) = channel();
        let (icons_tx, icons_rx) = channel();
        let ops_refresh = refresh_tx.clone();
        let mut engine = Engine {
            channels: Some(Channels {
                query: query_tx,
                refresh: refresh_tx,
                operations: ops_tx,
                icons: icons_tx,
            }),
            shared: Arc::new(Shared {
                stopping: AtomicBool::new(false),
                running: Mutex::new(None),
            }),
            threads: Vec::new(),
        };

        // On a spawn error, returning drops `engine`, which joins the threads already started.
        let (shared, cb) = (engine.shared.clone(), on_event.clone());
        let query = Queries {
            catalog: query_catalog,
            installable: installable.clone(),
        };
        engine.spawn("store-query", move || query.run(query_rx, &shared, &cb))?;
        let (shared, cb) = (engine.shared.clone(), on_event.clone());
        engine.spawn("store-refresh", move || {
            refresh_loop(store, refresh_catalog, refresh_rx, &shared, &cb)
        })?;
        let (shared, cb) = (engine.shared.clone(), on_event.clone());
        let operations = Operations {
            catalog: ops_catalog,
            installer,
            installable,
            refresh: ops_refresh,
        };
        engine.spawn("store-operations", move || {
            operations.run(ops_rx, &shared, &cb)
        })?;
        let icons_rx = Arc::new(Mutex::new(icons_rx));
        for n in 0..ICON_THREADS {
            let (shared, cb, rx) = (engine.shared.clone(), on_event.clone(), icons_rx.clone());
            engine.spawn(&format!("store-icons-{n}"), move || {
                icon_loop(&rx, &shared, &cb)
            })?;
        }
        Ok(engine)
    }

    fn spawn(&mut self, name: &str, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
        let handle = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(work)?;
        self.threads.push(handle);
        Ok(())
    }

    /// Hands `command` to the thread that does it. Never blocks; a command the engine can no
    /// longer take is dropped.
    pub fn send(&self, command: Command) {
        let Some(channels) = &self.channels else {
            return;
        };
        // A send only fails when the receiving thread is gone; the command is dropped then.
        match command {
            Command::Refresh => {
                let _ = channels.refresh.send(RefreshJob::All);
            }
            Command::Install { key, source } => {
                let kind = OpKind::Install;
                let _ = channels.operations.send(OpJob { kind, key, source });
            }
            Command::Remove { key, source } => {
                let kind = OpKind::Remove;
                let _ = channels.operations.send(OpJob { kind, key, source });
            }
            Command::CancelOperation => {
                if let Some(token) = self.shared.running().as_ref() {
                    token.cancel();
                }
            }
            Command::LoadIcon { path, side: _ } => {
                let _ = channels.icons.send(path);
            }
            query => {
                let _ = channels.query.send(query);
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        {
            let running = self.shared.running();
            self.shared.stopping.store(true, Ordering::SeqCst);
            if let Some(token) = running.as_ref() {
                token.cancel();
            }
        }
        // Closing the channels ends every thread's receive loop.
        self.channels = None;
        for handle in self.threads.drain(..) {
            // A thread only ends by panicking if `emit`'s guard failed; nothing to do then.
            let _ = handle.join();
        }
    }
}

struct Queries {
    catalog: Catalog,
    installable: Vec<SourceId>,
}

impl Queries {
    fn run(&self, commands: Receiver<Command>, shared: &Shared, on_event: &Callback) {
        while let Ok(first) = commands.recv() {
            let mut batch = vec![first];
            batch.extend(commands.try_iter());
            for command in coalesce(batch) {
                if shared.stopping() {
                    return;
                }
                let answer = catch_unwind(AssertUnwindSafe(|| self.answer(command)))
                    .unwrap_or_else(|_| Some(Err("internal error: a query panicked".to_string())));
                match answer {
                    Some(Ok(event)) => emit(on_event, event),
                    Some(Err(message)) => emit(on_event, Event::Error(message)),
                    None => {}
                }
            }
        }
    }

    fn answer(&self, command: Command) -> Option<Result<Event, String>> {
        let c = &self.catalog;
        let result = match command {
            Command::Chips => c.chips().map(Event::Chips),
            Command::Home { per_row, seed } => c.home(per_row, seed).map(Event::Home),
            Command::Search {
                generation,
                query,
                source,
            } => c
                .search(&query, source.as_ref(), SEARCH_LIMIT)
                .map(|apps| Event::SearchResults { generation, apps }),
            Command::Open { key } => c.app(&key).map(|app| {
                Event::AppPage(app.map(|app| {
                    AppView {
                        installable: app
                            .variants
                            .iter()
                            .map(|p| p.source.clone())
                            .filter(|s| self.installable.contains(s))
                            .collect(),
                        app,
                    }
                }))
            }),
            Command::Installed => c.installed().map(Event::Installed),
            Command::Updates => c.updates().map(Event::Updates),
            // `Engine::send` routes the others elsewhere.
            _ => return None,
        };
        Some(result.map_err(|e| e.to_string()))
    }
}

fn refresh_loop(
    mut store: Store,
    catalog: Catalog,
    jobs: Receiver<RefreshJob>,
    shared: &Shared,
    on_event: &Callback,
) {
    while let Ok(job) = jobs.recv() {
        if shared.stopping() {
            return;
        }
        let only = match &job {
            RefreshJob::All => None,
            RefreshJob::Only(sources) => Some(sources.as_slice()),
        };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            store.refresh_with(only, &|source, stage| {
                let state = match stage {
                    RefreshStage::Started => RefreshState::Started,
                    RefreshStage::Finished(Outcome::Updated { packages }) => {
                        RefreshState::Updated(*packages)
                    }
                    RefreshStage::Finished(Outcome::Unavailable { reason }) => {
                        RefreshState::Unavailable(reason.clone())
                    }
                    RefreshStage::Finished(Outcome::Failed(message)) => {
                        RefreshState::Failed(message.clone())
                    }
                };
                emit(
                    on_event,
                    Event::Refresh {
                        source: source.clone(),
                        state,
                    },
                );
            });
        }));
        match catch_unwind(AssertUnwindSafe(|| catalog.chips())) {
            Ok(Ok(chips)) => emit(on_event, Event::Chips(chips)),
            Ok(Err(e)) => emit(on_event, Event::Error(e.to_string())),
            Err(_) => emit(
                on_event,
                Event::Error("internal error: chips panicked".into()),
            ),
        }
        // Always sent, so the UI never stays in a refreshing state.
        emit(on_event, Event::RefreshDone);
    }
}

struct Operations {
    catalog: Catalog,
    installer: Arc<dyn Installer>,
    installable: Vec<SourceId>,
    refresh: Sender<RefreshJob>,
}

impl Operations {
    fn run(&self, jobs: Receiver<OpJob>, shared: &Shared, on_event: &Callback) {
        while let Ok(job) = jobs.recv() {
            let token = CancelToken::default();
            {
                let mut running = shared.running();
                // Checked under the lock `Drop` sets it with: a queued operation never starts
                // once the engine is going away.
                if shared.stopping() {
                    return;
                }
                *running = Some(token.clone());
            }
            let mut ran_installer = false;
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.perform(&job, &token, &mut ran_installer, on_event)
            }))
            .unwrap_or_else(|_| Err("internal error: the operation failed unexpectedly".into()));
            *shared.running() = None;
            emit(on_event, Event::Operation(OpState::Finished(result)));
            if ran_installer {
                let _ = self.refresh.send(RefreshJob::Only(vec![job.source]));
            }
        }
    }

    /// Resolves the package and runs the installer. Returns the summary or the error message;
    /// `ran_installer` tells whether the system may have changed.
    fn perform(
        &self,
        job: &OpJob,
        token: &CancelToken,
        ran_installer: &mut bool,
        on_event: &Callback,
    ) -> Result<String, String> {
        if !self.installable.contains(&job.source) {
            return Err(format!(
                "{} packages cannot be installed or removed here",
                printable(job.source.as_str())
            ));
        }
        let app = self
            .catalog
            .app(&job.key)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{} is not in the index", printable(&job.key)))?;
        let variant = app.variant(&job.source).ok_or_else(|| {
            format!(
                "{} is not available from {}",
                printable(&app.title),
                printable(job.source.as_str())
            )
        })?;
        // The name is untrusted text from the index: it only ever goes into the typed request,
        // which the helper validates again.
        let packages = vec![variant.name.clone()];
        let (request, verb) = match job.kind {
            OpKind::Install => (Request::Install { packages }, "Installing"),
            OpKind::Remove => (Request::Remove { packages }, "Removing"),
        };
        emit(
            on_event,
            Event::Operation(OpState::Started(format!(
                "{verb} {}",
                printable(&variant.name)
            ))),
        );
        let mut changes = None;
        let mut error = None;
        *ran_installer = true;
        let outcome = self
            .installer
            .run(&request, token, &mut |event| match event {
                store_protocol::Event::Progress {
                    stage,
                    package,
                    percent,
                } => {
                    let mut message = stage_verb(stage).to_string();
                    if let Some(package) = package {
                        message.push(' ');
                        message.push_str(&printable(&package));
                    }
                    emit(
                        on_event,
                        Event::Operation(OpState::Progress { message, percent }),
                    );
                }
                store_protocol::Event::CancelIgnored => emit(
                    on_event,
                    Event::Operation(OpState::Progress {
                        message: "Too late to cancel; finishing".into(),
                        percent: None,
                    }),
                ),
                store_protocol::Event::Done { changes: done } => changes = Some(done),
                store_protocol::Event::Error { code, message } => {
                    error.get_or_insert(error_message(code, &message));
                }
                store_protocol::Event::Started { .. } => {}
            });
        match (outcome, error) {
            (Err(e), _) => Err(install_error_message(&e)),
            (Ok(()), Some(message)) => Err(message),
            (Ok(()), None) => Ok(summary(changes.as_deref())),
        }
    }
}

fn icon_loop(jobs: &Mutex<Receiver<String>>, shared: &Shared, on_event: &Callback) {
    loop {
        // The lock is held only while waiting for a job, never while decoding or emitting.
        let job = jobs.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok(path) = job else { return };
        if shared.stopping() {
            return;
        }
        // `icons::load` never panics.
        let image = icons::load(Path::new(&path)).map(Arc::new);
        emit(on_event, Event::Icon { path, image });
    }
}

fn stage_verb(stage: Stage) -> &'static str {
    match stage {
        Stage::Refresh => "Refreshing",
        Stage::Resolve => "Resolving",
        Stage::Download => "Downloading",
        Stage::Verify => "Verifying",
        Stage::Hooks => "Running hooks",
        Stage::Install => "Installing",
        Stage::Upgrade => "Upgrading",
        Stage::Remove => "Removing",
    }
}

fn error_message(code: ErrorCode, message: &str) -> String {
    match code {
        ErrorCode::Cancelled => "cancelled".to_string(),
        _ => printable(message),
    }
}

fn install_error_message(error: &InstallError) -> String {
    match error {
        InstallError::Dismissed => "authentication cancelled".to_string(),
        InstallError::Rejected { code, message } => error_message(*code, message),
        other => printable(&other.to_string()),
    }
}

fn summary(changes: Option<&[store_protocol::Change]>) -> String {
    let Some(changes) = changes else {
        return "Done".to_string();
    };
    if changes.is_empty() {
        return "Nothing to do".to_string();
    }
    let parts: Vec<String> = changes
        .iter()
        .map(|c| {
            let verb = match c.kind {
                ChangeKind::Installed => "Installed",
                ChangeKind::Removed => "Removed",
                ChangeKind::Upgraded => "Upgraded",
            };
            match &c.version {
                Some(version) => format!("{verb} {} {}", printable(&c.package), printable(version)),
                None => format!("{verb} {}", printable(&c.package)),
            }
        })
        .collect();
    parts.join(", ")
}

/// Untrusted text (package names, helper messages) with control characters replaced, so it
/// cannot break the line it is shown on.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
