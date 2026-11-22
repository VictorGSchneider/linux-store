//! What differs from machine to machine and is the same for every front end: which backends
//! exist, how the privileged installer is started, and where the index lives.
use std::ffi::OsString;
use std::path::PathBuf;

use store_backend_aur::AurBackend;
use store_backend_flatpak::FlatpakBackend;
use store_backend_pacman::PacmanBackend;
use store_core::Backend;
use store_installer_pkexec::PkexecInstaller;

/// The sources of this system: pacman, Flatpak and the AUR, in that order.
pub fn backends() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(PacmanBackend::from_system()),
        Box::new(FlatpakBackend::from_system()),
        Box::new(AurBackend::new()),
    ]
}

/// The installer that runs the privileged helper through pkexec.
pub fn installer() -> PkexecInstaller {
    PkexecInstaller::system()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_db_path_prefers_xdg_cache_home() {
        let path = default_db_path_from(
            Some(OsString::from("/x/cache")),
            Some(OsString::from("/home/u")),
        );
        assert_eq!(path, Some(PathBuf::from("/x/cache/linux-store/index.db")));
    }

    #[test]
    fn default_db_path_falls_back_to_home_cache() {
        let path = default_db_path_from(None, Some(OsString::from("/home/u")));
        assert_eq!(
            path,
            Some(PathBuf::from("/home/u/.cache/linux-store/index.db"))
        );
        assert_eq!(default_db_path_from(None, None), None);
    }
}
