//! `store`: a small CLI over `store-core`: refresh, search, list, and install, remove or upgrade.
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use std::collections::HashMap;

use clap::{Parser, Subcommand};
use store_core::{CancelToken, InstallError, Installer, Outcome, Package, Store};
use store_protocol::{ChangeKind, Event, Operation, Request, Stage, is_valid_name};

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
        /// How many results to show for each source
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// One package per line with its description, instead of a column per source
        #[arg(long)]
        plain: bool,
        /// Total width of the columns (default: the terminal's width, else 100)
        #[arg(long)]
        width: Option<usize>,
    },
    /// List installed packages.
    Installed,
    /// Install packages from the configured repositories (asks for your password).
    Install {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Remove installed packages (asks for your password).
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Refresh the package databases and upgrade the whole system (asks for your password).
    Upgrade,
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

/// Text from a repository or from the helper is untrusted: a control character (an escape
/// sequence, a carriage return, a bell) must never reach the terminal as such.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Like [`printable`] for multi-line messages: newlines and tabs are kept.
fn printable_block(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '?'
            } else {
                c
            }
        })
        .collect()
}

fn format_package(p: &Package) -> String {
    let installed = if p.is_installed() { " [installed]" } else { "" };
    let title = p.title.as_deref().map(printable);
    let description = p.description.as_deref().map(printable);
    let detail = match (title, description) {
        (Some(title), Some(description)) => format!(" — {title}: {description}"),
        (Some(title), None) => format!(" — {title}"),
        (None, Some(description)) => format!(" — {description}"),
        (None, None) => String::new(),
    };
    format!(
        "{}/{} {} {}{}{}",
        printable(&p.source.to_string()),
        printable(p.repo.as_deref().unwrap_or("-")),
        printable(&p.name),
        printable(&p.version),
        installed,
        detail
    )
}

/// Runs the CLI. `args` is the full argv (program name first). `make_store` builds the store
/// from the resolved index path. Returns the process exit code.
pub fn run<W: Write, E: Write>(
    args: &[OsString],
    make_store: impl FnOnce(&Path) -> store_core::Result<Store>,
    installer: &dyn Installer,
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

    // Changing the system never needs the search index, so it is not even opened.
    let request = match &cli.command {
        Command::Install { packages } => Some(Request::Install {
            packages: packages.clone(),
        }),
        Command::Remove { packages } => Some(Request::Remove {
            packages: packages.clone(),
        }),
        Command::Upgrade => Some(Request::Upgrade),
        _ => None,
    };
    if let Some(request) = request {
        return run_installer(&request, installer, out, err);
    }

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
                    Outcome::Unavailable { reason: None } => {
                        writeln!(out, "{}: unavailable", s.source)
                    }
                    Outcome::Unavailable {
                        reason: Some(reason),
                    } => {
                        writeln!(
                            out,
                            "{}: unavailable ({})",
                            s.source,
                            printable_block(reason)
                        )
                    }
                    Outcome::Failed(message) => {
                        writeln!(out, "{}: failed: {}", s.source, printable_block(message))
                    }
                };
            }
            let all_failed = !report.sources.is_empty()
                && report
                    .sources
                    .iter()
                    .all(|s| matches!(s.outcome, Outcome::Failed(_)));
            i32::from(all_failed)
        }
        Command::Search {
            query,
            limit,
            plain: true,
            ..
        } => print_packages(store.search(&query.join(" "), limit), out, err),
        Command::Search {
            query,
            limit,
            plain: false,
            width,
        } => match store.search_by_source(&query.join(" "), limit) {
            Ok(groups) => {
                let width = width.unwrap_or_else(terminal_width);
                let _ = write!(out, "{}", render_columns(&groups, width));
                0
            }
            Err(e) => {
                let _ = writeln!(err, "store: {}", printable_block(&e.to_string()));
                1
            }
        },
        Command::Installed => print_packages(store.installed(), out, err),
        Command::Install { .. } | Command::Remove { .. } | Command::Upgrade => {
            unreachable!("handled before the index is opened")
        }
    }
}

fn operation_name(op: Operation) -> &'static str {
    match op {
        Operation::Install => "install",
        Operation::Remove => "remove",
        Operation::Upgrade => "upgrade",
    }
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Refresh => "refresh",
        Stage::Resolve => "resolve",
        Stage::Download => "download",
        Stage::Verify => "verify",
        Stage::Hooks => "hooks",
        Stage::Install => "install",
        Stage::Upgrade => "upgrade",
        Stage::Remove => "remove",
    }
}

/// One line for an event, or `None` for the ones reported another way (errors come back as the
/// result of the run and are printed once, on stderr).
fn describe(event: &Event) -> Option<String> {
    match event {
        Event::Started { op } => Some(format!("started {}", operation_name(*op))),
        Event::Progress {
            stage,
            package,
            percent,
        } => {
            let mut line = format!("[{}]", stage_name(*stage));
            if let Some(package) = package {
                line.push_str(&format!(" {}", printable(package)));
            }
            if let Some(percent) = percent {
                line.push_str(&format!(" {percent}%"));
            }
            Some(line)
        }
        Event::Done { changes } if changes.is_empty() => Some("done: nothing to do".to_string()),
        Event::Done { changes } => {
            let parts: Vec<String> = changes
                .iter()
                .map(|c| {
                    let verb = match c.kind {
                        ChangeKind::Installed => "installed",
                        ChangeKind::Removed => "removed",
                        ChangeKind::Upgraded => "upgraded",
                    };
                    match &c.version {
                        Some(version) => {
                            format!("{verb} {} {}", printable(&c.package), printable(version))
                        }
                        None => format!("{verb} {}", printable(&c.package)),
                    }
                })
                .collect();
            Some(format!("done: {}", parts.join(", ")))
        }
        Event::Error { .. } => None,
        Event::CancelIgnored => Some("too late to cancel; finishing the operation".to_string()),
    }
}

/// Decides which progress events are worth a line: the first one for a step, then one per ten
/// percent, and the final 100%. Without this a removal prints a line for every single percent.
#[derive(Default)]
struct ProgressFilter {
    last_percent: HashMap<String, u8>,
}

impl ProgressFilter {
    fn wanted(&mut self, event: &Event) -> bool {
        let Event::Progress {
            stage,
            package,
            percent: Some(percent),
        } = event
        else {
            return true;
        };
        let key = format!(
            "{}/{}",
            stage_name(*stage),
            package.as_deref().unwrap_or("")
        );
        let show = match self.last_percent.get(&key) {
            None => true,
            Some(last) => (*percent == 100 && *last != 100) || percent / 10 != last / 10,
        };
        if show {
            self.last_percent.insert(key, *percent);
        }
        show
    }
}

/// Runs an install, remove or upgrade request through the installer and prints its progress.
fn run_installer<W: Write, E: Write>(
    request: &Request,
    installer: &dyn Installer,
    out: &mut W,
    err: &mut E,
) -> i32 {
    // A bad name is refused before the helper starts, so it never costs a password prompt.
    if let Request::Install { packages } | Request::Remove { packages } = request
        && let Some(bad) = packages.iter().find(|name| !is_valid_name(name))
    {
        let _ = writeln!(
            err,
            "store: invalid package name {:?} (names use lowercase letters, digits and @ . _ + -)",
            printable(bad)
        );
        return 1;
    }
    let mut last_line = String::new();
    let mut filter = ProgressFilter::default();
    let result = installer.run(request, &CancelToken::default(), &mut |event| {
        if !filter.wanted(&event) {
            return;
        }
        if let Some(line) = describe(&event)
            && line != last_line
        {
            let _ = writeln!(out, "{line}");
            last_line = line;
        }
    });
    match result {
        Ok(()) => 0,
        Err(InstallError::Dismissed) => {
            let _ = writeln!(err, "store: {}", InstallError::Dismissed);
            126
        }
        Err(InstallError::NotAuthorized(text)) => {
            let _ = writeln!(
                err,
                "store: not authorized: {}\n\
                 If no dialog appeared, a polkit authentication agent may not be running in your session.",
                printable_block(&text)
            );
            127
        }
        Err(e) => {
            let _ = writeln!(err, "store: {}", printable_block(&e.to_string()));
            1
        }
    }
}

fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(width, _)| usize::from(width.0))
        .filter(|w| *w >= 20)
        .unwrap_or(100)
}

/// Shortens `text` to `width` characters, ending in `…` when something was cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// One column per source, side by side: a header, a rule, then `name version` for each package
/// (`✓` marks an installed one). No line is wider than `width`.
fn render_columns(groups: &[(store_core::SourceId, Vec<Package>)], width: usize) -> String {
    const GAP: &str = "  ";
    if groups.is_empty() {
        return String::new();
    }
    let gaps = GAP.chars().count() * (groups.len() - 1);
    let column = (width.saturating_sub(gaps) / groups.len()).max(8);
    let rows = groups
        .iter()
        .map(|(_, found)| found.len())
        .max()
        .unwrap_or(0);
    let cell = |text: &str| format!("{:<column$}", fit(text, column));
    let mut lines = Vec::with_capacity(rows + 2);
    let header = |g: &(store_core::SourceId, Vec<Package>)| {
        cell(&printable(&format!("{} ({})", g.0, g.1.len())))
    };
    lines.push(groups.iter().map(header).collect::<Vec<_>>().join(GAP));
    lines.push(
        groups
            .iter()
            .map(|_| "─".repeat(column))
            .collect::<Vec<_>>()
            .join(GAP),
    );
    for row in 0..rows {
        let cells: Vec<String> = groups
            .iter()
            .map(|(_, found)| match found.get(row) {
                Some(p) => {
                    let mark = if p.is_installed() { " ✓" } else { "" };
                    let tail = format!(" {}{mark}", printable(&p.version));
                    // Keep the version visible: shorten the name first.
                    let name_room = column.saturating_sub(tail.chars().count()).max(4);
                    cell(&format!("{}{}", fit(&printable(&p.name), name_room), tail))
                }
                None => cell(""),
            })
            .collect();
        lines.push(cells.join(GAP));
    }
    let mut text = lines
        .iter()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    text
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
            let _ = writeln!(err, "store: {}", printable_block(&e.to_string()));
            1
        }
    }
}
