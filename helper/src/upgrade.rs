//! Refresh the databases and upgrade the system, as one operation (like `pacman -Syu`).
use store_protocol::Change;

use crate::{CancelGate, Emitter, Environment, HelperError, txn};

/// The only place the sync databases are refreshed: a refresh without an upgrade is how an Arch
/// system ends up partially upgraded, so there is no operation that does it alone.
pub fn upgrade(
    env: &Environment,
    emitter: &Emitter,
    gate: &CancelGate,
) -> Result<Vec<Change>, HelperError> {
    gate.checkpoint()?;
    let (mut alpm, questions) = txn::open(env, emitter, gate)?;
    alpm.syncdbs_mut()
        .update(false)
        .map_err(|e| txn::map_error(e, "cannot refresh the package databases"))?;
    // The refreshed databases are only safe together with the upgrade (a refresh alone is the
    // partial-upgrade state), so from here on the operation runs to the end: no cancel.
    gate.commit_started();
    txn::run(&mut alpm, env.trans_flags(), &questions, gate, |alpm| {
        alpm.sync_sysupgrade(false)
            .map_err(|e| txn::map_error(e, "cannot plan the system upgrade"))
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use store_protocol::{ChangeKind, ErrorCode, Event};

    use crate::fixture::{Fixture, RecordingAborter};
    use crate::{CancelGate, Emitter, install, upgrade};

    fn gate() -> CancelGate {
        CancelGate::new(Arc::new(RecordingAborter::default()))
    }

    fn quiet() -> Emitter {
        let sink: Rc<RefCell<Vec<Event>>> = Rc::default();
        Emitter::new(move |e| sink.borrow_mut().push(e))
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn upgrade_refreshes_and_installs_newer_versions() {
        let fx = Fixture::new();
        fx.publish("hello", "1.0-1", &[]);
        fx.sync();
        install(&fx.env(), &names(&["hello"]), &quiet(), &gate()).unwrap();
        fx.publish("hello", "2.0-1", &[]); // published, but the system's database is still old
        let changes = upgrade(&fx.env(), &quiet(), &gate()).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            (
                changes[0].package.as_str(),
                changes[0].kind,
                changes[0].version.as_deref()
            ),
            ("hello", ChangeKind::Upgraded, Some("2.0-1"))
        );
        assert_eq!(fx.installed(), ["hello-2.0-1"]);
        assert!(!fx.lock_exists());
    }

    #[test]
    fn upgrade_with_nothing_to_do_is_empty() {
        let fx = Fixture::new();
        fx.publish("hello", "1.0-1", &[]);
        fx.sync();
        install(&fx.env(), &names(&["hello"]), &quiet(), &gate()).unwrap();
        assert_eq!(upgrade(&fx.env(), &quiet(), &gate()).unwrap(), vec![]);
        assert_eq!(fx.installed(), ["hello-1.0-1"]);
        assert!(!fx.lock_exists());
    }

    #[test]
    fn upgrade_cannot_be_cancelled_once_the_databases_are_refreshed() {
        // A refresh without the upgrade is the partial-upgrade state the design forbids, so after
        // the refresh the upgrade runs to the end.
        let fx = Fixture::new();
        fx.publish("hello", "1.0-1", &[]);
        fx.sync();
        install(&fx.env(), &names(&["hello"]), &quiet(), &gate()).unwrap();
        fx.publish("hello", "2.0-1", &[]);
        let gate = gate();
        let (emitter, answer) =
            crate::fixture::cancelling_at(&gate, store_protocol::Stage::Resolve);
        let changes = upgrade(&fx.env(), &emitter, &gate).unwrap();
        assert_eq!(answer.get(), Some(crate::CancelOutcome::TooLate));
        assert_eq!(changes.len(), 1);
        assert_eq!(fx.installed(), ["hello-2.0-1"]);
    }

    #[test]
    fn upgrade_cancelled_before_refresh_leaves_databases_alone() {
        let fx = Fixture::new();
        fx.publish("hello", "1.0-1", &[]);
        fx.sync();
        install(&fx.env(), &names(&["hello"]), &quiet(), &gate()).unwrap();
        fx.publish("hello", "2.0-1", &[]);
        let before = fx.sync_db_bytes();
        let gate = gate();
        gate.request_cancel();
        let err = upgrade(&fx.env(), &quiet(), &gate).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert_eq!(
            fx.sync_db_bytes(),
            before,
            "the sync database must not be refreshed"
        );
        assert_eq!(fx.installed(), ["hello-1.0-1"]);
    }
}
