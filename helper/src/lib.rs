//! The privileged helper: libalpm transactions behind a one-line JSON protocol.
mod env;
mod error;
mod events;
mod gate;
mod install;
mod protect;
mod remove;
mod txn;
mod upgrade;

#[cfg(test)]
mod fixture;

pub use env::Environment;
pub use error::HelperError;
pub use events::Emitter;
pub use gate::{Aborter, CancelGate, CancelOutcome};
pub use install::install;
pub use protect::protected_packages;
pub use remove::remove;
pub use upgrade::upgrade;
