# webspec-index web UI

Static Preact app that browses the webspec-index database of WHATWG, W3C, and TC39 specifications.

## Running locally

```sh
cd web
npm install
npm run dev        # Vite dev server on http://localhost:5173
npm test           # vitest unit tests (router + dispatch + WorkerClient + MockClient)
npm run build      # type-check + production build → dist/
```

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
