//! Packages the helper refuses to remove, whatever the request says.
use std::collections::HashSet;

use alpm::Alpm;

/// Always protected by name: the package manager itself, the base meta package and this project.
pub(crate) const ALWAYS_PROTECTED: [&str; 4] =
    ["pacman", "base", "linux-store", "linux-store-helper"];

/// The fixed names plus everything the installed `base` package requires, directly or not.
pub fn protected_packages(alpm: &Alpm) -> HashSet<String> {
    let mut protected: HashSet<String> = ALWAYS_PROTECTED.iter().map(|s| s.to_string()).collect();
    let local = alpm.localdb();
    let mut pending = vec!["base".to_string()];
    let mut seen = HashSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Ok(pkg) = local.pkg(name.as_str()) else {
            continue;
        };
        for dep in pkg.depends() {
            // Resolve each dependency to the installed package that actually satisfies it.
            if let Some(provider) = local.pkgs().find_satisfier(dep.to_string()) {
                protected.insert(provider.name().to_string());
                pending.push(provider.name().to_string());
            }
        }
    }
    protected
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use store_protocol::Event;

    use crate::fixture::{Fixture, RecordingAborter};
    use crate::{CancelGate, Emitter, install};

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

    use crate::protected_packages;

    #[test]
    fn protected_set_includes_base_closure_and_the_fixed_names() {
        let fx = system();
        let alpm = fx.env().open().unwrap();
        let set = protected_packages(&alpm);
        for name in [
            "coreutils-fix",
            "base",
            "pacman",
            "linux-store",
            "linux-store-helper",
        ] {
            assert!(set.contains(name), "{name} missing from {set:?}");
        }
        assert!(!set.contains("app") && !set.contains("libfoo"), "{set:?}");
    }
}
