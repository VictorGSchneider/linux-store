//! Connects the window to the engine. Everything here runs on the UI thread: engine events are
//! marshalled with `upgrade_in_event_loop`, so the state lives in a thread-local and `Image`s
//! (not `Send`) are only ever built and held here.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use store_app::{AppView, Command, Engine, EngineConfig, Event, OpState, RefreshState};
use store_core::{App, Row, SourceId};

use crate::convert;
use crate::state::{Nav, OpGate, PageKeys, Place, SETTLE_TIMEOUT, status_visible_for};
use crate::{AppWindow, CardData, Page, TextUtil};

/// Cards per home row (the row component instantiates every card it gets).
const PER_ROW: usize = 14;
/// Side the icons are shown at, informational for the engine.
const ICON_SIDE: u32 = 64;
/// Decoded images kept for the run; past this the map is cleared (cards keep their own copy).
const MAX_IMAGES: usize = 400;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    engine: Engine,
    seed: u64,
    generation: u64,
    query: String,
    filter: Option<SourceId>,
    chips: Vec<(SourceId, usize)>,
    images: HashMap<String, slint::Image>,
    requested: HashSet<String>,
    /// Card key to icon path, for the cards in `rows` and in `results`.
    row_icons: HashMap<String, String>,
    result_icons: HashMap<String, String>,
    installed_icons: HashMap<String, String>,
    updates_icons: HashMap<String, String>,
    /// The page below the app page, and so where Back goes.
    nav: Nav,
    refreshes_done: usize,
    /// Which app the user asked for and which one the page shows.
    keys: PageKeys,
    /// The icon path of the page's app, to fill in when the image arrives.
    page_icon: Option<String>,
    /// An install or remove was sent and has not finished, or its result is not on the page
    /// yet: clicks are ignored meanwhile.
    gate: OpGate,
    /// Ends the settling after `SETTLE_TIMEOUT` if the reload never shows up.
    settle_timer: slint::Timer,
    /// How many `Open`s were sent and how many `AppPage` answers came back (the query thread
    /// answers in order), so the gate knows which answer was asked after the reload.
    opens_sent: u64,
    opens_answered: u64,
    /// The package each source button of the open page acts on (source, package name).
    page_packages: Vec<(String, String)>,
    /// The last operation's result and the app it belongs to.
    status: Option<Status>,
    /// Sources being refreshed right now.
    refreshing: HashSet<SourceId>,
    /// What the screen shows, so an identical answer (a re-query after a refresh that changed
    /// nothing there) does not rebuild the models and lose focus, selection or icons.
    shown_rows: Option<Vec<Row>>,
    shown_results: Option<Vec<App>>,
    shown_installed: Option<Vec<App>>,
    shown_updates: Option<Vec<App>>,
    shown_page: Option<AppView>,
}

struct Status {
    key: Option<String>,
    text: String,
    error: bool,
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Starts the engine, connects the callbacks and asks for the first screen and a refresh.
/// Call on the UI thread, before the event loop runs.
pub fn install(ui: &AppWindow, config: EngineConfig) -> Result<(), store_core::Error> {
    ui.global::<TextUtil>()
        .on_initial(|name| convert::initial(name.as_str()).into());

    let weak = ui.as_weak();
    let engine = Engine::start(config, move |event| {
        // Err means the event loop is gone (shutdown): drop the event.
        let _ = weak.upgrade_in_event_loop(move |ui| {
            with_state(|s| s.handle(&ui, event));
        });
    })?;

    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    let state = State {
        engine,
        seed: convert::day_of_year(days),
        generation: 0,
        query: String::new(),
        filter: None,
        chips: Vec::new(),
        images: HashMap::new(),
        requested: HashSet::new(),
        row_icons: HashMap::new(),
        result_icons: HashMap::new(),
        installed_icons: HashMap::new(),
        updates_icons: HashMap::new(),
        nav: Nav::default(),
        refreshes_done: 0,
        keys: PageKeys::default(),
        page_icon: None,
        gate: OpGate::default(),
        settle_timer: slint::Timer::default(),
        opens_sent: 0,
        opens_answered: 0,
        page_packages: Vec::new(),
        status: None,
        refreshing: HashSet::new(),
        shown_rows: None,
        shown_results: None,
        shown_installed: None,
        shown_updates: None,
        shown_page: None,
    };
    state.engine.send(Command::Chips);
    state.send_home();
    state.engine.send(Command::Refresh);
    STATE.with(|s| *s.borrow_mut() = Some(state));

    let weak = ui.as_weak();
    ui.on_search_edited(move |text| {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.search_edited(&ui, text.as_str()));
        }
    });
    let weak = ui.as_weak();
    ui.on_chip_clicked(move |source| {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.chip_clicked(&ui, source.as_str()));
        }
    });
    let weak = ui.as_weak();
    ui.on_show_home(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_query("".into());
            with_state(|s| s.search_edited(&ui, ""));
        }
    });
    let weak = ui.as_weak();
    ui.on_show_installed(move || {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.show_list(&ui, Place::Installed));
        }
    });
    let weak = ui.as_weak();
    ui.on_show_updates(move || {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.show_list(&ui, Place::Updates));
        }
    });
    let weak = ui.as_weak();
    ui.on_card_clicked(move |key| {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.card_clicked(&ui, key.as_str()));
        }
    });
    let weak = ui.as_weak();
    ui.on_back(move || {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.back(&ui));
        }
    });
    let weak = ui.as_weak();
    ui.on_install_clicked(move |source| {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.operation_clicked(&ui, source.as_str(), false));
        }
    });
    let weak = ui.as_weak();
    ui.on_remove_clicked(move |source| {
        if let Some(ui) = weak.upgrade() {
            with_state(|s| s.operation_clicked(&ui, source.as_str(), true));
        }
    });
    ui.on_cancel_clicked(|| {
        with_state(|s| s.engine.send(Command::CancelOperation));
    });
    ui.on_open_link(|url| open_link(url.as_str()));
    Ok(())
}

/// Opens `url` in the user's browser with `xdg-open`, only for `http://` and `https://` URLs.
/// No shell, no waiting (a thread reaps the child), all stdio closed.
pub fn open_link(url: &str) {
    if !convert::is_openable_url(url) {
        return;
    }
    let spawned = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("linux-store: cannot run xdg-open: {e}"),
    }
}

/// Drops the engine (stopping its threads). Call after the event loop ends.
pub fn shutdown() {
    let state = STATE.with(|s| s.borrow_mut().take());
    drop(state);
}

/// Feeds `event` to the window as if the engine had sent it, for tools driving the window
/// (examples that need an error or a late answer at an exact moment). Never touches the system.
// Exists for the headless examples only; not part of the application's API.
#[doc(hidden)]
pub fn inject_event(ui: &AppWindow, event: Event) {
    with_state(|s| s.handle(ui, event));
}

/// How many refreshes have finished so far (for tools driving the window).
// Exists for the headless examples only; not part of the application's API.
#[doc(hidden)]
pub fn refreshes_done() -> usize {
    with_state(|s| s.refreshes_done).unwrap_or(0)
}

impl State {
    fn send_home(&self) {
        self.engine.send(Command::Home {
            per_row: PER_ROW,
            seed: self.seed,
        });
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn handle(&mut self, ui: &AppWindow, event: Event) {
        match event {
            Event::Chips(chips) => {
                self.chips = chips;
                self.show_chips(ui);
            }
            Event::Home(rows) => self.show_rows(ui, rows),
            Event::SearchResults { generation, apps } => {
                if generation == self.generation && self.searching() {
                    self.show_results(ui, apps);
                    ui.set_searching(false);
                }
            }
            Event::Refresh { source, state } => {
                match state {
                    RefreshState::Started => {
                        self.refreshing.insert(source);
                    }
                    RefreshState::Updated(_) => {
                        self.refreshing.remove(&source);
                        // The source is written: show it now, without waiting for the slower
                        // sources of the pass.
                        self.requery(ui);
                    }
                    RefreshState::Unavailable(_) | RefreshState::Failed(_) => {
                        self.refreshing.remove(&source);
                    }
                }
                ui.set_refreshing(!self.refreshing.is_empty());
                self.show_chips(ui);
            }
            Event::RefreshDone => {
                self.refreshes_done += 1;
                self.refreshing.clear();
                ui.set_refreshing(false);
                self.requery(ui);
            }
            Event::AppPage(view) => {
                self.opens_answered += 1;
                self.show_page(ui, view);
                self.gate.page_answered(self.opens_answered);
                self.show_gate(ui);
            }
            Event::Operation(state) => self.operation_event(ui, state),
            Event::OperationSettled => {
                // The source is reloaded: ask for the page again, and keep the buttons closed
                // until that answer is on screen.
                let reopen = self.reopen_page(ui);
                self.gate.settled(reopen);
                self.show_gate(ui);
            }
            Event::Icon {
                path,
                image: Some(rgba),
            } => {
                let image = convert::image_from_rgba(&rgba);
                if self.images.len() >= MAX_IMAGES {
                    self.images.clear();
                    self.requested.clear();
                }
                self.images.insert(path.clone(), image.clone());
                self.apply_icon(ui, &path, &image);
            }
            Event::Error(message) => {
                eprintln!("linux-store: {message}");
                ui.set_searching(false);
                // The operation state belongs to Started/Progress/Finished only: an unrelated
                // error must not hide a running operation or let a second one start.
            }
            // Both lists are cheap: the model is always updated, only the visible page shows it
            // and an answer never changes the page.
            Event::Installed(apps) => self.show_installed(ui, apps),
            Event::Updates(apps) => self.show_updates(ui, apps),
            Event::Icon { image: None, .. } => {}
        }
    }

    /// The index changed (a source was written): asks again for everything on screen. At most
    /// one set of requests per refresh event; the query thread answers them in order.
    fn requery(&mut self, ui: &AppWindow) {
        self.engine.send(Command::Chips);
        self.send_home();
        if self.searching() {
            self.run_search(ui);
        }
        // The list on show (or under the app page) may have changed: an operation refreshes its
        // source, so a just installed app appears and a removed one goes.
        self.request_list(self.nav.place());
        // Reload the open page (its status stays).
        self.reopen_page(ui);
    }

    /// Asks for the open app page again. Returns the sequence number of that `Open`, or `None`
    /// when no app page is open.
    fn reopen_page(&mut self, ui: &AppWindow) -> Option<u64> {
        if ui.get_page() != Page::App {
            return None;
        }
        let key = self.keys.page()?.to_string();
        self.keys.request(&key);
        Some(self.send_open(key))
    }

    fn send_open(&mut self, key: String) -> u64 {
        self.opens_sent += 1;
        self.engine.send(Command::Open { key });
        self.opens_sent
    }

    fn show_chips(&self, ui: &AppWindow) {
        let views = convert::chip_views(&self.chips, self.filter.as_ref(), &|s| {
            self.refreshing.contains(s)
        });
        ui.set_chips(ModelRc::new(VecModel::from(convert::chip_data(views))));
    }

    fn show_rows(&mut self, ui: &AppWindow, rows: Vec<Row>) {
        if self.shown_rows.as_ref() == Some(&rows) {
            return;
        }
        let images = &self.images;
        let icon_of = |p: &str| images.get(p).cloned();
        let data: Vec<_> = rows
            .iter()
            .map(|r| convert::row_data(r, &icon_of))
            .collect();
        ui.set_rows(ModelRc::new(VecModel::from(data)));
        self.row_icons = self.note_icons(rows.iter().flat_map(|r| &r.apps));
        self.shown_rows = Some(rows);
    }

    fn show_results(&mut self, ui: &AppWindow, apps: Vec<App>) {
        if self.shown_results.as_ref() == Some(&apps) {
            return;
        }
        let images = &self.images;
        let data = convert::cards(&apps, &|p| images.get(p).cloned());
        ui.set_results(ModelRc::new(VecModel::from(data)));
        self.result_icons = self.note_icons(apps.iter());
        self.shown_results = Some(apps);
    }

    fn show_installed(&mut self, ui: &AppWindow, apps: Vec<App>) {
        if self.shown_installed.as_ref() == Some(&apps) {
            return;
        }
        let images = &self.images;
        let data = convert::cards(&apps, &|p| images.get(p).cloned());
        ui.set_installed_count(convert::count_text(data.len()).into());
        ui.set_installed_empty(convert::empty_kind(Place::Installed, data.len()));
        ui.set_installed(ModelRc::new(VecModel::from(data)));
        self.installed_icons = self.note_icons(apps.iter());
        self.shown_installed = Some(apps);
    }

    fn show_updates(&mut self, ui: &AppWindow, apps: Vec<App>) {
        if self.shown_updates.as_ref() == Some(&apps) {
            return;
        }
        let images = &self.images;
        let data = convert::cards(&apps, &|p| images.get(p).cloned());
        ui.set_updates_count(convert::count_text(data.len()).into());
        ui.set_updates_empty(convert::empty_kind(Place::Updates, data.len()));
        ui.set_updates(ModelRc::new(VecModel::from(data)));
        self.updates_icons = self.note_icons(apps.iter());
        self.shown_updates = Some(apps);
    }

    /// Asks the engine for the list of `place`, if it is a list page.
    fn request_list(&self, place: Place) {
        match place {
            Place::Installed => self.engine.send(Command::Installed),
            Place::Updates => self.engine.send(Command::Updates),
            Place::Home => {}
        }
    }

    /// Maps card keys to icon paths, and asks the engine once for each path not loaded yet.
    fn note_icons<'a>(&mut self, apps: impl Iterator<Item = &'a App>) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for app in apps {
            let Some(path) = app.icon() else { continue };
            map.insert(app.key.clone(), path.to_string());
            if !self.images.contains_key(path) && self.requested.insert(path.to_string()) {
                self.engine.send(Command::LoadIcon {
                    path: path.to_string(),
                    side: ICON_SIDE,
                });
            }
        }
        map
    }

    fn apply_icon(&self, ui: &AppWindow, path: &str, image: &slint::Image) {
        let apply = |cards: ModelRc<CardData>, paths: &HashMap<String, String>| {
            for i in 0..cards.row_count() {
                let Some(mut card) = cards.row_data(i) else {
                    continue;
                };
                if paths.get(card.key.as_str()).map(String::as_str) == Some(path) {
                    card.icon = image.clone();
                    card.has_icon = true;
                    cards.set_row_data(i, card);
                }
            }
        };
        let rows = ui.get_rows();
        for r in 0..rows.row_count() {
            if let Some(row) = rows.row_data(r) {
                apply(row.cards, &self.row_icons);
            }
        }
        apply(ui.get_results(), &self.result_icons);
        apply(ui.get_installed(), &self.installed_icons);
        apply(ui.get_updates(), &self.updates_icons);
        if self.page_icon.as_deref() == Some(path) {
            let mut page = ui.get_app_page();
            page.icon = image.clone();
            page.has_icon = true;
            ui.set_app_page(page);
        }
    }

    fn card_clicked(&mut self, ui: &AppWindow, key: &str) {
        ui.set_app_missing(false);
        self.keys.request(key);
        self.send_open(key.to_string());
    }

    fn show_page(&mut self, ui: &AppWindow, view: Option<AppView>) {
        // Only the app asked for last: the user may have clicked another one meanwhile, or left.
        if !self.keys.waiting() {
            return;
        }
        let Some(view) = view else {
            self.keys.not_found();
            self.nav.go(Place::Home);
            ui.set_page(Page::Home);
            ui.set_app_missing(true);
            return;
        };
        if !self.keys.accept(&view.app.key) {
            return;
        }
        if self.shown_page.as_ref() == Some(&view) && ui.get_page() == Page::App {
            // The same page again (a re-query after a refresh): keep the model, and the focus.
            return;
        }
        let pv = convert::page_view(&view);
        self.page_packages = pv
            .sources
            .iter()
            .map(|b| (b.source.clone(), b.package.clone()))
            .collect();
        self.shown_page = Some(view);
        let icon = pv
            .icon_path
            .as_deref()
            .and_then(|p| self.images.get(p).cloned());
        if let Some(path) = &pv.icon_path
            && icon.is_none()
            && self.requested.insert(path.clone())
        {
            self.engine.send(Command::LoadIcon {
                path: path.clone(),
                side: ICON_SIDE,
            });
        }
        self.page_icon = pv.icon_path.clone();
        ui.set_app_page(convert::page_data(&pv, icon));
        ui.set_app_missing(false);
        self.show_status(ui);
        ui.set_page(Page::App);
    }

    /// The operation's result shows only on the page of the app it was started for.
    fn show_status(&self, ui: &AppWindow) {
        let shown = self
            .status
            .as_ref()
            .filter(|s| status_visible_for(s.key.as_deref(), self.keys.page()));
        ui.set_op_status(shown.map_or("", |s| s.text.as_str()).into());
        ui.set_op_status_error(shown.is_some_and(|s| s.error));
    }

    /// Every way of leaving the app page (Back, search, chips, Home, Installeds, Updates):
    /// answers still in flight become stale. A running operation is not cancelled.
    /// `place` is the page the user goes to; the app page's own Back returns to where it came from.
    fn leave_app_page(&mut self, ui: &AppWindow, place: Place) {
        self.keys.leave();
        self.page_icon = None;
        self.page_packages.clear();
        self.shown_page = None;
        self.nav.go(place);
        ui.set_page(match place {
            Place::Home => Page::Home,
            Place::Installed => Page::Installed,
            Place::Updates => Page::Updates,
        });
        self.show_status(ui);
    }

    fn back(&mut self, ui: &AppWindow) {
        let place = self.nav.back();
        self.leave_app_page(ui, place);
    }

    fn show_list(&mut self, ui: &AppWindow, place: Place) {
        self.leave_app_page(ui, place);
        self.request_list(place);
    }

    fn operation_clicked(&mut self, ui: &AppWindow, source: &str, remove: bool) {
        // The package the button showed: the engine refuses if the index holds another now.
        let Some(package) = self
            .page_packages
            .iter()
            .find(|(s, _)| s == source)
            .map(|(_, p)| p.clone())
        else {
            return;
        };
        let Some(key) = self.gate.click(
            self.keys.page().map(str::to_string).as_deref(),
            ui.get_op_active(),
        ) else {
            return;
        };
        self.show_gate(ui);
        let source = SourceId::new(source);
        self.engine.send(if remove {
            Command::Remove {
                key,
                source,
                package,
            }
        } else {
            Command::Install {
                key,
                source,
                package,
            }
        });
    }

    /// Mirrors the gate on the page: the "updating" look while it settles.
    fn show_gate(&self, ui: &AppWindow) {
        ui.set_op_settling(self.gate.settling());
    }

    fn operation_event(&mut self, ui: &AppWindow, state: OpState) {
        match state {
            OpState::Started(message) => {
                self.gate.started();
                self.show_gate(ui);
                self.status = None;
                self.show_status(ui);
                ui.set_op_message(
                    convert::sanitize_text(&message, convert::MAX_MESSAGE_CHARS).into(),
                );
                ui.set_op_percent(-1);
                ui.set_op_active(true);
            }
            OpState::Progress { message, percent } => {
                ui.set_op_message(
                    convert::sanitize_text(&message, convert::MAX_MESSAGE_CHARS).into(),
                );
                ui.set_op_percent(percent.map_or(-1, |p| i32::from(p.min(100))));
            }
            OpState::Finished(result) => self.finish_operation(ui, result),
        }
    }

    /// Ends the operation state on every path (also a refusal that never started).
    fn finish_operation(&mut self, ui: &AppWindow, result: Result<String, String>) {
        let key = self.gate.finished(Instant::now());
        self.show_gate(ui);
        // Bounds the settling: if the reload never shows up, the buttons come back anyway.
        // Restarting the timer makes an earlier operation's timer irrelevant.
        let weak = ui.as_weak();
        self.settle_timer
            .start(slint::TimerMode::SingleShot, SETTLE_TIMEOUT, move || {
                if let Some(ui) = weak.upgrade() {
                    with_state(|s| {
                        s.gate.tick(Instant::now());
                        s.show_gate(&ui);
                    });
                }
            });
        // The operation bar goes now; only the page buttons wait for the reload.
        ui.set_op_active(false);
        ui.set_op_percent(-1);
        ui.set_op_message("".into());
        let (text, error) = match result {
            Ok(summary) => (summary, false),
            Err(message) => (message, true),
        };
        self.status = Some(Status {
            key,
            text: convert::sanitize_text(&text, convert::MAX_MESSAGE_CHARS),
            error,
        });
        self.show_status(ui);
    }

    fn search_edited(&mut self, ui: &AppWindow, text: &str) {
        self.leave_app_page(ui, Place::Home);
        self.query = text.to_string();
        if self.searching() {
            self.run_search(ui);
        } else {
            // Going home: drop in-flight answers, clear the results.
            self.generation += 1;
            self.result_icons.clear();
            self.shown_results = None;
            ui.set_results(ModelRc::default());
            ui.set_searching(false);
        }
    }

    fn run_search(&mut self, ui: &AppWindow) {
        self.generation += 1;
        ui.set_searching(true);
        self.engine.send(Command::Search {
            generation: self.generation,
            query: self.query.clone(),
            source: self.filter.clone(),
        });
    }

    fn chip_clicked(&mut self, ui: &AppWindow, source: &str) {
        self.leave_app_page(ui, Place::Home);
        let id = SourceId::new(source);
        self.filter = if self.filter.as_ref() == Some(&id) {
            None
        } else {
            Some(id)
        };
        self.show_chips(ui);
        if self.searching() {
            self.run_search(ui);
        }
    }
}
