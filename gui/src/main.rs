// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;
use std::sync::Arc;

use linux_store::{AppWindow, wiring};
use slint::ComponentHandle;
use store_core::{Host, SourceId};
use store_system::{backends, default_db_path, installer};

fn main() -> ExitCode {
    let Some(db_path) = default_db_path() else {
        eprintln!("linux-store: cannot find a cache directory (set HOME or XDG_CACHE_HOME)");
        return ExitCode::FAILURE;
    };
    let config = store_app::EngineConfig {
        db_path,
        backends: backends(),
        installer: Arc::new(installer()),
        installable: vec![SourceId::new("pacman")],
        host: Host::detect(),
    };
    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("linux-store: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: store_app::EngineConfig) -> Result<(), Box<dyn std::error::Error>> {
    let ui = AppWindow::new()?;
    wiring::install(&ui, config)?;
    ui.run()?;
    wiring::shutdown();
    Ok(())
}
