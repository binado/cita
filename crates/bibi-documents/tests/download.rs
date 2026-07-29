//! Retrieval against a local server: naming, validation, and never clobbering.

use bibi_core::ArxivId;
use bibi_documents::{ArtifactClient, ArtifactKind, Error, default_filename};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

/// The pair a chunked download reports: bytes received so far and the
/// `Content-Length` advertised by the server, if any.
type ProgressEvent = (usize, Option<u64>);

/// The shared cell a `download_with_progress` test writes into.
type ProgressSink = Arc<Mutex<Vec<ProgressEvent>>>;

/// A server answering a scripted sequence of bodies.
struct Server {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

enum Reply {
    Fixed(u16, Vec<u8>),
    Chunked(Vec<Vec<u8>>),
}

impl Server {
    fn new(replies: Vec<(u16, Vec<u8>)>) -> Self {
        Self::serve(
            replies
                .into_iter()
                .map(|(status, body)| Reply::Fixed(status, body))
                .collect(),
        )
    }

    fn chunked(chunks: Vec<Vec<u8>>) -> Self {
        Self::serve(vec![Reply::Chunked(chunks)])
    }

    fn serve(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding a test server");
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        thread::spawn(move || {
            for reply in replies {
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
                match reply {
                    Reply::Fixed(status, body) => {
                        let mut response = format!(
                            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        response.extend_from_slice(&body);
                        let _ = stream.write_all(&response);
                    }
                    Reply::Chunked(chunks) => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                        );
                        for chunk in chunks {
                            let _ = write!(stream, "{:x}\r\n", chunk.len());
                            let _ = stream.write_all(&chunk);
                            let _ = stream.write_all(b"\r\n");
                        }
                        let _ = stream.write_all(b"0\r\n\r\n");
                    }
                }
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

/// A minimal gzip member: enough magic bytes to pass the kind check.
fn gzip(body: &str) -> Vec<u8> {
    let mut bytes = vec![0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 0x03];
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

fn client(server: &Server) -> ArtifactClient {
    ArtifactClient::builder()
        .base_url(&server.base_url)
        .build()
        .unwrap()
}

fn arxiv(value: &str) -> ArxivId {
    ArxivId::new(value).unwrap()
}

/// A temporary directory plus the destination inside it.
fn destination(name: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(name);
    (directory, path)
}

/// Files beside the destination, so a leftover temporary would be visible.
fn siblings(path: &Path) -> usize {
    std::fs::read_dir(path.parent().unwrap()).unwrap().count()
}

#[tokio::test]
async fn a_download_lands_under_the_name_the_caller_chose() {
    let server = Server::new(vec![(200, pdf("body"))]);
    let (_directory, path) = destination("wherever.pdf");

    client(&server)
        .download(&arxiv("1207.7214"), ArtifactKind::Pdf, &path)
        .await
        .unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), pdf("body"));
    // Requested under the versionless identifier, from the pdf endpoint.
    assert_eq!(server.requests(), vec!["/pdf/1207.7214"]);
    // Nothing else was written: no temporary survived the publish.
    assert_eq!(siblings(&path), 1);
}

#[tokio::test]
async fn the_default_name_is_the_identifier_and_the_kind() {
    let server = Server::new(vec![(200, gzip("archive"))]);
    let directory = tempfile::tempdir().unwrap();
    let id = arxiv("hep-th/9901001v3");
    let path = directory
        .path()
        .join(default_filename(&id, ArtifactKind::Source));

    client(&server)
        .download(&id, ArtifactKind::Source, &path)
        .await
        .unwrap();

    assert!(path.ends_with("hep-th-9901001.tar.gz"));
    assert!(path.exists());
    // A legacy identifier is one filename but two URL path components.
    assert_eq!(server.requests(), vec!["/e-print/hep-th/9901001"]);
}

#[tokio::test]
async fn an_existing_destination_is_never_replaced() {
    let server = Server::new(vec![(200, pdf("new"))]);
    let (_directory, path) = destination("1207.7214.pdf");
    std::fs::write(&path, "mine").unwrap();

    let error = client(&server)
        .download(&arxiv("1207.7214"), ArtifactKind::Pdf, &path)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::DestinationExists { .. }));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
    // The refusal is decided before the request, so nothing was downloaded.
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn a_holding_page_is_refused_and_writes_nothing() {
    let server = Server::new(vec![(200, b"<!DOCTYPE html><html>".to_vec())]);
    let (_directory, path) = destination("1207.7214.pdf");

    let error = client(&server)
        .download(&arxiv("1207.7214"), ArtifactKind::Pdf, &path)
        .await
        .unwrap_err();

    // arXiv answers with HTML for a withdrawn work; that is not a PDF, and a
    // rejected response must leave no file and no temporary behind.
    assert!(matches!(error, Error::InvalidArtifact { .. }));
    assert!(!path.exists());
    assert_eq!(siblings(&path), 0);
}

#[tokio::test]
async fn a_pdf_is_not_a_source_archive() {
    let server = Server::new(vec![(200, pdf("body"))]);
    let (_directory, path) = destination("1207.7214.tar.gz");

    let error = client(&server)
        .download(&arxiv("1207.7214"), ArtifactKind::Source, &path)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::InvalidArtifact { .. }));
    assert!(!path.exists());
}

#[tokio::test]
async fn a_missing_artifact_says_so_rather_than_reporting_a_status() {
    let server = Server::new(vec![(404, Vec::new())]);
    let (_directory, path) = destination("2401.00001.pdf");

    let error = client(&server)
        .download(&arxiv("2401.00001"), ArtifactKind::Pdf, &path)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::NotFound { .. }));
    assert!(!path.exists());
}

#[tokio::test]
async fn any_other_status_is_reported_with_its_code() {
    let server = Server::new(vec![(503, Vec::new())]);
    let (_directory, path) = destination("2401.00001.pdf");

    let error = client(&server)
        .download(&arxiv("2401.00001"), ArtifactKind::Pdf, &path)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Status { status: 503, .. }));
    assert!(!path.exists());
}

#[tokio::test]
async fn a_response_that_grows_past_the_bound_is_abandoned() {
    // Chunked, so no Content-Length announces the size in advance and the
    // bound has to hold while the body streams in.
    let server = Server::chunked(vec![pdf("x"), vec![b'y'; 4096]]);
    let (_directory, path) = destination("2401.00001.pdf");

    let error = ArtifactClient::builder()
        .base_url(&server.base_url)
        .max_bytes(64)
        .build()
        .unwrap()
        .download(&arxiv("2401.00001"), ArtifactKind::Pdf, &path)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::ArtifactTooLarge { limit: 64, .. }));
    assert!(!path.exists());
    assert_eq!(siblings(&path), 0);
}

#[tokio::test]
async fn the_progress_callback_receives_a_length_and_every_chunk() {
    // A chunked reply, so the receiver sees the body in several pieces and the
    // `Content-Length` is intentionally absent.
    let body = pdf(&"x".repeat(60));
    let chunks: Vec<Vec<u8>> = body.chunks(20).map(<[u8]>::to_vec).collect();
    let chunk_count = chunks.len();
    let server = Server::chunked(chunks);

    let (_directory, path) = destination("2401.00001.pdf");
    let record: ProgressSink = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&record);

    client(&server)
        .download_with_progress(
            &arxiv("2401.00001"),
            ArtifactKind::Pdf,
            &path,
            move |bytes, total| recorder.lock().unwrap().push((bytes, total)),
        )
        .await
        .unwrap();

    let events = record.lock().unwrap().clone();
    // At least one event was emitted per chunk, beginning before any bytes.
    assert!(events.len() > chunk_count, "{events:?}");
    // First event: position 0, total unknown (chunked).
    assert_eq!(events[0], (0, None));
    // Last event: full body length, total still unknown.
    let last = *events.last().unwrap();
    assert_eq!(last, (body.len(), None));
    // The position never goes backwards between events.
    for pair in events.windows(2) {
        assert!(pair[1].0 >= pair[0].0, "{events:?}");
    }
}

#[tokio::test]
async fn the_progress_callback_reports_a_known_total_when_the_server_announces_one() {
    let body = pdf("abcdefghijklmnopqrstuvwxyz");
    let server = Server::new(vec![(200, body.clone())]);

    let (_directory, path) = destination("2401.00001.pdf");
    let record: ProgressSink = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&record);

    client(&server)
        .download_with_progress(
            &arxiv("2401.00001"),
            ArtifactKind::Pdf,
            &path,
            move |bytes, total| recorder.lock().unwrap().push((bytes, total)),
        )
        .await
        .unwrap();

    let events = record.lock().unwrap().clone();
    assert!(!events.is_empty(), "{events:?}");
    let total = body.len();
    // Every event carries the same advertised total once a Content-Length header has set it.
    for (bytes, announced) in &events {
        assert_eq!(*announced, Some(total as u64), "{events:?}");
        assert!(*bytes <= total, "{events:?}");
    }
    // And the final position reaches the advertised total.
    assert_eq!(events.last().unwrap().0, total);
}
