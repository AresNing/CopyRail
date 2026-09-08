#!/bin/sh
# Only installed into a separate QA bundle. Never launch normal history mode.
set -eu
native_qa_binary_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
umask 077
# A new private log per launch, retained outside the disposable data profile.
# This wrapper only runs synthetic isolation, never normal clipboard history.
native_qa_session_log="$(mktemp "$native_qa_binary_dir/../../../native-qa-session.log.XXXXXX")"
exec "$native_qa_binary_dir/pasters-desktop" --native-ui-test "$@" >> "$native_qa_session_log" 2>&1
