# webspec-index web UI

Static Preact app that browses the webspec-index database of WHATWG, W3C, and TC39 specifications.

## Running locally

**Setup loop:**

```sh
# 1. Build the wasm package (only needed after Rust changes)
./crates/webspec-index-wasm/build.sh

# 2. Put a database export into public/db/. A real one, a few specs for a quick loop:
webspec-index export-web --out web/public/db --specs HTML,DOM,FETCH,URL,INFRA
# or every WHATWG/W3C/TC39 spec (the index must hold the effects graph; `update` builds it):
webspec-index export-web --out web/public/db --providers whatwg,w3c,tc39

# 3. Start the dev server
cd web && npm run dev   # runs prepare:wasm, then Vite on http://localhost:5173
```

The browser end-to-end test never touches `public/db`: it builds without copying `public/`
and serves the fixture export from `target/fixture-export` itself.

```sh
cargo run --example fixture_export -- target/fixture-export
cd web && npm run build:e2e && npm run e2e     # Firefox; E2E_DB_DIR overrides the fixture dir
```

Other commands:

```sh
npm test           # vitest unit tests
npm run build      # prepare:wasm + type-check + production build → dist/ (copies public/db)
npm run build:mock # build with VITE_BACKEND=mock (no wasm, uses in-memory fixtures)
npm run prepare:db [DIR]   # copy an export directory into public/db/ (default target/fixture-export)
```

### VITE_BACKEND=mock escape hatch

Set `VITE_BACKEND=mock` (or use `npm run build:mock`) to build without the wasm
backend. The app serves mock data from in-memory fixtures. Useful for UI-only
work when the wasm package is not built.

### Generated directories

`src/wasm/` and `public/db/` are **generated** — their contents are produced by
`npm run prepare:wasm` and `npm run prepare:db` respectively and should not be
committed. The exception is `src/wasm/webspec_index_wasm.d.ts`, which is a
hand-maintained TypeScript declaration file that **is** committed.

## Architecture (10 lines)

- **Router** (`src/router.ts`): pure hash router; `parseRoute` / `routeToHash`.
- **Dispatch** (`src/dispatch.ts`): `routeForQuery(input)` converts a search-box string to a Route; `navigateForQuery` applies it.
- **API types** (`src/api/types.ts`): TypeScript mirror of the Rust model structs. Response envelope is adjacent-tagged: `{ "type": "query", "result": { ...QueryResult } }`. Errors are flat: `{ "type": "error", "code": "...", "message": "..." }`.
- **Client** (`src/api/client.ts`): `WebspecClient` interface; `WorkerClient` (postMessage, promise correlation, error-event rejection) and `MockClient` (in-memory fixtures).
- **Worker** (`src/worker.ts`): Web Worker skeleton; currently serves `MockClient`. Swap `loadBackend()` for the wasm client once `webspec-index-wasm` is built.
- **App** (`src/app.tsx`): creates the worker + `WorkerClient`, reads hash, dispatches to one of five views, hosts the theme toggle.
- **Views** (`src/views/`): `Landing`, `Headings`, `Section`, `Search`, `NotFound`, `TracePanel` (placeholder).
- **Styles** (`src/styles/`): hand-written CSS with design tokens; light/dark themes via `prefers-color-scheme` and a manual toggle stored in `localStorage`.
- The worker currently serves **mock data** — real queries need the wasm backend (phase 5 of the implementation plan).
- `base: './'` in `vite.config.ts` makes the built site work under any GitHub Pages subpath.

## Source

<https://github.com/jnjaeschke/webspec-index>
