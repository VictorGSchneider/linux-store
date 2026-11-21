//! Test doubles, available with the `testing` feature.
use crate::{Backend, Error, Package, Result, SourceId};

/// A package with version `1.0`, not installed, and empty/zero optional fields.
pub fn pkg(source: &str, name: &str, description: &str) -> Package {
    Package {
        source: SourceId::new(source),
        name: name.into(),
        version: "1.0".into(),
        installed_version: None,
        title: None,
        description: Some(description.into()),
        repo: None,
        url: None,
        download_size: 0,
        installed_size: 0,
    }
}

pub struct FakeBackend {
    source: SourceId,
    packages: Vec<Package>,
    available: bool,
    reason: Option<String>,
    failure: Option<String>,
}

impl FakeBackend {
    pub fn new(source: &str, packages: Vec<Package>) -> FakeBackend {
        FakeBackend {
            source: SourceId::new(source),
            packages,
            available: true,
            reason: None,
            failure: None,
        }
    }

    pub fn unavailable(mut self) -> FakeBackend {
        self.available = false;
        self
    }

    pub fn unavailable_because(mut self, reason: &str) -> FakeBackend {
        self.available = false;
        self.reason = Some(reason.into());
        self
    }

    pub fn failing(mut self, message: &str) -> FakeBackend {
        self.failure = Some(message.into());
        self
    }
}

impl Backend for FakeBackend {
    fn source(&self) -> SourceId {
        self.source.clone()
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn unavailable_reason(&self) -> Option<String> {
        self.reason.clone()
    }

    fn packages(&self) -> Result<Vec<Package>> {
        match &self.failure {
            Some(message) => Err(Error::Backend {
                source_id: self.source.clone(),
                message: message.clone(),
            }),
            None => Ok(self.packages.clone()),
        }
    }
}

/// An `Installer` that replays canned events and records what it was asked. The result can be
/// replayed once: `InstallError` holds an `io::Error` and is not `Clone`.
pub struct FakeInstaller {
    events: Vec<store_protocol::Event>,
    result: std::sync::Mutex<Option<std::result::Result<(), crate::InstallError>>>,
    requests: std::sync::Mutex<Vec<store_protocol::Request>>,
}

impl FakeInstaller {
    pub fn new(
        events: Vec<store_protocol::Event>,
        result: std::result::Result<(), crate::InstallError>,
    ) -> FakeInstaller {
        FakeInstaller {
            events,
            result: std::sync::Mutex::new(Some(result)),
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<store_protocol::Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl crate::Installer for FakeInstaller {
    fn run(
        &self,
        request: &store_protocol::Request,
        _cancel: &crate::CancelToken,
        on_event: &mut dyn FnMut(store_protocol::Event),
    ) -> std::result::Result<(), crate::InstallError> {
        self.requests.lock().unwrap().push(request.clone());
        for event in &self.events {
            on_event(event.clone());
        }
        self.result.lock().unwrap().take().unwrap_or_else(|| {
            Err(crate::InstallError::Failed {
                exit_code: None,
                stderr: "FakeInstaller was run twice".to_string(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_backend_returns_its_packages() {
        let pkgs = vec![pkg("fake", "a", "x")];
        let b = FakeBackend::new("fake", pkgs.clone());
        assert_eq!(b.packages().unwrap(), pkgs);
        assert_eq!(b.source(), SourceId::new("fake"));
        assert!(b.is_available());
    }

    #[test]
    fn fake_failing_backend_returns_backend_error() {
        let b = FakeBackend::new("fake", vec![]).failing("boom");
        match b.packages() {
            Err(Error::Backend { message, .. }) => assert_eq!(message, "boom"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn fake_unavailable_backend_reports_unavailable() {
        let b = FakeBackend::new("fake", vec![]).unavailable();
        assert!(!b.is_available());
    }
}
