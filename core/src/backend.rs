use crate::{Package, Result, SourceId};

/// A source of packages (pacman, AUR, Flatpak, ...). Read-only for now.
pub trait Backend: Send + Sync {
    fn source(&self) -> SourceId;

    /// Whether this source can be used on the current system.
    fn is_available(&self) -> bool;

    /// Everything the source knows: catalog entries (with `installed_version` set where
    /// installed) plus installed packages absent from the catalog (`repo: None`).
    fn packages(&self) -> Result<Vec<Package>>;
}
