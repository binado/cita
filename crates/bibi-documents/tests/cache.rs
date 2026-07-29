//! The cache against a local server: paths, replacement, and eviction.

use bibi_core::ArxivId;
use bibi_documents::{ArtifactKind, CleanMode, DocumentStore, FetchOutcome, FetchPolicy};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

/// A server answering a scripted sequence of bodies.
struct Server {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn new(replies: Vec<(u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding a test server");
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        thread::spawn(move || {
            for (status, body) in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }
                loop {
                    let mut header = String::new();
                    match reader.read_line(&mut header) {
                        Ok(0) => break,
                        Ok(_) if header.trim().is_empty() => break,
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                log.lock().unwrap().push(
                    request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned(),
                );
                let mut response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(&body);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        Self { base_url, requests }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn pdf(body: &str) -> Vec<u8> {
    format!("%PDF-1.7\n{body}").into_bytes()
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, *contents).unwrap();
    }
    let tar = builder.into_inner().unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tar).unwrap();
    encoder.finish().unwrap()
}

fn store(directory: &tempfile::TempDir, server: &Server) -> DocumentStore {
    DocumentStore::builder(directory.path().join("bibi"))
        .base_url(&server.base_url)
        .build()
        .unwrap()
}

fn arxiv(value: &str) -> ArxivId {
    ArxivId::new(value).unwrap()
}

#[tokio::test]
async fn a_pdf_is_cached_at_its_modern_path_and_reused() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(vec![(200, pdf("first"))]);
    let store = store(&directory, &server);
    let id = arxiv("1207.7214v3");

    let downloaded = store
        .fetch(&id, ArtifactKind::Pdf, FetchPolicy::UseCache)
        .await
        .unwrap();
    assert!(matches!(downloaded, FetchOutcome::Downloaded(_)));
    assert_eq!(
        downloaded.path(),
        directory
            .path()
            .join("bibi/documents/arxiv/1207.7214/paper.pdf")
    );
    assert_eq!(server.requests(), ["/pdf/1207.7214"]);

    // A second fetch answers from the cache, with no request at all.
    let cached = store
        .fetch(&id, ArtifactKind::Pdf, FetchPolicy::UseCache)
        .await
        .unwrap();
    assert!(matches!(cached, FetchOutcome::Cached(_)));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_legacy_identifier_keeps_its_archive_directory() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(vec![(200, pdf("legacy"))]);
    let store = store(&directory, &server);

    let outcome = store
        .fetch(
            &arxiv("hep-th/9901001"),
            ArtifactKind::Pdf,
            FetchPolicy::UseCache,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.path(),
        directory
            .path()
            .join("bibi/documents/arxiv/hep-th/9901001/paper.pdf")
    );
    assert_eq!(server.requests(), ["/pdf/hep-th/9901001"]);
}

#[tokio::test]
async fn forcing_replaces_the_cached_pdf_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(vec![(200, pdf("first")), (200, pdf("second"))]);
    let store = store(&directory, &server);
    let id = arxiv("1207.7214");

    let path = store
        .fetch(&id, ArtifactKind::Pdf, FetchPolicy::UseCache)
        .await
        .unwrap()
        .path()
        .to_path_buf();
    assert!(std::fs::read_to_string(&path).unwrap().contains("first"));

    store
        .fetch(&id, ArtifactKind::Pdf, FetchPolicy::Force)
        .await
        .unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("second"));
    // No debris beside the replaced artifact.
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}

#[tokio::test]
async fn a_source_package_is_extracted_and_replaced_as_a_whole_tree() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(vec![
        (200, archive(&[("paper.tex", b"first")])),
        (
            200,
            archive(&[("paper.tex", b"second"), ("extra.tex", b"new")]),
        ),
    ]);
    let store = store(&directory, &server);
    let id = arxiv("1207.7214");

    let path = store
        .fetch(&id, ArtifactKind::Source, FetchPolicy::UseCache)
        .await
        .unwrap()
        .path()
        .to_path_buf();
    assert_eq!(
        path,
        directory
            .path()
            .join("bibi/documents/arxiv/1207.7214/source")
    );
    assert_eq!(
        std::fs::read_to_string(path.join("paper.tex")).unwrap(),
        "first"
    );

    store
        .fetch(&id, ArtifactKind::Source, FetchPolicy::Force)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(path.join("paper.tex")).unwrap(),
        "second"
    );
    assert!(path.join("extra.tex").is_file());
    // The replaced tree is gone, not left beside the new one.
    let siblings = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
    assert_eq!(siblings, 1);
}

#[tokio::test]
async fn a_response_that_is_not_the_artifact_is_refused_and_nothing_is_published() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(vec![
        (200, b"<html>withdrawn</html>".to_vec()),
        (200, b"not a gzip archive".to_vec()),
        (404, Vec::new()),
    ]);
    let store = store(&directory, &server);
    let id = arxiv("1207.7214");

    assert!(
        store
            .fetch(&id, ArtifactKind::Pdf, FetchPolicy::UseCache)
            .await
            .is_err()
    );
    assert!(
        store
            .fetch(&id, ArtifactKind::Source, FetchPolicy::UseCache)
            .await
            .is_err()
    );
    let missing = store
        .fetch(&id, ArtifactKind::Pdf, FetchPolicy::UseCache)
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("has no PDF"));
    // Nothing was published for any of them.
    assert!(
        !directory
            .path()
            .join("bibi/documents/arxiv/1207.7214/paper.pdf")
            .exists()
    );
}

#[tokio::test]
async fn cleaning_reports_before_it_removes_and_stays_inside_its_own_subtree() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(vec![
        (200, pdf("first")),
        (200, archive(&[("paper.tex", b"source")])),
    ]);
    let store = store(&directory, &server);
    store
        .fetch(
            &arxiv("1207.7214"),
            ArtifactKind::Pdf,
            FetchPolicy::UseCache,
        )
        .await
        .unwrap();
    store
        .fetch(
            &arxiv("2401.00001"),
            ArtifactKind::Source,
            FetchPolicy::UseCache,
        )
        .await
        .unwrap();

    // Something else's file in the shared cache root must survive.
    let neighbour = directory.path().join("bibi/not-bibis-business");
    std::fs::write(&neighbour, "keep me").unwrap();

    let preview = store.clean(CleanMode::DryRun).unwrap();
    assert_eq!(preview.files, 2);
    assert!(preview.bytes > 0);
    assert!(!preview.removed);
    assert!(store.cache_root().join("documents").exists());

    let removed = store.clean(CleanMode::All).unwrap();
    assert!(removed.removed);
    assert_eq!(removed.files, 2);
    assert!(!store.cache_root().join("documents").exists());
    assert_eq!(std::fs::read_to_string(&neighbour).unwrap(), "keep me");

    // Cleaning an already-clean cache is not an error.
    let again = store.clean(CleanMode::All).unwrap();
    assert_eq!(again.files, 0);
}

#[test]
fn a_cache_root_bibi_must_not_own_is_refused_at_construction() {
    for root in ["/", ""] {
        assert!(DocumentStore::new(root).is_err(), "{root}");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        assert!(DocumentStore::new(&home).is_err());
    }
}
