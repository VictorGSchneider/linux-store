//! The steps every operation shares: open a session, run one transaction, map errors.
use std::cell::RefCell;
use std::rc::Rc;

use alpm::{Alpm, PrepareData, PrepareError, TransFlag};
use store_protocol::{Change, ChangeKind, ErrorCode};

use crate::events::{self, QuestionLog};
use crate::{CancelGate, Emitter, Environment, HelperError};

/// Opens libalpm and wires the callbacks. Nothing is touched until a transaction starts.
pub(crate) fn open(
    env: &Environment,
    emitter: &Emitter,
    gate: &CancelGate,
) -> Result<(Alpm, Rc<RefCell<QuestionLog>>), HelperError> {
    let alpm = env.open()?;
    let questions = events::attach(&alpm, emitter, gate);
    Ok((alpm, questions))
}

pub(crate) fn map_error(error: alpm::Error, context: &str) -> HelperError {
    let code = match error {
        alpm::Error::HandleLock => ErrorCode::Locked,
        alpm::Error::ConflictingDeps | alpm::Error::FileConflicts => ErrorCode::Conflict,
        alpm::Error::UnsatisfiedDeps => ErrorCode::DependencyBroken,
        _ => ErrorCode::Failed,
    };
    let message = if code == ErrorCode::Locked {
        format!(
            "{context}: {error}; another package manager may be running, and the helper never removes the lock file"
        )
    } else {
        format!("{context}: {error}")
    };
    HelperError::new(code, message)
}

/// Runs one transaction: `build` adds its targets, then prepare, commit, release. The
/// transaction is always released. Returns what changed.
pub(crate) fn run(
    alpm: &mut Alpm,
    flags: TransFlag,
    questions: &Rc<RefCell<QuestionLog>>,
    gate: &CancelGate,
    build: impl FnOnce(&mut Alpm) -> Result<(), HelperError>,
) -> Result<Vec<Change>, HelperError> {
    alpm.trans_init(flags)
        .map_err(|e| map_error(e, "cannot start the transaction"))?;
    let result = transact(alpm, questions, gate, build);
    let _ = alpm.trans_release();
    result
}

fn transact(
    alpm: &mut Alpm,
    questions: &Rc<RefCell<QuestionLog>>,
    gate: &CancelGate,
    build: impl FnOnce(&mut Alpm) -> Result<(), HelperError>,
) -> Result<Vec<Change>, HelperError> {
    build(alpm)?;
    gate.checkpoint()?;
    if alpm.trans_add().is_empty() && alpm.trans_remove().is_empty() {
        return Ok(Vec::new()); // nothing to do, e.g. an upgrade of an up-to-date system
    }
    alpm.trans_prepare().map_err(|e| {
        let detail = describe(&e);
        map_error(
            e.error(),
            &format!("cannot prepare the transaction ({detail})"),
        )
    })?;
    if !questions.borrow().ambiguous.is_empty() {
        return Err(HelperError::new(
            ErrorCode::Conflict,
            format!(
                "{}; name the provider you want and retry",
                questions.borrow().ambiguous.join(", ")
            ),
        ));
    }
    let plan = plan(alpm);
    gate.open_window()?;
    let committed = alpm.trans_commit();
    // The lock is released right after this, whatever happened: a cancel from here on is too late.
    gate.commit_finished();
    committed.map_err(|e| {
        let mut error = map_error(e.error(), "the transaction failed");
        if questions.borrow().unknown_key {
            error.message.push_str(
                "; a package signature key is not trusted: import it with pacman-key and retry",
            );
        }
        error
    })?;
    Ok(plan)
}

/// Installed packages (not ignored by `IgnorePkg`) that the sync databases have a newer version
/// of. A non-empty list means the databases are ahead of the system, so installing anything on
/// top of them would be a partial upgrade.
pub(crate) fn pending_upgrades(alpm: &Alpm) -> Vec<String> {
    let mut pending: Vec<String> = alpm
        .localdb()
        .pkgs()
        .iter()
        .filter(|pkg| !pkg.should_ignore())
        .filter(|pkg| pkg.sync_new_version(alpm.syncdbs()).is_some())
        .map(|pkg| pkg.name().to_string())
        .collect();
    pending.sort();
    pending
}

/// What the prepared transaction is going to do.
fn plan(alpm: &Alpm) -> Vec<Change> {
    let mut changes = Vec::new();
    for pkg in alpm.trans_add() {
        let kind = if alpm.localdb().pkg(pkg.name()).is_ok() {
            ChangeKind::Upgraded
        } else {
            ChangeKind::Installed
        };
        changes.push(Change {
            package: pkg.name().to_string(),
            kind,
            version: Some(pkg.version().to_string()),
        });
    }
    for pkg in alpm.trans_remove() {
        changes.push(Change {
            package: pkg.name().to_string(),
            kind: ChangeKind::Removed,
            version: Some(pkg.version().to_string()),
        });
    }
    changes
}

/// Which packages libalpm says are in the way: broken dependencies, conflicts, bad architectures.
fn describe(error: &PrepareError<'_>) -> String {
    match error.data() {
        None => String::from("no further detail"),
        Some(PrepareData::UnsatisfiedDeps(missing)) => missing
            .iter()
            .map(|m| format!("{} needs {}", m.target(), m.depend()))
            .collect::<Vec<_>>()
            .join(", "),
        Some(PrepareData::ConflictingDeps(conflicts)) => conflicts
            .iter()
            .map(|c| {
                format!(
                    "{} conflicts with {}",
                    c.package1().name(),
                    c.package2().name()
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        Some(PrepareData::PkgInvalidArch(packages)) => packages
            .iter()
            .map(|p| format!("{} has an unsupported architecture", p.name()))
            .collect::<Vec<_>>()
            .join(", "),
    }
}
