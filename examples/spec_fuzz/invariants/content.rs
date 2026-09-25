//! Content conservation: C1 links, C2 words, C3 presence, O1 order (report-only).

use std::collections::BTreeMap;

use serde_json::json;
use webspec_index::model::SectionType;

use super::{Ctx, Evidence, Failure, Invariant, Outcome, SectionCtx};
use crate::oracle;

const MAX_ITEMS: usize = 40;

fn section<'c, 'a>(ctx: &Ctx<'c, 'a>) -> Option<&'c SectionCtx<'a>> {
    match ctx {
        Ctx::Section(s) => Some(s),
        Ctx::Spec(_) => None,
    }
}

/// Heading content continues with its subsections after the own prose; order-aware comparisons
/// look only at the part that can correspond to the own prose.
fn own_prose_prefix<'v>(s: &SectionCtx, rendered: &'v [String], src_len: usize) -> &'v [String] {
    if s.region.shape.is_heading() {
        &rendered[..rendered.len().min(src_len + src_len / 2 + 50)]
    } else {
        rendered
    }
}

pub struct C1;
impl Invariant for C1 {
    fn id(&self) -> &'static str {
        "C1"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if s.ty() == SectionType::Idl || !s.region.shape.has_region() {
            return Outcome::NotApplicable;
        }
        let src = s.src_text();
        let expected = src.expected_links();
        let got = &s.md().links;
        let one_sided = s.region.shape.is_heading();
        let miss = oracle::missing(&expected, got);
        let extra = if one_sided {
            Vec::new()
        } else {
            oracle::missing(got, &expected)
        };
        if miss.is_empty() && extra.is_empty() {
            return Outcome::Pass;
        }
        let mut items = Vec::new();
        let links: Vec<_> = src.links.iter().filter(|l| l.cat.expected_link()).collect();
        let got_prefix = own_prose_prefix(s, got, expected.len());
        for i in oracle::lost_positions(&expected, got_prefix, &miss)
            .into_iter()
            .take(MAX_ITEMS)
        {
            let l = links[i];
            let host = l.node.parent().unwrap_or(l.node);
            let text = scraper::ElementRef::wrap(l.node)
                .map(|e| e.text().collect::<String>())
                .unwrap_or_default();
            items.push(
                Evidence::new("missing", l.abs.clone(), s.construct(l.node))
                    .source(Some(oracle::outer_html_excerpt(host, 300)))
                    .rendered(
                        oracle::rendered_excerpt(s.content(), text.trim())
                            .map(|x| oracle::excerpt(&x, 300)),
                    ),
            );
        }
        for url in extra.iter().take(MAX_ITEMS) {
            let other = src.links.iter().find(|l| &l.abs == url);
            let construct = match other {
                Some(l) => format!("{:?}:{}", l.cat, s.construct(l.node)),
                None => "not-in-region".into(),
            };
            items.push(Evidence::new("extra", url.clone(), construct).rendered(
                oracle::rendered_excerpt(s.content(), url).map(|x| oracle::excerpt(&x, 300)),
            ));
        }
        Outcome::fail(
            items,
            json!({"missing": miss, "links": expected.len(), "one_sided": one_sided}),
            json!({"extra": extra, "links": got.len()}),
        )
    }
}

pub struct C2;
impl Invariant for C2 {
    fn id(&self) -> &'static str {
        "C2"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if !s.region.shape.has_region() {
            return Outcome::NotApplicable;
        }
        let sw = s.src_text().words_with_nodes();
        let src_words: Vec<String> = sw.iter().map(|(w, _)| w.clone()).collect();
        let md_words = oracle::words(&s.md().text);
        let miss = oracle::missing(&src_words, &md_words);
        if miss.is_empty() {
            return Outcome::Pass;
        }
        // Attribute each missing word to the element holding it; group by construct.
        let mut by_construct: BTreeMap<String, (Vec<String>, Option<String>)> = BTreeMap::new();
        let md_prefix = own_prose_prefix(s, &md_words, src_words.len());
        for i in oracle::lost_positions(&src_words, md_prefix, &miss) {
            let (w, node) = &sw[i];
            let construct = node
                .map(|n| s.block_construct(n))
                .unwrap_or_else(|| "text".into());
            let entry = by_construct
                .entry(construct)
                .or_insert_with(|| (Vec::new(), node.map(|n| oracle::outer_html_excerpt(n, 300))));
            entry.0.push(w.clone());
        }
        let items = by_construct
            .into_iter()
            .take(MAX_ITEMS)
            .map(|(construct, (mut ws, excerpt))| {
                ws.truncate(12);
                Evidence::new("missing-words", ws.join(" "), construct).source(excerpt)
            })
            .collect();
        Outcome::fail(
            items,
            json!({"missing": miss.iter().take(200).collect::<Vec<_>>(), "words": src_words.len()}),
            json!({"words": md_words.len()}),
        )
    }
}

pub struct C3;
impl Invariant for C3 {
    fn id(&self) -> &'static str {
        "C3"
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if !s.region.shape.has_region() {
            return Outcome::NotApplicable;
        }
        let has_text = !oracle::alnum(&s.src_text().text).is_empty();
        if !has_text {
            return Outcome::NotApplicable;
        }
        Outcome::check(!s.content().trim().is_empty(), || Failure {
            items: vec![
                Evidence::new("empty-content", s.anchor(), s.region.shape.as_str())
                    .source(Some(oracle::excerpt(&s.region_html(), 400))),
            ],
            expected: json!({"nonempty": true}),
            actual: json!({"content": s.section.content_text}),
        })
    }
}

/// Order-sensitive word diff, evaluated where C2 passes so that it reports reorderings only.
pub struct O1;
impl Invariant for O1 {
    fn id(&self) -> &'static str {
        "O1"
    }
    fn report_only(&self) -> bool {
        true
    }
    fn check(&self, ctx: Ctx<'_, '_>) -> Outcome {
        let Some(s) = section(&ctx) else {
            return Outcome::NotApplicable;
        };
        if !s.region.shape.has_region() {
            return Outcome::NotApplicable;
        }
        let src_words = oracle::words(&s.src_text().text);
        let md_words = oracle::words(&s.md().text);
        if src_words.is_empty() || !oracle::missing(&src_words, &md_words).is_empty() {
            return Outcome::NotApplicable;
        }
        let ops = similar::capture_diff_slices_deadline(
            similar::Algorithm::Myers,
            &src_words,
            own_prose_prefix(s, &md_words, src_words.len()),
            Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
        );
        let mut runs: Vec<String> = Vec::new();
        let mut deleted = 0;
        for op in ops {
            if let similar::DiffOp::Delete {
                old_index, old_len, ..
            } = op
            {
                deleted += old_len;
                runs.push(src_words[old_index..old_index + old_len].join(" "));
            }
        }
        if deleted == 0 {
            return Outcome::Pass;
        }
        runs.sort_by_key(|r| std::cmp::Reverse(r.len()));
        let items = vec![Evidence::new(
            "reordered",
            oracle::excerpt(&runs[0], 200),
            s.region.shape.as_str(),
        )];
        Outcome::fail(
            items,
            json!({"words": src_words.len()}),
            json!({"deleted_words": deleted, "runs": runs.iter().take(5).collect::<Vec<_>>()}),
        )
    }
}
