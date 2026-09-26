//! Parallel parse of fetched spec documents.

use crate::model::ParsedSpec;
use crate::parse::markdown::{encode_memo, MarkdownMemo};
use rayon::prelude::*;
use std::sync::{mpsc, Arc};

pub(crate) struct ParseJob {
    pub spec_name: String,
    pub base_url: String,
    pub html: Arc<String>,
    pub content_hash: String,
    pub previous_memo: MarkdownMemo,
}

pub(crate) struct ParsedHtml {
    pub content_hash: String,
    pub parsed: ParsedSpec,
    pub structure_json: String,
    pub state: crate::state::StateSpec,
    pub memo: Vec<u8>,
}

/// Parse `jobs` on a pool of `threads` workers. Results are in job order.
/// Every job's document is held in memory at once, so callers pass bounded
/// chunks (see [`chunk_len`]).
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
    pool.install(|| jobs.into_par_iter().map(parse_one).collect())
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
/// and IDL to extract state on its own document.
pub(crate) fn parse_one(job: ParseJob) -> anyhow::Result<ParsedHtml> {
    let (tx, rx) = mpsc::sync_channel::<Arc<ParsedSpec>>(1);
    let ParseJob {
        spec_name,
        base_url,
        html,
        content_hash,
        previous_memo,
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
        move || -> anyhow::Result<(String, crate::state::StateSpec)> {
            let document = scraper::Html::parse_document(&html);
            let structure = crate::parse::steps::extract_step_structure_from_document(
                &document,
                &spec_name,
                &base_url,
                &synthetic_sha,
            );
            let parsed = rx
                .recv()
                .map_err(|_| anyhow::anyhow!("section worker failed"))?;
            let state = crate::state::extract_state(&crate::state::StateInputs {
                document: &document,
                spec: &spec_name,
                base_url: &base_url,
                snapshot_sha: &synthetic_sha,
                structure: &structure,
                sections: &parsed.sections,
                idl_definitions: &parsed.idl_definitions,
                catalog: crate::state::extract::bundled_catalog(),
            });
            Ok((serde_json::to_string(&structure)?, state))
        },
    );
    let (parsed, memo) = sections?;
    let (structure_json, state) = structure_and_state?;
    let parsed =
        Arc::try_unwrap(parsed).map_err(|_| anyhow::anyhow!("parsed spec still shared"))?;
    Ok(ParsedHtml {
        content_hash,
        parsed,
        structure_json,
        state,
        memo,
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
    fn single_thread_pool_does_not_deadlock() {
        let job = ParseJob {
            spec_name: "T".into(),
            base_url: "https://t.test/".into(),
            html: std::sync::Arc::new(
                include_str!("../../tests/fixtures/effects/structure/wattsi.html").into(),
            ),
            content_hash: "x".into(),
            previous_memo: MarkdownMemo::new(),
        };
        assert!(parse_jobs(vec![job], 1)[0].is_ok());
    }
}
