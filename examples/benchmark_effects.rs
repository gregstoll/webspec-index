//! Offline, isolated benchmark of effects over supplied original HTML snapshots.
//! Input: {"sources":[{"spec":"HTML","file":"page.html","base_url":"https://.../page.html"}],"subject":{"spec":"HTML","anchor":"navigate"}}
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, time::Instant};
use webspec_index::{db, effects::*, parse};

#[derive(Deserialize)]
struct Input {
    sources: Vec<Page>,
    subject: SubjectSelector,
}
#[derive(Deserialize)]
struct Page {
    spec: String,
    file: PathBuf,
    base_url: String,
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn peak() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|s| s.starts_with("VmHWM:"))
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: cargo run --release --example benchmark_effects -- corpus.json")?;
    let input: Input = serde_json::from_slice(&std::fs::read(&path)?)?;
    let temporary = tempfile::tempdir()?;
    std::env::set_var("SPEC_INDEX_TEST_DB", temporary.path().join("index.db"));
    let conn = db::open_or_create_db()?;
    let mut grouped = BTreeMap::<String, Vec<(String, String)>>::new();
    let mut bytes = 0;
    for page in input.sources {
        let html = std::fs::read_to_string(path.parent().unwrap().join(page.file))?;
        bytes += html.len();
        grouped
            .entry(page.spec)
            .or_default()
            .push((page.base_url, html));
    }
    let mut algorithm_count = 0;
    let mut step_count = 0;
    let started = Instant::now();
    for (spec, pages) in grouped {
        let (id, base): (i64, String) = conn.query_row(
            "SELECT id,base_url FROM specs WHERE name=?1",
            [&spec],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut digest = Sha256::new();
        for (url, html) in &pages {
            digest.update(url.as_bytes());
            digest.update([0]);
            digest.update(html.as_bytes());
            digest.update([0]);
        }
        let hash = format!("{:x}", digest.finalize());
        let sha = format!("hash:{hash}");
        let tx = conn.unchecked_transaction()?;
        let snapshot = db::write::insert_snapshot(&tx, id, &sha, "2026-09-13")?;
        let mut structure = parse::steps::StructuralSpec {
            version: parse::steps::STRUCTURE_VERSION.into(),
            spec: spec.clone(),
            snapshot_sha: sha.clone(),
            anchors: vec![],
            algorithms: vec![],
            issues: vec![],
        };
        for (url, html) in pages {
            let parsed = parse::parse_spec(&html, &spec, &url)?;
            db::write::insert_sections_bulk(&tx, snapshot, &parsed.sections)?;
            db::write::insert_refs_bulk(&tx, snapshot, &parsed.references)?;
            db::write::insert_idl_defs_bulk(&tx, snapshot, &parsed.idl_definitions)?;
            let part = parse::steps::extract_step_structure(&html, &spec, &url, &sha);
            structure.anchors.extend(part.anchors);
            structure.algorithms.extend(part.algorithms);
            structure.issues.extend(part.issues);
        }
        structure.anchors.sort();
        structure.anchors.dedup();
        structure
            .algorithms
            .sort_by(|a, b| a.source.section_anchor.cmp(&b.source.section_anchor));
        structure
            .algorithms
            .dedup_by(|a, b| a.source.section_anchor == b.source.section_anchor);
        algorithm_count += structure.algorithms.len();
        step_count += structure
            .algorithms
            .iter()
            .map(|a| a.steps.len())
            .sum::<usize>();
        db::effects::store_structure(
            &tx,
            snapshot,
            parse::steps::STRUCTURE_VERSION,
            &serde_json::to_string(&structure)?,
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        db::write::record_update_check(
            &tx,
            id,
            &now,
            Some(&now),
            Some(&hash),
            Some(parse::INDEX_VERSION),
        )?;
        tx.commit()?;
        eprintln!("indexed {spec} ({base})");
    }
    let indexing_ms = ms(started);
    eprintln!("indexed memory {}", peak());
    let subject = format!("{}#{}", input.subject.spec, input.subject.anchor);
    let started = Instant::now();
    let off = query_section_with_effects(
        &subject,
        None,
        EffectsOptions {
            mode: EffectsMode::Off,
            ..Default::default()
        },
    )
    .await?;
    let off_ms = ms(started);
    anyhow::ensure!(
        off.query.sha.starts_with("hash:"),
        "query did not use pinned source"
    );
    let started = Instant::now();
    let cold = query_section_with_effects(&subject, None, EffectsOptions::default()).await?;
    let cold_ms = ms(started);
    eprintln!("cold memory {}", peak());
    let mut warm = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let result = query_section_with_effects(&subject, None, EffectsOptions::default()).await?;
        anyhow::ensure!(result.effects == cold.effects, "warm effects differ");
        warm.push(ms(started));
    }
    let started = Instant::now();
    let explained = explain_effects(&ExplainEffectsRequest {
        schema_version: 1,
        subject: input.subject.clone(),
        options: EffectsOptions::default(),
        filter: None,
        explanation: ExplanationOptions::default(),
    })?;
    let explanation_ms = ms(started);
    let started = Instant::now();
    let all = recompute_effects(&RecomputeEffectsRequest {
        schema_version: 1,
        scope: AnalysisScope::All,
        options: EffectsOptions::default(),
    })?;
    let all_ms = ms(started);
    let mut sizes = BTreeMap::new();
    for (table, column) in [
        ("effect_structures", "structure_json"),
        ("effect_runs", "artifact_json"),
        ("effect_subjects", "summary_json"),
        ("effect_local_matches", "payload_json"),
    ] {
        let size: i64 = conn.query_row(
            &format!("SELECT coalesce(sum(length({column})),0) FROM {table}"),
            [],
            |r| r.get(0),
        )?;
        sizes.insert(table, size);
    }
    warm.sort_by(f64::total_cmp);
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|s| s.starts_with("VmHWM:"))
                .map(str::to_owned)
        });
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "storage_bytes":sizes,"corpus_bytes":bytes,"algorithms":algorithm_count,"steps":step_count,
            "indexing_ms":indexing_ms,"query_effects_off_ms":off_ms,"cold_query_ms":cold_ms,
            "warm_query_median_ms":warm[10],"warm_query_p95_ms":warm[18],"bounded_explanation_ms":explanation_ms,
            "indexed_corpus_recompute_ms":all_ms,"peak_memory":rss,
            "effects":cold.effects,"effects_status":cold.effects_status,
            "explanation_groups":explained.explanations.len(),"input_manifest":all.input_manifest,
            "processed_subjects":all.processed_subjects.len(),"unprocessed_subjects":all.unprocessed_subjects.len(),
            "bodies":all.body_count,"relationships":all.relationship_count,"states":all.state_count
        }))?
    );
    Ok(())
}
