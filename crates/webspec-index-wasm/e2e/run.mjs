import { firefox, chromium } from "playwright";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const fixtureDir = path.resolve(
  process.argv[2] || path.join(__dirname, "..", "..", "..", "target", "fixture-export")
);
const engineName = process.argv[3] || "firefox";

const siteDir = path.join(__dirname, "site");

fs.rmSync(siteDir, { recursive: true, force: true });
fs.mkdirSync(siteDir, { recursive: true });

const copy = (src, dst) => {
  fs.mkdirSync(path.dirname(dst), { recursive: true });
  fs.cpSync(src, dst, { recursive: true });
};

copy(path.join(__dirname, "index.html"), path.join(siteDir, "index.html"));
copy(path.join(__dirname, "worker.js"), path.join(siteDir, "worker.js"));
copy(path.join(__dirname, "..", "pkg"), path.join(siteDir, "pkg"));
copy(fixtureDir, path.join(siteDir, "db"));

function serveRange(req, res) {
  const urlPath = decodeURIComponent(req.url.split("?")[0]);
  const filePath = path.join(siteDir, urlPath === "/" ? "index.html" : urlPath);

  let stat;
  try {
    stat = fs.statSync(filePath);
  } catch {
    res.writeHead(404);
    res.end("Not found");
    return;
  }

  const ext = path.extname(filePath);
  const mime = {
    ".html": "text/html",
    ".js": "application/javascript",
    ".wasm": "application/wasm",
    ".json": "application/json",
    ".bin": "application/octet-stream",
  }[ext] || "application/octet-stream";

  const rangeHeader = req.headers["range"];
  if (rangeHeader) {
    const m = rangeHeader.match(/bytes=(\d+)-(\d+)/);
    if (!m) {
      res.writeHead(416);
      res.end();
      return;
    }
    const start = Number(m[1]);
    const end = Number(m[2]);
    const length = end - start + 1;
    res.writeHead(206, {
      "Content-Type": mime,
      "Content-Range": `bytes ${start}-${end}/${stat.size}`,
      "Content-Length": length,
      "Access-Control-Allow-Origin": "*",
    });
    const stream = fs.createReadStream(filePath, { start, end });
    stream.pipe(res);
  } else {
    res.writeHead(200, {
      "Content-Type": mime,
      "Content-Length": stat.size,
      "Accept-Ranges": "bytes",
      "Access-Control-Allow-Origin": "*",
    });
    if (req.method === "HEAD") {
      res.end();
    } else {
      fs.createReadStream(filePath).pipe(res);
    }
  }
}

const server = http.createServer(serveRange);
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
const baseUrl = `http://127.0.0.1:${port}`;

console.log(`Serving ${siteDir} on ${baseUrl}`);
console.log(`Fixture: ${fixtureDir}`);

const browser = await (engineName === "chromium" ? chromium : firefox).launch();
const page = await browser.newPage();

page.on("console", (msg) => console.log(`[page] ${msg.text()}`));
page.on("pageerror", (err) => console.error(`[pageerror] ${err}`));

await page.goto(baseUrl);

await page.waitForFunction(
  () => document.getElementById("out")?.textContent?.includes("DONE"),
  { timeout: 30000 }
);

const text = await page.textContent("#out");
await browser.close();
server.close();

console.log("--- page output ---");
console.log(text);
console.log("---");

const lines = text.trim().split("\n");
const parsed = lines.filter((l) => l.startsWith("{")).map((l) => JSON.parse(l));

if (parsed.length < 6) {
  console.error(`Expected at least 6 JSON lines, got ${parsed.length}`);
  process.exit(1);
}

const [specs, query, search, refs, byUrl, statsResult] = parsed;

assert.equal(specs.type, "specs");
assert.deepEqual(
  specs.result.specs.map((s) => s.name),
  ["DOM", "HTML"]
);

assert.equal(query.type, "query");
assert.equal(query.result.anchor, "navigate");
assert.ok(
  query.result.content_html.includes('href="#/DOM/concept-tree"'),
  `content_html missing link: ${query.result.content_html}`
);
assert.equal(query.result.navigation.parent.anchor, "browsing");

assert.equal(search.type, "search");
assert.equal(search.result.results[0].anchor, "navigate");

assert.equal(refs.type, "refs");

assert.equal(byUrl.type, "query");
assert.equal(byUrl.result.anchor, "navigate");

const statsObj = typeof statsResult === "string" ? JSON.parse(statsResult) : statsResult;
assert.ok(
  statsObj.fetches > 0 && statsObj.fetches < 200,
  `unexpected fetch count: ${JSON.stringify(statsObj)}`
);

console.log("e2e ok", JSON.stringify(statsObj));
