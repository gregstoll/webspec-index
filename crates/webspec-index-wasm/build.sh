#!/bin/sh
set -eu
cd "$(dirname "$0")"
CC_wasm32_unknown_unknown=clang cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/webspec_index_wasm.wasm
ls -la pkg/webspec_index_wasm_bg.wasm
