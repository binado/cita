//! A local listener that answers a scripted sequence, so the CLI suite can
//! exercise provider-backed commands without touching the real network.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

pub struct TestServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    /// Serve `replies` in order, then stop accepting.
    pub fn new(replies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding a test server");
        let base_url = format!("http://{}/", listener.local_addr().expect("local address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);

        thread::spawn(move || {
            for body in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().expect("cloning the stream"));
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
                log.lock().expect("request log").push(
                    request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned(),
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        Self { base_url, requests }
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("request log").clone()
    }
}

/// A one-record INSPIRE search response.
pub fn hits(records: &[serde_json::Value]) -> String {
    serde_json::json!({"hits": {"hits": records}}).to_string()
}

/// A literature record with the fields bibi maps.
pub fn record(id: u64, texkey: &str, arxiv: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "updated": "2026-01-01T00:00:00+00:00",
        "metadata": {
            "control_number": id,
            "texkeys": [texkey],
            "titles": [{"title": title}],
            "authors": [{"full_name": "Aad, G."}],
            "collaborations": [{"value": "ATLAS"}],
            "publication_info": [{"year": 2012}],
            "arxiv_eprints": [{"value": arxiv}],
            "dois": [{"value": "10.1016/j.physletb.2012.08.020"}]
        }
    })
}
