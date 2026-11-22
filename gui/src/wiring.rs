//! Connects the window to the engine. Everything here runs on the UI thread: engine events are
//! marshalled with `upgrade_in_event_loop`, so the state lives in a thread-local and `Image`s
//! (not `Send`) are only ever built and held here.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use store_app::{Command, Engine, EngineConfig, Event};
use store_core::{App, Row, SourceId};

use crate::convert;
use crate::{AppWindow, CardData, TextUtil};

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
    refreshes_done: usize,
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
        refreshes_done: 0,
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
    Ok(())
}

/// Drops the engine (stopping its threads). Call after the event loop ends.
pub fn shutdown() {
    let state = STATE.with(|s| s.borrow_mut().take());
    drop(state);
}

/// How many refreshes have finished so far (for tools driving the window).
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
            Event::Home(rows) => self.show_rows(ui, &rows),
            Event::SearchResults { generation, apps } => {
                if generation == self.generation && self.searching() {
                    self.show_results(ui, &apps);
                    ui.set_searching(false);
                }
            }
            Event::RefreshDone => {
                self.refreshes_done += 1;
                self.engine.send(Command::Chips);
                self.send_home();
                if self.searching() {
                    self.run_search(ui);
                }
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
            }
            // Refresh progress, app pages, operations, installed and updates: later tasks.
            _ => {}
        }
    }

    fn show_chips(&self, ui: &AppWindow) {
        let views = convert::chip_views(&self.chips, self.filter.as_ref());
        ui.set_chips(ModelRc::new(VecModel::from(convert::chip_data(views))));
    }

    fn show_rows(&mut self, ui: &AppWindow, rows: &[Row]) {
        let images = &self.images;
        let icon_of = |p: &str| images.get(p).cloned();
        let data: Vec<_> = rows
            .iter()
            .map(|r| convert::row_data(r, &icon_of))
            .collect();
        ui.set_rows(ModelRc::new(VecModel::from(data)));
        self.row_icons = self.note_icons(rows.iter().flat_map(|r| &r.apps));
    }

    fn show_results(&mut self, ui: &AppWindow, apps: &[App]) {
        let images = &self.images;
        let data = convert::cards(apps, &|p| images.get(p).cloned());
        ui.set_results(ModelRc::new(VecModel::from(data)));
        self.result_icons = self.note_icons(apps.iter());
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
    }

    fn search_edited(&mut self, ui: &AppWindow, text: &str) {
        self.query = text.to_string();
        if self.searching() {
            self.run_search(ui);
        } else {
            // Going home: drop in-flight answers, clear the results.
            self.generation += 1;
            self.result_icons.clear();
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
