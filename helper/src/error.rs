use std::fmt;

use store_protocol::ErrorCode;

/// A failure the helper reports to its client: a stable code and a human message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperError {
    pub code: ErrorCode,
    pub message: String,
}

impl HelperError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> HelperError {
        HelperError {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for HelperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for HelperError {}
