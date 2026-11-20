//! Every privileged request leaves a line in the system journal: who asked and for what.
use store_protocol::{ErrorCode, Request};

/// `uid=1000 op=install packages=tree,sl`. The uid comes from `PKEXEC_UID`; anything that is not
/// all digits is shown as `unknown`, so it can never forge extra fields. Package names were
/// validated before this is called, so they cannot carry control characters either.
pub fn audit_line(caller_uid: Option<&str>, request: &Request) -> String {
    let uid = caller_uid
        .filter(|u| !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("unknown");
    match request {
        Request::Install { packages } => {
            format!("uid={uid} op=install packages={}", packages.join(","))
        }
        Request::Remove { packages } => {
            format!("uid={uid} op=remove packages={}", packages.join(","))
        }
        Request::Upgrade => format!("uid={uid} op=upgrade"),
        Request::Cancel => format!("uid={uid} op=cancel"),
    }
}

/// How an operation ended, for the second audit line.
pub enum AuditOutcome {
    Ok { changes: usize },
    Error(ErrorCode),
}

/// `uid=1000 op=install result=ok changes=2` or `... result=error code=conflict`: the audit trail
/// says what was asked (before) and what came of it (after).
pub fn audit_outcome_line(
    caller_uid: Option<&str>,
    request: &Request,
    outcome: &AuditOutcome,
) -> String {
    let uid = caller_uid
        .filter(|u| !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("unknown");
    let op = match request {
        Request::Install { .. } => "install",
        Request::Remove { .. } => "remove",
        Request::Upgrade => "upgrade",
        Request::Cancel => "cancel",
    };
    match outcome {
        AuditOutcome::Ok { changes } => format!("uid={uid} op={op} result=ok changes={changes}"),
        AuditOutcome::Error(code) => {
            format!("uid={uid} op={op} result=error code={}", code.as_str())
        }
    }
}

/// Tests must not write to the real system journal.
#[cfg(test)]
pub fn audit(_line: &str) {}

/// Writes `line` to the system log (the journal picks it up) as `linux-store-helper`.
#[cfg(not(test))]
pub fn audit(line: &str) {
    let Ok(text) = std::ffi::CString::new(line) else {
        return; // a validated line never contains NUL
    };
    // SAFETY: the identity and the format are static NUL-terminated strings, and `%s` receives
    // exactly one NUL-terminated argument that outlives the call.
    unsafe {
        libc::openlog(
            c"linux-store-helper".as_ptr(),
            libc::LOG_PID,
            libc::LOG_AUTHPRIV,
        );
        libc::syslog(libc::LOG_NOTICE, c"%s".as_ptr(), text.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use store_protocol::Request;

    use super::*;

    #[test]
    fn audit_line_formats_and_sanitises() {
        let install = Request::Install {
            packages: vec!["tree".into(), "sl".into()],
        };
        assert_eq!(
            audit_line(Some("1000"), &install),
            "uid=1000 op=install packages=tree,sl"
        );
        assert_eq!(
            audit_line(
                Some("0"),
                &Request::Remove {
                    packages: vec!["x".into()]
                }
            ),
            "uid=0 op=remove packages=x"
        );
        assert_eq!(
            audit_line(Some("1000"), &Request::Upgrade),
            "uid=1000 op=upgrade"
        );
        for bad in [
            None,
            Some("12x"),
            Some(""),
            Some("1000\nfake=line"),
            Some("-1"),
        ] {
            assert!(
                audit_line(bad, &Request::Upgrade).starts_with("uid=unknown "),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn audit_outcome_line_records_the_result() {
        use store_protocol::ErrorCode;
        let install = Request::Install {
            packages: vec!["tree".into()],
        };
        assert_eq!(
            audit_outcome_line(Some("1000"), &install, &AuditOutcome::Ok { changes: 2 }),
            "uid=1000 op=install result=ok changes=2"
        );
        assert_eq!(
            audit_outcome_line(
                Some("1000"),
                &install,
                &AuditOutcome::Error(ErrorCode::Conflict)
            ),
            "uid=1000 op=install result=error code=conflict"
        );
        assert_eq!(
            audit_outcome_line(None, &Request::Upgrade, &AuditOutcome::Ok { changes: 0 }),
            "uid=unknown op=upgrade result=ok changes=0"
        );
    }
}
