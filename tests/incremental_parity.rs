//! Spec §8 test 8: incremental refresh equals a from-scratch index plus a full
//! effects rebuild, for random upstream edits.
use std::collections::BTreeMap;

use rusqlite::Connection;
use webspec_index::db;
use webspec_index::effects::service::{publish, PublishMode};
use webspec_index::effects::{
    default_catalog, EffectsMode, EffectsOptions, EffectsRequest, SubjectSelector,
    EFFECTS_SCHEMA_VERSION,
};
use webspec_index::fetch::freshness::FreshnessOptions;
use webspec_index::fetch::testing::HttpStub;
use webspec_index::refresh::{refresh, EffectsOutcome, EffectsRefresh, LockPolicy, RefreshOptions};

const POOL: [(&str, &str); 8] = [
    ("DOM", "dom.spec.whatwg.org"),
    ("INFRA", "infra.spec.whatwg.org"),
    ("URL", "url.spec.whatwg.org"),
    ("FETCH", "fetch.spec.whatwg.org"),
    ("CONSOLE", "console.spec.whatwg.org"),
    ("ENCODING", "encoding.spec.whatwg.org"),
    ("MIMESNIFF", "mimesniff.spec.whatwg.org"),
    ("STREAMS", "streams.spec.whatwg.org"),
];

const PHRASES: [&str; 2] = ["fire an event", "schedule work"];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone)]
struct SynthSpec {
    name: &'static str,
    host: &'static str,
    sections: Vec<Section>,
}

#[derive(Clone)]
struct Section {
    anchor: String,
    title: String,
    algorithm: Option<Vec<Step>>,
    definition: bool,
}

#[derive(Clone)]
struct Step {
    text: String,
    link: Option<(String, String)>,
}

enum Edit {
    StepText,
    EffectPhrase,
    InsertSection,
    DeleteSection,
    RenameAnchor,
    MoveSection,
    AddLink,
    RemoveLink,
    AlgorithmToDefinition,
    AddSpec,
    RemoveSpec,
    NoOpSameValidators,
    NoOpNewValidators,
    CatalogChange,
}

fn render(spec: &SynthSpec) -> String {
    let mut html = String::from("<!doctype html>\n");
    for sec in &spec.sections {
        if let Some(steps) = &sec.algorithm {
            html.push_str(&format!(
                "<div class=\"algorithm\">\n<p>To <dfn id=\"{anchor}\">{title}</dfn>:</p>\n<ol>\n",
                anchor = sec.anchor,
                title = sec.title,
            ));
            for step in steps {
                let link_html = if let Some((host, anchor)) = &step.link {
                    format!(
                        " <a href=\"https://{host}/#{anchor}\">{anchor}</a>",
                        host = host,
                        anchor = anchor
                    )
                } else {
                    String::new()
                };
                html.push_str(&format!(
                    "<li><p>{text}{link}</p></li>\n",
                    text = step.text,
                    link = link_html
                ));
            }
            html.push_str("</ol>\n</div>\n");
        } else {
            html.push_str(&format!(
                "<p><dfn id=\"{anchor}\">{title}</dfn> is a concept.</p>\n",
                anchor = sec.anchor,
                title = sec.title,
            ));
        }
    }
    html
}

fn gen_anchor(rng: &mut Rng, prefix: &str) -> String {
    format!("{}-{:04x}", prefix, rng.next() & 0xffff)
}

fn gen_section(rng: &mut Rng, spec_idx: usize, all_specs: &[SynthSpec]) -> Section {
    let anchor = gen_anchor(rng, "sec");
    let title = format!("Section {:04x}", rng.next() & 0xffff);
    let is_algo = rng.below(3) > 0;
    if is_algo {
        let step_count = 1 + rng.below(4) as usize;
        let steps = (0..step_count)
            .map(|i| {
                let phrase_idx = rng.below(3) as usize;
                let text = if phrase_idx < 2 {
                    PHRASES[phrase_idx].to_string()
                } else {
                    format!("Step {}", i + 1)
                };
                let has_link = rng.below(3) == 0 && !all_specs.is_empty();
                let link = if has_link {
                    let target_spec_idx = rng.below(all_specs.len() as u64) as usize;
                    let target_spec = &all_specs[target_spec_idx];
                    if target_spec.sections.is_empty() || spec_idx == target_spec_idx {
                        None
                    } else {
                        let sec_idx = rng.below(target_spec.sections.len() as u64) as usize;
                        let target_anchor = target_spec.sections[sec_idx].anchor.clone();
                        Some((target_spec.host.to_string(), target_anchor))
                    }
                } else {
                    None
                };
                Step { text, link }
            })
            .collect();
        Section {
            anchor,
            title,
            algorithm: Some(steps),
            definition: false,
        }
    } else {
        Section {
            anchor,
            title,
            algorithm: None,
            definition: true,
        }
    }
}

fn gen_specs(rng: &mut Rng) -> Vec<SynthSpec> {
    let count = 3 + rng.below(6) as usize;
    let mut pool_indices: Vec<usize> = (0..POOL.len()).collect();
    for i in (1..pool_indices.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        pool_indices.swap(i, j);
    }
    let selected = pool_indices[..count].to_vec();

    let mut specs: Vec<SynthSpec> = selected
        .iter()
        .map(|&idx| SynthSpec {
            name: POOL[idx].0,
            host: POOL[idx].1,
            sections: Vec::new(),
        })
        .collect();

    for spec_idx in 0..specs.len() {
        let sec_count = 3 + rng.below(10) as usize;
        for _ in 0..sec_count {
            let sec = gen_section(rng, spec_idx, &specs);
            specs[spec_idx].sections.push(sec);
        }
    }
    specs
}

fn pick_algorithm_section_mut(rng: &mut Rng, specs: &mut [SynthSpec]) -> Option<(usize, usize)> {
    let candidates: Vec<(usize, usize)> = specs
        .iter()
        .enumerate()
        .flat_map(|(si, s)| {
            s.sections
                .iter()
                .enumerate()
                .filter(|(_, sec)| sec.algorithm.is_some())
                .map(move |(i, _)| (si, i))
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    Some(candidates[rng.below(candidates.len() as u64) as usize])
}

fn apply_edit(
    rng: &mut Rng,
    specs: &mut Vec<SynthSpec>,
    step_num: usize,
    extra_rule_paths: &mut Vec<String>,
    catalog_dir: &mut Option<tempfile::TempDir>,
) -> (Edit, Option<usize>) {
    let n_edits: u64 = 14;
    match rng.below(n_edits) {
        0 => {
            let edited_si = if let Some((si, seci)) = pick_algorithm_section_mut(rng, specs) {
                let steps = specs[si].sections[seci].algorithm.as_mut().unwrap();
                if !steps.is_empty() {
                    let step_idx = rng.below(steps.len() as u64) as usize;
                    steps[step_idx].text = format!("Updated step text at step {step_num}");
                }
                Some(si)
            } else {
                None
            };
            (Edit::StepText, edited_si)
        }
        1 => {
            let edited_si = if let Some((si, seci)) = pick_algorithm_section_mut(rng, specs) {
                let steps = specs[si].sections[seci].algorithm.as_mut().unwrap();
                if !steps.is_empty() {
                    let step_idx = rng.below(steps.len() as u64) as usize;
                    let phrase = PHRASES[rng.below(2) as usize];
                    steps[step_idx].text = phrase.to_string();
                }
                Some(si)
            } else {
                None
            };
            (Edit::EffectPhrase, edited_si)
        }
        2 => {
            let si = rng.below(specs.len() as u64) as usize;
            let sec = gen_section(rng, si, specs);
            let insert_at = if specs[si].sections.is_empty() {
                0
            } else {
                rng.below(specs[si].sections.len() as u64 + 1) as usize
            };
            specs[si].sections.insert(insert_at, sec);
            (Edit::InsertSection, Some(si))
        }
        3 => {
            let si = rng.below(specs.len() as u64) as usize;
            if specs[si].sections.len() > 1 {
                let idx = rng.below(specs[si].sections.len() as u64) as usize;
                specs[si].sections.remove(idx);
            }
            (Edit::DeleteSection, Some(si))
        }
        4 => {
            let si = rng.below(specs.len() as u64) as usize;
            if !specs[si].sections.is_empty() {
                let idx = rng.below(specs[si].sections.len() as u64) as usize;
                specs[si].sections[idx].anchor = gen_anchor(rng, "renamed");
            }
            (Edit::RenameAnchor, Some(si))
        }
        5 => {
            let si = rng.below(specs.len() as u64) as usize;
            if specs[si].sections.len() > 1 {
                let from = rng.below(specs[si].sections.len() as u64) as usize;
                let to = rng.below(specs[si].sections.len() as u64) as usize;
                if from != to {
                    let sec = specs[si].sections.remove(from);
                    let insert = if to > from { to - 1 } else { to };
                    specs[si].sections.insert(insert, sec);
                }
            }
            (Edit::MoveSection, Some(si))
        }
        6 => {
            let edited_si = if let Some((si, seci)) = pick_algorithm_section_mut(rng, specs) {
                let step_count = specs[si].sections[seci]
                    .algorithm
                    .as_ref()
                    .map_or(0, |v| v.len());
                let other_specs: Vec<usize> = (0..specs.len()).filter(|&i| i != si).collect();
                if step_count > 0 && !other_specs.is_empty() {
                    let step_idx = rng.below(step_count as u64) as usize;
                    let target_si = other_specs[rng.below(other_specs.len() as u64) as usize];
                    let target_link = if !specs[target_si].sections.is_empty() {
                        let target_sec_idx =
                            rng.below(specs[target_si].sections.len() as u64) as usize;
                        let target_anchor =
                            specs[target_si].sections[target_sec_idx].anchor.clone();
                        Some((specs[target_si].host.to_string(), target_anchor))
                    } else {
                        None
                    };
                    if let Some(link) = target_link {
                        specs[si].sections[seci].algorithm.as_mut().unwrap()[step_idx].link =
                            Some(link);
                    }
                    Some(si)
                } else {
                    None
                }
            } else {
                None
            };
            (Edit::AddLink, edited_si)
        }
        7 => {
            let edited_si = if let Some((si, seci)) = pick_algorithm_section_mut(rng, specs) {
                let steps = specs[si].sections[seci].algorithm.as_mut().unwrap();
                if !steps.is_empty() {
                    let step_idx = rng.below(steps.len() as u64) as usize;
                    steps[step_idx].link = None;
                }
                Some(si)
            } else {
                None
            };
            (Edit::RemoveLink, edited_si)
        }
        8 => {
            let edited_si = if let Some((si, seci)) = pick_algorithm_section_mut(rng, specs) {
                specs[si].sections[seci].algorithm = None;
                specs[si].sections[seci].definition = true;
                Some(si)
            } else {
                None
            };
            (Edit::AlgorithmToDefinition, edited_si)
        }
        9 => {
            let available: Vec<usize> = (0..POOL.len())
                .filter(|&i| !specs.iter().any(|s| s.name == POOL[i].0))
                .collect();
            let added_idx = if !available.is_empty() {
                let idx = available[rng.below(available.len() as u64) as usize];
                let mut new_spec = SynthSpec {
                    name: POOL[idx].0,
                    host: POOL[idx].1,
                    sections: Vec::new(),
                };
                let sec_count = 3 + rng.below(5) as usize;
                let all_snapshot: Vec<SynthSpec> = specs.to_vec();
                for _ in 0..sec_count {
                    let sec = gen_section(rng, specs.len(), &all_snapshot);
                    new_spec.sections.push(sec);
                }
                specs.push(new_spec);
                Some(specs.len() - 1)
            } else {
                None
            };
            (Edit::AddSpec, added_idx)
        }
        10 => {
            let removed_si = if specs.len() > 3 {
                let idx = rng.below(specs.len() as u64) as usize;
                specs.remove(idx);
                let remaining_idx = if idx < specs.len() {
                    idx
                } else {
                    specs.len() - 1
                };
                Some(remaining_idx)
            } else {
                None
            };
            (Edit::RemoveSpec, removed_si)
        }
        11 => (Edit::NoOpSameValidators, None),
        12 => (Edit::NoOpNewValidators, None),
        _ => {
            let dir = catalog_dir.get_or_insert_with(|| tempfile::tempdir().unwrap());
            let anchor = if !specs.is_empty() && !specs[0].sections.is_empty() {
                specs[0].sections[0].anchor.clone()
            } else {
                "concept".to_string()
            };
            let spec_name = specs[0].name;
            let summary_yaml = format!(
                "schema: 1\npackage: parity-extra-{step_num}\nsummaries:\n  - id: extra-{step_num}\n    subject: {spec_name}#{anchor}\n    expect_text: parity\n    reason: parity test extra catalog\n    emit:\n      kind: scheduling.promise-continuation\n      params: {{}}\n    continuations: separate\n",
            );
            std::fs::write(dir.path().join("summaries.yaml"), &summary_yaml).unwrap();
            let path = dir.path().to_string_lossy().into_owned();
            if !extra_rule_paths.contains(&path) {
                extra_rule_paths.push(path);
            }
            (Edit::CatalogChange, Some(0))
        }
    }
}

fn snapshot_id_to_spec(conn: &Connection) -> BTreeMap<i64, String> {
    let mut stmt = conn
        .prepare(
            "SELECT sn.id, sp.name FROM snapshots sn
             JOIN specs sp ON sn.spec_id = sp.id
             WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%'",
        )
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn in_clause(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    format!("({})", quoted.join(","))
}

fn dump(conn: &Connection, current_specs: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let spec_filter = in_clause(current_specs);
    let id_map = snapshot_id_to_spec(conn);

    let sid_to_spec = |sid: i64| {
        id_map
            .get(&sid)
            .cloned()
            .unwrap_or_else(|| format!("?{sid}"))
    };

    let mut sections_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, s.anchor, s.title, s.content_text, s.section_type,
                        s.parent_anchor, s.prev_anchor, s.next_anchor, s.depth, s.number, s.ord
                 FROM sections s
                 JOIN snapshots sn ON s.snapshot_id = sn.id
                 JOIN specs sp ON sn.spec_id = sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
                 ORDER BY sp.name, s.anchor",
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..11usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    sections_rows.sort();
    out.insert("sections".to_string(), sections_rows);

    let mut refs_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, r.from_anchor, r.to_spec, r.to_anchor,
                        r.step_path, r.step_text, r.guard_path, r.call_site_id, r.kind, r.ord
                 FROM refs r
                 JOIN snapshots sn ON r.snapshot_id = sn.id
                 JOIN specs sp ON sn.spec_id = sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
                 ORDER BY sp.name, r.from_anchor, r.ord, r.id",
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..10usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    refs_rows.sort();
    out.insert("refs".to_string(), refs_rows);

    let mut idl_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, i.anchor, i.name, i.owner, i.kind, i.canonical_name, i.idl_text, i.ord
                 FROM idl_defs i
                 JOIN snapshots sn ON i.snapshot_id = sn.id
                 JOIN specs sp ON sn.spec_id = sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
                 ORDER BY sp.name, i.anchor, i.kind",
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..8usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    idl_rows.sort();
    out.insert("idl_defs".to_string(), idl_rows);

    let mut snap_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, sn.sha, sn.is_latest,
                        sn.pr_number, sn.index_version
                 FROM snapshots sn
                 JOIN specs sp ON sn.spec_id = sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
                 ORDER BY sp.name"
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..5usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    snap_rows.sort();
    out.insert("snapshots".to_string(), snap_rows);

    let mut uc_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, uc.content_hash, uc.index_version
                 FROM update_checks uc
                 JOIN specs sp ON uc.spec_id = sp.id
                 WHERE sp.name IN {spec_filter}
                 ORDER BY sp.name",
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..3usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    uc_rows.sort();
    out.insert("update_checks".to_string(), uc_rows);

    let mut es_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, e.representation_version, e.structure_json, e.structure_bytes
                 FROM effect_structures e
                 JOIN snapshots sn ON e.snapshot_id = sn.id
                 JOIN specs sp ON sn.spec_id = sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
                 ORDER BY sp.name"
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..4usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    es_rows.sort();
    out.insert("effect_structures".to_string(), es_rows);

    let mut ef_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT spec, config_key, payload FROM effect_fragments WHERE spec IN {spec_filter} ORDER BY spec"
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..3usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    ef_rows.sort();
    out.insert("effect_fragments".to_string(), ef_rows);

    let mut mm_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, mm.payload
                 FROM markdown_memo mm
                 JOIN snapshots sn ON mm.snapshot_id = sn.id
                 JOIN specs sp ON sn.spec_id = sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
                 ORDER BY sp.name",
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..2usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    mm_rows.sort();
    out.insert("markdown_memo".to_string(), mm_rows);

    let mut esum_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT subject_key, spec, payload FROM effect_summaries WHERE spec IN {spec_filter} ORDER BY subject_key"
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..3usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    esum_rows.sort();
    out.insert("effect_summaries".to_string(), esum_rows);

    let mut esite_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT key, spec, json FROM effect_sites WHERE spec IN {spec_filter} ORDER BY key"
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..3usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    esite_rows.sort();
    out.insert("effect_sites".to_string(), esite_rows);

    let mut epub_rows: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT config_key, budget_key, frozen FROM effect_publication")
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..3usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    epub_rows.sort();
    out.insert("effect_publication".to_string(), epub_rows);

    let mut edep_rows: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT spec, dep, edges FROM effect_spec_deps WHERE spec IN {spec_filter} ORDER BY spec, dep"
            ))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((0..3usize)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap().to_owned()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    edep_rows.sort();
    out.insert("effect_spec_deps".to_string(), edep_rows);

    for table in webspec_index::db::state::STATE_TABLES {
        let sql = format!(
            "SELECT sp.name, t.* FROM {table} t
             JOIN snapshots sn ON sn.id = t.snapshot_id
             JOIN specs sp ON sn.spec_id = sp.id
             WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
             ORDER BY sp.name, t.snapshot_id"
        );
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ncols = stmt.column_count();
        let mut rows: Vec<String> = stmt
            .query_map([], |row| {
                let mut parts = Vec::with_capacity(ncols);
                for i in 0..ncols {
                    if i == 1 {
                        continue;
                    }
                    parts.push(format!("{:?}", row.get_ref(i).unwrap().to_owned()));
                }
                Ok(parts.join("|"))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows.sort();
        out.insert(format!("state:{table}"), rows);
    }

    let anchors: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sp.name, s.anchor FROM sections s
                 JOIN snapshots sn ON s.snapshot_id = sn.id
                 JOIN specs sp ON sn.spec_id = sp.id
                 WHERE sn.pr_number IS NULL AND sn.sha LIKE 'hash:%' AND sp.name IN {spec_filter}
                 ORDER BY sp.name, s.anchor"
            ))
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };

    let mut api_rows: Vec<String> = Vec::new();
    let current_spec_set: std::collections::HashSet<&str> =
        current_specs.iter().map(String::as_str).collect();

    let sampled: Vec<(String, String)> = {
        let mut by_spec: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (spec, anchor) in &anchors {
            by_spec
                .entry(spec.clone())
                .or_default()
                .push(anchor.clone());
        }
        let mut sample = Vec::new();
        for (spec, anchors_list) in &by_spec {
            let take = anchors_list.len().min(5);
            for anchor in &anchors_list[..take] {
                sample.push((spec.clone(), anchor.clone()));
            }
        }
        sample
    };

    for (spec, anchor) in &sampled {
        if let Ok(Some(result)) = webspec_index::query_section_from_conn(conn, spec, anchor) {
            api_rows.push(format!("qsec|{spec}|{anchor}|{}", result.section_type));
        }

        if let Ok(Some(snap_id)) = webspec_index::db::queries::get_snapshot(conn, spec) {
            {
                if let Ok(edges) =
                    webspec_index::db::queries::get_outgoing_edges(conn, snap_id, anchor, None)
                {
                    for e in &edges {
                        api_rows.push(format!("out|{spec}|{anchor}|{}|{}", e.spec, e.anchor));
                    }
                }
                if let Ok(children) =
                    webspec_index::db::queries::get_children(conn, snap_id, anchor)
                {
                    for (ch_anchor, _, _) in &children {
                        api_rows.push(format!("child|{spec}|{anchor}|{ch_anchor}"));
                    }
                }
            }
        }

        if let Ok(in_edges) =
            webspec_index::db::queries::get_incoming_edges(conn, spec, anchor, None)
        {
            for e in &in_edges {
                if current_spec_set.contains(e.spec.as_str()) {
                    api_rows.push(format!("in|{spec}|{anchor}|{}|{}", e.spec, e.anchor));
                }
            }
        }

        let request = EffectsRequest {
            schema_version: EFFECTS_SCHEMA_VERSION,
            subject: SubjectSelector {
                spec: spec.clone(),
                anchor: anchor.clone(),
                step_path: None,
                step_id: None,
                body_id: None,
            },
            options: EffectsOptions {
                mode: EffectsMode::Cached,
                ..EffectsOptions::default()
            },
            filter: None,
        };
        if let Ok(result) = webspec_index::effects::service::get_effect_summary_on(conn, &request) {
            let state = match &result.effects_status {
                webspec_index::effects::EffectsStatus::Ready { .. } => "ready",
                webspec_index::effects::EffectsStatus::Pending { .. } => "pending",
                webspec_index::effects::EffectsStatus::Unavailable { .. } => "unavailable",
                webspec_index::effects::EffectsStatus::Error { .. } => "error",
                webspec_index::effects::EffectsStatus::Disabled { .. } => "disabled",
            };
            api_rows.push(format!("eff|{spec}|{anchor}|{state}"));
        }
    }

    for word in PHRASES.iter().flat_map(|p| p.split_whitespace()) {
        if let Ok(results) = webspec_index::search_sections_fts(conn, word, None, 20) {
            for r in &results {
                if current_spec_set.contains(r.spec.as_str()) {
                    api_rows.push(format!("fts|{word}|{}|{}", r.spec, r.anchor));
                }
            }
        }
    }

    api_rows.sort();
    api_rows.dedup();
    out.insert("api".to_string(), api_rows);

    let _ = sid_to_spec;
    out
}

fn options_for(
    stub: &HttpStub,
    effects: EffectsRefresh,
    rule_paths: Vec<String>,
) -> RefreshOptions {
    RefreshOptions {
        freshness: FreshnessOptions {
            origin: Some(stub.origin()),
            ..FreshnessOptions::default()
        },
        check_interval: chrono::Duration::zero(),
        effects_options: EffectsOptions {
            rule_paths,
            ..EffectsOptions::default()
        },
        ..RefreshOptions::new(LockPolicy::Block, effects)
    }
}

async fn run_seed(seed: u64) {
    let mut rng = Rng(seed);
    let mut specs = gen_specs(&mut rng);

    let stub = HttpStub::start();
    let dir_a = tempfile::tempdir().unwrap();
    let conn_a = db::open_db_at(&dir_a.path().join("index.db")).unwrap();

    let mut extra_rule_paths: Vec<String> = Vec::new();
    let mut catalog_dir: Option<tempfile::TempDir> = None;

    for s in &specs {
        let key = format!("{}/", s.host);
        let html = render(s);
        stub.put(&key, &html, Some("W/\"s0\""), None);
    }

    let spec_names_a: Vec<String> = specs.iter().map(|s| s.name.to_string()).collect();
    let opts_a_init = options_for(&stub, EffectsRefresh::Off, extra_rule_paths.clone());
    refresh(&conn_a, &spec_names_a, &opts_a_init)
        .await
        .unwrap_or_else(|e| panic!("seed {seed} initial refresh A failed: {e}"));
    {
        let catalog_a = webspec_index::effects::default_catalog(&extra_rule_paths)
            .unwrap_or_else(|e| panic!("seed {seed} initial catalog A: {e}"));
        publish(
            &conn_a,
            &catalog_a,
            &webspec_index::effects::EffectsOptions {
                rule_paths: extra_rule_paths.clone(),
                ..webspec_index::effects::EffectsOptions::default()
            },
            PublishMode::Rebuild,
            std::collections::BTreeMap::new(),
        )
        .unwrap_or_else(|e| panic!("seed {seed} initial publish A failed: {e}"));
    }

    for step in 1usize..=20 {
        let spec_names_before: std::collections::HashSet<String> =
            specs.iter().map(|s| s.name.to_string()).collect();

        let (edit, edited_si) = apply_edit(
            &mut rng,
            &mut specs,
            step,
            &mut extra_rule_paths,
            &mut catalog_dir,
        );

        if matches!(edit, Edit::RemoveSpec) {
            let spec_names_after: std::collections::HashSet<String> =
                specs.iter().map(|s| s.name.to_string()).collect();
            for removed_name in spec_names_before.difference(&spec_names_after) {
                use rusqlite::OptionalExtension;
                let spec_id: Option<i64> = conn_a
                    .query_row(
                        "SELECT id FROM specs WHERE name=?1",
                        [removed_name],
                        |row| row.get(0),
                    )
                    .optional()
                    .unwrap_or(None);
                if let Some(spec_id) = spec_id {
                    let _ = webspec_index::db::write::delete_spec_data(&conn_a, spec_id);
                }
            }
        }

        let is_noop = matches!(edit, Edit::NoOpSameValidators | Edit::NoOpNewValidators);

        if matches!(edit, Edit::NoOpNewValidators) {
            for s in &specs {
                let key = format!("{}/", s.host);
                let html = render(s);
                let etag = format!("W/\"noop-{step}\"");
                stub.put(&key, &html, Some(&etag), None);
            }
        } else if !is_noop {
            if let Some(si) = edited_si {
                if si < specs.len() {
                    let s = &specs[si];
                    let key = format!("{}/", s.host);
                    let html = render(s);
                    let etag = format!("W/\"s{step}\"");
                    stub.put(&key, &html, Some(&etag), None);
                }
            }
        }

        let _ = edited_si;
        let current_spec_names: Vec<String> = specs.iter().map(|s| s.name.to_string()).collect();
        let opts_a = options_for(
            &stub,
            EffectsRefresh::Inline {
                budget: std::time::Duration::from_secs(60),
            },
            extra_rule_paths.clone(),
        );
        let report_a = refresh(&conn_a, &current_spec_names, &opts_a)
            .await
            .unwrap_or_else(|e| panic!("seed {seed} step {step} refresh failed: {e}"));

        if is_noop {
            assert!(
                report_a.parsed.is_empty(),
                "seed {seed} step {step} ({edit_name}): expected parsed empty, got {:?}",
                report_a.parsed,
                edit_name = if matches!(edit, Edit::NoOpSameValidators) {
                    "NoOpSameValidators"
                } else {
                    "NoOpNewValidators"
                },
            );
            assert!(
                report_a.fragments_built.is_empty(),
                "seed {seed} step {step}: expected fragments_built empty, got {:?}",
                report_a.fragments_built
            );
            assert_eq!(
                report_a.effects,
                EffectsOutcome::Current,
                "seed {seed} step {step}: expected effects Current"
            );
        }

        let dir_b = tempfile::tempdir().unwrap();
        let conn_b = db::open_db_at(&dir_b.path().join("index.db")).unwrap();
        let opts_b = options_for(&stub, EffectsRefresh::Off, Vec::new());
        refresh(&conn_b, &current_spec_names, &opts_b)
            .await
            .unwrap_or_else(|e| panic!("seed {seed} step {step} refresh B failed: {e}"));

        let catalog_b = default_catalog(&extra_rule_paths)
            .unwrap_or_else(|e| panic!("seed {seed} step {step} catalog B: {e}"));
        let effects_opts_b = EffectsOptions {
            rule_paths: extra_rule_paths.clone(),
            ..EffectsOptions::default()
        };
        publish(
            &conn_b,
            &catalog_b,
            &effects_opts_b,
            PublishMode::Rebuild,
            BTreeMap::new(),
        )
        .unwrap_or_else(|e| panic!("seed {seed} step {step} publish B failed: {e}"));

        let dump_a = dump(&conn_a, &current_spec_names);
        let dump_b = dump(&conn_b, &current_spec_names);

        if dump_a != dump_b {
            let edit_name = match &edit {
                Edit::StepText => "StepText",
                Edit::EffectPhrase => "EffectPhrase",
                Edit::InsertSection => "InsertSection",
                Edit::DeleteSection => "DeleteSection",
                Edit::RenameAnchor => "RenameAnchor",
                Edit::MoveSection => "MoveSection",
                Edit::AddLink => "AddLink",
                Edit::RemoveLink => "RemoveLink",
                Edit::AlgorithmToDefinition => "AlgorithmToDefinition",
                Edit::AddSpec => "AddSpec",
                Edit::RemoveSpec => "RemoveSpec",
                Edit::NoOpSameValidators => "NoOpSameValidators",
                Edit::NoOpNewValidators => "NoOpNewValidators",
                Edit::CatalogChange => "CatalogChange",
            };

            let mut first_diff = String::new();
            'outer: for (table, rows_a) in &dump_a {
                let rows_b = dump_b.get(table).cloned().unwrap_or_default();
                if *rows_a != rows_b {
                    let a_set: std::collections::BTreeSet<_> = rows_a.iter().cloned().collect();
                    let b_set: std::collections::BTreeSet<_> = rows_b.iter().cloned().collect();
                    if let Some(row) = a_set.difference(&b_set).next() {
                        first_diff = format!("table={table} A-only: {row}");
                        break 'outer;
                    }
                    if let Some(row) = b_set.difference(&a_set).next() {
                        first_diff = format!("table={table} B-only: {row}");
                        break 'outer;
                    }
                }
            }
            for table in dump_b.keys() {
                if !dump_a.contains_key(table) {
                    first_diff = format!("table={table} missing in A");
                    break;
                }
            }

            panic!("Parity failure: seed={seed} step={step} edit={edit_name}\n{first_diff}");
        }
    }
}

#[test]
fn incremental_parity() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let seeds: u64 = std::env::var("WEBSPEC_PARITY_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    for seed in 1..=seeds {
        rt.block_on(run_seed(seed));
    }
}
