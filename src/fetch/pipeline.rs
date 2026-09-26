//! Parallel parse of fetched spec documents.

use crate::effects::catalog::Catalog;
use crate::effects::fragment::{build_fragment, encode_fragment, FragmentInput};
use crate::model::ParsedSpec;
use crate::parse::markdown::{encode_memo, MarkdownMemo};
use rayon::prelude::*;
use std::sync::{mpsc, Arc};

/// What a parse needs to build the spec's effects fragment beside its state.
#[derive(Clone)]
pub(crate) struct FragmentContext {
    pub catalog: Arc<Catalog>,
    pub environment: String,
}

/// An encoded fragment and the configuration it was built under.
pub(crate) struct BuiltFragment {
    pub config_key: String,
    pub payload: Vec<u8>,
}

pub(crate) struct ParseJob {
    pub spec_name: String,
    pub base_url: String,
    pub html: Arc<String>,
    pub content_hash: String,
    pub previous_memo: MarkdownMemo,
    /// Set when effects are on.
    pub fragment: Option<FragmentContext>,
    /// Causes `parse_one` to panic; used only in cfg(test) to exercise
    /// per-job `catch_unwind` isolation and the graceful-degradation path
    /// in `parse_and_write`.
    #[cfg(test)]
    pub(crate) test_fail: bool,
}

pub(crate) struct ParsedHtml {
    pub content_hash: String,
    pub parsed: ParsedSpec,
    pub structure_json: String,
    pub state: crate::state::StateSpec,
    pub memo: Vec<u8>,
    pub fragment: Option<BuiltFragment>,
}

/// Parse `jobs` on a pool of `threads` workers. Results are in job order.
/// Every job's document is held in memory at once, so callers pass bounded
/// chunks (see [`chunk_len`]).
///
/// A panic inside a single job is caught per-job via `catch_unwind` so it
/// does not cancel the remaining jobs.
pub(crate) fn parse_jobs(jobs: Vec<ParseJob>, threads: usize) -> Vec<anyhow::Result<ParsedHtml>> {
    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
    {
        Ok(pool) => pool,
        Err(e) => {
            let message = format!("parse thread pool: {e}");
            return jobs
                .iter()
                .map(|_| Err(anyhow::anyhow!("{message}")))
                .collect();
        }
    };
    pool.install(|| {
        jobs.into_par_iter()
            .map(|job| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_one(job)))
                    .unwrap_or_else(|payload| {
                        let msg = payload
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown panic payload");
                        Err(anyhow::anyhow!("parse worker panicked: {msg}"))
                    })
            })
            .collect()
    })
}

/// Worker count for [`parse_jobs`].
pub(crate) fn parse_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Largest chunk of jobs to hand [`parse_jobs`] before writing its results.
pub(crate) fn chunk_len(threads: usize) -> usize {
    2 * threads.max(1)
}

/// Parse one document on two workers, each with its own `Html`: the first
/// extracts sections, references and IDL (through the markdown memo), the
/// second extracts the step structure, then takes the first worker's sections
/// and IDL to extract state on its own document and, when effects are on, to
/// build the effects fragment beside it.
pub(crate) fn parse_one(job: ParseJob) -> anyhow::Result<ParsedHtml> {
    #[cfg(test)]
    if job.test_fail {
        panic!("test-injected parse panic for {}", job.spec_name);
    }
    let (tx, rx) = mpsc::sync_channel::<Arc<ParsedSpec>>(1);
    let ParseJob {
        spec_name,
        base_url,
        html,
        content_hash,
        previous_memo,
        fragment: fragment_context,
        #[cfg(test)]
            test_fail: _,
    } = job;
    let synthetic_sha = format!("hash:{content_hash}");
    let (html_a, spec_a, base_a) = (html.clone(), spec_name.clone(), base_url.clone());
    // rayon::join runs the section worker first on this thread and leaves the
    // structure worker in this thread's deque. Either this thread runs it after
    // the section worker has sent (no wait), or another worker steals it and
    // blocks on `recv` while this thread finishes the sender, so one thread is
    // enough. The section worker must not itself wait on rayon work: while
    // waiting, this thread could pop the structure worker from its own deque
    // and block on a message only the suspended section worker can send.
    // The sender moves into the section worker, so its failure drops the
    // sender and the structure worker's `recv` returns an error.
    let (sections, structure_and_state) = rayon::join(
        move || -> anyhow::Result<(Arc<ParsedSpec>, Vec<u8>)> {
            let document = scraper::Html::parse_document(&html_a);
            let (parsed, memo, _) =
                crate::parse::parse_spec_document_memo(&document, &spec_a, &base_a, previous_memo)?;
            let parsed = Arc::new(parsed);
            let _ = tx.send(parsed.clone());
            Ok((parsed, encode_memo(&memo)))
        },
        move || -> anyhow::Result<(String, crate::state::StateSpec, Option<BuiltFragment>)> {
            let document = scraper::Html::parse_document(&html);
            let structure = crate::parse::steps::extract_step_structure_from_document(
                &document,
                &spec_name,
                &base_url,
                &synthetic_sha,
            );
            // Compute body-node ids once so prose_sources does not repeat the
            // find_algorithm_candidates pass that extract_step_structure_from_document
            // already ran.
            let body_nodes = crate::parse::steps::structural_body_nodes(&document);
            let parsed = rx
                .recv()
                .map_err(|_| anyhow::anyhow!("section worker failed"))?;
            // The document is not `Sync`, so state stays on this thread while the
            // fragment builds on another. The structure worker may wait on rayon
            // work here: past `recv`, the sender it depends on has already sent.
            let mut fragment = None;
            let state = rayon::in_place_scope(|scope| {
                if let Some(context) = fragment_context {
                    let (fragment, parsed, structure) = (&mut fragment, &parsed, &structure);
                    let (spec, base, sha) = (&spec_name, &base_url, &synthetic_sha);
                    scope.spawn(move |_| {
                        let anchors = crate::effects::service::parsed_anchors(
                            spec,
                            base,
                            parsed,
                            structure,
                            &context.catalog,
                        );
                        let built = build_fragment(&FragmentInput {
                            spec,
                            snapshot_sha: sha,
                            base_url: base,
                            structure: Some(structure),
                            anchors: &anchors,
                            catalog: &context.catalog,
                            environment: &context.environment,
                        });
                        *fragment = Some(BuiltFragment {
                            config_key: crate::effects::service::config_key(
                                &context.catalog.content_digest,
                                &context.environment,
                            ),
                            payload: encode_fragment(&built),
                        });
                    });
                }
                crate::state::extract_state(&crate::state::StateInputs {
                    document: &document,
                    spec: &spec_name,
                    base_url: &base_url,
                    snapshot_sha: &synthetic_sha,
                    structure: &structure,
                    sections: &parsed.sections,
                    idl_definitions: &parsed.idl_definitions,
                    catalog: crate::state::extract::bundled_catalog(),
                    body_nodes: Some(body_nodes),
                })
            });
            Ok((serde_json::to_string(&structure)?, state, fragment))
        },
    );
    let (parsed, memo) = sections?;
    let (structure_json, state, fragment) = structure_and_state?;
    let parsed =
        Arc::try_unwrap(parsed).map_err(|_| anyhow::anyhow!("parsed spec still shared"))?;
    Ok(ParsedHtml {
        content_hash,
        parsed,
        structure_json,
        state,
        memo,
        fragment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::markdown::MarkdownMemo;

    #[test]
    fn parallel_parse_equals_sequential_parse() {
        let htmls = [
            (
                "HTML",
                "https://html.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/structure/wattsi.html"),
            ),
            (
                "DOM",
                "https://dom.spec.whatwg.org/",
                include_str!("../../tests/fixtures/effects/structure/bikeshed.html"),
            ),
            (
                "ECMA-262",
                "https://tc39.es/ecma262/",
                include_str!("../../tests/fixtures/effects/structure/ecmarkup.html"),
            ),
        ];
        let jobs = || {
            htmls
                .iter()
                .map(|(spec, base, html)| ParseJob {
                    spec_name: spec.to_string(),
                    base_url: base.to_string(),
                    html: std::sync::Arc::new(html.to_string()),
                    content_hash: crate::fetch::hash_bytes(html.as_bytes()),
                    previous_memo: MarkdownMemo::new(),
                    fragment: None,
                    test_fail: false,
                })
                .collect::<Vec<_>>()
        };
        let parallel = parse_jobs(jobs(), 4);
        let sequential: Vec<_> = jobs().into_iter().map(parse_one).collect();
        assert_eq!(parallel.len(), sequential.len());
        for (p, s) in parallel.iter().zip(&sequential) {
            let (p, s) = (p.as_ref().unwrap(), s.as_ref().unwrap());
            assert_eq!(format!("{:?}", p.parsed), format!("{:?}", s.parsed));
            assert_eq!(p.structure_json, s.structure_json);
            assert_eq!(p.state, s.state);
            assert_eq!(p.memo, s.memo);
        }
    }

    #[test]
    fn parsed_fragment_equals_the_fragment_built_from_the_stored_parse() {
        use crate::effects::service::{config_key, publish, PublishMode};
        use crate::effects::{default_catalog, fragment::decode_fragment, EffectsOptions};
        let html = include_str!("../../tests/fixtures/effects/structure/wattsi.html");
        let (spec, base) = ("HTML", "https://html.spec.whatwg.org/");
        let catalog = default_catalog(&[]).unwrap();
        let options = EffectsOptions::default();
        let conn = crate::db::open_test_db().unwrap();
        super::super::index_html(&conn, spec, base, "whatwg", html.into()).unwrap();
        publish(
            &conn,
            &catalog,
            &options,
            PublishMode::Rebuild,
            Default::default(),
        )
        .unwrap();
        let key = config_key(&catalog.content_digest, &options.environment);
        let stored = crate::db::effects::load_fragment_payloads(&conn, &key).unwrap();
        let parsed = parse_jobs(
            vec![ParseJob {
                spec_name: spec.into(),
                base_url: base.into(),
                html: Arc::new(html.into()),
                content_hash: super::super::hash_html(html),
                previous_memo: MarkdownMemo::new(),
                fragment: Some(FragmentContext {
                    catalog: Arc::new(catalog),
                    environment: options.environment,
                }),
                test_fail: false,
            }],
            1,
        )
        .remove(0)
        .unwrap();
        let built = parsed.fragment.unwrap();
        assert_eq!(built.config_key, key);
        assert_eq!(
            decode_fragment(&built.payload).unwrap(),
            decode_fragment(&stored[0].1).unwrap()
        );
    }

    #[test]
    fn single_thread_pool_does_not_deadlock() {
        let job = ParseJob {
            spec_name: "T".into(),
            base_url: "https://t.test/".into(),
            html: std::sync::Arc::new(
                include_str!("../../tests/fixtures/effects/structure/wattsi.html").into(),
            ),
            content_hash: "x".into(),
            previous_memo: MarkdownMemo::new(),
            fragment: None,
            test_fail: false,
        };
        assert!(parse_jobs(vec![job], 1)[0].is_ok());
    }

    #[test]
    fn catch_unwind_isolates_job_panic() {
        let html = include_str!("../../tests/fixtures/effects/structure/wattsi.html");
        let make_job = |name: &str, fail: bool| ParseJob {
            spec_name: name.into(),
            base_url: "https://t.test/".into(),
            html: std::sync::Arc::new(html.into()),
            content_hash: name.into(),
            previous_memo: MarkdownMemo::new(),
            fragment: None,
            test_fail: fail,
        };
        let results = parse_jobs(vec![make_job("OK", false), make_job("FAIL", true)], 2);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok(), "non-panicking job must succeed");
        assert!(
            results[1].is_err(),
            "panicking job must return Err, not kill other jobs"
        );
        let msg = results[1]
            .as_ref()
            .err()
            .expect("FAIL job must be Err")
            .to_string();
        assert!(msg.contains("panicked"), "error must note the panic: {msg}");
    }
}
