//! A local HTTP server standing in for spec origins in tests. Point
//! [`FreshnessOptions::origin`](super::freshness::FreshnessOptions) (or
//! `WEBSPEC_FETCH_ORIGIN`) at [`HttpStub::origin`]: `https://host/path` is then
//! requested as `{origin}/host/path`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Entry {
    body: String,
    etag: Option<String>,
    last_modified: Option<String>,
    status: Option<u16>,
    delay: Duration,
}

/// A request as the stub saw it: `(host_and_path, If-None-Match, If-Modified-Since)`.
pub type StubRequest = (String, Option<String>, Option<String>);

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    requests: Vec<StubRequest>,
}

pub struct HttpStub {
    address: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl HttpStub {
    /// Listen on an ephemeral port of 127.0.0.1, one thread per connection.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the HTTP stub");
        let address = listener.local_addr().expect("stub address");
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, &state);
                });
            }
        });
        Self { address, state }
    }

    /// `http://127.0.0.1:PORT`
    pub fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Serve `body` at `host_and_path` (`dom.spec.whatwg.org/`) with the given
    /// validators.
    pub fn put(
        &self,
        host_and_path: &str,
        body: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) {
        let mut state = self.state.lock().unwrap();
        let entry = state.entries.entry(host_and_path.to_owned()).or_default();
        entry.body = body.to_owned();
        entry.etag = etag.map(str::to_owned);
        entry.last_modified = last_modified.map(str::to_owned);
        entry.status = None;
    }

    /// Answer `host_and_path` with `status` and an empty body.
    pub fn fail(&self, host_and_path: &str, status: u16) {
        let mut state = self.state.lock().unwrap();
        state
            .entries
            .entry(host_and_path.to_owned())
            .or_default()
            .status = Some(status);
    }

    /// Wait `delay` before answering `host_and_path`.
    pub fn delay(&self, host_and_path: &str, delay: Duration) {
        let mut state = self.state.lock().unwrap();
        state
            .entries
            .entry(host_and_path.to_owned())
            .or_default()
            .delay = delay;
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<StubRequest> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let target = line.split_whitespace().nth(1).unwrap_or("/");
    let key = target.trim_start_matches('/').to_owned();
    let (mut if_none_match, mut if_modified_since) = (None, None);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let value = Some(value.trim().to_owned());
            if name.eq_ignore_ascii_case("if-none-match") {
                if_none_match = value;
            } else if name.eq_ignore_ascii_case("if-modified-since") {
                if_modified_since = value;
            }
        }
    }

    let delay = {
        let mut state = state.lock().unwrap();
        state.requests.push((
            key.clone(),
            if_none_match.clone(),
            if_modified_since.clone(),
        ));
        state.entries.get(&key).map_or(Duration::ZERO, |e| e.delay)
    };
    std::thread::sleep(delay);

    let response = {
        let state = state.lock().unwrap();
        match state.entries.get(&key) {
            None => respond(404, &[], ""),
            Some(Entry {
                status: Some(status),
                ..
            }) => respond(*status, &[], ""),
            Some(entry) => {
                let validators: Vec<_> = [
                    ("ETag", &entry.etag),
                    ("Last-Modified", &entry.last_modified),
                ]
                .into_iter()
                .filter_map(|(name, value)| value.as_deref().map(|v| (name, v)))
                .collect();
                let not_modified = (if_none_match.is_some() && if_none_match == entry.etag)
                    || (if_modified_since.is_some() && if_modified_since == entry.last_modified);
                if not_modified {
                    respond(304, &validators, "")
                } else {
                    respond(200, &validators, &entry.body)
                }
            }
        }
    };
    let mut stream = stream;
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn respond(status: u16, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!("HTTP/1.1 {status} Stub\r\n");
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    out
}
