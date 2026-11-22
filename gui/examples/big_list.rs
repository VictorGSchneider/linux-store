//! Development tool: a huge Installeds list (FAKE backend, nothing touches the system) scrolled
//! to its true end and rendered at several scale factors and widths, to check that the software
//! renderer's i16 coordinates are never exceeded and that the last cards are drawn. Also drives
//! the keyboard (Tab, PageDown, End, Home) and asserts what moves.
//!
//! XDG_CACHE_HOME=/some/tmp SLINT_BACKEND=headless cargo run -p linux-store \
//!     --example big_list --features slint/mcp -- OUT_DIR [COUNT]
//!
//! Writes OUT_DIR/big-{wide-1x,wide-2x,narrow-1x,narrow-2x}-end.png, big-keys-*.png.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use linux_store::{AppWindow, Page, wiring};
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Model};
use store_core::testing::{FakeBackend, FakeInstaller, pkg};
use store_core::{Host, InstallError, SourceId};

fn save(ui: &AppWindow, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let img = ui.window().take_snapshot()?;
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, img.width(), img.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(img.as_bytes())?;
    println!("wrote {path} ({}x{})", img.width(), img.height());
    Ok(())
}

fn press(ui: &AppWindow, key: Key) {
    let text: slint::SharedString = key.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

fn scroll_end(ui: &AppWindow) {
    ui.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(300.0, 500.0),
        delta_x: 0.0,
        delta_y: -10_000_000.0,
    });
}

fn scroll_home(ui: &AppWindow) {
    ui.window().dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(300.0, 500.0),
        delta_x: 0.0,
        delta_y: 10_000_000.0,
    });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("output directory");
    let count: usize = args.next().map_or(3000, |c| c.parse().expect("count"));
    std::fs::create_dir_all(&dir)?;

    let packages: Vec<_> = (0..count)
        .map(|i| {
            let mut p = pkg(
                "pacman",
                &format!("pkg{i:05}"),
                &format!("Package number {i}"),
            );
            p.installed_version = Some("1.0-1".into());
            p
        })
        .collect();
    let db = std::env::var("XDG_CACHE_HOME")?;
    let config = store_app::EngineConfig {
        db_path: std::path::PathBuf::from(db).join("big-list.db"),
        backends: vec![Box::new(FakeBackend::new("pacman", packages))],
        installer: Arc::new(FakeInstaller::new(vec![], Err(InstallError::Dismissed))),
        installable: vec![SourceId::new("pacman")],
        host: Host::with_native("pacman"),
    };
    let _ = std::fs::remove_file(&config.db_path);
    let ui = AppWindow::new()?;
    wiring::install(&ui, config)?;
    ui.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
    ui.show()?;

    let idx = Rc::new(Cell::new(0usize));
    let mark = Rc::new(Cell::new(Instant::now()));
    let failed = Rc::new(Cell::new(false));
    let last_title = Rc::new(RefCell::new(String::new()));
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let (dir2, failed2) = (dir.clone(), failed.clone());
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let settled = mark.get().elapsed() > Duration::from_millis(800);
            let mut next = false;
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                match idx.get() {
                    0 => {
                        if wiring::refreshes_done() >= 1 {
                            ui.invoke_show_installed();
                            next = true;
                        }
                    }
                    1 => {
                        if settled && ui.get_installed().row_count() == count {
                            println!("installed: {count} cards");
                            *last_title.borrow_mut() = ui
                                .get_installed()
                                .row_data(count - 1)
                                .unwrap()
                                .title
                                .to_string();
                            scroll_end(&ui);
                            next = true;
                        }
                    }
                    2 => {
                        if settled {
                            save(&ui, &format!("{dir2}/big-wide-1x-end.png"))?;
                            ui.window().set_size(slint::LogicalSize::new(500.0, 800.0));
                            scroll_end(&ui);
                            next = true;
                        }
                    }
                    3 => {
                        if settled {
                            save(&ui, &format!("{dir2}/big-narrow-1x-end.png"))?;
                            ui.window().set_size(slint::LogicalSize::new(1200.0, 800.0));
                            scroll_home(&ui);
                            next = true;
                        }
                    }
                    4 => {
                        if settled {
                            // Tab until the list takes the focus (one stop), then walk it
                            let mut tabs = 0;
                            while ui.get_list_selected() < 0 && tabs < 12 {
                                press(&ui, Key::Tab);
                                tabs += 1;
                            }
                            println!("tabs to reach the list: {tabs}");
                            assert_eq!(ui.get_list_selected(), 0, "Tab must reach the list");
                            for _ in 0..40 {
                                press(&ui, Key::RightArrow);
                            }
                            assert_eq!(ui.get_list_selected(), 40);
                            next = true;
                        }
                    }
                    5 => {
                        if settled {
                            save(&ui, &format!("{dir2}/big-keys-right40.png"))?;
                            for _ in 0..3 {
                                press(&ui, Key::PageDown);
                            }
                            let sel = ui.get_list_selected();
                            println!("after 3x PageDown: {sel}");
                            assert!(sel > 80, "PageDown moves a screenful");
                            next = true;
                        }
                    }
                    6 => {
                        if settled {
                            save(&ui, &format!("{dir2}/big-keys-pagedown.png"))?;
                            press(&ui, Key::End);
                            assert_eq!(ui.get_list_selected() as usize, count - 1);
                            next = true;
                        }
                    }
                    7 => {
                        if settled {
                            save(&ui, &format!("{dir2}/big-keys-end.png"))?;
                            press(&ui, Key::Home);
                            assert_eq!(ui.get_list_selected(), 0);
                            for _ in 0..8 {
                                press(&ui, Key::DownArrow);
                            }
                            next = true;
                        }
                    }
                    8 => {
                        if settled {
                            save(&ui, &format!("{dir2}/big-keys-down.png"))?;
                            press(&ui, Key::Return);
                            next = true;
                        }
                    }
                    9 => {
                        if settled {
                            assert_eq!(ui.get_page(), Page::App, "Return opens the selection");
                            println!("Return opened {}", ui.get_app_page().title);
                            ui.invoke_back();
                            assert_eq!(ui.get_page(), Page::Installed);
                            assert_eq!(ui.get_list_selected(), -1);
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
