//! How the store changes the system: one trait, so the GUI and the CLI do not care whether the
//! work is done by the pkexec helper, by PackageKit on another distribution, or by a fake.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use store_protocol::{ErrorCode, Event, Request};

/// Cancels a running operation from another thread. Cloning shares the same flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The helper understood the request and refused or failed it.
    #[error("{message}")]
    Rejected { code: ErrorCode, message: String },
    #[error("the authentication dialog was dismissed")]
    Dismissed,
    #[error("not authorized: {0}")]
    NotAuthorized(String),
    #[error("cannot start the helper: {0}")]
    Spawn(std::io::Error),
    #[error("the helper broke the protocol: {0}")]
    Protocol(String),
    #[error("the helper failed (exit code {exit_code:?}): {stderr}")]
    Failed {
        exit_code: Option<i32>,
        stderr: String,
    },
}

pub trait Installer: Send + Sync {
    /// Runs one request, calling `on_event` for every event as it arrives. Setting `cancel`
    /// asks the operation to stop; whether it still can depends on how far it got.
    fn run(
        &self,
        request: &Request,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<(), InstallError>;
}
#[cfg(test)]
mod tests {
    use store_protocol::{ErrorCode, Event, Operation, Request};

    use super::*;
    use crate::testing::FakeInstaller;

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let token = CancelToken::default();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn fake_installer_records_requests_and_replays_events() {
        let events = vec![
            Event::Started {
                op: Operation::Upgrade,
            },
            Event::Done { changes: vec![] },
        ];
        let fake = FakeInstaller::new(events.clone(), Ok(()));
        let mut seen = Vec::new();
        let result = fake.run(&Request::Upgrade, &CancelToken::default(), &mut |e| {
            seen.push(e)
        });
        assert!(result.is_ok());
        assert_eq!(seen, events);
        assert_eq!(fake.requests(), [Request::Upgrade]);
    }

    #[test]
    fn fake_installer_can_fail_once() {
        let fake = FakeInstaller::new(
            vec![],
            Err(InstallError::Rejected {
                code: ErrorCode::NotFound,
                message: "ghost".into(),
            }),
        );
        let first = fake.run(&Request::Upgrade, &CancelToken::default(), &mut |_| {});
        assert!(matches!(
            first,
            Err(InstallError::Rejected {
                code: ErrorCode::NotFound,
                ..
            })
        ));
        let second = fake.run(&Request::Upgrade, &CancelToken::default(), &mut |_| {});
        assert!(
            matches!(second, Err(InstallError::Failed { .. })),
            "a second call must not replay the result"
        );
    }

    #[test]
    fn install_errors_have_readable_messages() {
        assert_eq!(
            InstallError::Dismissed.to_string(),
            "the authentication dialog was dismissed"
        );
        assert!(
            InstallError::NotAuthorized("denied".into())
                .to_string()
                .contains("denied")
        );
        let rejected = InstallError::Rejected {
            code: ErrorCode::Protected,
            message: "refusing".into(),
        };
        assert_eq!(rejected.to_string(), "refusing");
    }
}
