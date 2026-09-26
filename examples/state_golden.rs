//! Whole-corpus golden check and coverage print (spec §12.3–§12.4).
//!
//! Usage: cargo run --release --example state_golden -- HTML.html DOM.html
//! Exits 1 on any golden mismatch or coverage floor violation.
#[path = "../tests/state_golden.rs"]
#[allow(dead_code)]
mod golden;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [html, dom] = args.as_slice() else {
        eprintln!("usage: state_golden HTML.html DOM.html");
        std::process::exit(2)
    };
    let conn = webspec_index::db::open_in_memory().unwrap();
    let start = std::time::Instant::now();
    let h = webspec_index::state::testing::index_offline(
        &conn,
        "HTML",
        "https://html.spec.whatwg.org/",
        &std::fs::read_to_string(html).unwrap(),
    )
    .unwrap();
    let d = webspec_index::state::testing::index_offline(
        &conn,
        "DOM",
        "https://dom.spec.whatwg.org/",
        &std::fs::read_to_string(dom).unwrap(),
    )
    .unwrap();
    eprintln!("indexed in {:?}", start.elapsed());
    let failures = golden::check(&conn);
    let mut exit = 0;
    for f in &failures {
        println!("GOLDEN {f}");
        exit = 1;
    }
    for (spec, snapshot) in [("HTML", h), ("DOM", d)] {
        let state = webspec_index::db::state::load_state_model(&conn, snapshot)
            .unwrap()
            .unwrap();
        let c = &state.coverage;
        let owned: u32 = c.written_fields_owned.values().sum();
        let owner_pct = 100.0 * owned as f64 / c.written_fields.max(1) as f64;
        let set_pct = 100.0 * c.set_structured as f64 / c.set_total.max(1) as f64;
        let occ: u32 = c.occurrences.values().sum();
        let unclassified_pct =
            100.0 * *c.occurrences.get("unclassified").unwrap_or(&0) as f64 / occ.max(1) as f64;
        println!(
            "{spec}: written-field owners {owner_pct:.1}% ({owned}/{}), structured Set {set_pct:.1}% ({}/{}), unclassified {unclassified_pct:.2}% of {occ}",
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
        let (owner_floor, set_floor, unclassified_ceiling) = if spec == "HTML" {
            (82.0, 93.0, 2.7)
        } else {
            (94.0, 95.0, 1.9)
        };
        if owner_pct < owner_floor || set_pct < set_floor || unclassified_pct > unclassified_ceiling
        {
            println!("FLOOR {spec} below §12.4");
            exit = 1;
        }
        for unresolved in &c.written_fields_unresolved {
            println!("  unresolved owner: {spec}#{unresolved}");
        }
    }
    std::process::exit(exit);
}
