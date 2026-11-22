//! Development tool: drives the REAL wiring and engine headless with FAKE backends and a FAKE
//! installer (nothing touches the system) and snapshots the Installeds and Updates pages.
//!
//! XDG_CACHE_HOME=/some/tmp SLINT_BACKEND=headless cargo run -p linux-store \
//!     --example fake_lists --features slint/mcp -- OUT_DIR [populated|empty]
//!
//! populated: writes OUT_DIR/fake-{installed,updates,installed-back,updates-narrow}.png and
//! asserts that Back from an app opened on a list returns to that list.
//! empty: nothing installed, nothing to update; writes OUT_DIR/empty-{installed,updates}.png.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use linux_store::{AppWindow, Page, wiring};
use slint::{ComponentHandle, Model};
use store_core::testing::{FakeBackend, FakeInstaller, pkg};
use store_core::{Host, InstallError, SourceId};

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
    let empty = args.next().as_deref() == Some("empty");
    std::fs::create_dir_all(&dir)?;

    let mut packages = Vec::new();
    let mut flatpaks = Vec::new();
    if !empty {
        let mut htop = pkg("pacman", "htop", "Interactive process viewer");
        htop.installed_version = Some("3.2.0-1".into());
        htop.version = "3.3.0-1".into();
        htop.upgradable = true;
        let mut bash = pkg("pacman", "bash", "The GNU Bourne Again shell");
        bash.installed_version = Some("5.2-1".into());
        bash.version = "5.2-1".into();
        let mut vim = pkg("pacman", "vim", "Vi Improved text editor");
        vim.installed_version = Some("9.1-1".into());
        vim.version = "9.1.1-1".into();
        vim.upgradable = true;
        packages.extend([htop, bash, vim]);
        packages.push(pkg("pacman", "nano", "Pico clone editor"));
        let mut flat = pkg("flatpak", "org.gnome.Calculator", "Calculator");
        flat.title = Some("Calculator".into());
        flat.app_id = Some("org.gnome.Calculator".into());
        flat.installed_version = Some("46".into());
        flatpaks.push(flat);
    } else {
        packages.push(pkg("pacman", "nano", "Pico clone editor"));
    }

    let db = std::env::var("XDG_CACHE_HOME")?;
    let config = store_app::EngineConfig {
        db_path: std::path::PathBuf::from(db).join("fake-lists.db"),
        backends: vec![
            Box::new(FakeBackend::new("pacman", packages)),
            Box::new(FakeBackend::new("flatpak", flatpaks)),
        ],
        installer: Arc::new(FakeInstaller::new(vec![], Err(InstallError::Dismissed))),
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
    let failed = Rc::new(Cell::new(false));
    let first_key = Rc::new(RefCell::new(String::new()));
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
                match (empty, idx.get()) {
                    (_, 0) => {
                        if wiring::refreshes_done() >= 1 {
                            ui.invoke_show_installed();
                            assert_eq!(ui.get_page(), Page::Installed);
                            next = true;
                        }
                    }
                    (true, 1) => {
                        if settled {
                            assert_eq!(ui.get_installed().row_count(), 0);
                            save(&ui, &format!("{dir2}/empty-installed.png"))?;
                            ui.invoke_show_updates();
                            next = true;
                        }
                    }
                    (true, 2) => {
                        if settled {
                            assert_eq!(ui.get_updates().row_count(), 0);
                            save(&ui, &format!("{dir2}/empty-updates.png"))?;
                            next = true;
                        }
                    }
                    (false, 1) => {
                        if settled {
                            println!("installed: {} cards", ui.get_installed().row_count());
                            assert_eq!(ui.get_installed().row_count(), 4);
                            assert_eq!(ui.get_installed_count(), "(4)");
                            save(&ui, &format!("{dir2}/fake-installed.png"))?;
                            ui.invoke_show_updates();
                            next = true;
                        }
                    }
                    (false, 2) => {
                        if settled {
                            println!("updates: {} cards", ui.get_updates().row_count());
                            assert_eq!(ui.get_updates().row_count(), 2);
                            assert_eq!(ui.get_page(), Page::Updates);
                            save(&ui, &format!("{dir2}/fake-updates.png"))?;
                            // a late list answer must never switch the page
                            ui.invoke_show_home();
                            wiring::inject_event(&ui, store_app::Event::Installed(vec![]));
                            wiring::inject_event(&ui, store_app::Event::Updates(vec![]));
                            assert_eq!(ui.get_page(), Page::Home, "late lists keep the page");
                            // ask for both again, the engine's answers replace the injected ones
                            ui.invoke_show_installed();
                            ui.invoke_show_updates();
                            next = true;
                        }
                    }
                    (false, 3) => {
                        // the injected empty lists were replaced by the engine's answer
                        if settled && ui.get_updates().row_count() == 2 {
                            let card = ui.get_updates().row_data(0).unwrap();
                            *first_key.borrow_mut() = card.key.to_string();
                            ui.invoke_card_clicked(card.key);
                            next = true;
                        }
                    }
                    (false, 4) => {
                        if ui.get_page() == Page::App && settled {
                            ui.invoke_back();
                            assert_eq!(ui.get_page(), Page::Updates, "Back returns to Updates");
                            assert_eq!(ui.get_updates().row_count(), 2, "list contents kept");
                            ui.invoke_show_installed();
                            let card = ui.get_installed().row_data(0).unwrap();
                            ui.invoke_card_clicked(card.key);
                            next = true;
                        }
                    }
                    (false, 5) => {
                        if ui.get_page() == Page::App && settled {
                            ui.invoke_back();
                            assert_eq!(
                                ui.get_page(),
                                Page::Installed,
                                "Back returns to Installeds"
                            );
                            assert_eq!(ui.get_installed().row_count(), 4);
                            println!("back from an app returns to the list it came from");
                            save(&ui, &format!("{dir2}/fake-installed-back.png"))?;
                            ui.invoke_show_updates();
                            ui.window().set_size(slint::PhysicalSize::new(500, 700));
                            next = true;
                        }
                    }
                    (false, 6) => {
                        if settled {
                            save(&ui, &format!("{dir2}/fake-updates-narrow.png"))?;
                            // from Home, Back goes Home
                            ui.invoke_show_home();
                            let card = ui.get_rows().row_data(0).and_then(|r| r.cards.row_data(0));
                            if let Some(card) = card {
                                ui.invoke_card_clicked(card.key);
                            }
                            next = true;
                        }
                    }
                    (false, 7) => {
                        if settled {
                            if ui.get_page() == Page::App {
                                ui.invoke_back();
                            }
                            assert_eq!(ui.get_page(), Page::Home, "Back from Home's app is Home");
                            println!("back from an app opened on Home returns Home");
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
