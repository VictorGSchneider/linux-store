//! Requests: what the GUI side may ask the helper to do. Everything here is untrusted input.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub const MAX_PACKAGES: usize = 200;
pub const MAX_NAME_BYTES: usize = 255;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// An operation and, for install and remove, package names. Never paths, URLs or options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Install { packages: Vec<String> },
    Remove { packages: Vec<String> },
    Upgrade,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    #[error("request is larger than 64 KiB")]
    TooLarge,
    #[error("request is not valid UTF-8")]
    NotUtf8,
    #[error("request is empty")]
    Blank,
    #[error("request must be a single line")]
    MultiLine,
    #[error("request is not valid for this protocol: {0}")]
    Json(String),
    #[error("unknown operation {0:?}")]
    UnknownOp(String),
    #[error("invalid package name {0:?}")]
    InvalidName(String),
    #[error("too many packages (the limit is 200)")]
    TooMany,
    #[error("package {0:?} is listed twice")]
    Duplicate(String),
    #[error("{0} needs at least one package")]
    NoPackages(&'static str),
    #[error("{0} takes no packages")]
    UnexpectedPackages(&'static str),
}

/// What a request says is parsed into this first so unknown fields are refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    op: String,
    packages: Option<Vec<String>>,
}

/// Printable, short version of untrusted text for error messages: control characters become
/// `?` so a hostile request cannot smuggle escape sequences into logs or a terminal.
fn sanitize(text: &str) -> String {
    text.chars()
        .take(64)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// A pacman package name: lowercase ASCII letters, digits and `@ . _ + -`, not starting with
/// `-`, at most 255 bytes.
pub fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_NAME_BYTES {
        return false;
    }
    let plain = |b: u8| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'@' | b'.' | b'_' | b'+')
    };
    plain(bytes[0]) && bytes[1..].iter().all(|&b| plain(b) || b == b'-')
}

impl Request {
    /// Parses one request line, with or without its trailing newline.
    pub fn parse(line: &[u8]) -> Result<Request, RequestError> {
        if line.len() > MAX_REQUEST_BYTES {
            return Err(RequestError::TooLarge);
        }
        let text = std::str::from_utf8(line).map_err(|_| RequestError::NotUtf8)?;
        let body = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text);
        if body.contains('\n') || body.contains('\r') {
            return Err(RequestError::MultiLine);
        }
        if body.trim().is_empty() {
            return Err(RequestError::Blank);
        }
        let raw: Raw =
            serde_json::from_str(body).map_err(|e| RequestError::Json(sanitize(&e.to_string())))?;
        match (raw.op.as_str(), raw.packages) {
            ("install", packages) => Ok(Request::Install {
                packages: names("install", packages)?,
            }),
            ("remove", packages) => Ok(Request::Remove {
                packages: names("remove", packages)?,
            }),
            ("upgrade", None) => Ok(Request::Upgrade),
            ("upgrade", Some(_)) => Err(RequestError::UnexpectedPackages("upgrade")),
            ("cancel", None) => Ok(Request::Cancel),
            ("cancel", Some(_)) => Err(RequestError::UnexpectedPackages("cancel")),
            (other, _) => Err(RequestError::UnknownOp(sanitize(other))),
        }
    }

    /// The request as one JSON line ending in `\n`.
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a request always serializes");
        line.push('\n');
        line
    }
}

fn names(op: &'static str, packages: Option<Vec<String>>) -> Result<Vec<String>, RequestError> {
    let packages = packages
        .filter(|p| !p.is_empty())
        .ok_or(RequestError::NoPackages(op))?;
    if packages.len() > MAX_PACKAGES {
        return Err(RequestError::TooMany);
    }
    let mut seen = HashSet::new();
    for name in &packages {
        if !is_valid_name(name) {
            return Err(RequestError::InvalidName(sanitize(name)));
        }
        if !seen.insert(name.as_str()) {
            return Err(RequestError::Duplicate(sanitize(name)));
        }
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(json: &str) -> Vec<u8> {
        json.as_bytes().to_vec()
    }

    fn install(names: &[&str]) -> Vec<u8> {
        format!(
            r#"{{"op":"install","packages":{}}}"#,
            serde_json::to_string(names).unwrap()
        )
        .into_bytes()
    }

    #[test]
    fn parses_each_operation() {
        assert_eq!(
            Request::parse(&line(r#"{"op":"install","packages":["tree","sl"]}"#)).unwrap(),
            Request::Install {
                packages: vec!["tree".into(), "sl".into()]
            }
        );
        assert_eq!(
            Request::parse(&line(r#"{"op":"remove","packages":["tree"]}"#)).unwrap(),
            Request::Remove {
                packages: vec!["tree".into()]
            }
        );
        assert_eq!(
            Request::parse(&line(r#"{"op":"upgrade"}"#)).unwrap(),
            Request::Upgrade
        );
        assert_eq!(
            Request::parse(&line(r#"{"op":"cancel"}"#)).unwrap(),
            Request::Cancel
        );
    }

    #[test]
    fn to_line_roundtrips_and_ends_with_a_newline() {
        for request in [
            Request::Install {
                packages: vec!["a".into(), "b-c".into()],
            },
            Request::Remove {
                packages: vec!["x".into()],
            },
            Request::Upgrade,
            Request::Cancel,
        ] {
            let text = request.to_line();
            assert!(text.ends_with('\n') && !text[..text.len() - 1].contains('\n'));
            assert_eq!(Request::parse(text.as_bytes()).unwrap(), request);
        }
    }

    #[test]
    fn rejects_hostile_names() {
        let too_long = "a".repeat(MAX_NAME_BYTES + 1);
        for name in [
            "-x",
            "--root=/x",
            "../x",
            "a/b",
            "a b",
            "a;b",
            "a\nb",
            "a\u{0}b",
            "",
            "Tree",
            "é",
            too_long.as_str(),
        ] {
            assert!(
                Request::parse(&install(&[name])).is_err(),
                "accepted {name:?}"
            );
            assert!(!is_valid_name(name), "is_valid_name accepted {name:?}");
        }
        for name in ["tree", "lib32-gnutls", "gtk+3", "a.b_c@d", "0ad", "x"] {
            assert!(is_valid_name(name), "rejected {name:?}");
        }
    }

    #[test]
    fn rejects_too_many_or_duplicate_names() {
        let many: Vec<String> = (0..=MAX_PACKAGES).map(|i| format!("p{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(
            Request::parse(&install(&refs)).is_err(),
            "201 names accepted"
        );
        assert!(
            Request::parse(&install(&refs[..MAX_PACKAGES])).is_ok(),
            "200 names rejected"
        );
        assert!(Request::parse(&install(&["a", "a"])).is_err());
    }

    #[test]
    fn rejects_empty_install_and_remove_lists() {
        assert!(Request::parse(&line(r#"{"op":"install","packages":[]}"#)).is_err());
        assert!(Request::parse(&line(r#"{"op":"remove","packages":[]}"#)).is_err());
        assert!(Request::parse(&line(r#"{"op":"install"}"#)).is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_wrong_shapes() {
        for bad in [
            r#"{"op":"upgrade","packages":[]}"#,
            r#"{"op":"cancel","packages":["a"]}"#,
            r#"{"op":"install","package":["a"]}"#,
            r#"{"op":"install","packages":["a"],"extra":1}"#,
            r#"{"op":"format"}"#,
            r#"{"packages":["a"]}"#,
            r#"[]"#,
            r#""install""#,
            r#"{"op":"install","packages":"tree"}"#,
            r#"{"op":"install","packages":[1]}"#,
            r#"{"op":"upgrade"}{"op":"upgrade"}"#,
        ] {
            assert!(Request::parse(&line(bad)).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn rejects_oversized_invalid_utf8_and_blank_input() {
        let mut big = br#"{"op":"upgrade"}"#.to_vec();
        big.resize(MAX_REQUEST_BYTES + 1, b' ');
        assert!(matches!(Request::parse(&big), Err(RequestError::TooLarge)));
        assert!(matches!(
            Request::parse(b"\xff\xfe"),
            Err(RequestError::NotUtf8)
        ));
        assert!(matches!(Request::parse(b""), Err(RequestError::Blank)));
        assert!(matches!(Request::parse(b"   "), Err(RequestError::Blank)));
        assert!(matches!(Request::parse(b"\n"), Err(RequestError::Blank)));
        let two_lines = b"{\"op\":\"upgrade\"}\n{\"op\":\"cancel\"}".to_vec();
        assert!(matches!(
            Request::parse(&two_lines),
            Err(RequestError::MultiLine)
        ));
    }

    #[test]
    fn error_messages_do_not_echo_hostile_input_verbatim() {
        let err = Request::parse(&install(&["a\u{1b}[31mred"]))
            .unwrap_err()
            .to_string();
        assert!(!err.contains('\u{1b}'), "{err:?}");
    }
}
