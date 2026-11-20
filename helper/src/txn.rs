//! The steps every operation shares: open a session, run one transaction, map errors.
use std::cell::RefCell;
use std::rc::Rc;

use alpm::{Alpm, TransFlag};
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
    alpm.trans_prepare()
        .map_err(|e| map_error(e.error(), "cannot prepare the transaction"))?;
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
    alpm.trans_commit()
        .map_err(|e| map_error(e.error(), "the transaction failed"))?;
    Ok(plan)
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
