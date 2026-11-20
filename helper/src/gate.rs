//! Cancellation: when a cancel request may stop the work, and when it is too late.
//!
//! Before the transaction's commit a cancel only sets a flag that `checkpoint` reports. During
//! the commit's download phase (the "window", up to the first package operation) it aborts at
//! once. After the first package operation starts it is refused, so a transaction is never left
//! half applied.
use std::sync::{Arc, Mutex, MutexGuard};

use store_protocol::ErrorCode;

use crate::HelperError;

/// What to do when a cancel arrives inside the window. The real one removes the helper's own
/// lock file, reports the cancellation and exits the process.
pub trait Aborter: Send + Sync {
    fn abort(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    Cancelled,
    TooLate,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Planning,
    Window,
    Committed,
}

struct State {
    phase: Phase,
    cancelled: bool,
}

#[derive(Clone)]
pub struct CancelGate {
    state: Arc<Mutex<State>>,
    aborter: Arc<dyn Aborter>,
}

fn cancelled() -> HelperError {
    HelperError::new(ErrorCode::Cancelled, "cancelled before any change was made")
}

impl CancelGate {
    pub fn new(aborter: Arc<dyn Aborter>) -> CancelGate {
        CancelGate {
            state: Arc::new(Mutex::new(State {
                phase: Phase::Planning,
                cancelled: false,
            })),
            aborter,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks to cancel. Callable from any thread.
    pub fn request_cancel(&self) -> CancelOutcome {
        let mut state = self.lock();
        match state.phase {
            Phase::Committed => CancelOutcome::TooLate,
            Phase::Planning => {
                state.cancelled = true;
                CancelOutcome::Cancelled
            }
            Phase::Window => {
                state.cancelled = true;
                // The lock is held on purpose: `commit_started` must wait, so no package
                // operation can begin after the decision to abort.
                self.aborter.abort();
                CancelOutcome::Cancelled
            }
        }
    }

    /// `Err` with `Cancelled` once a cancel was accepted. Call between steps.
    pub fn checkpoint(&self) -> Result<(), HelperError> {
        if self.lock().cancelled {
            Err(cancelled())
        } else {
            Ok(())
        }
    }

    /// Call right before `trans_commit`. Fails if a cancel already won, which closes the gap
    /// between the last `checkpoint` and the commit.
    pub fn open_window(&self) -> Result<(), HelperError> {
        let mut state = self.lock();
        if state.cancelled {
            return Err(cancelled());
        }
        // Never reopen a window that already closed (an upgrade closes it after its refresh).
        if state.phase == Phase::Planning {
            state.phase = Phase::Window;
        }
        Ok(())
    }

    /// The point of no return: from here a cancel is refused. Called when pre-transaction hooks
    /// or the first package operation start, and by an upgrade once its refresh is done. A
    /// cancel that was only flagged before this point is dropped: it can no longer be honoured.
    pub fn commit_started(&self) {
        let mut state = self.lock();
        state.phase = Phase::Committed;
        state.cancelled = false;
    }

    /// Call as soon as `trans_commit` returns, whatever the result: the lock is released after
    /// this, and a later cancel must not remove a lock that is no longer ours.
    pub fn commit_finished(&self) {
        self.commit_started();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use store_protocol::ErrorCode;

    use super::*;
    use crate::fixture::RecordingAborter;

    fn gate() -> (CancelGate, Arc<RecordingAborter>) {
        let aborter = Arc::new(RecordingAborter::default());
        (CancelGate::new(aborter.clone()), aborter)
    }

    #[test]
    fn gate_before_window_defers_and_checkpoint_fails() {
        let (gate, aborter) = gate();
        assert!(gate.checkpoint().is_ok());
        assert_eq!(gate.request_cancel(), CancelOutcome::Cancelled);
        assert_eq!(
            aborter.0.load(Ordering::SeqCst),
            0,
            "must not abort while only planning"
        );
        assert_eq!(gate.checkpoint().unwrap_err().code, ErrorCode::Cancelled);
    }

    #[test]
    fn gate_in_window_aborts_immediately() {
        let (gate, aborter) = gate();
        gate.open_window().unwrap();
        assert_eq!(gate.request_cancel(), CancelOutcome::Cancelled);
        assert_eq!(aborter.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn gate_after_commit_started_is_too_late() {
        let (gate, aborter) = gate();
        gate.open_window().unwrap();
        gate.commit_started();
        assert_eq!(gate.request_cancel(), CancelOutcome::TooLate);
        assert_eq!(aborter.0.load(Ordering::SeqCst), 0);
        assert!(gate.checkpoint().is_ok());
    }

    #[test]
    fn the_window_never_reopens_once_the_commit_started() {
        let (gate, aborter) = gate();
        gate.open_window().unwrap();
        gate.commit_started();
        gate.open_window().unwrap();
        assert_eq!(gate.request_cancel(), CancelOutcome::TooLate);
        assert_eq!(aborter.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_finished_commit_is_too_late_to_cancel() {
        let (gate, aborter) = gate();
        gate.open_window().unwrap();
        gate.commit_finished();
        assert_eq!(gate.request_cancel(), CancelOutcome::TooLate);
        assert_eq!(
            aborter.0.load(Ordering::SeqCst),
            0,
            "a lock that is already released must not be removed"
        );
    }

    #[test]
    fn a_cancel_flagged_before_the_point_of_no_return_is_dropped_by_it() {
        let (gate, _aborter) = gate();
        assert_eq!(gate.request_cancel(), CancelOutcome::Cancelled);
        gate.commit_started();
        assert!(
            gate.checkpoint().is_ok(),
            "a cancel that can no longer be honoured must not fail the run"
        );
    }

    #[test]
    fn a_cancel_that_won_the_race_stops_the_window_from_opening() {
        let (gate, _aborter) = gate();
        gate.request_cancel();
        assert_eq!(gate.open_window().unwrap_err().code, ErrorCode::Cancelled);
    }
}
