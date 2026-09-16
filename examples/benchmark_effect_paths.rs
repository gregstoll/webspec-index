//! Benchmark path reconstruction from a saved analysis artifact, without a database or network.
use anyhow::{Context, Result};
use std::{fs, time::Instant};
use webspec_index::effects::{engine::AnalysisArtifact, ExplanationOptions, SubjectSelector};

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: benchmark_effect_paths ARTIFACT.json")?;
    let started = Instant::now();
    let artifact: AnalysisArtifact = serde_json::from_slice(&fs::read(path)?)?;
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let subject = SubjectSelector {
        spec: "HTML".into(),
        anchor: "navigate".into(),
        step_id: None,
        step_path: None,
        body_id: None,
    };
    let started = Instant::now();
    let result = artifact.explain(&subject, None, &ExplanationOptions::default(), &artifact.sites)?;
    println!(
        "{}",
        serde_json::json!({
            "load_ms": load_ms, "explain_ms": started.elapsed().as_secs_f64()*1000.0,
            "nodes": artifact.nodes.len(), "relationships": artifact.relationships.len(), "states": artifact.counts.states,
            "effects": result.explanations.len(), "paths": result.explanations.iter().map(|e|e.witnesses.len()).sum::<usize>(),
            "effects_without_paths": result.explanations.iter().filter(|e|e.witnesses.is_empty()).count(),
            "search_limit_diagnostics": result.explanations.iter().flat_map(|e| &e.issues).filter(|i|i.code==webspec_index::effects::IssueCode::WitnessBudget).count()
        })
    );
    Ok(())
}
