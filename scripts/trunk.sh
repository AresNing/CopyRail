#!/bin/sh

set -eu

if command -v rustup >/dev/null 2>&1; then
  rustup_cargo="$(rustup which cargo)"
  rustup_bin="${rustup_cargo%/cargo}"
  PATH="${rustup_bin}:${PATH}"
  rustup_lib="${rustup_bin%/bin}/lib"
  DYLD_FALLBACK_LIBRARY_PATH="${rustup_lib}:${DYLD_FALLBACK_LIBRARY_PATH:-/usr/local/lib:/usr/lib}"
  export PATH
  export DYLD_FALLBACK_LIBRARY_PATH
fi

if [ "${NO_COLOR:-}" = "1" ]; then
  NO_COLOR=true
  export NO_COLOR
fi

exec trunk "$@"
