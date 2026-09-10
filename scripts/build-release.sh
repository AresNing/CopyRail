#!/bin/sh
# Build the public Apple Silicon application. Packaging is separate so the
# final compiled UI can be checked before any assets are published.
set -eu
project_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || {
  echo 'The public build currently requires an Apple Silicon Mac.' >&2; exit 1;
}
export MACOSX_DEPLOYMENT_TARGET=13.0
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$project_root=/copyrail --remap-path-prefix=$HOME/.cargo=/cargo"
cd "$project_root/apps/desktop"
cargo tauri build --no-sign --bundles app --config src-tauri/tauri.release.conf.json -- --no-default-features --locked
