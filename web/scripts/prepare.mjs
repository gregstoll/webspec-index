#!/usr/bin/env node
// prepare.mjs — copies build artefacts into the web source tree.
//
// Usage:
//   node scripts/prepare.mjs wasm
//     Copies webspec_index_wasm.js and webspec_index_wasm_bg.wasm from
//     ../crates/webspec-index-wasm/pkg/ into src/wasm/.
//     Does NOT copy .d.ts files; src/wasm/webspec_index_wasm.d.ts is
//     hand-maintained and committed — overwriting it would lose type edits.
//     Fails with exit code 1 if pkg/ is missing (run build.sh first).
//
//   node scripts/prepare.mjs db [dir]
//     Empties public/db/ and copies manifest.json and *.bin from <dir>.
//     <dir> defaults to $WEBSPEC_EXPORT_DIR, then ../target/fixture-export.

import { copyFileSync, mkdirSync, readdirSync, rmSync, statSync } from 'fs';
import { join, resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const webRoot = resolve(__dirname, '..');

function fmt(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

function copyFile(src, dest) {
  copyFileSync(src, dest);
  const size = statSync(dest).size;
  console.log(`  copied  ${dest.replace(webRoot + '/', '')}  (${fmt(size)})`);
}

function prepareWasm() {
  const pkgDir = resolve(webRoot, '../crates/webspec-index-wasm/pkg');
  let pkgStat;
  try {
    pkgStat = statSync(pkgDir);
  } catch {
    console.error(
      'ERROR: wasm package not found at ' + pkgDir + '\n' +
      'Run ./crates/webspec-index-wasm/build.sh first, then retry.'
    );
    process.exit(1);
  }
  if (!pkgStat.isDirectory()) {
    console.error('ERROR: ' + pkgDir + ' is not a directory');
    process.exit(1);
  }

  const destDir = join(webRoot, 'src', 'wasm');
  mkdirSync(destDir, { recursive: true });

  const files = [
    'webspec_index_wasm.js',
    'webspec_index_wasm_bg.wasm',
  ];

  console.log('prepare:wasm');
  for (const f of files) {
    const src = join(pkgDir, f);
    try {
      statSync(src);
    } catch {
      console.error('ERROR: expected file not found: ' + src);
      process.exit(1);
    }
    copyFile(src, join(destDir, f));
  }
}

function prepareDb(dir) {
  let srcDir = dir;
  if (!srcDir) {
    srcDir = process.env.WEBSPEC_EXPORT_DIR || resolve(webRoot, '../target/fixture-export');
  }
  srcDir = resolve(srcDir);

  let srcStat;
  try {
    srcStat = statSync(srcDir);
  } catch {
    console.error(
      'ERROR: export directory not found: ' + srcDir + '\n' +
      'Run: cargo run --example fixture_export -- target/fixture-export\n' +
      'or set WEBSPEC_EXPORT_DIR to the directory.'
    );
    process.exit(1);
  }
  if (!srcStat.isDirectory()) {
    console.error('ERROR: ' + srcDir + ' is not a directory');
    process.exit(1);
  }

  const destDir = join(webRoot, 'public', 'db');
  // Empty the destination directory
  try {
    rmSync(destDir, { recursive: true });
  } catch { /* ignore if missing */ }
  mkdirSync(destDir, { recursive: true });

  const entries = readdirSync(srcDir);
  const toCopy = entries.filter(f => f === 'manifest.json' || f.endsWith('.bin'));

  if (toCopy.length === 0) {
    console.error('ERROR: no manifest.json or *.bin files found in ' + srcDir);
    process.exit(1);
  }

  console.log('prepare:db  src=' + srcDir);
  for (const f of toCopy.sort()) {
    copyFile(join(srcDir, f), join(destDir, f));
  }
}

const [, , cmd, ...args] = process.argv;

if (cmd === 'wasm') {
  prepareWasm();
} else if (cmd === 'db') {
  prepareDb(args[0]);
} else {
  console.error(
    'Usage:\n' +
    '  node scripts/prepare.mjs wasm\n' +
    '  node scripts/prepare.mjs db [export-dir]'
  );
  process.exit(1);
}
