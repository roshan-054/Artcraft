#!/bin/sh
set -eu
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/photocraft_extension_quick_tools.wasm quick-tools.pcext
