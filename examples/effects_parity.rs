//! Corpus-wide parity check: compares `get_effect_summary_on` / `prepared_effect_details_on`
//! against a golden export made from the materialized effects DB.
//!
//! Usage:
//!   cargo run --release --example effects_parity -- target/golden-effects.sqlite [--analysis <id>] [--limit N] [--threads T]
//!
//! Exits 0 if every checked subject matches, 1 if any mismatch is found.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use rayon::prelude::*;
use rusqlite::Connection;
use serde_json::Value;
use webspec_index::{
    db,
    db::effects as gold_store,
    effects::{
        model::{
            EffectsMode, EffectsOptions, EffectsRequest, RequestErrorCode, SubjectSelector,
            EFFECTS_SCHEMA_VERSION,
        },
        service::{get_effect_summary_on, prepared_effect_details_on},
    },
};

fn normalize(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("input_manifest");
    }
    if let Some(status) = value.get_mut("effects_status") {
        if let Some(id) = status.get_mut("analysis_id") {
            *id = Value::String(String::new());
        }
    }
}

fn first_differing_lines(golden: &str, new: &str, max_lines: usize) -> String {
    let gl: Vec<&str> = golden.lines().collect();
    let nl: Vec<&str> = new.lines().collect();
    let mut out = String::from("(first differing lines — golden:-  new:+)\n");
    let mut shown = 0;
    for i in 0..gl.len().max(nl.len()) {
        let g = gl.get(i).copied().unwrap_or("");
        let n = nl.get(i).copied().unwrap_or("");
        if g != n {
            out.push_str(&format!("-{g}\n+{n}\n"));
            shown += 2;
            if shown >= max_lines {
                out.push_str("...\n");
                break;
            }
        }
    }
    out
}

struct GoldenSubject {
    sel: SubjectSelector,
    key: String,
    is_ambiguous: bool,
    golden_value: Value,
    witnesses: Vec<(String, Value)>,
}

fn make_request(sel: SubjectSelector) -> EffectsRequest {
    EffectsRequest {
        schema_version: EFFECTS_SCHEMA_VERSION,
        subject: sel,
        options: EffectsOptions {
            mode: EffectsMode::Cached,
            ..Default::default()
        },
        filter: None,
    }
}

fn main() {
    let mut args_iter = std::env::args().skip(1);
    let golden_path = PathBuf::from(args_iter.next().expect(
        "usage: effects_parity <golden.sqlite> [--analysis <id>] [--limit N] [--threads T]",
    ));
    let mut analysis_id = "an_78c733c3d20da90b".to_string();
    let mut limit: Option<usize> = None;
    let mut num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--analysis" => analysis_id = args_iter.next().expect("--analysis <id>"),
            "--limit" => limit = Some(args_iter.next().expect("--limit N").parse().expect("N")),
            "--threads" => num_threads = args_iter.next().expect("--threads T").parse().expect("T"),
            other => eprintln!("unknown argument: {other}"),
        }
    }

    let golden =
        Connection::open_with_flags(&golden_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open golden DB");

    // Load all subject keys ordered by subject_key, then apply limit.
    let all_keys: Vec<String> = {
        let mut stmt = golden
            .prepare(
                "SELECT subject_key FROM effect_subjects WHERE analysis_id=?1 ORDER BY subject_key",
            )
            .expect("prepare subject keys");
        stmt.query_map([&analysis_id], |row| row.get(0))
            .expect("query subject keys")
            .collect::<rusqlite::Result<_>>()
            .expect("collect subject keys")
    };

    let take = limit.unwrap_or(all_keys.len()).min(all_keys.len());
    let subject_keys: Vec<String> = all_keys.into_iter().take(take).collect();
    let key_set: HashSet<&str> = subject_keys.iter().map(String::as_str).collect();

    // Load all witness (subject_key, effect_id) pairs, filter to our subjects, then load JSON.
    let witness_pairs: Vec<(String, String)> = {
        let mut stmt = golden
            .prepare(
                "SELECT subject_key, effect_id FROM effect_witnesses \
                 WHERE analysis_id=?1 ORDER BY subject_key, effect_id",
            )
            .expect("prepare witness pairs");
        stmt.query_map([&analysis_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query witness pairs")
            .collect::<rusqlite::Result<_>>()
            .expect("collect witness pairs")
    };

    let mut witnesses_by_subject: HashMap<String, Vec<(String, Value)>> = HashMap::new();
    for (sk, eid) in witness_pairs {
        if !key_set.contains(sk.as_str()) {
            continue;
        }
        let json = gold_store::load_witness(&golden, &analysis_id, &sk, &eid)
            .expect("load_witness")
            .expect("witness row missing after key confirmed present");
        let val: Value = serde_json::from_str(&json).expect("parse witness json");
        witnesses_by_subject.entry(sk).or_default().push((eid, val));
    }

    // Build GoldenSubject for each key, expanding _issue_refs.
    let mut subjects: Vec<GoldenSubject> = subject_keys
        .iter()
        .map(|key| {
            let sel: SubjectSelector =
                serde_json::from_str(key).expect("parse subject_key as SubjectSelector");
            let json = gold_store::load_subject(&golden, &analysis_id, key)
                .expect("load_subject")
                .expect("subject row missing");
            let mut value: Value = serde_json::from_str(&json).expect("parse summary_json");

            let is_ambiguous = value.get("ambiguous_subjects").is_some();
            if !is_ambiguous {
                let refs: Vec<String> = value
                    .as_object_mut()
                    .and_then(|o| o.remove("_issue_refs"))
                    .map(|v| serde_json::from_value(v).expect("parse _issue_refs"))
                    .unwrap_or_default();
                let issue_jsons =
                    gold_store::load_issues(&golden, &analysis_id, &refs).expect("load_issues");
                let issue_vals: Vec<Value> = issue_jsons
                    .iter()
                    .map(|j| serde_json::from_str(j).expect("parse issue json"))
                    .collect();
                value["issues"] = Value::Array(issue_vals);
                normalize(&mut value);
            }

            let witnesses = witnesses_by_subject.remove(key).unwrap_or_default();
            GoldenSubject {
                sel,
                key: key.clone(),
                is_ambiguous,
                golden_value: value,
                witnesses,
            }
        })
        .collect();

    // Sort by (spec, anchor) for per-section analysis cache locality.
    subjects.sort_by(|a, b| (&a.sel.spec, &a.sel.anchor).cmp(&(&b.sel.spec, &b.sel.anchor)));

    let total = subjects.len();
    println!(
        "Loaded {total} subjects from golden (analysis={analysis_id}, limit={limit:?}, threads={num_threads})"
    );

    let matched = AtomicUsize::new(0);
    let mismatched = AtomicUsize::new(0);
    let start = Instant::now();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build()
        .expect("rayon thread pool");

    let chunk_size = (total + num_threads - 1) / num_threads.max(1);
    let chunks: Vec<&[GoldenSubject]> = subjects.chunks(chunk_size.max(1)).collect();

    pool.install(|| {
        chunks.par_iter().for_each(|chunk| {
            let dev = db::open_or_create_db().expect("open dev DB");
            for gs in chunk.iter() {
                let req = make_request(gs.sel.clone());

                if gs.is_ambiguous {
                    match get_effect_summary_on(&dev, &req) {
                        Err(e) if e.code == RequestErrorCode::AmbiguousSubject => {
                            matched.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(_) => {
                            eprintln!("MISMATCH {}: expected AmbiguousSubject but got Ok", gs.key);
                            mismatched.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            eprintln!(
                                "MISMATCH {}: expected AmbiguousSubject but got {:?}: {}",
                                gs.key, e.code, e.message
                            );
                            mismatched.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    continue;
                }

                let result = match get_effect_summary_on(&dev, &req) {
                    Err(e) => {
                        eprintln!(
                            "MISMATCH {}: new path returned error {:?}: {}",
                            gs.key, e.code, e.message
                        );
                        mismatched.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    Ok(r) => r,
                };

                let mut new_val =
                    serde_json::to_value(&result).expect("serialize EffectSummaryResult");
                normalize(&mut new_val);

                let summary_ok = if gs.golden_value != new_val {
                    let gp = serde_json::to_string_pretty(&gs.golden_value).unwrap_or_default();
                    let np = serde_json::to_string_pretty(&new_val).unwrap_or_default();
                    let diff = first_differing_lines(&gp, &np, 40);
                    eprintln!("MISMATCH summary {}\n{diff}", gs.key);
                    false
                } else {
                    true
                };

                let witness_ok = if gs.witnesses.is_empty() {
                    true
                } else {
                    match prepared_effect_details_on(&dev, &req) {
                        Err(e) => {
                            eprintln!(
                                "MISMATCH {}: prepared_effect_details_on error: {}",
                                gs.key, e.message
                            );
                            false
                        }
                        Ok(details) => {
                            let mut all_ok = true;
                            for (geid, gw) in &gs.witnesses {
                                let new_w = details
                                    .explanations
                                    .iter()
                                    .find(|e| &e.effect_id == geid)
                                    .and_then(|e| e.witnesses.first());
                                match new_w {
                                    None => {
                                        eprintln!(
                                            "MISMATCH witness {} effect {}: witness missing on new side",
                                            gs.key, geid
                                        );
                                        all_ok = false;
                                    }
                                    Some(w) => {
                                        let nw_val =
                                            serde_json::to_value(w).expect("serialize Witness");
                                        if gw != &nw_val {
                                            let gp = serde_json::to_string_pretty(gw)
                                                .unwrap_or_default();
                                            let np = serde_json::to_string_pretty(&nw_val)
                                                .unwrap_or_default();
                                            let diff = first_differing_lines(&gp, &np, 40);
                                            eprintln!(
                                                "MISMATCH witness {} effect {}\n{diff}",
                                                gs.key, geid
                                            );
                                            all_ok = false;
                                        }
                                    }
                                }
                            }
                            all_ok
                        }
                    }
                };

                if summary_ok && witness_ok {
                    matched.fetch_add(1, Ordering::Relaxed);
                } else {
                    mismatched.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    });

    let elapsed = start.elapsed();
    let m = matched.load(Ordering::Relaxed);
    let mm = mismatched.load(Ordering::Relaxed);
    println!(
        "subjects: {total}, matched: {m}, mismatched: {mm}, elapsed: {:.2}s",
        elapsed.as_secs_f64()
    );
    if mm > 0 {
        std::process::exit(1);
    }
}
