//! The privileged helper: libalpm transactions behind a one-line JSON protocol.
mod abort;
mod audit;
mod env;
mod error;
mod events;
mod gate;
mod harden;
mod install;
mod protect;
mod remove;
mod serve;
mod txn;
mod upgrade;

#[cfg(test)]
mod fixture;

pub use abort::ProcessAborter;
pub use audit::{audit, audit_line};
pub use env::Environment;
pub use error::HelperError;
pub use events::Emitter;
pub use gate::{Aborter, CancelGate, CancelOutcome};
pub use harden::harden_process;
pub use install::install;
pub use protect::protected_packages;
pub use remove::remove;
pub use serve::{ensure_root, serve};
pub use upgrade::upgrade;
