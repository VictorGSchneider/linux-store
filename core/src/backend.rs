use crate::{Package, Result, SourceId};

/// A source of packages (pacman, AUR, Flatpak, ...). Read-only for now.
pub trait Backend: Send + Sync {
    fn source(&self) -> SourceId;

    /// Whether this source can be used on the current system.
    fn is_available(&self) -> bool;

    /// Why the source is not available, when it can say. Shown next to "unavailable".
    fn unavailable_reason(&self) -> Option<String> {
        None
    }

    /// Everything the source knows: catalog entries (with `installed_version` set where
    /// installed) plus installed packages absent from the catalog (`repo: None`). Names should be
    /// unique; if one repeats, the store keeps the first and ignores the rest.
    fn packages(&self) -> Result<Vec<Package>>;
}
