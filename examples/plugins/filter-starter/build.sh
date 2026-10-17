#!/bin/sh
set -eu
cd "$(dirname "$0")"
target_dir="${CARGO_TARGET_DIR:-target}"
cargo build --locked --release --target wasm32-unknown-unknown --target-dir "$target_dir"
cp "$target_dir/wasm32-unknown-unknown/release/photocraft_filter_channel_gain.wasm" Channel-Gain.wasm
printf '%s\n' 'Built Channel-Gain.wasm. Install it through Plugins > Install Plug-in.'
