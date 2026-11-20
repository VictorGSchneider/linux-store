use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;

use flate2::{Compression, write::GzEncoder};
use store_backend_aur::{AurBackend, DumpSource};
use store_core::{Backend, Error, SourceId};

const FIXTURE: &str = r#"[
  {"Name":"alpha","PackageBase":"alpha","Version":"1.0-1","Description":"First",
   "URL":"https://alpha.example","NumVotes":5,"Popularity":0.5,"OutOfDate":null,
   "Maintainer":"someone","FirstSubmitted":1,"LastModified":2,"URLPath":"/x","Extra":[1,2]},
  {"Name":"beta","Version":"2.0-1","Description":null,"URL":null}
]"#;

fn gz(json: &str) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(json.as_bytes()).unwrap();
    enc.finish().unwrap()
}

fn dump_file(dir: &tempfile::TempDir, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join("dump.json.gz");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn backend_for(bytes: &[u8]) -> (tempfile::TempDir, AurBackend) {
    let dir = tempfile::tempdir().unwrap();
    let path = dump_file(&dir, bytes);
    (dir, AurBackend::with_source(DumpSource::File(path)))
}

/// Serves one HTTP response on a loopback port and returns the URL.
fn serve_once(status: &str, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body);
    });
    format!("http://127.0.0.1:{port}/dump.json.gz")
}

fn backend_error(result: store_core::Result<Vec<store_core::Package>>) -> String {
    match result {
        Err(Error::Backend { source_id, message }) => {
            assert_eq!(source_id, SourceId::new("aur"));
            message
        }
        other => panic!("expected a backend error, got {other:?}"),
    }
}

#[test]
fn parses_entries() {
    let (_dir, backend) = backend_for(&gz(FIXTURE));
    let packages = backend.packages().unwrap();
    assert_eq!(packages.len(), 2);
    let alpha = &packages[0];
    assert_eq!(alpha.source, SourceId::new("aur"));
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.version, "1.0-1");
    assert_eq!(alpha.description.as_deref(), Some("First"));
    assert_eq!(alpha.url.as_deref(), Some("https://alpha.example"));
    assert_eq!(alpha.repo.as_deref(), Some("aur"));
    assert_eq!(alpha.title, None);
    assert_eq!(alpha.installed_version, None);
    let beta = &packages[1];
    assert_eq!(beta.description, None);
    assert_eq!(beta.url, None);
}

#[test]
fn ignores_unknown_fields() {
    let json = r#"[{"Name":"x","Version":"1","Brand":{"new":true},"Another":null}]"#;
    let (_dir, backend) = backend_for(&gz(json));
    assert_eq!(backend.packages().unwrap().len(), 1);
}

#[test]
fn truncated_gzip_is_an_error() {
    let bytes = gz(FIXTURE);
    let (_dir, backend) = backend_for(&bytes[..bytes.len() / 2]);
    backend_error(backend.packages());
}

#[test]
fn garbled_json_is_an_error() {
    let (_dir, backend) = backend_for(&gz("this is not json"));
    backend_error(backend.packages());
}

#[test]
fn empty_array_is_an_error() {
    let (_dir, backend) = backend_for(&gz("[]"));
    let message = backend_error(backend.packages());
    assert!(message.contains("no packages"), "{message}");
}

#[test]
fn duplicate_names_keep_the_first() {
    let json = r#"[{"Name":"dup","Version":"1"},{"Name":"dup","Version":"2"},{"Name":"other","Version":"1"}]"#;
    let (_dir, backend) = backend_for(&gz(json));
    let packages = backend.packages().unwrap();
    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0].version, "1");
}

#[test]
fn fetches_over_http() {
    let url = serve_once("200 OK", gz(FIXTURE));
    let backend = AurBackend::with_source(DumpSource::Url(url));
    assert!(backend.is_available());
    assert_eq!(backend.packages().unwrap().len(), 2);
}

#[test]
fn non_200_status_is_an_error() {
    let url = serve_once("503 Service Unavailable", gz(FIXTURE));
    let backend = AurBackend::with_source(DumpSource::Url(url));
    backend_error(backend.packages());
}

#[test]
fn missing_file_is_unavailable() {
    let backend = AurBackend::with_source(DumpSource::File("/nonexistent/dump.json.gz".into()));
    assert!(!backend.is_available());
    backend_error(backend.packages());
}

#[test]
#[ignore = "downloads the real AUR dump; run manually"]
fn real_aur_smoke() {
    let packages = AurBackend::new().packages().unwrap();
    assert!(packages.len() > 50_000, "only {} packages", packages.len());
}
