use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::TempDir;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

struct LspClient {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    refresh_requests: usize,
    lens_refresh_requests: usize,
}

impl LspClient {
    fn start(db_path: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_webspec-index"))
            .arg("lsp")
            .env("SPEC_INDEX_TEST_DB", db_path)
            .env("CODEX_SANDBOX", "1")
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .env("NO_PROXY", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start webspec-index lsp");
        let stdin = child.stdin.take().expect("LSP stdin");
        let stdout = child.stdout.take().expect("LSP stdout");
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = read_message(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            messages,
            refresh_requests: 0,
            lens_refresh_requests: 0,
        }
    }

    fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).expect("serialize JSON-RPC message");
        let stdin = self.stdin.as_mut().expect("LSP stdin is open");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        stdin.write_all(&body).unwrap();
        stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.response(id)
    }

    fn response(&mut self, id: u64) -> Value {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let message = self.recv_until(deadline);
            if message.get("method").is_some() {
                self.handle_server_message(message);
                continue;
            }
            if message.get("id") == Some(&json!(id)) {
                assert!(
                    message.get("error").is_none(),
                    "LSP error response: {message}"
                );
                return message.get("result").cloned().unwrap_or(Value::Null);
            }
        }
    }

    fn wait_for_refresh(&mut self) {
        self.wait_for_refresh_after(0);
    }

    fn request_error(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let message = self.recv_until(deadline);
            if message.get("method").is_some() {
                self.handle_server_message(message);
                continue;
            }
            if message["id"] == id {
                assert!(
                    message.get("error").is_some(),
                    "expected an error: {message}"
                );
                return message["error"].clone();
            }
        }
    }

    fn wait_for_refresh_after(&mut self, previous: usize) {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        while self.refresh_requests <= previous {
            let message = self.recv_until(deadline);
            self.handle_server_message(message);
        }
    }

    fn wait_for_notification(&mut self, method: &str) -> Value {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let message = self.recv_until(deadline);
            if message.get("method").and_then(Value::as_str) == Some(method)
                && message.get("id").is_none()
            {
                return message;
            }
            self.handle_server_message(message);
        }
    }

    fn assert_no_refresh_for(&mut self, duration: Duration) {
        let previous = self.refresh_requests;
        let deadline = Instant::now() + duration;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self.messages.recv_timeout(remaining) {
                Ok(message) => self.handle_server_message(message),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(error) => panic!("LSP output closed unexpectedly: {error}"),
            }
        }
        assert_eq!(
            self.refresh_requests, previous,
            "a cached inlay hint scheduled another effects refresh"
        );
    }

    fn recv_until(&self, deadline: Instant) -> Value {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("timed out waiting for an LSP message");
        self.messages
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("timed out waiting for an LSP message: {error}"))
    }

    fn handle_server_message(&mut self, message: Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        let result = match method {
            "workspace/inlayHint/refresh" => {
                self.refresh_requests += 1;
                Value::Null
            }
            "workspace/codeLens/refresh" => {
                self.lens_refresh_requests += 1;
                Value::Null
            }
            "workspace/configuration" => {
                let count = message["params"]["items"].as_array().map_or(0, Vec::len);
                Value::Array(vec![json!({}); count])
            }
            _ => panic!("unexpected server request: {message}"),
        };
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn stop(mut self) {
        self.send(json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}));
        let _ = self.response(99);
        self.send(json!({"jsonrpc": "2.0", "method": "exit"}));
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("LSP process did not exit after shutdown");
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn read_message(reader: &mut BufReader<impl Read>) -> Option<Value> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; content_length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn seed_spec(conn: &Connection, name: &str, base_url: &str, html: &str) {
    let sha = format!("hash:lsp-{name}");
    let parsed = webspec_index::parse::parse_spec(html, name, base_url).unwrap();
    let structure = webspec_index::parse::steps::extract_step_structure(html, name, base_url, &sha);
    let spec_id =
        webspec_index::db::write::insert_or_get_spec(conn, name, base_url, "test").unwrap();
    let snapshot_id =
        webspec_index::db::write::insert_snapshot(conn, spec_id, &sha, "2026-09-13T00:00:00Z")
            .unwrap();
    webspec_index::db::write::insert_sections_bulk(conn, snapshot_id, &parsed.sections).unwrap();
    webspec_index::db::write::insert_refs_bulk(conn, snapshot_id, &parsed.references).unwrap();
    webspec_index::db::write::insert_idl_defs_bulk(conn, snapshot_id, &parsed.idl_definitions)
        .unwrap();
    webspec_index::db::effects::store_structure(
        conn,
        snapshot_id,
        webspec_index::parse::steps::STRUCTURE_VERSION,
        &serde_json::to_string(&structure).unwrap(),
    )
    .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    webspec_index::db::write::record_update_check(
        conn,
        spec_id,
        &now,
        Some(&now),
        Some("lsp-fixture"),
        Some(webspec_index::parse::INDEX_VERSION),
    )
    .unwrap();
}

fn fixture_db() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open(dir.path().join("index.db")).unwrap();
    webspec_index::db::schema::initialize_schema(&conn).unwrap();
    webspec_index::db::schema::run_migrations(&conn).unwrap();
    webspec_index::db::schema::set_indexed_version(&conn, webspec_index::parse::INDEX_VERSION)
        .unwrap();
    seed_spec(
        &conn,
        "DOM",
        "https://dom.spec.whatwg.org",
        r#"<p><dfn id="concept-event-fire">fire an event</dfn> is a primitive.</p>"#,
    );
    seed_spec(
        &conn,
        "HTML",
        "https://html.spec.whatwg.org",
        r#"
          <div class="algorithm">
            <p>To <dfn id="integration-effect">run the integration effect</dfn>:</p>
            <ol>
              <li><p><a href="https://html.spec.whatwg.org/#queue-a-microtask">Queue a microtask</a>.</p></li>
            </ol>
          </div>
          <p><dfn id="queue-a-microtask">queue a microtask</dfn> is a primitive.</p>
        "#,
    );
    assert!(
        !webspec_index::db::schema::purge_if_version_changed(
            &conn,
            webspec_index::parse::INDEX_VERSION,
        )
        .unwrap(),
        "fixture would be purged when the LSP opens it"
    );
    drop(conn);
    dir
}

#[test]
fn lsp_effect_hint_refreshes_once_then_serves_cached_details() {
    let db = fixture_db();
    let mut client = LspClient::start(&db.path().join("index.db"));
    let initialized = client.request(
        1,
        "initialize",
        json!({
            "processId": null,
            "rootUri": null,
            "capabilities": {"workspace": {"inlayHint": {"refreshSupport": true}, "codeLens": {"refreshSupport": true}}},
            "initializationOptions": {"effectsEnabled": true, "effectsMaxBadges": 3}
        }),
    );
    assert_eq!(initialized["capabilities"]["inlayHintProvider"], true);
    client.notify("initialized", json!({}));

    let uri = "file:///tmp/webspec-lsp-effects.cpp";
    let text = "// https://html.spec.whatwg.org/#integration-effect\nvoid f() {\n  // Step 1. Queue a microtask.\n}\n";
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {"uri": uri, "languageId": "cpp", "version": 1, "text": text}
        }),
    );
    client.wait_for_notification("textDocument/publishDiagnostics");

    let hint_params = json!({
        "textDocument": {"uri": uri},
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 20}}
    });
    let cold = client.request(2, "textDocument/inlayHint", hint_params.clone());
    assert_eq!(cold[0]["label"], " ✓", "unexpected cold hint: {cold}");
    client.wait_for_refresh();

    let conn = Connection::open(db.path().join("index.db")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM effect_runs", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "a cold automatic hint must not create an analysis run"
    );
    let lenses = client.request(
        10,
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    assert!(!lenses
        .as_array()
        .unwrap()
        .iter()
        .any(|lens| lens["command"]["command"] == "webspecLens.showEffects"));
    let cold_hover = client.request(
        11,
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": 2, "character": 17}}),
    );
    assert!(!cold_hover.to_string().contains("Possible effects"));
    assert!(!cold_hover.to_string().contains("No effects detected"));
    // A separate indexing/maintenance process prepares all summaries. An already
    // running editor must notice publication after previously caching a miss.
    let precompute = Command::new(env!("CARGO_BIN_EXE_webspec-index"))
        .args(["effects", "--all"])
        .env("SPEC_INDEX_TEST_DB", db.path().join("index.db"))
        .output()
        .unwrap();
    assert!(
        precompute.status.success(),
        "{}",
        String::from_utf8_lossy(&precompute.stderr)
    );
    let has_graph: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM effect_graph WHERE id=1)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(has_graph);
    let previous = client.refresh_requests;
    let _ = client.request(9, "textDocument/inlayHint", hint_params.clone());
    client.wait_for_refresh_after(previous);

    let warm = client.request(3, "textDocument/inlayHint", hint_params.clone());
    let label = warm[0]["label"].as_str().unwrap();
    assert!(label == " ✓", "unexpected hint: {warm}");
    let tooltip = warm[0]["tooltip"]["value"].as_str().unwrap();
    assert!(
        tooltip.contains("### Possible effects"),
        "unexpected tooltip: {warm}"
    );
    assert!(
        tooltip.contains("- may queue a microtask in `HTML#integration-effect:1`"),
        "unexpected tooltip: {warm}"
    );
    assert!(
        !tooltip.contains("### Effect traces"),
        "automatic hints must not reconstruct traces: {warm}"
    );

    assert!(!tooltip.contains("webspec-index"));
    assert!(!tooltip.contains("[partial]"));

    let hover = client.request(
        4,
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": 2, "character": 17}}),
    );
    let hover_markdown = hover["contents"]["value"].as_str().unwrap();
    assert!(
        hover_markdown.contains("- may queue a microtask in `HTML#integration-effect:1`"),
        "unexpected hover: {hover}"
    );
    assert!(
        !hover_markdown.contains("### Effect traces"),
        "automatic hovers must not reconstruct traces: {hover}"
    );

    assert!(
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM effect_graph WHERE id=1)",
            [],
            |row| row.get::<_, bool>(0)
        )
        .unwrap(),
        "graph must still be present after warm interactions"
    );

    let warm_lenses = client.request(
        12,
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    let effects_lenses: Vec<_> = warm_lenses
        .as_array()
        .unwrap()
        .iter()
        .filter(|lens| lens["command"]["command"] == "webspecLens.showEffects")
        .collect();
    assert_eq!(effects_lenses.len(), 1);
    let lens = effects_lenses[0];
    assert_eq!(
        lens["range"]["start"]["line"], 3,
        "the lens belongs below the comment"
    );
    assert_eq!(lens["command"]["title"], "May do async work");
    assert_eq!(
        lens["command"]["arguments"][0]["source_position"]["line"], 2,
        "hover must open at the step comment, not at the CodeLens row"
    );
    assert!(
        client.lens_refresh_requests > 0,
        "prepared rows must refresh CodeLens as well as hints"
    );
    let mut stale = lens["command"]["arguments"][0].clone();
    stale["document_version"] = json!(2);
    assert!(
        client.request_error(16, "webspec/preparedEffectDocument", stale)["message"]
            .as_str()
            .unwrap()
            .contains("source changed")
    );
    let mut stale = lens["command"]["arguments"][0].clone();
    stale["expected_sha"] = json!("hash:old");
    assert!(
        client.request_error(17, "webspec/preparedEffectDocument", stale)["message"]
            .as_str()
            .unwrap()
            .contains("spec changed")
    );
    let prepared = client.request(
        13,
        "webspec/preparedEffectDocument",
        lens["command"]["arguments"][0].clone(),
    );
    let details = prepared["effect_details"].as_array().unwrap();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["headline"], "may queue a microtask");
    assert!(details[0]["markdown"]
        .as_str()
        .unwrap()
        .contains("**Effect endpoint:**"));
    assert!(!details[0]["markdown"]
        .as_str()
        .unwrap()
        .contains("<details>"));
    let preview = prepared["markdown"].as_str().unwrap();
    assert!(preview.starts_with("# May do async work"));
    assert!(preview.contains("**Effect endpoint:**"));
    assert!(!preview.contains("ef_"));
    assert!(!preview.contains("--max-witness-states"));

    let document = client.request(
        6,
        "webspec/effectExplanationMarkdownAtPosition",
        json!({"textDocument": {"uri": uri}, "position": {"line": 2, "character": 17}}),
    );
    let markdown = document["markdown"].as_str().unwrap();
    assert_eq!(document["subject"]["step_path"], json!([1]));
    assert!(markdown.contains("<details>\n<summary>May queue a microtask (<code>ef_"));
    assert!(markdown.contains("### Effect traces"));
    assert!(!markdown.contains(" → "));
    assert!(!markdown.contains("May occur with timing"));
    let explicit_document = client.request(
        7,
        "webspec/effectExplanationMarkdown",
        json!({"schema_version": 1, "subject": {"spec": "HTML", "anchor": "integration-effect", "step_path": [1]}}),
    );
    assert_eq!(explicit_document, document);

    // The clickable preview must work even when the graph cannot be decoded.
    conn.execute(
        "UPDATE effect_runs SET artifact_json='invalid graph JSON'",
        [],
    )
    .unwrap();
    assert_eq!(
        client.request(
            14,
            "webspec/preparedEffectDocument",
            lens["command"]["arguments"][0].clone()
        ),
        prepared
    );

    let previous = client.refresh_requests;
    client.request(15, "textDocument/inlayHint", hint_params.clone());
    client.wait_for_refresh_after(previous);
    let cached = client.request(5, "textDocument/inlayHint", hint_params.clone());
    assert!(cached[0]["label"].as_str().unwrap().eq(" ✓"));
    client.assert_no_refresh_for(Duration::from_millis(600));
    client.stop();

    // A persistent read failure must not feed a refresh/resubmission loop.
    conn.execute("UPDATE effect_subjects SET summary_json='invalid JSON'", [])
        .unwrap();
    let mut client = LspClient::start(&db.path().join("index.db"));
    client.request(
        1,
        "initialize",
        json!({
            "processId": null, "rootUri": null,
            "capabilities": {"workspace": {"inlayHint": {"refreshSupport": true}}}
        }),
    );
    client.notify("initialized", json!({}));
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": uri, "languageId": "cpp", "version": 1, "text": text
        }}),
    );
    client.request(2, "textDocument/inlayHint", hint_params.clone());
    client.assert_no_refresh_for(Duration::from_millis(400));
    client.request(3, "textDocument/inlayHint", hint_params);
    client.assert_no_refresh_for(Duration::from_millis(400));
    client.stop();
}
