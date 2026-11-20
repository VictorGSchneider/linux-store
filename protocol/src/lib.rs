//! The one-line JSON protocol between the GUI side and the privileged helper.
mod event;
mod request;

pub use event::{Change, ChangeKind, ErrorCode, Event, EventError, Operation, Stage};
pub use request::{
    MAX_NAME_BYTES, MAX_PACKAGES, MAX_REQUEST_BYTES, Request, RequestError, is_valid_name,
};

/// Process exit codes of the helper. `pkexec` adds its own: 126 (the user dismissed the
/// authentication dialog) and 127 (not authorized, or an error before the helper started).
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_INVALID: i32 = 2;
pub const EXIT_CANCELLED: i32 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_distinct() {
        let codes = [EXIT_OK, EXIT_FAILED, EXIT_INVALID, EXIT_CANCELLED];
        let unique: std::collections::BTreeSet<i32> = codes.iter().copied().collect();
        assert_eq!(unique.len(), codes.len());
        assert!(!codes.contains(&126) && !codes.contains(&127));
    }
}
