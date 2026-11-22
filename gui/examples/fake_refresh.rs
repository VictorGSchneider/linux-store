//! Development tool: drives the REAL wiring and engine headless with FAKE backends whose
//! collections the tool releases one by one (nothing touches the system), and checks that the
//! screen follows each source as it finishes instead of waiting for the slowest one.
//!
//! XDG_CACHE_HOME=/some/tmp SLINT_BACKEND=headless cargo run -p linux-store \
//!     --example fake_refresh --features slint/mcp -- OUT_DIR [first-run|seeded]
//!
//! first-run (empty index): writes OUT_DIR/refresh-{empty,one,two,done}.png and asserts that the
//! home shows "Building the package index…" first, then gains the pacman row and chip, then the
//! flatpak ones, both while the slow AUR is still running.
//! seeded (an index from an earlier run): writes OUT_DIR/seeded-{updating,done}.png and asserts
//! that every chip shows "updating" until its source is reloaded.
use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use linux_store::{AppWindow, wiring};
use slint::{ComponentHandle, Model};
use store_core::testing::{FakeBackend, FakeInstaller, pkg};
use store_core::{Backend, Host, Index, InstallError, Package, SourceId, Store};

/// A backend whose `packages()` waits until the tool releases it (at most 60 s).
struct Gated {
    source: &'static str,
    packages: Vec<Package>,
    gate: Mutex<Receiver<()>>,
}

impl Backend for Gated {
    fn source(&self) -> SourceId {
        SourceId::new(self.source)
    }
    fn is_available(&self) -> bool {
        true
    }
    fn unavailable_reason(&self) -> Option<String> {
        None
    }
    fn packages(&self) -> store_core::Result<Vec<Package>> {
        let _ = self
            .gate
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(60));
        Ok(self.packages.clone())
    }
}

fn gated(source: &'static str, packages: Vec<Package>) -> (Box<dyn Backend>, Sender<()>) {
    let (release, gate) = channel();
    let backend = Gated {
        source,
        packages,
        gate: Mutex::new(gate),
    };
    (Box::new(backend), release)
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

fn sources() -> [(&'static str, Vec<Package>); 3] {
    let named = |source: &str, names: &[&str]| -> Vec<Package> {
        names
            .iter()
            .map(|n| pkg(source, n, "A sample package"))
            .collect()
    };
    [
        ("pacman", named("pacman", &["htop", "nano", "vim", "tree"])),
        ("flatpak", named("flatpak", &["org.example.Paint"])),
        ("aur", named("aur", &["yay", "paru"])),
    ]
}

/// (source, updating) of every chip on screen.
fn chips(ui: &AppWindow) -> Vec<(String, bool)> {
    ui.get_chips()
        .iter()
        .map(|c| (c.source.to_string(), c.updating))
        .collect()
}

fn rows(ui: &AppWindow) -> Vec<String> {
    ui.get_rows().iter().map(|r| r.title.to_string()).collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("output directory");
    let seeded = args.next().as_deref() == Some("seeded");
    std::fs::create_dir_all(&dir)?;
    let db = std::path::PathBuf::from(std::env::var("XDG_CACHE_HOME")?).join("fake-refresh.db");
    let _ = std::fs::remove_file(&db);
    if seeded {
        // The index an earlier run left behind.
        let backends: Vec<Box<dyn Backend>> = sources()
            .into_iter()
            .map(|(s, p)| Box::new(FakeBackend::new(s, p)) as Box<dyn Backend>)
            .collect();
        Store::new(Index::open(&db)?, backends).refresh();
    }
    let mut backends = Vec::new();
    let mut releases = Vec::new();
    for (source, packages) in sources() {
        let (backend, release) = gated(source, packages);
        backends.push(backend);
        releases.push(release);
    }
    let config = store_app::EngineConfig {
        db_path: db,
        backends,
        installer: Arc::new(FakeInstaller::new(vec![], Err(InstallError::Dismissed))),
        installable: vec![SourceId::new("pacman")],
        host: Host::with_native("pacman"),
    };
    let ui = AppWindow::new()?;
    wiring::install(&ui, config)?;
    ui.window().set_size(slint::PhysicalSize::new(1000, 700));
    ui.show()?;

    let idx = Rc::new(Cell::new(0usize));
    let mark = Rc::new(Cell::new(Instant::now()));
    let failed = Rc::new(Cell::new(false));
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let (dir2, failed2) = (dir.clone(), failed.clone());
    let name = if seeded { "seeded" } else { "refresh" };
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let settled = mark.get().elapsed() > Duration::from_millis(800);
            let mut next = false;
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                match (seeded, idx.get()) {
                    (false, 0) => {
                        if settled {
                            // Nothing collected yet: an empty home that says why.
                            assert!(ui.get_refreshing());
                            assert!(rows(&ui).is_empty());
                            assert!(chips(&ui).is_empty());
                            save(&ui, &format!("{dir2}/{name}-empty.png"))?;
                            releases[0].send(())?;
                            next = true;
                        }
                    }
                    (false, 1) => {
                        if rows(&ui) == ["pacman"] && settled {
                            assert_eq!(chips(&ui), [("pacman".to_string(), false)]);
                            assert_eq!(wiring::refreshes_done(), 0, "the pass is still running");
                            println!("pacman shown after {:?}", mark.get().elapsed());
                            save(&ui, &format!("{dir2}/{name}-one.png"))?;
                            releases[1].send(())?;
                            next = true;
                        }
                    }
                    (false, 2) => {
                        if rows(&ui) == ["pacman", "flatpak"] && settled {
                            assert_eq!(chips(&ui).len(), 2);
                            assert_eq!(wiring::refreshes_done(), 0, "the AUR still runs");
                            assert!(ui.get_refreshing());
                            save(&ui, &format!("{dir2}/{name}-two.png"))?;
                            releases[2].send(())?;
                            next = true;
                        }
                    }
                    (true, 0) => {
                        if settled {
                            // The old index shows at once; every chip says it is updating.
                            assert_eq!(rows(&ui), ["pacman", "flatpak", "aur"]);
                            assert!(chips(&ui).iter().all(|(_, updating)| *updating));
                            save(&ui, &format!("{dir2}/{name}-updating.png"))?;
                            releases[0].send(())?;
                            next = true;
                        }
                    }
                    (true, 1) => {
                        if settled {
                            let c = chips(&ui);
                            assert_eq!(c[0], ("pacman".to_string(), false), "{c:?}");
                            assert!(c[1].1 && c[2].1, "{c:?}");
                            releases[1].send(())?;
                            releases[2].send(())?;
                            next = true;
                        }
                    }
                    (true, 2) => next = true,
                    (_, 3) => {
                        if wiring::refreshes_done() >= 1 && settled {
                            assert_eq!(rows(&ui), ["pacman", "flatpak", "aur"]);
                            assert!(chips(&ui).iter().all(|(_, updating)| !updating));
                            assert!(!ui.get_refreshing());
                            save(&ui, &format!("{dir2}/{name}-done.png"))?;
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
