#!/bin/sh
# Private Compact fixture only; the shared wrapper enforces isolation.
set -eu
native_compact_binary_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
exec "$native_compact_binary_dir/native-qa-launcher" --compact "$@"
