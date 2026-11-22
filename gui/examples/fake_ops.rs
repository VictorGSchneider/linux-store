//! Development tool: drives the REAL wiring and engine headless with FAKE backends and a FAKE
//! installer (nothing touches the system, the real installer is never built), and snapshots the
//! app page through an install, a cancel, a dismissed authentication and an immediate refusal.
//!
//! XDG_CACHE_HOME=/some/tmp SLINT_BACKEND=headless cargo run -p linux-store \
//!     --example fake_ops --features slint/mcp -- OUT_DIR
//!
//! Writes OUT_DIR/fake-{page,op,cancelled,error,narrow,installed,settling,settled}.png.
//! After a successful install it asserts that the page buttons stay closed ("Updating…") until
//! the page shows the installed package, and that a click meanwhile starts nothing.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use linux_store::{AppWindow, Page, wiring};
use slint::{ComponentHandle, Model};
use store_core::testing::{FakeBackend, pkg};
use store_core::{Backend, CancelToken, Host, InstallError, Installer, Package, SourceId};
use store_protocol::{ErrorCode, Event, Request, Stage};

/// The pacman packages as the "system" has them; the installer changes them.
type System = Arc<Mutex<Vec<Package>>>;

/// A pacman backend that reads `System`, taking `delay_ms` per call (a slow reload).
struct Live {
    system: System,
    delay_ms: Arc<AtomicU64>,
}

impl Backend for Live {
    fn source(&self) -> SourceId {
        SourceId::new("pacman")
    }
    fn is_available(&self) -> bool {
        true
    }
    fn unavailable_reason(&self) -> Option<String> {
        None
    }
    fn packages(&self) -> store_core::Result<Vec<Package>> {
        let snapshot = self.system.lock().unwrap().clone();
        std::thread::sleep(Duration::from_millis(self.delay_ms.load(Ordering::SeqCst)));
        Ok(snapshot)
    }
}

#[derive(Clone, Copy)]
enum Mode {
    /// Marks the package installed and succeeds.
    Succeed,
    /// Reports 40% and then waits for the cancel token.
    Hold,
    Dismissed,
    /// A long message with control characters, as a hostile helper might send.
    Hostile,
}

struct Scripted {
    mode: Arc<Mutex<Mode>>,
    runs: Arc<AtomicUsize>,
    system: System,
}

impl Installer for Scripted {
    fn run(
        &self,
        _request: &Request,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<(), InstallError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let mode = *self.mode.lock().unwrap();
        match mode {
            Mode::Succeed => {
                if let Request::Install { packages } = _request {
                    for p in self.system.lock().unwrap().iter_mut() {
                        if packages.contains(&p.name) {
                            p.installed_version = Some(p.version.clone());
                        }
                    }
                }
                Ok(())
            }
            Mode::Hold => {
                on_event(Event::Progress {
                    stage: Stage::Download,
                    package: Some("firefox".into()),
                    percent: Some(40),
                });
                let start = Instant::now();
                while !cancel.is_cancelled() && start.elapsed() < Duration::from_secs(60) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(InstallError::Rejected {
                    code: ErrorCode::Cancelled,
                    message: "cancelled".into(),
                })
            }
            Mode::Dismissed => Err(InstallError::Dismissed),
            Mode::Hostile => Err(InstallError::Rejected {
                code: ErrorCode::Failed,
                message: format!("bad \u{1b}[31mred\u{7}\n{}", "x".repeat(1000)),
            }),
        }
    }
}

fn save(ui: &AppWindow, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let img = ui.window().take_snapshot()?;
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, img.width(), img.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(img.as_bytes())?;
    println!("wrote {path}");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).expect("output directory");
    std::fs::create_dir_all(&dir)?;
    let mut firefox = pkg("pacman", "firefox", "Fast, private web browser");
    firefox.url = Some("https://www.mozilla.org/firefox/".into());
    firefox.installed_size = 280 * 1024 * 1024;
    firefox.app_id = Some("org.mozilla.firefox".into());
    let mut flat = pkg(
        "flatpak",
        "org.mozilla.firefox",
        "Fast, private web browser",
    );
    flat.title = Some("Firefox".into());
    flat.app_id = Some("org.mozilla.firefox".into());
    flat.version = "133.0".into();
    let mut tool = pkg("pacman", "htop", "Interactive process viewer");
    tool.installed_version = Some("3.3.0-1".into());
    tool.installed_size = 400 * 1024;

    let mode = Arc::new(Mutex::new(Mode::Hold));
    let runs = Arc::new(AtomicUsize::new(0));
    let system: System = Arc::new(Mutex::new(vec![firefox, tool]));
    let delay_ms = Arc::new(AtomicU64::new(0));
    let db = std::env::var("XDG_CACHE_HOME")?;
    let config = store_app::EngineConfig {
        db_path: std::path::PathBuf::from(db).join("fake-ops.db"),
        backends: vec![
            Box::new(Live {
                system: system.clone(),
                delay_ms: delay_ms.clone(),
            }),
            Box::new(FakeBackend::new("flatpak", vec![flat])),
        ],
        installer: Arc::new(Scripted {
            mode: mode.clone(),
            runs: runs.clone(),
            system,
        }),
        installable: vec![SourceId::new("pacman")],
        host: Host::with_native("pacman"),
    };
    let _ = std::fs::remove_file(&config.db_path);
    let ui = AppWindow::new()?;
    wiring::install(&ui, config)?;
    ui.window().set_size(slint::PhysicalSize::new(1000, 700));
    ui.show()?;

    let idx = Rc::new(Cell::new(0usize));
    let mark = Rc::new(Cell::new(Instant::now()));
    let key = Rc::new(RefCell::new(String::new()));
    let htop_key = Rc::new(RefCell::new(String::new()));
    let runs_before = Rc::new(Cell::new(0usize));
    let failed = Rc::new(Cell::new(false));
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let (dir2, failed2) = (dir.clone(), failed.clone());
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let settled = mark.get().elapsed() > Duration::from_millis(1200);
            let mut next = false;
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                match idx.get() {
                    0 => {
                        if wiring::refreshes_done() >= 1 {
                            ui.set_query("firefox".into());
                            ui.invoke_search_edited("firefox".into());
                            next = true;
                        }
                    }
                    1 => {
                        if let Some(card) = ui.get_results().row_data(0) {
                            *key.borrow_mut() = card.key.to_string();
                            println!("opening {}", card.key);
                            ui.invoke_card_clicked(card.key);
                            next = true;
                        }
                    }
                    2 => {
                        if ui.get_page() == Page::App && settled {
                            let p = ui.get_app_page();
                            let order: Vec<_> = p
                                .sources
                                .iter()
                                .map(|s| (s.source.to_string(), s.enabled))
                                .collect();
                            println!("sources: {order:?}");
                            save(&ui, &format!("{dir2}/fake-page.png"))?;
                            ui.invoke_install_clicked("pacman".into());
                            next = true;
                        }
                    }
                    3 => {
                        if ui.get_op_active() && ui.get_op_percent() == 40 && settled {
                            // a second click while it runs must not start another operation
                            ui.invoke_install_clicked("pacman".into());
                            save(&ui, &format!("{dir2}/fake-op.png"))?;
                            ui.invoke_cancel_clicked();
                            next = true;
                        }
                    }
                    4 => {
                        if !ui.get_op_active()
                            && !ui.get_op_settling()
                            && ui.get_op_status() != ""
                            && settled
                        {
                            println!(
                                "after cancel: status={:?} error={} runs={}",
                                ui.get_op_status(),
                                ui.get_op_status_error(),
                                runs.load(Ordering::SeqCst)
                            );
                            assert_eq!(runs.load(Ordering::SeqCst), 1);
                            save(&ui, &format!("{dir2}/fake-cancelled.png"))?;
                            *mode.lock().unwrap() = Mode::Dismissed;
                            ui.invoke_install_clicked("pacman".into());
                            next = true;
                        }
                    }
                    5 => {
                        if !ui.get_op_active()
                            && ui.get_op_status() == "authentication cancelled"
                            && settled
                        {
                            save(&ui, &format!("{dir2}/fake-error.png"))?;
                            ui.window().set_size(slint::PhysicalSize::new(500, 700));
                            next = true;
                        }
                    }
                    6 => {
                        if settled {
                            save(&ui, &format!("{dir2}/fake-narrow.png"))?;
                            ui.window().set_size(slint::PhysicalSize::new(1000, 700));
                            // an immediate refusal: no Started, the UI must not stay active
                            ui.invoke_install_clicked("flatpak".into());
                            next = true;
                        }
                    }
                    7 => {
                        if settled {
                            println!(
                                "refusal: active={} status={:?} error={}",
                                ui.get_op_active(),
                                ui.get_op_status(),
                                ui.get_op_status_error()
                            );
                            assert!(!ui.get_op_active());
                            assert!(ui.get_op_status_error());
                            *mode.lock().unwrap() = Mode::Hostile;
                            ui.invoke_install_clicked("pacman".into());
                            next = true;
                        }
                    }
                    8 => {
                        if settled && !ui.get_op_active() {
                            let s = ui.get_op_status();
                            println!(
                                "hostile status: {} chars, {:?}",
                                s.chars().count(),
                                s.chars().take(20).collect::<String>()
                            );
                            assert!(s.chars().count() <= 300);
                            assert!(!s.chars().any(char::is_control));
                            // back keeps the search; then open the installed app
                            ui.invoke_back();
                            assert_eq!(ui.get_page(), Page::Home);
                            assert!(ui.get_results().row_count() > 0, "search stays");
                            ui.set_query("htop".into());
                            ui.invoke_search_edited("htop".into());
                            next = true;
                        }
                    }
                    9 => {
                        if let Some(card) = ui.get_results().row_data(0)
                            && card.title == "htop"
                        {
                            *htop_key.borrow_mut() = card.key.to_string();
                            ui.invoke_card_clicked(card.key);
                            next = true;
                        }
                    }
                    10 => {
                        if ui.get_page() == Page::App && settled {
                            save(&ui, &format!("{dir2}/fake-installed.png"))?;
                            next = true;
                        }
                    }
                    11 => {
                        // Fix round 1: operation on A (Firefox), then B (htop).
                        ui.invoke_back();
                        ui.invoke_card_clicked(key.borrow().as_str().into());
                        next = true;
                    }
                    12 => {
                        if ui.get_page() == Page::App
                            && ui.get_app_page().title == "Firefox"
                            && settled
                        {
                            *mode.lock().unwrap() = Mode::Hold;
                            runs_before.set(runs.load(Ordering::SeqCst));
                            ui.invoke_install_clicked("pacman".into());
                            next = true;
                        }
                    }
                    13 => {
                        if ui.get_op_active() && ui.get_op_percent() == 40 && settled {
                            // an unrelated error must not touch the running operation
                            wiring::inject_event(&ui, store_app::Event::Error("boom".into()));
                            assert!(ui.get_op_active(), "Event::Error must keep op-active");
                            ui.invoke_install_clicked("pacman".into());
                            next = true;
                        }
                    }
                    14 => {
                        if settled {
                            assert!(ui.get_op_active());
                            assert_eq!(
                                runs.load(Ordering::SeqCst),
                                runs_before.get() + 1,
                                "the second click after the error must be ignored"
                            );
                            println!("error during op: still active, one run only");
                            // click B, leave through Home, then A late answer for B arrives
                            ui.invoke_card_clicked(htop_key.borrow().as_str().into());
                            ui.invoke_show_home();
                            let mut late = pkg("pacman", "htop", "late");
                            late.installed_version = Some("3.3.0-1".into());
                            let app = store_core::App {
                                key: htop_key.borrow().clone(),
                                title: "htop".into(),
                                variants: vec![late],
                            };
                            wiring::inject_event(
                                &ui,
                                store_app::Event::AppPage(Some(store_app::AppView {
                                    app,
                                    installable: vec![],
                                })),
                            );
                            next = true;
                        }
                    }
                    15 => {
                        if settled {
                            assert_eq!(ui.get_page(), Page::Home, "a late AppPage must not reopen");
                            assert!(ui.get_op_active(), "the bar stays on every page");
                            println!("late AppPage after leaving: ignored, bar still shown");
                            save(&ui, &format!("{dir2}/fake-late-home.png"))?;
                            ui.invoke_card_clicked(htop_key.borrow().as_str().into());
                            next = true;
                        }
                    }
                    16 => {
                        if ui.get_page() == Page::App
                            && ui.get_app_page().title == "htop"
                            && settled
                        {
                            assert_eq!(ui.get_op_status(), "");
                            ui.invoke_cancel_clicked();
                            next = true;
                        }
                    }
                    17 => {
                        if !ui.get_op_active() && settled {
                            assert_eq!(ui.get_op_status(), "", "A's result must not show on B");
                            println!("B page after A finished: no status");
                            save(&ui, &format!("{dir2}/fake-b-after-a.png"))?;
                            ui.invoke_back();
                            ui.invoke_card_clicked(key.borrow().as_str().into());
                            next = true;
                        }
                    }
                    18 => {
                        if ui.get_page() == Page::App
                            && ui.get_app_page().title == "Firefox"
                            && settled
                        {
                            assert_eq!(ui.get_op_status(), "cancelled");
                            println!("A page: status {:?}", ui.get_op_status());
                            save(&ui, &format!("{dir2}/fake-a-status.png"))?;
                            next = true;
                        }
                    }
                    19 => {
                        if settled && !ui.get_op_settling() {
                            // A successful install whose reload takes 2 s.
                            *mode.lock().unwrap() = Mode::Succeed;
                            delay_ms.store(2000, Ordering::SeqCst);
                            runs_before.set(runs.load(Ordering::SeqCst));
                            let first = ui.get_app_page().sources.row_data(0).unwrap();
                            assert!(!first.installed);
                            ui.invoke_install_clicked("pacman".into());
                            next = true;
                        }
                    }
                    20 => {
                        if !ui.get_op_active() && ui.get_op_status() == "Done" {
                            // Finished: the bar is gone, the buttons wait for the reload.
                            assert!(ui.get_op_settling(), "buttons must wait after Finished");
                            let first = ui.get_app_page().sources.row_data(0).unwrap();
                            assert!(!first.installed, "the page is not reloaded yet");
                            ui.invoke_install_clicked("pacman".into());
                            println!("settling: status {:?}", ui.get_op_status());
                            save(&ui, &format!("{dir2}/fake-settling.png"))?;
                            next = true;
                        }
                    }
                    21 => {
                        if !ui.get_op_settling() {
                            let first = ui.get_app_page().sources.row_data(0).unwrap();
                            assert!(first.installed, "the page shows the result when it opens");
                            assert_eq!(
                                runs.load(Ordering::SeqCst),
                                runs_before.get() + 1,
                                "a click while settling must be ignored"
                            );
                            println!(
                                "settled after {:?}: installed, one run only",
                                mark.get().elapsed()
                            );
                            delay_ms.store(0, Ordering::SeqCst);
                            next = true;
                        }
                    }
                    22 => {
                        if settled {
                            save(&ui, &format!("{dir2}/fake-settled.png"))?;
                            next = true;
                        }
                    }
                    _ => {
                        let _ = slint::quit_event_loop();
                    }
                }
                Ok(())
            })();
            if next {
                idx.set(idx.get() + 1);
                mark.set(Instant::now());
            }
            if let Err(e) = result {
                eprintln!("failed: {e}");
                failed2.set(true);
                let _ = slint::quit_event_loop();
            }
        },
    );
    slint::run_event_loop()?;
    wiring::shutdown();
    if failed.get() {
        return Err("failed".into());
    }
    println!("finished");
    Ok(())
}
