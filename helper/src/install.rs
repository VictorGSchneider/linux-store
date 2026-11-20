//! Install packages by name from the configured repositories, as one transaction.
use store_protocol::{Change, ErrorCode};

use crate::{CancelGate, Emitter, Environment, HelperError, txn};

/// Installs `packages` (already validated names) together with their dependencies.
///
/// Every name is resolved in the sync databases first: any unknown name fails the whole request
/// with `NotFound` and nothing is changed.
pub fn install(
    env: &Environment,
    packages: &[String],
    emitter: &Emitter,
    gate: &CancelGate,
) -> Result<Vec<Change>, HelperError> {
    gate.checkpoint()?;
    let (mut alpm, questions) = txn::open(env, emitter, gate)?;
    let missing: Vec<&str> = packages
        .iter()
        .map(String::as_str)
        .filter(|name| !alpm.syncdbs().iter().any(|db| db.pkg(*name).is_ok()))
        .collect();
    if !missing.is_empty() {
        return Err(HelperError::new(
            ErrorCode::NotFound,
            format!(
                "not found in the configured repositories: {}",
                missing.join(", ")
            ),
        ));
    }
    txn::run(&mut alpm, env.trans_flags(), &questions, gate, |alpm| {
        for name in packages {
            let added = {
                let pkg = alpm
                    .syncdbs()
                    .iter()
                    .find_map(|db| db.pkg(name.as_str()).ok())
                    .expect("resolved above");
                alpm.trans_add_pkg(pkg).map_err(|e| e.error)
            };
            added.map_err(|e| txn::map_error(e, &format!("cannot add {name}")))?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use store_protocol::{ChangeKind, ErrorCode, Event, Stage};

    use crate::fixture::{Fixture, RecordingAborter};
    use crate::{CancelGate, Emitter, Environment, install};

    fn gate() -> CancelGate {
        CancelGate::new(Arc::new(RecordingAborter::default()))
    }

    fn collecting() -> (Emitter, Rc<RefCell<Vec<Event>>>) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        (Emitter::new(move |e| sink.borrow_mut().push(e)), events)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn world() -> Fixture {
        let fx = Fixture::new();
        fx.publish("libfoo", "1.0-1", &[]);
        fx.publish("app", "1.0-1", &["libfoo"]);
        fx.sync();
        fx
    }

    #[test]
    fn install_resolves_dependencies_and_writes_files() {
        let fx = world();
        let (emitter, events) = collecting();
        let changes = install(&fx.env(), &names(&["app"]), &emitter, &gate()).unwrap();
        let mut installed: Vec<&str> = changes.iter().map(|c| c.package.as_str()).collect();
        installed.sort();
        assert_eq!(installed, ["app", "libfoo"]);
        assert!(
            changes
                .iter()
                .all(|c| c.kind == ChangeKind::Installed && c.version.as_deref() == Some("1.0-1"))
        );
        assert_eq!(fx.installed(), ["app-1.0-1", "libfoo-1.0-1"]);
        assert!(fx.file_exists("usr/bin/app"));
        assert!(!fx.lock_exists(), "the lock must be released");
        let events = events.borrow();
        let stage_seen = |wanted: Stage| {
            events
                .iter()
                .any(|e| matches!(e, Event::Progress { stage, .. } if *stage == wanted))
        };
        assert!(
            stage_seen(Stage::Download),
            "no download progress in {events:?}"
        );
        assert!(
            stage_seen(Stage::Install),
            "no install progress in {events:?}"
        );
    }

    #[test]
    fn install_unknown_name_is_not_found_and_changes_nothing() {
        let fx = world();
        let (emitter, _) = collecting();
        let err = install(
            &fx.env(),
            &names(&["app", "ghost", "phantom"]),
            &emitter,
            &gate(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(
            err.message.contains("ghost") && err.message.contains("phantom"),
            "{}",
            err.message
        );
        assert!(
            fx.installed().is_empty(),
            "nothing may be installed: {:?}",
            fx.installed()
        );
        assert!(!fx.lock_exists());
    }

    #[test]
    fn install_conflict_leaves_the_database_untouched() {
        let fx = Fixture::new();
        fx.publish_full("a", "1.0-1", &[], &[]);
        fx.publish_full("b", "1.0-1", &[], &["a"]);
        fx.sync();
        let (emitter, _) = collecting();
        install(&fx.env(), &names(&["a"]), &emitter, &gate()).unwrap();
        let err = install(&fx.env(), &names(&["b"]), &emitter, &gate()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict, "{}", err.message);
        assert_eq!(fx.installed(), ["a-1.0-1"]);
        assert!(!fx.lock_exists());
    }

    #[test]
    fn install_reports_a_stale_lock() {
        let fx = world();
        std::fs::write(fx.lock_path(), b"").unwrap();
        let (emitter, _) = collecting();
        let err = install(&fx.env(), &names(&["app"]), &emitter, &gate()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Locked);
        assert!(err.message.contains("never removes"), "{}", err.message);
        assert!(
            fx.lock_exists(),
            "the helper must never delete a lock it did not create"
        );
        assert!(fx.installed().is_empty());
    }

    #[test]
    fn download_events_are_throttled() {
        let fx = world();
        let (emitter, events) = collecting();
        install(&fx.env(), &names(&["app"]), &emitter, &gate()).unwrap();
        let downloads_of_app = events
            .borrow()
            .iter()
            .filter(|e| matches!(e, Event::Progress { stage: Stage::Download, package: Some(p), .. } if p == "app"))
            .count();
        assert!(
            (1..=101).contains(&downloads_of_app),
            "{downloads_of_app} download events for app"
        );
    }

    #[test]
    fn cancel_before_start_changes_nothing() {
        let fx = world();
        let (emitter, _) = collecting();
        let gate = gate();
        gate.request_cancel();
        // The environment must not even be opened once a cancel was accepted.
        let err = install(
            &Environment::unreachable(),
            &names(&["app"]),
            &emitter,
            &gate,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert!(fx.installed().is_empty());
    }

    #[test]
    fn package_names_are_recovered_from_file_names() {
        use crate::events::package_from_filename;
        assert_eq!(
            package_from_filename("tree-2.3.2-1-x86_64.pkg.tar.zst").as_deref(),
            Some("tree")
        );
        assert_eq!(
            package_from_filename("lib32-gnutls-3.8.13-3-x86_64.pkg.tar.zst").as_deref(),
            Some("lib32-gnutls")
        );
        assert_eq!(
            package_from_filename("app-1.0-1-any.pkg.tar.zst.sig").as_deref(),
            Some("app")
        );
        assert_eq!(package_from_filename("core.db"), None);
        assert_eq!(package_from_filename("weird.pkg.tar.zst"), None);
    }
}
