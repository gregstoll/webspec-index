//! Spec §5 re-measurement on the real IR: parameters, kept fractions by algorithm size, Let/Set/
//! Mutate growth and rebinding, plus build time and payload size.
//!
//! Usage: cargo run --release --example slice_stats -- HTML.html DOM.html
use std::collections::{BTreeSet, HashMap, HashSet};
use webspec_index::state::slice::{build_slice_indexes, DefKind, SliceIndex};

fn forward(index: &SliceIndex, seed: u32) -> (usize, usize, usize) {
    let mentioned = |set: &BTreeSet<u32>| {
        index
            .steps
            .iter()
            .filter(|s| s.mentions.iter().any(|v| set.contains(v)))
            .count()
    };
    let mut set = BTreeSet::from([seed]);
    let direct = mentioned(&set);
    let grow = |kinds: &[DefKind], set: &mut BTreeSet<u32>| loop {
        let before = set.len();
        for e in &index.edges {
            if kinds.contains(&e.kind) && e.uses.iter().any(|u| set.contains(u)) {
                if let Some(v) = e.var {
                    set.insert(v);
                }
            }
        }
        if set.len() == before {
            break;
        }
    };
    grow(&[DefKind::Let], &mut set);
    let with_let = mentioned(&set);
    grow(&[DefKind::Let, DefKind::Set, DefKind::Mutate], &mut set);
    (direct, with_let, mentioned(&set))
}

/// §5.2 kept steps: matched + enclosing ancestors + var-less children of matched steps.
fn kept(index: &SliceIndex, seed: u32) -> usize {
    let mut vars = BTreeSet::from([seed]);
    let grow = |kinds: &[DefKind], vars: &mut BTreeSet<u32>| loop {
        let before = vars.len();
        for e in &index.edges {
            if kinds.contains(&e.kind) && e.uses.iter().any(|u| vars.contains(u)) {
                if let Some(v) = e.var {
                    vars.insert(v);
                }
            }
        }
        if vars.len() == before {
            break;
        }
    };
    grow(&[DefKind::Let], &mut vars);
    grow(&[DefKind::Let, DefKind::Set, DefKind::Mutate], &mut vars);

    let matched: BTreeSet<usize> = (0..index.steps.len())
        .filter(|&i| index.steps[i].mentions.iter().any(|v| vars.contains(v)))
        .collect();

    let mut enclosing: BTreeSet<usize> = BTreeSet::new();
    for &m in &matched {
        let mut cur = index.steps[m].parent;
        while let Some(p) = cur {
            let pi = p as usize;
            if matched.contains(&pi) || enclosing.contains(&pi) {
                break;
            }
            enclosing.insert(pi);
            cur = index.steps[pi].parent;
        }
    }

    let mut varless: BTreeSet<usize> = BTreeSet::new();
    for &m in &matched {
        for c in index.children(Some(m)) {
            if !matched.contains(&c) && index.steps[c].mentions.is_empty() {
                varless.insert(c);
            }
        }
    }

    matched.len() + enclosing.len() + varless.len()
}

fn main() {
    // Regex to extract *varname* tokens (identifier-like names) from markdown intro text.
    let star_re = regex::Regex::new(r"\*([A-Za-z_][A-Za-z0-9_-]*)\*").unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    for (spec, path) in ["HTML", "DOM"].iter().zip(&args) {
        let html = std::fs::read_to_string(path).unwrap();
        let base = webspec_index::state::testing::base_url(spec);
        let structure =
            webspec_index::parse::steps::extract_step_structure(&html, spec, base, "hash:m");
        let state = webspec_index::state::testing::extract_html(&html, spec);
        let start = std::time::Instant::now();
        let indexes = build_slice_indexes(&structure, &state);
        let build = start.elapsed();
        let bytes: usize = indexes
            .iter()
            .map(|i| serde_json::to_string(i).unwrap().len())
            .sum();
        let steps: usize = indexes.iter().map(|i| i.steps.len()).sum();
        let (mut pairs, mut let_grows, mut any_grows, mut sums) = (0, 0, 0, (0, 0, 0));
        for index in &indexes {
            for seed in 0..index.vars.len() as u32 {
                let (d, l, a) = forward(index, seed);
                pairs += 1;
                let_grows += usize::from(l > d);
                any_grows += usize::from(a > d);
                sums = (sums.0 + d, sums.1 + l, sums.2 + a);
            }
        }
        let edges: usize = indexes.iter().map(|i| i.edges.len()).sum();
        println!(
            "{spec}: {} algorithms, {steps} steps, {edges} edges, build {build:?}, payload {bytes} B",
            indexes.len()
        );
        println!(
            "{spec}: (algorithm, variable) pairs {pairs}; Let adds steps in {let_grows}; \
             Let+Set+Mutate in {any_grows}; matched direct/+Let/+Set,Mutate {}/{}/{}",
            sums.0, sums.1, sums.2
        );

        // §5.1: parameter table — intro variables (text before first step list in content_text)
        // that also appear in the algorithm's steps. Uses parse_spec for content_text.
        let parsed = webspec_index::parse::parse_spec(&html, spec, base).unwrap();
        let section_by_anchor: HashMap<&str, &webspec_index::model::ParsedSection> = parsed
            .sections
            .iter()
            .map(|s| (s.anchor.as_str(), s))
            .collect();
        let mut intro_total = 0usize;
        let mut intro_mentioned = 0usize;
        for index in &indexes {
            let Some(section) = section_by_anchor.get(index.anchor.as_str()) else {
                continue;
            };
            let Some(content) = &section.content_text else {
                continue;
            };
            // The intro is the text before the first numbered step (`\n1. `).
            let intro = content.split("\n1. ").next().unwrap_or("");
            let step_mentioned: HashSet<u32> = index
                .steps
                .iter()
                .flat_map(|s| s.mentions.iter().copied())
                .collect();
            for cap in star_re.captures_iter(intro) {
                let name = cap.get(1).unwrap().as_str();
                intro_total += 1;
                if let Some(v) = index.var(name) {
                    if step_mentioned.contains(&v) {
                        intro_mentioned += 1;
                    }
                }
            }
        }
        println!(
            "{spec}: §5.1 intro (parameter) vars: {intro_total} total, \
             {intro_mentioned} mentioned in steps ({:.0}%)",
            if intro_total > 0 {
                100.0 * intro_mentioned as f64 / intro_total as f64
            } else {
                0.0
            }
        );

        // §5.2: kept-fraction table by size bucket (1–9, 10–29, 30+ steps).
        println!("{spec}: §5.2 kept fraction by algorithm size:");
        let bucket_bounds = [(1usize, 9usize), (10, 29), (30, usize::MAX)];
        let bucket_labels = ["1–9 steps", "10–29 steps", "30+ steps"];
        for (&(lo, hi), label) in bucket_bounds.iter().zip(bucket_labels.iter()) {
            let mut pair_count = 0usize;
            let mut kept_num = 0usize;
            let mut kept_den = 0usize;
            for index in &indexes {
                let n = index.steps.len();
                if n < lo || n > hi {
                    continue;
                }
                for seed in 0..index.vars.len() as u32 {
                    pair_count += 1;
                    kept_num += kept(index, seed);
                    kept_den += n;
                }
            }
            let pct = if kept_den > 0 {
                100.0 * kept_num as f64 / kept_den as f64
            } else {
                0.0
            };
            println!("  {label}: {pair_count} pairs, kept {:.0}%", pct);
        }

        // §5.5: rebinding count — variable names with Let edges at more than one step.
        let mut rebound_count = 0usize;
        for index in &indexes {
            let mut let_steps: HashMap<u32, HashSet<u32>> = HashMap::new();
            for e in &index.edges {
                if e.kind == DefKind::Let {
                    if let Some(v) = e.var {
                        let_steps.entry(v).or_default().insert(e.step);
                    }
                }
            }
            for steps_set in let_steps.values() {
                if steps_set.len() > 1 {
                    rebound_count += 1;
                }
            }
        }
        println!("{spec}: §5.5 rebound names (Let at >1 step): {rebound_count}");
        println!();
    }
}
