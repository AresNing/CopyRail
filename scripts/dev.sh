#!/bin/sh

set -eu

project_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

cd "$project_root/apps/desktop"
../../scripts/trunk.sh build --config Trunk.toml

cd "$project_root"
exec cargo run -p pasters-desktop
