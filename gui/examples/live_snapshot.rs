//! Development tool: runs the REAL wiring and engine headless and snapshots the window.
//! Use a throwaway cache dir, the first refresh downloads the AUR dump:
//!
//! XDG_CACHE_HOME=/some/tmp SLINT_BACKEND=headless cargo run -p linux-store \
//!     --example live_snapshot --features slint/mcp -- OUT_DIR [QUERY] [SOURCE]
//!
//! Writes OUT_DIR/{home,search,filtered,page,page-narrow,installed-page}.png. Operations use a FAKE
//! installer (never the real one) and none is started.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use linux_store::{AppWindow, wiring};
use slint::{ComponentHandle, Model};
use store_core::testing::FakeInstaller;
use store_core::{Host, InstallError, SourceId};

enum Step {
    WaitRefresh,
    Settle(Instant, &'static str),
    Search(Instant),
    Filter(Instant),
    Page(Instant, &'static str),
    Done,
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
    let source = args.next().unwrap_or_else(|| "flatpak".into());
    let installed_query = args.next().unwrap_or_else(|| "bash".into());
    std::fs::create_dir_all(&dir)?;
    let db_path = store_system::default_db_path().expect("cache dir");
    println!("index: {}", db_path.display());
    let config = store_app::EngineConfig {
        db_path,
        backends: store_system::backends(),
        installer: Arc::new(FakeInstaller::new(vec![], Err(InstallError::Dismissed))),
        installable: vec![SourceId::new("pacman")],
        host: Host::detect(),
    };
    let ui = AppWindow::new()?;
    wiring::install(&ui, config)?;
    ui.window().set_size(slint::PhysicalSize::new(1200, 800));
    ui.show()?;

    let step = Rc::new(RefCell::new(Step::WaitRefresh));
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
            let mut step = step.borrow_mut();
            let wait = Duration::from_secs(6);
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                match &*step {
                    Step::WaitRefresh => {
                        if wiring::refreshes_done() >= 1
                            || started.elapsed() > Duration::from_secs(600)
                        {
                            println!("refresh done after {:?}", started.elapsed());
                            *step = Step::Settle(Instant::now(), "home");
                        }
                    }
                    Step::Settle(t, name) => {
                        if t.elapsed() > wait {
                            save(&ui, &format!("{dir2}/{name}.png"))?;
                            *step = if *name == "home" {
                                ui.set_query(query.as_str().into());
                                ui.invoke_search_edited(query.as_str().into());
                                Step::Search(Instant::now())
                            } else {
                                Step::Done
                            };
                        }
                    }
                    Step::Search(t) => {
                        if t.elapsed() > wait {
                            save(&ui, &format!("{dir2}/search.png"))?;
                            ui.invoke_chip_clicked(source.as_str().into());
                            *step = Step::Filter(Instant::now());
                        }
                    }
                    Step::Filter(t) => {
                        if t.elapsed() > wait {
                            save(&ui, &format!("{dir2}/filtered.png"))?;
                            ui.invoke_chip_clicked(source.as_str().into());
                            *step = Step::Page(Instant::now(), "page");
                        }
                    }
                    Step::Page(t, name) => {
                        if t.elapsed() > wait {
                            let name = *name;
                            if name == "page" {
                                // open the first search result (the filter is off again)
                                let first = ui.get_results().row_data(0).ok_or("no results")?;
                                println!("opening {} ({})", first.title, first.key);
                                ui.invoke_card_clicked(first.key);
                                *step = Step::Page(Instant::now(), "page-open");
                            } else if name == "page-open" {
                                save(&ui, &format!("{dir2}/page.png"))?;
                                ui.window().set_size(slint::PhysicalSize::new(480, 800));
                                *step = Step::Page(Instant::now(), "page-narrow");
                            } else if name == "page-narrow" {
                                save(&ui, &format!("{dir2}/page-narrow.png"))?;
                                ui.window().set_size(slint::PhysicalSize::new(1200, 800));
                                ui.invoke_back();
                                ui.set_query(installed_query.as_str().into());
                                ui.invoke_search_edited(installed_query.as_str().into());
                                *step = Step::Page(Instant::now(), "installed-search");
                            } else if name == "installed-search" {
                                let first = ui.get_results().row_data(0).ok_or("no results")?;
                                println!("opening {} ({})", first.title, first.key);
                                ui.invoke_card_clicked(first.key);
                                *step = Step::Page(Instant::now(), "installed-open");
                            } else {
                                save(&ui, &format!("{dir2}/installed-page.png"))?;
                                *step = Step::Done;
                            }
                        }
                    }
                    Step::Done => {
                        let _ = slint::quit_event_loop();
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("snapshot failed: {e}");
                failed2.set(true);
                let _ = slint::quit_event_loop();
            }
        },
    );
    slint::run_event_loop()?;
    wiring::shutdown();
    if failed.get() {
        return Err("snapshot failed".into());
    }
    Ok(())
}
