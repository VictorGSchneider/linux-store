//! Events: what the helper reports back, one JSON object per line.
use serde::{Deserialize, Serialize};

use crate::MAX_REQUEST_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Install,
    Remove,
    Upgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Refresh,
    Resolve,
    Download,
    Verify,
    /// Pacman hooks (snapshots, cache refreshes...) run before and after the packages.
    Hooks,
    Install,
    Upgrade,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Installed,
    Removed,
    Upgraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    NotFound,
    Conflict,
    Locked,
    Protected,
    DependencyBroken,
    Cancelled,
    TooLate,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub package: String,
    pub kind: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Started {
        op: Operation,
    },
    Progress {
        stage: Stage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        package: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
    },
    Done {
        changes: Vec<Change>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    /// A cancel request arrived too late to stop the operation, which goes on to finish.
    CancelIgnored,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EventError {
    #[error("event line is larger than 64 KiB")]
    TooLarge,
    #[error("event line is not valid UTF-8")]
    NotUtf8,
    #[error("event line is not a valid event: {0}")]
    Json(String),
}

impl Event {
    /// The event as one JSON line ending in `\n`.
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("an event always serializes");
        line.push('\n');
        line
    }

    /// Parses one event line, with or without its trailing newline.
    pub fn parse(line: &[u8]) -> Result<Event, EventError> {
        if line.len() > MAX_REQUEST_BYTES {
            return Err(EventError::TooLarge);
        }
        let text = std::str::from_utf8(line).map_err(|_| EventError::NotUtf8)?;
        let body = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text);
        serde_json::from_str(body).map_err(|e| EventError::Json(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_roundtrip_and_have_stable_json() {
        let events = [
            Event::Started {
                op: Operation::Install,
            },
            Event::Progress {
                stage: Stage::Download,
                package: Some("tree".into()),
                percent: Some(42),
            },
            Event::Progress {
                stage: Stage::Resolve,
                package: None,
                percent: None,
            },
            Event::Progress {
                stage: Stage::Hooks,
                package: None,
                percent: None,
            },
            Event::Done {
                changes: vec![Change {
                    package: "tree".into(),
                    kind: ChangeKind::Installed,
                    version: Some("2.3.2-1".into()),
                }],
            },
            Event::Error {
                code: ErrorCode::NotFound,
                message: "x".into(),
            },
            Event::CancelIgnored,
        ];
        for event in events {
            let text = event.to_line();
            assert!(text.ends_with('\n') && !text[..text.len() - 1].contains('\n'));
            assert_eq!(Event::parse(text.as_bytes()).unwrap(), event);
        }
        assert_eq!(
            Event::Progress {
                stage: Stage::Download,
                package: Some("tree".into()),
                percent: Some(42)
            }
            .to_line(),
            "{\"event\":\"progress\",\"stage\":\"download\",\"package\":\"tree\",\"percent\":42}\n"
        );
        assert_eq!(
            Event::Error {
                code: ErrorCode::NotFound,
                message: "x".into()
            }
            .to_line(),
            "{\"event\":\"error\",\"code\":\"not_found\",\"message\":\"x\"}\n"
        );
        assert_eq!(
            Event::CancelIgnored.to_line(),
            "{\"event\":\"cancel_ignored\"}\n"
        );
    }

    #[test]
    fn every_error_code_has_a_snake_case_name() {
        let names: Vec<String> = [
            ErrorCode::InvalidRequest,
            ErrorCode::NotFound,
            ErrorCode::Conflict,
            ErrorCode::Locked,
            ErrorCode::Protected,
            ErrorCode::DependencyBroken,
            ErrorCode::Cancelled,
            ErrorCode::TooLate,
            ErrorCode::Failed,
        ]
        .iter()
        .map(|c| serde_json::to_string(c).unwrap())
        .collect();
        assert_eq!(
            names,
            [
                "\"invalid_request\"",
                "\"not_found\"",
                "\"conflict\"",
                "\"locked\"",
                "\"protected\"",
                "\"dependency_broken\"",
                "\"cancelled\"",
                "\"too_late\"",
                "\"failed\""
            ]
        );
    }

    #[test]
    fn parse_rejects_garbage_and_oversized_lines() {
        assert!(Event::parse(b"not json").is_err());
        assert!(Event::parse(b"").is_err());
        assert!(Event::parse(b"{\"event\":\"nope\"}").is_err());
        let mut big = b"{\"event\":\"cancel_ignored\"}".to_vec();
        big.resize(crate::MAX_REQUEST_BYTES + 1, b' ');
        assert!(Event::parse(&big).is_err());
    }
}
