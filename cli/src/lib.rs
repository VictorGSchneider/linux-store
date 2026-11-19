//! `store`: a small CLI over `store-core`, used to refresh, search and list packages.
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use store_core::{Outcome, Package, Store};

#[derive(Parser)]
#[command(
    name = "store",
    version,
    about = "Search and list packages from every source"
)]
struct Cli {
    /// Path of the index database (default: the user cache directory).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Reload the index from every available source.
    Refresh,
    /// Search the index.
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List installed packages.
    Installed,
}

/// `$XDG_CACHE_HOME/linux-store/index.db`, else `$HOME/.cache/linux-store/index.db`.
pub fn default_db_path_from(
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    let cache = match xdg_cache_home.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|v| !v.is_empty())?).join(".cache"),
    };
    Some(cache.join("linux-store").join("index.db"))
}

pub fn default_db_path() -> Option<PathBuf> {
    default_db_path_from(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

fn format_package(p: &Package) -> String {
    let installed = if p.is_installed() { " [installed]" } else { "" };
    let description = p
        .description
        .as_deref()
        .map(|d| format!(" — {d}"))
        .unwrap_or_default();
    format!(
        "{}/{} {} {}{}{}",
        p.source,
        p.repo.as_deref().unwrap_or("-"),
        p.name,
        p.version,
        installed,
        description
    )
}

/// Runs the CLI. `args` is the full argv (program name first). `make_store` builds the store
/// from the resolved index path. Returns the process exit code.
pub fn run<W: Write, E: Write>(
    args: &[String],
    make_store: impl FnOnce(&Path) -> store_core::Result<Store>,
    out: &mut W,
    err: &mut E,
) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let code = e.exit_code();
            let rendered = e.render().to_string();
            let _ = if e.use_stderr() {
                write!(err, "{rendered}")
            } else {
                write!(out, "{rendered}")
            };
            return code;
        }
    };

    let Some(db_path) = cli.db.or_else(default_db_path) else {
        let _ = writeln!(
            err,
            "store: cannot find a cache directory; pass --db <path>"
        );
        return 1;
    };
    if let Some(parent) = db_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        let _ = writeln!(
            err,
            "store: cannot open index at {}: {e}",
            db_path.display()
        );
        return 1;
    }
    let mut store = match make_store(&db_path) {
        Ok(store) => store,
        Err(e) => {
            let _ = writeln!(
                err,
                "store: cannot open index at {}: {e}",
                db_path.display()
            );
            return 1;
        }
    };

    match cli.command {
        Command::Refresh => {
            let report = store.refresh();
            for s in &report.sources {
                let _ = match &s.outcome {
                    Outcome::Updated { packages } => {
                        writeln!(out, "{}: {packages} packages", s.source)
                    }
                    Outcome::Unavailable => writeln!(out, "{}: unavailable", s.source),
                    Outcome::Failed(message) => writeln!(out, "{}: failed: {message}", s.source),
                };
            }
            let all_failed = !report.sources.is_empty()
                && report
                    .sources
                    .iter()
                    .all(|s| matches!(s.outcome, Outcome::Failed(_)));
            i32::from(all_failed)
        }
        Command::Search { query, limit } => {
            print_packages(store.search(&query.join(" "), limit), out, err)
        }
        Command::Installed => print_packages(store.installed(), out, err),
    }
}

fn print_packages<W: Write, E: Write>(
    result: store_core::Result<Vec<Package>>,
    out: &mut W,
    err: &mut E,
) -> i32 {
    match result {
        Ok(packages) => {
            for p in &packages {
                let _ = writeln!(out, "{}", format_package(p));
            }
            0
        }
        Err(e) => {
            let _ = writeln!(err, "store: {e}");
            1
        }
    }
}
