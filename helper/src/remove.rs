//! Remove installed packages by name, as one transaction, with no cascade.
use store_protocol::{Change, ErrorCode};

use crate::protect::{ALWAYS_PROTECTED, protected_packages};
use crate::{CancelGate, Emitter, Environment, HelperError, txn};

fn refuse<'a>(offenders: impl Iterator<Item = &'a str>) -> Result<(), HelperError> {
    let offenders: Vec<&str> = offenders.collect();
    if offenders.is_empty() {
        return Ok(());
    }
    Err(HelperError::new(
        ErrorCode::Protected,
        format!(
            "refusing to remove protected packages: {}",
            offenders.join(", ")
        ),
    ))
}

/// Removes `packages` (already validated names). Every name must be installed and none may be
/// protected; a package that others depend on is refused by libalpm, listing the dependents.
pub fn remove(
    env: &Environment,
    packages: &[String],
    emitter: &Emitter,
    gate: &CancelGate,
) -> Result<Vec<Change>, HelperError> {
    gate.checkpoint()?;
    // The fixed names need no database, so they are refused before libalpm is even opened.
    refuse(
        packages
            .iter()
            .map(String::as_str)
            .filter(|n| ALWAYS_PROTECTED.contains(n)),
    )?;
    let (mut alpm, questions) = txn::open(env, emitter, gate)?;
    txn::run(&mut alpm, env.trans_flags(), &questions, gate, |alpm| {
        // Decided only now that the lock is held: libalpm does not reload its database cache,
        // so a view read earlier could be stale if another package manager just changed things.
        let missing: Vec<&str> = packages
            .iter()
            .map(String::as_str)
            .filter(|name| alpm.localdb().pkg(*name).is_err())
            .collect();
        if !missing.is_empty() {
            return Err(HelperError::new(
                ErrorCode::NotFound,
                format!("not installed: {}", missing.join(", ")),
            ));
        }
        let protected = protected_packages(alpm);
        refuse(
            packages
                .iter()
                .map(String::as_str)
                .filter(|n| protected.contains(*n)),
        )?;
        for name in packages {
            let removed = {
                let pkg = alpm.localdb().pkg(name.as_str()).expect("checked above");
                alpm.trans_remove_pkg(pkg)
            };
            removed.map_err(|e| txn::map_error(e, &format!("cannot remove {name}")))?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use store_protocol::{ChangeKind, ErrorCode, Event};

    use crate::fixture::{Fixture, RecordingAborter};
    use crate::{CancelGate, Emitter, Environment, install, remove};

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

    /// A system with `app` (needs `libfoo`), a `base` that needs `coreutils-fix`, and the
    /// packages the helper must never remove, all installed for real.
    fn system() -> Fixture {
        let fx = Fixture::new();
        for name in [
            "libfoo",
            "coreutils-fix",
            "pacman",
            "linux-store",
            "linux-store-helper",
        ] {
            fx.publish(name, "1.0-1", &[]);
        }
        fx.publish("app", "1.0-1", &["libfoo"]);
        fx.publish("base", "1.0-1", &["coreutils-fix"]);
        fx.publish("tree", "1.0-1", &[]);
        fx.sync();
        let all = names(&["app", "base", "pacman", "linux-store", "linux-store-helper"]);
        install(&fx.env(), &all, &quiet(), &gate()).unwrap();
        fx
    }

    #[test]
    fn remove_deletes_files_and_keeps_shared_dependencies() {
        let fx = system();
        let changes = remove(&fx.env(), &names(&["app"]), &quiet(), &gate()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            (changes[0].package.as_str(), changes[0].kind),
            ("app", ChangeKind::Removed)
        );
        assert!(!fx.installed().iter().any(|p| p.starts_with("app-")));
        assert!(
            fx.installed().iter().any(|p| p.starts_with("libfoo-")),
            "the dependency stays"
        );
        assert!(!fx.file_exists("usr/bin/app"));
        assert!(!fx.lock_exists());
    }

    #[test]
    fn remove_with_dependents_is_refused() {
        let fx = system();
        let before = fx.installed();
        let err = remove(&fx.env(), &names(&["libfoo"]), &quiet(), &gate()).unwrap_err();
        assert_eq!(err.code, ErrorCode::DependencyBroken, "{}", err.message);
        assert!(
            err.message.contains("app"),
            "the dependents must be named: {}",
            err.message
        );
        assert_eq!(fx.installed(), before);
        assert!(!fx.lock_exists());
    }

    #[test]
    fn remove_unknown_or_not_installed_names_are_not_found() {
        let fx = system();
        let before = fx.installed();
        for name in ["ghost", "tree"] {
            let err = remove(&fx.env(), &names(&["app", name]), &quiet(), &gate()).unwrap_err();
            assert_eq!(err.code, ErrorCode::NotFound, "{name}");
            assert!(err.message.contains(name), "{}", err.message);
        }
        assert_eq!(
            fx.installed(),
            before,
            "nothing may be removed when one name is unknown"
        );
    }

    #[test]
    fn remove_protected_is_refused_even_alongside_other_names() {
        let fx = system();
        let before = fx.installed();
        for name in [
            "pacman",
            "base",
            "coreutils-fix",
            "linux-store",
            "linux-store-helper",
        ] {
            let err = remove(&fx.env(), &names(&["app", name]), &quiet(), &gate()).unwrap_err();
            assert_eq!(err.code, ErrorCode::Protected, "{name}: {}", err.message);
            assert!(err.message.contains(name), "{}", err.message);
        }
        assert_eq!(
            fx.installed(),
            before,
            "app must stay installed: the request is all or nothing"
        );
        assert!(!fx.lock_exists());
    }

    #[test]
    fn fixed_protected_names_are_refused_without_opening_libalpm() {
        let err = remove(
            &Environment::unreachable(),
            &names(&["pacman"]),
            &quiet(),
            &gate(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Protected);
    }

    #[test]
    fn remove_checks_run_under_the_lock() {
        let fx = system();
        std::fs::write(fx.lock_path(), b"").unwrap();
        let err = remove(&fx.env(), &names(&["ghost"]), &quiet(), &gate()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Locked, "{}", err.message);
    }

    #[test]
    fn cancel_before_start_removes_nothing() {
        let fx = system();
        let before = fx.installed();
        let gate = gate();
        gate.request_cancel();
        let err = remove(&fx.env(), &names(&["app"]), &quiet(), &gate).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert_eq!(fx.installed(), before);
    }
}
