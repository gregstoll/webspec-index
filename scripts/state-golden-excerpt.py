#!/usr/bin/env python3
"""Cut verbatim excerpts of a spec snapshot for the state golden tests.

Usage: uv run --with lxml python scripts/state-golden-excerpt.py SNAPSHOT.html OUT.html ANCHOR...

For each anchor it copies one unit, in document order, without rewriting markup:
the enclosing div.algorithm / div[data-algorithm]; else, for an algorithm dfn, its
block plus the following <ol>/<dl> sibling; else, for a list-item dfn, the list and
its intro paragraph; else the IDL <pre>; else the innermost p/li/dd/dt block; for a
heading anchor, the heading element. Units are deduplicated, and units nested in
another selected unit are dropped. A missing anchor is an error.
"""
import sys
from lxml import html as lh

BLOCKS = {"p", "li", "dd", "dt", "td"}


def unit(el):
    for a in el.iterancestors():
        if a.tag == "div" and ("algorithm" in (a.get("class") or "").split() or a.get("data-algorithm") is not None):
            return [a]
    if el.tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
        return [el]
    for a in el.iterancestors():
        if a.tag == "pre":
            return [a]
    block = next((a for a in el.iterancestors() if a.tag in BLOCKS), None)
    if block is None:
        return [el]
    if block.tag in {"li", "dt", "dd"} or (block.getparent() is not None and block.getparent().tag in {"li", "dd"}):
        item = block if block.tag in {"li", "dt", "dd"} else block.getparent()
        lst = item.getparent()
        if lst is not None and lst.tag in {"ul", "dl"}:
            intro = lst.getprevious()
            while intro is not None and "note" in (intro.get("class") or "").split():
                intro = intro.getprevious()
            return [x for x in (intro, lst) if x is not None]
    nxt = block.getnext()
    if nxt is not None and nxt.tag in {"ol", "dl"}:
        return [block, nxt]
    return [block]


def main():
    src, out, anchors = sys.argv[1], sys.argv[2], sys.argv[3:]
    doc = lh.parse(src).getroot()
    order = {el: i for i, el in enumerate(doc.iter())}
    units = []
    for anchor in anchors:
        found = doc.xpath(f'//*[@id="{anchor}"]')
        if not found:
            sys.exit(f"missing anchor {anchor}")
        for el in unit(found[0]):
            if el not in units:
                units.append(el)
    units = [el for el in units if not any(u in units for u in el.iterancestors())]
    units.sort(key=lambda el: order[el])
    body = "\n".join(lh.tostring(el, encoding="unicode", with_tail=False) for el in units)
    with open(out, "w", encoding="utf-8") as f:
        f.write(f"<!doctype html><html><body>\n{body}\n</body></html>\n")


if __name__ == "__main__":
    main()
