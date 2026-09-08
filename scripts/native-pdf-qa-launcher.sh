#!/bin/sh
# Separate synthetic scenario; the common wrapper still enforces isolation.
set -eu
native_pdf_binary_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
exec "$native_pdf_binary_dir/native-qa-launcher" --scenario=pdf "$@"
