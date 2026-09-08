#!/bin/sh
# Private PDF + Compact fixture; no user document or normal-profile path.
set -eu
native_pdf_compact_binary_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
exec "$native_pdf_compact_binary_dir/native-qa-launcher" --scenario=pdf --compact "$@"
