//! Hermetic client tests: batching, 429 retries, and texkey matching over a
//! local `TcpListener`.

use bibi_core::Locator;
use bibi_inspire_client::{Client, Error, RetryEvent};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn client(base: &str) -> Client {
    Client::builder()
        .base_url(base)
        .retry_fallback(Duration::ZERO)
        .build()
        .unwrap()
}

fn response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
}

/// Canned responses plus the request lines the server saw.
struct TestServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// Serve canned responses matched by request content.
///
/// Each response pairs with a substring of the request line (usually
/// `format=json` or `format=bibtex`), because the client fetches the two
/// halves concurrently and either can arrive first. When nothing matches, the
/// last served response repeats, which keeps rate-limit and not-found tests
/// deterministic for both request halves. The accept loop runs for the rest
/// of the test; callers read the shared request log once the client is done.
fn server(responses: Vec<(&'static str, String)>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let logged = Arc::clone(&requests);
    thread::spawn(move || {
        let mut pending = responses;
        let mut last: Option<String> = None;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            while let Ok(length) = stream.read(&mut chunk) {
                bytes.extend_from_slice(&chunk[..length]);
                if length == 0 || bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let line = String::from_utf8_lossy(&bytes)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            if line.is_empty() {
                continue;
            }
            let response = match pending
                .iter()
                .position(|(matcher, _)| line.contains(matcher))
            {
                Some(at) => pending.remove(at).1,
                None => last.clone().unwrap_or_else(|| {
                    response("500 Internal Server Error", "", "no canned response")
                }),
            };
            last = Some(response.clone());
            // The client may have cancelled its half of a concurrent pair;
            // a failed write is not a test failure.
            let _ = stream.write_all(response.as_bytes());
            logged.lock().unwrap().push(line);
        }
    });
    TestServer {
        base: format!("http://{address}/"),
        requests,
    }
}

fn json_record(id: u64) -> String {
    format!(
        r#"{{"id":"{id}","updated":"2026-01-01T00:00:00Z","metadata":{{"titles":[{{"title":"Title {id}"}}],"texkeys":["Key{id}"]}}}}"#
    )
}

fn json_record_with_doi(id: u64, doi: &str) -> String {
    format!(
        r#"{{"id":"{id}","updated":"2026-01-01T00:00:00Z","metadata":{{"titles":[{{"title":"Title {id}"}}],"texkeys":["Key{id}"],"dois":[{{"value":"{doi}"}}]}}}}"#
    )
}

fn search_json(ids: &[u64]) -> String {
    let hits = ids
        .iter()
        .map(|id| json_record(*id))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"hits":{{"hits":[{hits}]}}}}"#)
}

fn search_bibtex(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| format!("@misc{{Key{id},title={{Title {id}}}}}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Serve every connection in its own thread, answering each request after
/// `latency` so benchmarks can tell sequential from concurrent clients.
fn latency_server(latency: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            thread::spawn(move || {
                let mut bytes = [0; 32768];
                let length = stream.read(&mut bytes).unwrap();
                let line = String::from_utf8_lossy(&bytes[..length])
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned();
                thread::sleep(latency);
                let body = routed_body(&line);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
        }
    });
    format!("http://{address}/")
}

/// Answer a request line by shape: batched refresh searches, single-record
/// JSON, and single-record BibTeX.
fn routed_body(line: &str) -> String {
    if line.contains("format=bibtex") {
        match query_ids(line) {
            Some(ids) => search_bibtex(&ids),
            None => format!(
                "@misc{{Key{},title={{Title {}}}}}",
                path_id(line),
                path_id(line)
            ),
        }
    } else {
        match query_ids(line) {
            Some(ids) => search_json(&ids),
            None => json_record(path_id(line)),
        }
    }
}

/// The record ids in a `control_number:<id> or ...` refresh query, when the
/// request carries one.
fn query_ids(line: &str) -> Option<Vec<u64>> {
    let target = line.split_whitespace().nth(1)?;
    let query = target.split("q=").nth(1)?.split('&').next()?;
    let query = query.replace("%3A", ":").replace('+', " ");
    if !query.contains("control_number") {
        return None;
    }
    Some(
        query
            .split(" or ")
            .filter_map(|part| part.strip_prefix("control_number:")?.parse().ok())
            .collect(),
    )
}

/// The numeric id in `/api/literature/<id>?...`.
fn path_id(line: &str) -> u64 {
    let target = line.split_whitespace().nth(1).unwrap();
    target
        .trim_start_matches('/')
        .split('/')
        .nth(2)
        .and_then(|segment| segment.split('?').next())
        .and_then(|segment| segment.parse().ok())
        .unwrap()
}

/// Quantify what sync spends on network latency: every locator costs two
/// round trips (JSON plus BibTeX), so sequential resolution grows linearly
/// with the bibliography while bounded concurrency stays flat.
#[tokio::test]
async fn resolves_many_locators_with_bounded_concurrency() {
    let base = latency_server(Duration::from_millis(50));
    let client = client(&base);
    let locators = (1..=16).map(Locator::Inspire).collect::<Vec<_>>();

    let start = Instant::now();
    for locator in &locators {
        client.resolve_snapshot(locator).await.unwrap();
    }
    let sequential = start.elapsed();

    let start = Instant::now();
    let resolved = client.resolve_snapshots(&locators).await;
    let concurrent = start.elapsed();
    assert_eq!(resolved.len(), locators.len());
    assert!(resolved.iter().all(|result| result.is_ok()));

    eprintln!(
        "resolving {} locators: sequential {sequential:?}, concurrent {concurrent:?}",
        locators.len()
    );
    assert!(
        concurrent < sequential / 3,
        "concurrency should hide per-request latency: {sequential:?} vs {concurrent:?}"
    );
}

/// Refresh joins the JSON and BibTeX halves of every batch instead of paying
/// for them one after the other.
#[tokio::test]
async fn refresh_fetches_json_and_bibtex_concurrently() {
    let base = latency_server(Duration::from_millis(100));
    let client = client(&base);
    let ids = (1..=50).collect::<Vec<_>>();
    let start = Instant::now();
    let records = client.refresh_records(&ids).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(records.len(), ids.len());
    eprintln!("refreshing {} records in one batch: {elapsed:?}", ids.len());
    assert!(
        elapsed < Duration::from_millis(180),
        "two sequential 100ms requests would take 200ms: {elapsed:?}"
    );
}

#[tokio::test]
async fn json_only_resolution_performs_exactly_one_request() {
    let server = server(vec![(
        "format=json",
        response("200 OK", "", &json_record(42)),
    )]);
    let reference = client(&server.base)
        .resolve_reference(&Locator::Inspire(42))
        .await
        .unwrap();
    assert_eq!(reference.title, "Title 42");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("format=json"), "{}", requests[0]);
    assert!(!requests[0].contains("format=bibtex"), "{}", requests[0]);
}

#[tokio::test]
async fn retries_a_429_using_retry_after_and_succeeds() {
    let server = server(vec![
        (
            "format=json",
            response("429 Too Many Requests", "Retry-After: 0\r\n", ""),
        ),
        ("format=json", response("200 OK", "", &json_record(42))),
    ]);
    let reference = client(&server.base)
        .resolve_reference(&Locator::Inspire(42))
        .await
        .unwrap();
    assert_eq!(reference.title, "Title 42");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test]
async fn retry_observer_receives_each_event_before_retry() {
    let server = server(vec![
        (
            "format=json",
            response("429 Too Many Requests", "Retry-After: 0\r\n", ""),
        ),
        (
            "format=json",
            response("429 Too Many Requests", "Retry-After: 0\r\n", ""),
        ),
        ("format=json", response("200 OK", "", &json_record(42))),
    ]);
    let events = Arc::new(Mutex::new(Vec::<RetryEvent>::new()));
    let observed = Arc::clone(&events);
    let client = Client::builder()
        .base_url(&server.base)
        .retry_fallback(Duration::ZERO)
        .on_retry(move |event| observed.lock().unwrap().push(event.clone()))
        .build()
        .unwrap();
    client
        .resolve_reference(&Locator::Inspire(42))
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 3);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].resource, "inspire:42");
    assert_eq!(events[0].delay, Duration::ZERO);
    assert_eq!(events[0].attempt, 1);
    assert_eq!(events[1].attempt, 2);
    assert!(events.iter().all(|event| event.max_retries == 3));
}

#[tokio::test]
async fn gives_up_after_three_429_retries_with_the_response_body() {
    let responses = (0..4)
        .map(|_| {
            (
                "format=json",
                response("429 Too Many Requests", "", "slow down"),
            )
        })
        .collect();
    let server = server(responses);
    let error = client(&server.base)
        .resolve_reference(&Locator::Inspire(42))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            Error::HttpStatus { status, body } if status.as_u16() == 429 && body == "slow down"
        ),
        "{error}"
    );
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn refresh_splits_batches_at_one_hundred_records() {
    let first = (1..=100).collect::<Vec<u64>>();
    let second = [101];
    let server = server(vec![
        ("format=json", response("200 OK", "", &search_json(&first))),
        (
            "format=bibtex",
            response("200 OK", "", &search_bibtex(&first)),
        ),
        ("format=json", response("200 OK", "", &search_json(&second))),
        (
            "format=bibtex",
            response("200 OK", "", &search_bibtex(&second)),
        ),
    ]);
    let ids = (1..=101).collect::<Vec<u64>>();
    let snapshots = client(&server.base).refresh_records(&ids).await.unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.record_id)
            .collect::<Vec<_>>(),
        ids
    );
    // Each batch fetches its JSON and BibTeX concurrently, so the two halves
    // of a batch can arrive in either order.
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let batch_one = &requests[..2];
    assert!(
        batch_one.iter().all(|request| request.contains("size=100")),
        "{batch_one:?}"
    );
    assert!(
        batch_one
            .iter()
            .any(|request| request.contains("format=json"))
            && batch_one
                .iter()
                .any(|request| request.contains("format=bibtex")),
        "{batch_one:?}"
    );
    let batch_two = &requests[2..];
    assert!(
        batch_two.iter().all(|request| {
            request.contains("control_number%3A101") && request.contains("size=1")
        }),
        "{batch_two:?}"
    );
}

#[tokio::test]
async fn resolve_snapshot_rejects_bibtex_response_with_zero_entries() {
    let server = server(vec![
        ("format=json", response("200 OK", "", &json_record(42))),
        ("format=bibtex", response("200 OK", "", "")),
    ]);
    let error = client(&server.base)
        .resolve_snapshot(&Locator::Inspire(42))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("0 BibTeX entries"), "{error}");
}

#[tokio::test]
async fn resolve_snapshot_rejects_bibtex_response_with_two_entries() {
    let server = server(vec![
        ("format=json", response("200 OK", "", &json_record(42))),
        (
            "format=bibtex",
            response("200 OK", "", "@misc{A,title={A}}\n\n@misc{B,title={B}}"),
        ),
    ]);
    let error = client(&server.base)
        .resolve_snapshot(&Locator::Inspire(42))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("2 BibTeX entries"), "{error}");
}

#[tokio::test]
async fn resolve_snapshot_rejects_bibtex_key_not_among_json_texkeys() {
    let server = server(vec![
        ("format=json", response("200 OK", "", &json_record(42))),
        (
            "format=bibtex",
            response("200 OK", "", "@misc{Other,title={Title 42}}"),
        ),
    ]);
    let error = client(&server.base)
        .resolve_snapshot(&Locator::Inspire(42))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("is not one of the INSPIRE texkeys"),
        "{error}"
    );
}

#[tokio::test]
async fn resolve_snapshot_rejects_identity_mismatch_between_json_and_bibtex() {
    let server = server(vec![
        (
            "format=json",
            response("200 OK", "", &json_record_with_doi(42, "10.1/mismatch")),
        ),
        (
            "format=bibtex",
            response(
                "200 OK",
                "",
                "@misc{Key42,title={Title 42},doi={10.1/actual}}",
            ),
        ),
    ]);
    let error = client(&server.base)
        .resolve_snapshot(&Locator::Inspire(42))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("do not identify the same record"),
        "{error}"
    );
}

#[tokio::test]
async fn rejects_a_record_whose_texkeys_match_no_bibtex_entry() {
    let server = server(vec![
        ("format=json", response("200 OK", "", &search_json(&[7]))),
        (
            "format=bibtex",
            response("200 OK", "", "@misc{Other,title={Other}}"),
        ),
    ]);
    let error = client(&server.base)
        .refresh_records(&[7])
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("matched 0 BibTeX entries"),
        "{error}"
    );
}

#[tokio::test]
async fn stores_the_bibtex_matched_texkey_not_the_first_json_texkey() {
    let record_json = concat!(
        r#"{"id":"7","updated":"2026-01-01T00:00:00Z","metadata":{"titles":[{"title":"Title 7"}],"#,
        r#""texkeys":["Wrong:2026","Right:2026"]}}"#
    );
    let search = format!(r#"{{"hits":{{"hits":[{record_json}]}}}}"#);
    let server = server(vec![
        ("format=json", response("200 OK", "", &search)),
        (
            "format=bibtex",
            response("200 OK", "", "@misc{Right:2026,title={Title 7}}"),
        ),
    ]);
    let records = client(&server.base).refresh_records(&[7]).await.unwrap();
    assert_eq!(records[0].texkey, "Right:2026");
}

#[tokio::test]
async fn rejects_unexplained_refresh_results() {
    let server = server(vec![
        ("format=json", response("200 OK", "", &search_json(&[7, 8]))),
        (
            "format=bibtex",
            response("200 OK", "", &search_bibtex(&[7])),
        ),
    ]);
    let error = client(&server.base)
        .refresh_records(&[7])
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unexplained refresh results"),
        "{error}"
    );
}
