#!/bin/sh
set -eu
cd "$(dirname "$0")"
cargo build --locked --release --target wasm32-unknown-unknown "$@"
target_dir=${CARGO_TARGET_DIR:-target}
cp "$target_dir/wasm32-unknown-unknown/release/hello_photocraft.wasm" Hello-PhotoCraft.pcext
printf '%s\n' 'Built Hello-PhotoCraft.pcext. Install it through Plugins > Manage Plugins.'
