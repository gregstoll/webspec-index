---
name: spec-trace
description: Trace a claim about web spec behaviour through the algorithm call chain and produce a Bugzilla-ready markdown trace ending in a REACHED / NOT REACHED / INCONCLUSIVE verdict. Use when a bug says "the spec says X should happen" and you need to check whether it does, when asked where in the spec some event gets fired or some state gets set, or when a reporter's expected-results section needs a spec citation it did not provide.
argument-hint: "[--gecko] <question in prose>"
allowed-tools:
  - Bash(webspec-index:*)
  - Bash(searchfox-cli:*)
  - Read
---

# Spec Trace

**Question**: $ARGUMENTS

Produce an ordered chain of spec algorithm invocations from a web-visible entry point to the
behaviour in question, quoting the deciding step at each hop, ending in a verdict. The output is
pasted into a Bugzilla comment as an argument, so every link must resolve and every quote must be
verbatim. Pass `--gecko` to also map the trace to Gecko source.

`webspec-index trace` does the search and supplies each hop's step number, verbatim text, guard
steps and call-site link. Your work is deciding which of the routes it returns actually apply.

## Phase 0: Resolve

- **Entry anchor** — the API the scenario starts from. `webspec-index idl 'Interface.member()'`
  resolves IDL members directly; use `search` or `anchors` for non-IDL entry points.
- **Target anchor** — what the question asks about. Events are indexed as definitions:
  `webspec-index anchors '*eventname*' --spec HTML`.
- **Scenario facts** — the preconditions that decide branches. State them in the output; a trace
  is only valid for the scenario it assumed, and most wrong traces are wrong because an unstated
  assumption silently picked a branch. If the question leaves a branch-deciding fact open, name
  the assumption and trace the branch the reporter most plausibly meant.

## Phase 1: Enumerate routes

```bash
webspec-index trace '<SPEC#entry>' '<SPEC#target>' --max-depth 9 -l 40 --format markdown
```

Defaults to `--kind step`, so prose mentions, notes and IDL tables are already excluded — those
are roughly 60% of the reference graph and all of them are non-calls.

Read the header before the traces. `Found N trace(s)` with `truncated: false` means the enumeration
is exhaustive within that depth, which is what lets a NOT REACHED verdict claim more than "I did
not find one". If it says the search was truncated, raise `-l`, or narrow the endpoints, and say
so in the output if it still truncates.

When a route points into a spec that has never been indexed, `trace` indexes it and searches
again, printing `indexing N spec(s) the route touches` to stderr. That costs about a second per
spec on the first run and nothing afterwards, so a cross-spec trace may pause briefly. It gives up
after two rounds; if the line keeps appearing, the chain reaches further than the search will
follow and the verdict is INCONCLUSIVE.

Start at `--max-depth 9`. Real chains run longer than they look — eight or nine hops is ordinary.
A depth that is too low reports zero traces, which reads exactly like a genuine NOT REACHED and is
the easiest way to get this wrong. If you get zero traces, raise the depth once before believing
it.

If the target is reachable only through a different entry point than you assumed, `refs
'<SPEC#target>' --direction incoming --kind step -l 50` shows who really calls it.

`--detail` picks how much of each hop to show:

- `verbose` (default) — step text, guards and call-site link. What you judge from.
- `edges` — `SPEC#anchor` plus step number, no prose and no URLs. About a third the size, and
  every token is an identifier `query` and `refs` accept. Use it to compare route shapes when
  there are many, then re-run verbose on the ones you mean to judge.
- `compact` — one linked line per hop, with an edge's repeated call sites collapsed onto it. Best
  for pasting a route into a comment where the reader wants links rather than quotations.

Do not judge from a reduced level: the guards it drops are exactly what decides whether a route is
taken, and reading sections to recover them costs far more than carrying them did.

## Phase 2: Judge each route

The tool proves a route exists in the reference graph. It cannot know your scenario. For each
route, decide whether it is actually taken, using what each hop already gives you:

- **Guards.** Does a `- under:` condition contradict a scenario fact? That kills the route.
- **Arguments.** Does the step text pass, or omit, an optional argument the callee branches on?
  An omitted argument frequently disables the branch the whole question turns on.
- **Ordering.** Compare step numbers within a section. A call at step 20 runs before one at step
  24, so state it may depend on has not been set up yet, or has already been torn down.
- **Identity.** Does the hop act on the object the question is about? An algorithm that aborts
  *the previous* ongoing navigation does not report *this* one.

Read the full section only when a hop's own text and guards are not enough:
`webspec-index query '<SPEC#anchor>' --format markdown`.

Record why each rejected path is rejected. Those reasons are the output, not scratch work.

## Phase 3: Render

````markdown
**Question:** <the question>

**Scenario:** <the preconditions assumed>

1) [`<SPEC#algorithm>` step N](<call-site url>) calls `<SPEC#callee>`, <argument fact that decides the branch>
   - under: <guard>
   > <verbatim step text>

2) …

**Verdict: <REACHED | NOT REACHED | INCONCLUSIVE>** — <one sentence>

| Rejected path | Rejected because |
|---|---|
````

The markdown from `trace` is already this shape; keep its hop links and quoted steps verbatim and
add the scenario, the verdict and the rejected-path table.

- Hop links point at the **call site**, not the callee's definition, when the spec generator
  emitted a per-reference id. Keep them — landing the reader on the calling line is the whole
  point. Roughly a third of hops have no such id, and a step that calls nothing never does, so the
  deciding step of a NOT REACHED verdict usually has none. Link the section and name the step in
  the visible text instead: ``[`HTML#navigate` step 24.2](https://html.spec.whatwg.org/#navigate)``.
  A reader finds a numbered step; they cannot find a link that goes nowhere.

  **Never use `:~:text=` fragments.** A text directive matches rendered prose, which no tool here
  can verify and which the spec editors change without notice; when it stops matching it fails
  silently, dropping the reader at the top of the document with nothing to indicate the link was
  meant to point somewhere. A section anchor plus a step number in the visible text degrades
  instead: if the step moves, the reader is still on the right algorithm and can see which step was
  meant.
- Say explicitly where the chain returns to an earlier algorithm; that unwind is usually where the
  answer lives.

Exactly one verdict:

- **REACHED** — the spec specifies the behaviour; name the step. Firefox not doing it is a bug.
- **NOT REACHED** — no route survived judgement. The rejected-paths table is what makes this an
  argument rather than an assertion, and it is what a reporter will push back on. Also consider
  whether the spec omission is itself the defect, in which case the follow-up is a spec issue and
  Firefox is compliant.
- **INCONCLUSIVE** — the chain hit a UA-defined step, an unindexed spec, or the search truncated.
  Say where it stopped and what would resolve it. Never soften this into another verdict for
  tidiness.

## `--gecko` mode

Append a table mapping the trace to Gecko source, keyed on canonical URL:

```bash
searchfox-cli --spec-refs '<url>'
```

- Separate code hits from test hits, and call out algorithms with no hits — an unreferenced spec
  algorithm is often the reason for the bug.
- One row per distinct URL, not per hop. An algorithm the chain re-enters collapses into one row
  listing the hops it covers.
- Add the rejected algorithms as `(not reached)` rows. For a NOT REACHED verdict they carry the
  weight: the code exists and simply is not wired to this path.
- Discard matches under `.claude/skills/` and `.agents/skills/`. In-tree skill documentation
  quotes spec URLs in its examples and is indexed as code, in both copies.

`--permalink` is accepted but currently returns the same `/source/` URLs as `--link`, so do not
promise commit-pinned links.

## Hard rules

**Every quote comes from tool output.** `trace` and `refs` emit plain step text; use it as given.
`query` renders markdown, so its step text carries `*emphasis*` around variable names and inline
`[links](...)`. Stripping that syntax to recover the spec's own prose is not paraphrasing; changing
a word is. Never reconstruct from memory, and never present a truncated fragment as a quotation. A
trace with an invented quote loses the argument it was written to win.

**Validate every anchor** with `webspec-index exists '<SPEC#anchor>'` before it ships, including
the ones in the rejected-paths table. Those are the links a reader clicks to push back. Exit code
0 means the anchor resolves.

**Trace the spec as of today.** If the bug references an unlanded spec PR, add `--pr N` to the
`query`, `refs` and `exists` calls, and say in the output which PR the trace assumes. `trace` does
not take `--pr`; against a PR, fall back to `refs --kind step` in both directions.

**Distinguish exhausted from truncated.** `truncated: false` and zero traces is evidence that no
route exists among indexed specs. A truncated search is not, and must be reported as INCONCLUSIVE.
