#!/bin/sh
# Needs the wasm32-unknown-unknown target, a wasm-bindgen-cli matching the wasm-bindgen
# version in Cargo.lock, and Node.js.
set -e
cd "$(dirname "$0")"
cargo build --release --locked --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir www/pkg target/wasm32-unknown-unknown/release/blip_web.wasm
[ node_modules/.package-lock.json -nt package-lock.json ] || npm ci
npm run build
