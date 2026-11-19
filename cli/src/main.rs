use std::process::ExitCode;

use store_backend_pacman::PacmanBackend;
use store_core::{Index, Store};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let code = store_cli::run(
        &args,
        |path| Ok(Store::new(Index::open(path)?, vec![Box::new(PacmanBackend::from_system())])),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(code as u8)
}
