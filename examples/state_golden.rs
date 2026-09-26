//! Whole-corpus golden check and coverage print (spec §12.3–§12.4).
//!
//! Usage: cargo run --release --example state_golden -- [--review] HTML.html DOM.html
//! Indexes both snapshots twice: with the bundled catalog (golden set, bundled floors) and
//! with an empty catalog (grammar-only floors). `--review` prints the unclassified review
//! items of the bundled run. Exits 1 on any golden mismatch or coverage floor violation.
#[path = "../tests/state_golden.rs"]
#[allow(dead_code)]
mod golden;

use webspec_index::state::catalog::StateCatalog;
use webspec_index::state::model::CoverageCounters;

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

fn coverage(conn: &rusqlite::Connection, snapshot: i64) -> CoverageCounters {
    webspec_index::db::state::load_state_model(conn, snapshot)
        .unwrap()
        .unwrap()
        .coverage
}

/// Prints the §12.4 numbers for one run; returns false when a floor is violated.
fn report(run: &str, spec: &str, c: &CoverageCounters, floors: &Floors) -> bool {
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
        let c = coverage(&conn, snapshot);
        if !report("bundled", spec, &c, &bundled_floors(spec)) {
            exit = 1;
        }
        if review {
            for item in &c.unclassified_review {
                println!(
                    "  review: {spec}#{}{} [{}] {}",
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
    }

    let (conn, snapshots) = index(&html, &dom, &StateCatalog::default());
    for (spec, snapshot) in ["HTML", "DOM"].into_iter().zip(snapshots) {
        if !report(
            "grammar",
            spec,
            &coverage(&conn, snapshot),
            &grammar_floors(spec),
        ) {
            exit = 1;
        }
    }
    std::process::exit(exit);
}
