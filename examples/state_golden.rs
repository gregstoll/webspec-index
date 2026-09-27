//! Whole-corpus golden check and coverage print (spec §12.3–§12.4, statement IR §6.4–§6.6).
//!
//! Usage: cargo run --release --example state_golden -- [--review] HTML.html DOM.html
//! Indexes both snapshots twice: with the bundled catalog (golden set, bundled floors) and
//! with an empty catalog (grammar-only floors). `--review` prints the unclassified review
//! items, unparsed assertions and undeclared variables of the bundled run. Exits 1 on any
//! golden mismatch or coverage floor violation.
#[path = "../tests/state_golden.rs"]
#[allow(dead_code)]
mod golden;

use std::collections::{BTreeSet, HashSet};
use webspec_index::state::catalog::StateCatalog;
use webspec_index::state::ir::{Expr, Origin, SourceContext, StatementKind};
use webspec_index::state::model::{
    AnchorTarget, ReviewItem, SignatureForm, StateSpec, TypeBasis, TypeExpr,
};

struct Floors {
    owner: f64,
    set: f64,
    unclassified: f64,
}

/// §12.4 floors with the bundled catalog: owner resolution is the value measured after the
/// bundled YAML (HTML fields whose owner the spec text does not state stay unresolved), and
/// the verb rules lower the unclassified ceiling.
fn bundled_floors(spec: &str) -> Floors {
    if spec == "HTML" {
        Floors {
            owner: 93.0,
            set: 91.0,
            unclassified: 2.2,
        }
    } else {
        Floors {
            owner: 100.0,
            set: 95.0,
            unclassified: 1.9,
        }
    }
}

/// §12.4 floors with the grammar alone.
fn grammar_floors(spec: &str) -> Floors {
    if spec == "HTML" {
        Floors {
            owner: 77.0,
            set: 91.0,
            unclassified: 2.7,
        }
    } else {
        Floors {
            owner: 94.0,
            set: 95.0,
            unclassified: 2.0,
        }
    }
}

fn index(html: &str, dom: &str, catalog: &StateCatalog) -> (rusqlite::Connection, [i64; 2]) {
    let conn = webspec_index::db::open_in_memory().unwrap();
    let h = webspec_index::state::testing::index_offline_with(
        &conn,
        "HTML",
        "https://html.spec.whatwg.org/",
        html,
        catalog,
    )
    .unwrap();
    let d = webspec_index::state::testing::index_offline_with(
        &conn,
        "DOM",
        "https://dom.spec.whatwg.org/",
        dom,
        catalog,
    )
    .unwrap();
    (conn, [h, d])
}

fn model(conn: &rusqlite::Connection, snapshot: i64) -> StateSpec {
    webspec_index::db::state::load_state_model(conn, snapshot)
        .unwrap()
        .unwrap()
}

fn ratio(part: u32, whole: u32) -> String {
    format!(
        "{part}/{whole} ({:.1}%)",
        100.0 * part as f64 / whole.max(1) as f64
    )
}

/// The §6.4–§6.6 statement measurements. The `Let`/`Return` rates cover every
/// source; the §6.5 probe read algorithm steps only, so the step-only rates
/// follow. The §6.6 probe checked `To` algorithms only, so their count follows
/// the all-subject one.
fn print_statements(state: &StateSpec) {
    let c = &state.coverage;
    let mut heads: Vec<(&String, &u32)> = c.step_heads.iter().collect();
    heads.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let top: Vec<String> = heads
        .iter()
        .take(10)
        .map(|(tag, n)| format!("{tag} {n}"))
        .collect();
    println!(
        "  step heads recognized {}; top: {}",
        ratio(c.steps_recognized, c.steps),
        top.join(", ")
    );
    println!(
        "  If parsed {}, Assert parsed {}, For each bound {}",
        ratio(c.if_parsed, c.if_total),
        ratio(c.assert_parsed, c.assert_total),
        ratio(c.foreach_bound, c.foreach_total)
    );
    let opaque = |slot: &str| {
        let prefix = format!("{slot}:");
        let total: u32 = c
            .expr_forms
            .iter()
            .filter(|(form, _)| form.starts_with(&prefix))
            .map(|(_, n)| n)
            .sum();
        let opaque = c
            .expr_forms
            .get(&format!("{slot}:opaque"))
            .copied()
            .unwrap_or(0);
        ratio(opaque, total)
    };
    println!(
        "  Let opaque {}, Return opaque {}; expr forms {:?}",
        opaque("let"),
        opaque("return"),
        c.expr_forms
    );
    let step_sources: HashSet<&str> = state
        .sources
        .iter()
        .filter(|s| matches!(s.context, SourceContext::Algorithm { .. }))
        .map(|s| s.id.as_str())
        .collect();
    let (mut lets, mut returns) = ((0, 0), (0, 0));
    for statement in &state.statements {
        if !step_sources.contains(statement.source_id.as_str()) {
            continue;
        }
        let (count, value) = match &statement.kind {
            StatementKind::Let { value, .. } => (&mut lets, Some(value)),
            StatementKind::Return { value } => (&mut returns, value.as_ref()),
            _ => continue,
        };
        count.0 += u32::from(matches!(value, Some(Expr::Opaque { .. })));
        count.1 += 1;
    }
    println!(
        "  in algorithm steps: Let opaque {}, Return opaque {}",
        ratio(lets.0, lets.1),
        ratio(returns.0, returns.1)
    );
    let algorithms: BTreeSet<&str> = c
        .undeclared_review
        .iter()
        .map(|item| item.subject.as_str())
        .collect();
    let to_algorithms: HashSet<&AnchorTarget> = state
        .signatures
        .iter()
        .filter(|s| s.form == SignatureForm::To)
        .map(|s| &s.algorithm)
        .collect();
    let (mut to_undeclared, mut to_with_any) = (0, 0);
    for origins in state
        .var_origins
        .iter()
        .filter(|o| to_algorithms.contains(&o.subject))
    {
        let n = origins
            .vars
            .iter()
            .filter(|v| v.origin == Origin::Undeclared)
            .count();
        to_undeclared += n;
        to_with_any += usize::from(n > 0);
    }
    println!(
        "  undeclared vars {} in {} algorithms (To algorithms: {to_undeclared} in {to_with_any} of {}); calls by form {:?}",
        c.undeclared_vars,
        algorithms.len(),
        to_algorithms.len(),
        c.calls_by_form
    );
}

/// Review items; their subject already carries the spec.
fn print_review(label: &str, items: &[ReviewItem]) {
    for item in items {
        println!(
            "  {label}: {}{} [{}] {}",
            item.subject,
            item.step_path
                .as_deref()
                .map(|p| format!(":{p}"))
                .unwrap_or_default(),
            item.target,
            item.text
        );
    }
}

/// Prints the §12.4 numbers for one run; returns false when a floor is violated.
fn report(run: &str, spec: &str, state: &StateSpec, floors: &Floors) -> bool {
    let (c, signatures) = (&state.coverage, &state.signatures);
    let owned: u32 = c.written_fields_owned.values().sum();
    let owner_pct = 100.0 * owned as f64 / c.written_fields.max(1) as f64;
    let set_pct = 100.0 * c.set_structured as f64 / c.set_total.max(1) as f64;
    let occ: u32 = c.occurrences.values().sum();
    let unclassified_pct =
        100.0 * *c.occurrences.get("unclassified").unwrap_or(&0) as f64 / occ.max(1) as f64;
    println!(
        "{spec} ({run}): written-field owners {owner_pct:.1}% ({owned}/{}), structured Set {set_pct:.1}% ({}/{}), unclassified {unclassified_pct:.2}% of {occ}",
        c.written_fields, c.set_structured, c.set_total
    );
    println!(
        "  concept dfns {}, owner candidates {} (resolved {}), by rule {:?}",
        c.concept_dfns, c.owner_candidates, c.owner_resolved, c.owner_by_rule
    );
    println!(
        "  written-field owners by rule {:?}",
        c.written_fields_owned
    );
    println!("  statements {:?}", c.statements);
    println!("  occurrences {:?}", c.occurrences);
    let pct = |part: u32, whole: u32| 100.0 * part as f64 / whole.max(1) as f64;
    let params: u32 = c.to_params.values().sum();
    let typed: u32 = ["explicit", "name_resolved", "dfn_for"]
        .iter()
        .filter_map(|basis| c.to_params.get(*basis))
        .sum();
    println!(
        "  signatures: template {}/{} ({:.1}%), forms {:?}, To params typed {:.1}%, IDL from IDL {}/{}",
        c.template_signatures,
        c.algorithms,
        pct(c.template_signatures, c.algorithms),
        c.intro_forms,
        pct(typed, params),
        c.idl_from_idl,
        c.idl_signatures
    );
    // §6.2 counts IDL signatures as "with template" and types `To` intros only.
    let with_idl = c.template_signatures + c.idl_signatures;
    let (to_typed, to_params) = signatures
        .iter()
        .filter(|s| s.form == SignatureForm::To)
        .flat_map(|s| &s.params)
        .fold((0, 0), |(typed, all), p| {
            let is_typed = match p.type_basis {
                TypeBasis::Explicit => !matches!(p.ty, TypeExpr::Opaque { .. }),
                TypeBasis::NameResolved | TypeBasis::DfnFor => true,
                TypeBasis::Unknown => false,
            };
            (typed + u32::from(is_typed), all + 1)
        });
    println!(
        "  signatures (§6.2 terms): template or IDL {with_idl}/{} ({:.1}%), To-form params typed {to_typed}/{to_params} ({:.1}%)",
        c.algorithms,
        pct(with_idl, c.algorithms),
        pct(to_typed, to_params)
    );
    print_statements(state);
    for unresolved in &c.written_fields_unresolved {
        println!("  unresolved owner: {spec}#{unresolved}");
    }
    let ok = owner_pct >= floors.owner
        && set_pct >= floors.set
        && unclassified_pct <= floors.unclassified;
    if !ok {
        println!("FLOOR {spec} ({run}) below §12.4");
    }
    ok
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let review = args.iter().any(|a| a == "--review");
    args.retain(|a| a != "--review");
    let [html, dom] = args.as_slice() else {
        eprintln!("usage: state_golden [--review] HTML.html DOM.html");
        std::process::exit(2)
    };
    let html = std::fs::read_to_string(html).unwrap();
    let dom = std::fs::read_to_string(dom).unwrap();
    let mut exit = 0;

    let start = std::time::Instant::now();
    let (conn, snapshots) = index(
        &html,
        &dom,
        webspec_index::state::extract::bundled_catalog(),
    );
    eprintln!("indexed (bundled) in {:?}", start.elapsed());
    for f in golden::check(&conn) {
        println!("GOLDEN {f}");
        exit = 1;
    }
    for (spec, snapshot) in ["HTML", "DOM"].into_iter().zip(snapshots) {
        let state = model(&conn, snapshot);
        if !report("bundled", spec, &state, &bundled_floors(spec)) {
            exit = 1;
        }
        if review {
            let c = &state.coverage;
            print_review("review", &c.unclassified_review);
            print_review("assert", &c.assert_review);
            print_review("undeclared", &c.undeclared_review);
        }
    }

    let (conn, snapshots) = index(&html, &dom, &StateCatalog::default());
    for (spec, snapshot) in ["HTML", "DOM"].into_iter().zip(snapshots) {
        if !report(
            "grammar",
            spec,
            &model(&conn, snapshot),
            &grammar_floors(spec),
        ) {
            exit = 1;
        }
    }
    std::process::exit(exit);
}
