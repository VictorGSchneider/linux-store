use std::process::ExitCode;

use store_core::{Host, Index, Store};

fn main() -> ExitCode {
    // args_os: an argument that is not valid UTF-8 must be a usage error, not a panic.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let code = store_cli::run(
        &args,
        |path| {
            Ok(Store::new(Index::open(path)?, store_system::backends()).with_host(Host::detect()))
        },
        &store_system::installer(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(code as u8)
}
