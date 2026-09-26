//! Conditional-GET freshness checks for a batch of specs, run concurrently.

use super::{document_url, hash_bytes, http_client, is_respec_source, USER_AGENT};
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::StatusCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// A spec to check, with what the last check stored about it.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub spec_id: i64,
    pub spec_name: String,
    pub base_url: String,
    pub provider: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub source_hash: Option<String>,
    /// Whether [`FreshnessOptions::timeout`] caps the request. Only a
    /// conditional check has a cached snapshot to fall back to; first-time
    /// and forced downloads run to completion.
    pub time_limited: bool,
}

#[derive(Debug)]
pub(crate) enum Freshness {
    /// `304`: the stored document is current.
    NotModified {
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// `200` with the body already indexed; only the validators moved.
    SameContent {
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// A new document. `content_hash` is the hash of `html`, the document to
    /// parse; `source_hash` is the hash of the fetched body. They differ only
    /// for ReSpec sources, where `html` is the rendered document.
    Changed {
        html: String,
        content_hash: String,
        source_hash: String,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// Network error, timeout or error status. Nothing is known about the spec.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct FreshnessOptions {
    /// Per time-limited request, body included.
    pub timeout: Duration,
    pub max_in_flight: usize,
    /// Serve `https://host/path` from `{origin}/host/path` instead (tests).
    pub origin: Option<String>,
}

impl Default for FreshnessOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            max_in_flight: 64,
            origin: fetch_origin(),
        }
    }
}

/// The origin override from `WEBSPEC_FETCH_ORIGIN`.
pub(crate) fn fetch_origin() -> Option<String> {
    std::env::var("WEBSPEC_FETCH_ORIGIN")
        .ok()
        .filter(|origin| !origin.is_empty())
}

/// `url` as requested: with an origin, `https://host/path` becomes
/// `{origin}/host/path`.
pub(crate) fn effective_url(url: &str, origin: Option<&str>) -> String {
    let Some(origin) = origin else {
        return url.to_owned();
    };
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    format!("{}/{rest}", origin.trim_end_matches('/'))
}

/// Check every candidate, at most `max_in_flight` at a time. The results are
/// in candidate order.
pub(crate) async fn check_batch(
    candidates: &[Candidate],
    options: &FreshnessOptions,
) -> Vec<Freshness> {
    debug_assert!(
        candidates.iter().all(|c| c.provider != "itu"),
        "ITU PDFs have their own sync path"
    );
    let permits = Arc::new(Semaphore::new(options.max_in_flight.max(1)));
    let mut tasks = JoinSet::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let (candidate, options, permits) = (candidate.clone(), options.clone(), permits.clone());
        tasks.spawn(async move {
            let _permit = permits.acquire_owned().await;
            (index, check_one(&candidate, &options).await)
        });
    }
    let mut results: Vec<Option<Freshness>> = candidates.iter().map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        if let Ok((index, freshness)) = joined {
            results[index] = Some(freshness);
        }
    }
    results
        .into_iter()
        .map(|result| result.unwrap_or_else(|| Freshness::Failed("check task failed".into())))
        .collect()
}

async fn check_one(candidate: &Candidate, options: &FreshnessOptions) -> Freshness {
    let url = document_url(&candidate.base_url);
    let mut request = http_client()
        .get(effective_url(&url, options.origin.as_deref()))
        .header(reqwest::header::USER_AGENT, USER_AGENT);
    if candidate.time_limited {
        request = request.timeout(options.timeout);
    }
    if let Some(etag) = &candidate.etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = &candidate.last_modified {
        request = request.header(IF_MODIFIED_SINCE, last_modified);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => return Freshness::Failed(e.to_string()),
    };
    let header = |name| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let (etag, last_modified) = (header(ETAG), header(LAST_MODIFIED));
    let status = response.status();
    if status == StatusCode::NOT_MODIFIED {
        return Freshness::NotModified {
            etag: etag.or_else(|| candidate.etag.clone()),
            last_modified: last_modified.or_else(|| candidate.last_modified.clone()),
        };
    }
    if !status.is_success() {
        return Freshness::Failed(format!("HTTP {status}"));
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return Freshness::Failed(e.to_string()),
    };
    let source_hash = hash_bytes(body.as_bytes());
    if candidate.source_hash.as_deref() == Some(source_hash.as_str()) {
        return Freshness::SameContent {
            etag,
            last_modified,
        };
    }
    let html = if is_respec_source(&body) {
        render_respec(&url, body, options.origin.as_deref()).await
    } else {
        body
    };
    Freshness::Changed {
        content_hash: hash_bytes(html.as_bytes()),
        html,
        source_hash,
        etag,
        last_modified,
    }
}

/// The rendered document of a ReSpec source, or the source itself when the
/// spec-generator fails.
async fn render_respec(url: &str, source: String, origin: Option<&str>) -> String {
    eprintln!("note: {url} is a live ReSpec document; rendering via W3C spec-generator");
    match super::render_via_spec_generator(url, origin).await {
        Ok(rendered) => rendered,
        Err(e) => {
            eprintln!("warning: spec-generator failed ({e}), using raw HTML");
            source
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::testing::HttpStub;

    fn candidate(stub_path: &str, etag: Option<&str>, source_hash: Option<&str>) -> Candidate {
        Candidate {
            spec_id: 1,
            spec_name: "DOM".into(),
            base_url: format!("https://{stub_path}"),
            provider: "whatwg".into(),
            etag: etag.map(Into::into),
            last_modified: None,
            source_hash: source_hash.map(Into::into),
            time_limited: true,
        }
    }

    fn options(stub: &HttpStub) -> FreshnessOptions {
        FreshnessOptions {
            timeout: std::time::Duration::from_millis(300),
            max_in_flight: 8,
            origin: Some(stub.origin()),
        }
    }

    #[test]
    fn effective_url_moves_the_host_under_the_origin() {
        assert_eq!(
            effective_url("https://dom.spec.whatwg.org/", Some("http://127.0.0.1:9/")),
            "http://127.0.0.1:9/dom.spec.whatwg.org/"
        );
        assert_eq!(
            effective_url("https://dom.spec.whatwg.org/", None),
            "https://dom.spec.whatwg.org/"
        );
    }

    #[tokio::test]
    async fn validators_are_sent_and_304_is_not_modified() {
        let stub = HttpStub::start();
        stub.put(
            "dom.spec.whatwg.org/",
            "<p>x</p>",
            Some("W/\"1\""),
            Some("Fri, 25 Sep 2026 10:00:00 GMT"),
        );
        let out = check_batch(
            &[candidate("dom.spec.whatwg.org/", Some("W/\"1\""), None)],
            &options(&stub),
        )
        .await;
        assert!(matches!(out[0], Freshness::NotModified { .. }));
        assert_eq!(stub.requests()[0].1.as_deref(), Some("W/\"1\""));
    }

    #[tokio::test]
    async fn same_bytes_with_new_etag_only_updates_validators() {
        let stub = HttpStub::start();
        stub.put("dom.spec.whatwg.org/", "<p>x</p>", Some("W/\"2\""), None);
        let hash = crate::fetch::hash_bytes(b"<p>x</p>");
        let out = check_batch(
            &[candidate(
                "dom.spec.whatwg.org/",
                Some("W/\"1\""),
                Some(&hash),
            )],
            &options(&stub),
        )
        .await;
        match &out[0] {
            Freshness::SameContent { etag, .. } => assert_eq!(etag.as_deref(), Some("W/\"2\"")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn new_bytes_are_changed_with_hashes_and_validators() {
        let stub = HttpStub::start();
        stub.put("dom.spec.whatwg.org/", "<p>new</p>", Some("W/\"3\""), None);
        let out = check_batch(
            &[candidate(
                "dom.spec.whatwg.org/",
                Some("W/\"1\""),
                Some("old"),
            )],
            &options(&stub),
        )
        .await;
        match &out[0] {
            Freshness::Changed {
                html,
                content_hash,
                source_hash,
                etag,
                ..
            } => {
                assert_eq!(html, "<p>new</p>");
                assert_eq!(content_hash, &crate::fetch::hash_bytes(b"<p>new</p>"));
                assert_eq!(source_hash, content_hash);
                assert_eq!(etag.as_deref(), Some("W/\"3\""));
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn failed_check_keeps_cache_and_does_not_bump_last_checked() {
        let stub = HttpStub::start();
        stub.fail("dom.spec.whatwg.org/", 503);
        stub.put("infra.spec.whatwg.org/", "<p>y</p>", None, None);
        stub.delay("infra.spec.whatwg.org/", std::time::Duration::from_secs(2));
        let out = check_batch(
            &[
                candidate("dom.spec.whatwg.org/", None, None),
                candidate("infra.spec.whatwg.org/", None, None),
            ],
            &options(&stub),
        )
        .await;
        assert!(matches!(out[0], Freshness::Failed(_)));
        assert!(matches!(out[1], Freshness::Failed(_)), "timeout");
    }

    #[tokio::test]
    async fn batch_runs_requests_concurrently() {
        let stub = HttpStub::start();
        for host in ["a", "b", "c", "d"] {
            stub.put(&format!("{host}.spec.whatwg.org/"), "<p/>", None, None);
            stub.delay(
                &format!("{host}.spec.whatwg.org/"),
                std::time::Duration::from_millis(150),
            );
        }
        let candidates: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|h| candidate(&format!("{h}.spec.whatwg.org/"), None, None))
            .collect();
        let start = std::time::Instant::now();
        check_batch(
            &candidates,
            &FreshnessOptions {
                timeout: std::time::Duration::from_secs(2),
                ..options(&stub)
            },
        )
        .await;
        assert!(start.elapsed() < std::time::Duration::from_millis(450));
    }
}
