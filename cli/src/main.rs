use std::process::ExitCode;

use store_backend_aur::AurBackend;
use store_backend_flatpak::FlatpakBackend;
use store_backend_pacman::PacmanBackend;
use store_core::{Backend, Index, Store};
use store_installer_pkexec::PkexecInstaller;

fn main() -> ExitCode {
    // args_os: an argument that is not valid UTF-8 must be a usage error, not a panic.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let code = store_cli::run(
        &args,
        |path| {
            let backends: Vec<Box<dyn Backend>> = vec![
                Box::new(PacmanBackend::from_system()),
                Box::new(FlatpakBackend::from_system()),
                Box::new(AurBackend::new()),
            ];
            Ok(Store::new(Index::open(path)?, backends))
        },
        &PkexecInstaller::system(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(code as u8)
}
