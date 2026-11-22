//! Development tool: runs the REAL wiring and engine headless and snapshots the window.
//! Use a throwaway cache dir, the first refresh downloads the AUR dump:
//!
//! XDG_CACHE_HOME=/some/tmp SLINT_BACKEND=headless cargo run -p linux-store \
//!     --example live_snapshot --features slint/mcp -- OUT_DIR [QUERY] [SOURCE]
//!
//! Writes OUT_DIR/{home,search,filtered}.png. Does nothing that installs.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use linux_store::{AppWindow, wiring};
use slint::ComponentHandle;
use store_core::{Host, SourceId};

enum Step {
    WaitRefresh,
    Settle(Instant, &'static str),
    Search(Instant),
    Filter(Instant),
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
    std::fs::create_dir_all(&dir)?;
    let db_path = store_system::default_db_path().expect("cache dir");
    println!("index: {}", db_path.display());
    let config = store_app::EngineConfig {
        db_path,
        backends: store_system::backends(),
        installer: Arc::new(store_system::installer()),
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
                            *step = Step::Done;
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
