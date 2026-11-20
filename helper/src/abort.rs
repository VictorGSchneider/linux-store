//! What really happens when a cancel arrives while packages are still downloading.
use std::io::Write;
use std::path::PathBuf;

use store_protocol::{EXIT_CANCELLED, ErrorCode, Event};

use crate::Aborter;

/// Removes the helper's own database lock, reports the cancellation and exits. It is only called
/// inside the cancel gate's window, i.e. after this process took the lock and before it changed
/// any package, so nothing is left half applied.
pub struct ProcessAborter {
    lock_path: PathBuf,
}

impl ProcessAborter {
    pub fn new(lock_path: PathBuf) -> ProcessAborter {
        ProcessAborter { lock_path }
    }

    /// Everything the abort does except reporting and exiting. Touches only the lock file.
    pub(crate) fn cleanup_only(&self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

impl Aborter for ProcessAborter {
    fn abort(&self) {
        self.cleanup_only();
        let line = Event::Error {
            code: ErrorCode::Cancelled,
            message: "cancelled while downloading; nothing was changed".to_string(),
        }
        .to_line();
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
        std::process::exit(EXIT_CANCELLED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_aborter_removes_only_its_own_lock() {
        let dir = tempfile::tempdir().unwrap();
        let ours = dir.path().join("db.lck");
        let other = dir.path().join("other.lck");
        std::fs::write(&ours, b"").unwrap();
        std::fs::write(&other, b"").unwrap();
        ProcessAborter::new(ours.clone()).cleanup_only();
        assert!(!ours.exists(), "its own lock must go");
        assert!(other.exists(), "no other file may be touched");
        ProcessAborter::new(ours).cleanup_only(); // an already missing lock is fine
    }
}
