//! The privileged helper: libalpm transactions behind a one-line JSON protocol.
mod env;
mod error;
mod events;
mod gate;
mod install;
mod txn;

#[cfg(test)]
mod fixture;

pub use env::Environment;
pub use error::HelperError;
pub use events::Emitter;
pub use gate::{Aborter, CancelGate, CancelOutcome};
pub use install::install;
