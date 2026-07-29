//! A local HTTP server that answers like INSPIRE, and records what it was asked.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

/// One canned response.
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<String>,
}

impl Reply {
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            retry_after: None,
        }
    }

    pub fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            retry_after: None,
        }
    }

    pub fn rate_limited(retry_after: Option<&str>) -> Self {
        Self {
            status: 429,
            body: "slow down".into(),
            retry_after: retry_after.map(ToOwned::to_owned),
        }
    }
}

/// A server that answers requests from a script, in order.
pub struct TestServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    /// Serve `replies` in order, then close.
    ///
    /// Hermetic by construction: the suite never depends on the real network,
    /// and a test that issues more requests than it scripted fails by hanging
    /// up rather than by silently succeeding.
    pub fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding a test server");
        let base_url = format!("http://{}/", listener.local_addr().expect("local address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);

        thread::spawn(move || {
            for reply in replies {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut stream = stream;
                let mut reader = BufReader::new(stream.try_clone().expect("cloning the stream"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }
                // Drain the headers so the client sees a clean exchange.
                loop {
                    let mut header = String::new();
                    match reader.read_line(&mut header) {
                        Ok(0) => break,
                        Ok(_) if header.trim().is_empty() => break,
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                let target = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                log.lock().expect("request log").push(target);

                let mut response = format!(
                    "HTTP/1.1 {} X\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n",
                    reply.status,
                    reply.body.len()
                );
                if let Some(retry_after) = &reply.retry_after {
                    response.push_str(&format!("Retry-After: {retry_after}\r\n"));
                }
                response.push_str("Connection: close\r\n\r\n");
                response.push_str(&reply.body);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        Self { base_url, requests }
    }

    /// Every request target the server received, in order.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("request log").clone()
    }

    /// The decoded query of the nth request.
    pub fn query(&self, index: usize) -> String {
        let target = self.requests().get(index).cloned().unwrap_or_default();
        percent_decode(&target)
    }
}

/// Enough percent-decoding to read a query back in a test assertion.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("20");
                decoded.push(u8::from_str_radix(hex, 16).unwrap_or(b' '));
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
