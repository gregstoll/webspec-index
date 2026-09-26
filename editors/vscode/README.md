# webspec-lens

Hover over WHATWG/W3C/TC39 spec URLs in your code to see section content inline. Validate step comments against the spec algorithm and track implementation coverage.

## Features

### Spec URL hover

Hover any spec URL to see the section's rendered content without leaving your editor. Works in any file type — C++, Rust, JavaScript, Python, HTML, etc.

```cpp
// https://html.spec.whatwg.org/#navigate
//                                 ^ hover here to see the full algorithm
```

### Step validation

Step comments (e.g. `// 5.1. Let x be ...`) are matched against the spec algorithm. Mismatches and unknown steps show as warnings. Matching steps get a checkmark inlay hint.

```cpp
// https://html.spec.whatwg.org/#navigate
void DoNavigate(...) {
  // Step 1. Let cspNavigationType be ...    ✓  (matches spec)
  // Step 5.1. Assert: userInvolvement is    ⚠  (text differs)
  // Step 99. Nonexistent step               ⚠  (not in spec)
}
```

### Coverage code lens

A code lens above each spec URL shows how many algorithm steps are implemented:

```text
navigate: 7/23 steps | 2 warnings
```

### Possible effects

Exact and fuzzy step matches retain their checkmark. Steps with possible
effects have a separate, clickable CodeLens row below the entire step comment:

```text
// Step 12. …                                      ✓
   May do async work | May fire events | May run script | All effects
```

The row deduplicates reachable effects into categories. Clicking a category
opens a native editor hover at that step comment, scoped to the selected
category. Initially it shows only clickable headlines such as **may fire navigate
event**. Click a headline to expand its endpoints, representative path, captured
conditions and scheduling boundaries; click it again to collapse. Each effect
can be toggled independently, using already loaded content without a server
request. The hover receives focus so expanded details can be scrolled; Escape
dismisses it. The All effects link appears when categories exceed the configured
limit. Ordinary link and step hovers retain concise summaries; expanded details
clear when you move the cursor, switch editors, or edit the document.
Empty or unavailable analyses add no effects row; absence of a row does not
prove a step has no effects.

VS Code places CodeLens above its anchored line. The extension anchors these
rows to the line after the comment (or the last line at end of file).

`webspec-index update` prepares effects and representative paths after indexing
by default. For an existing database or changed rules, run
`webspec-index effects --all` (incremental — rebuilds only specs whose structure
changed) or `webspec-index effects --all --rebuild` (from scratch). Matching,
propagation, per-step summaries, and representative paths are prepared during
this operation. Hints, hovers, and category clicks only read stored results.
They never build analyses or search the graph. Newly published results become
available on the next editor request. If a query returns no effects with
`snapshot_changed`, the spec was updated and effects have not been rebuilt yet;
run `webspec-index effects --all`. Use the same rules/environment for indexing
and LSP. Broader trace exploration remains available in the CLI/API.

The palette command **webspec-lens: Show Spec Effects** opens the native hover
at the cursor; category rows open their prepared details in that same hover UI.

### Testing a local build

Build the server with `cargo install --offline --locked --path .` from the
repository root. In `editors/vscode`, run `npm run package`, then install the
generated VSIX using **Extensions: Install from VSIX...** and run
**Developer: Reload Window**.

Set `webspecLens.serverCommand` to the absolute path of your locally built
binary followed by `lsp`, for example:

```json
"webspecLens.serverCommand": ["/home/jan/.cargo/bin/webspec-index", "lsp"]
```

Open `examples/effects-editor-smoke.cpp` from the repository root. The recognized
parallel and event-dispatch step comments show clickable rows when analysis is prepared.
Hover a step or its spec URL for a summary. Place the cursor on a URL or step
comment and run **webspec-lens: Show Spec Effects** to open that hover using the
keyboard. Check the **webspec-lens** Output channel if the server fails to start.

## Setup

**No manual installation required.** If `webspec-index` is not found on your PATH, the extension will offer to download the correct binary for your platform automatically.

To install manually instead: `cargo binstall webspec-index` or `cargo install webspec-index`.

Spec data is fetched and cached automatically on first query — no setup step needed.
The spec and its transitive dependencies are checked at most once per 24 h; the
first query of the day may take a few seconds while the check completes.

## How it works

The extension launches a lightweight LSP server (`webspec-index lsp`) over stdio. All spec data is queried from a local SQLite database. Specs are fetched on first access and cached locally, so there are no network requests during normal editing after the initial fetch.

The server is auto-detected in this order:

1. `webspecLens.serverCommand` setting (if configured)
2. `webspec-index` on PATH
3. Previously downloaded binary (auto-updated when the extension updates)

## Settings

| Setting                      | Default     | Description                                                    |
| ---------------------------- | ----------- | -------------------------------------------------------------- |
| `webspecLens.enabled`        | `true`      | Enable or disable the extension                                |
| `webspecLens.serverCommand`  | auto-detect | Command to start the LSP server                                |
| `webspecLens.fuzzyThreshold` | `0.85`      | Jaro-Winkler similarity threshold for step matching (0.0–1.0)  |
| `webspecLens.effects.enabled` | `true` | Show possible effects in hints and hovers. |
| `webspecLens.effects.maxBadges` | `3` | Maximum categories in one effects row (plus All effects on overflow). |
| `webspecLens.effects.rulePaths` | `[]` | Additional local rule packages. |
| `webspecLens.effects.environment` | `web` | Host implementation environment. |

## License

MIT
