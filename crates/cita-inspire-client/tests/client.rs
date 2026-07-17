//! Hermetic client tests: batching, 429 retries, and texkey matching over a
//! local `TcpListener`.

use cita_core::Locator;
use cita_inspire_client::{Client, Error};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
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

/// Serve each canned response to one connection and return the request lines.
fn server(responses: Vec<String>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        responses
            .into_iter()
            .map(|response| {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let length = stream.read(&mut chunk).unwrap();
                    bytes.extend_from_slice(&chunk[..length]);
                    if length == 0 || bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                stream.write_all(response.as_bytes()).unwrap();
                String::from_utf8_lossy(&bytes)
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    });
    (format!("http://{address}/"), handle)
}

fn json_record(id: u64) -> String {
    format!(
        r#"{{"id":"{id}","updated":"2026-01-01T00:00:00Z","metadata":{{"titles":[{{"title":"Title {id}"}}],"texkeys":["Key{id}"]}}}}"#
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

#[tokio::test]
async fn retries_a_429_using_retry_after_and_succeeds() {
    let (base, handle) = server(vec![
        response("429 Too Many Requests", "Retry-After: 0\r\n", ""),
        response("200 OK", "", &json_record(42)),
    ]);
    let reference = client(&base)
        .resolve_reference(&Locator::Inspire(42))
        .await
        .unwrap();
    assert_eq!(reference.title, "Title 42");
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test]
async fn gives_up_after_three_429_retries_with_the_response_body() {
    let responses = (0..4)
        .map(|_| response("429 Too Many Requests", "", "slow down"))
        .collect();
    let (base, handle) = server(responses);
    let error = client(&base)
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
    assert_eq!(handle.join().unwrap().len(), 4);
}

#[tokio::test]
async fn refresh_splits_batches_at_one_hundred_records() {
    let first = (1..=100).collect::<Vec<u64>>();
    let second = [101];
    let (base, handle) = server(vec![
        response("200 OK", "", &search_json(&first)),
        response("200 OK", "", &search_bibtex(&first)),
        response("200 OK", "", &search_json(&second)),
        response("200 OK", "", &search_bibtex(&second)),
    ]);
    let ids = (1..=101).collect::<Vec<u64>>();
    let snapshots = client(&base).refresh_records(&ids).await.unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.record_id)
            .collect::<Vec<_>>(),
        ids
    );
    let requests = handle.join().unwrap();
    assert!(
        requests[0].contains("format=json") && requests[0].contains("size=100"),
        "{}",
        requests[0]
    );
    assert!(requests[1].contains("format=bibtex"), "{}", requests[1]);
    assert!(
        requests[2].contains("control_number%3A101") && requests[2].contains("size=1"),
        "{}",
        requests[2]
    );
}

#[tokio::test]
async fn rejects_a_record_whose_texkeys_match_no_bibtex_entry() {
    let (base, handle) = server(vec![
        response("200 OK", "", &search_json(&[7])),
        response("200 OK", "", "@misc{Other,title={Other}}"),
    ]);
    let error = client(&base).refresh_records(&[7]).await.unwrap_err();
    assert!(
        error.to_string().contains("matched 0 BibTeX entries"),
        "{error}"
    );
    handle.join().unwrap();
}

#[tokio::test]
async fn rejects_unexplained_refresh_results() {
    let (base, handle) = server(vec![
        response("200 OK", "", &search_json(&[7, 8])),
        response("200 OK", "", &search_bibtex(&[7])),
    ]);
    let error = client(&base).refresh_records(&[7]).await.unwrap_err();
    assert!(
        error.to_string().contains("unexplained refresh results"),
        "{error}"
    );
    handle.join().unwrap();
}
