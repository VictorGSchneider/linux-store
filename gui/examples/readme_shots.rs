//! Development tool: the README screenshots of the home page and an app page, from the REAL
//! backends and the real wiring, headless, with nothing of this machine's own state in them:
//! every package is shown as not installed and without an update, and packages that exist only
//! locally (installed from outside the repositories) are left out. The installer is a FAKE and
//! no operation is started. Use a throwaway cache dir; the first refresh downloads the AUR dump:
//!
//! XDG_CACHE_HOME=/some/tmp SLINT_BACKEND=headless cargo run -p linux-store \
//!     --example readme_shots --features slint/mcp -- OUT_DIR [APP_QUERY]
//!
//! Writes OUT_DIR/{home,app-page}.png (1200x800). APP_QUERY (default "firefox") picks the app.
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use linux_store::{AppWindow, Page, wiring};
use slint::{ComponentHandle, Model};
use store_core::testing::FakeInstaller;
use store_core::{Backend, Host, InstallError, Package, SourceId};

/// A real backend whose packages are shown as nobody's: not installed, no update, and without
/// the packages only this machine has.
struct Public(Box<dyn Backend>);

impl Backend for Public {
    fn source(&self) -> SourceId {
        self.0.source()
    }
    fn is_available(&self) -> bool {
        self.0.is_available()
    }
    fn unavailable_reason(&self) -> Option<String> {
        self.0.unavailable_reason()
    }
    fn packages(&self) -> store_core::Result<Vec<Package>> {
        let pacman = self.0.source().as_str() == "pacman";
        Ok(self
            .0
            .packages()?
            .into_iter()
            // A pacman package with no repository is installed from elsewhere (the AUR, a file).
            .filter(|p| !pacman || p.repo.is_some())
            .map(|mut p| {
                p.installed_version = None;
                p.upgradable = false;
                p
            })
            .collect())
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
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("output directory");
    let query = args.next().unwrap_or_else(|| "firefox".into());
    std::fs::create_dir_all(&dir)?;
    let config = store_app::EngineConfig {
        db_path: store_system::default_db_path().expect("cache dir"),
        backends: store_system::backends()
            .into_iter()
            .map(|b| Box::new(Public(b)) as Box<dyn Backend>)
            .collect(),
        installer: Arc::new(FakeInstaller::new(vec![], Err(InstallError::Dismissed))),
        installable: vec![SourceId::new("pacman")],
        host: Host::detect(),
    };
    let ui = AppWindow::new()?;
    wiring::install(&ui, config)?;
    ui.window().set_size(slint::PhysicalSize::new(1200, 800));
    ui.show()?;

    let step = Rc::new(Cell::new(0usize));
    let mark = Rc::new(Cell::new(Instant::now()));
    let started = Instant::now();
    let failed = Rc::new(Cell::new(false));
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let (dir2, failed2) = (dir.clone(), failed.clone());
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(200),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            // Long enough for the icons of the cards on screen to be decoded.
            let settled = mark.get().elapsed() > Duration::from_secs(6);
            let mut next = false;
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                match step.get() {
                    0 => {
                        if wiring::refreshes_done() >= 1 {
                            println!("refresh done after {:?}", started.elapsed());
                            next = true;
                        } else if started.elapsed() > Duration::from_secs(900) {
                            return Err("the refresh did not end".into());
                        }
                    }
                    1 => {
                        if settled {
                            save(&ui, &format!("{dir2}/home.png"))?;
                            ui.set_query(query.as_str().into());
                            ui.invoke_search_edited(query.as_str().into());
                            next = true;
                        }
                    }
                    2 => {
                        if settled {
                            let first = ui.get_results().row_data(0).ok_or("no results")?;
                            println!("opening {} ({})", first.title, first.key);
                            ui.invoke_card_clicked(first.key);
                            next = true;
                        }
                    }
                    3 => {
                        if settled {
                            assert_eq!(ui.get_page(), Page::App);
                            save(&ui, &format!("{dir2}/app-page.png"))?;
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
                step.set(step.get() + 1);
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
    Ok(())
}
