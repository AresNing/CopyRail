#!/bin/sh

set -eu

project_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$project_root"

cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy -p pasters-desktop-ui --target wasm32-unknown-unknown --locked -- -D warnings

cd "$project_root/apps/desktop"
../../scripts/trunk.sh build --config Trunk.toml
