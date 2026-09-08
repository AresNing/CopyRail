#!/bin/sh
# Prepare, but do not launch, the existing debug build with a fail-closed
# desktop entry point. The source app and its executable remain unchanged.
set -eu
native_qa_name="CopyRail Native QA"
native_qa_identifier="io.pasters.nativeqa"
native_qa_entry="native-qa-launcher"
case "$#:$*" in
  0:) ;;
  1:--pdf)
    native_qa_name="CopyRail PDF QA"
    native_qa_identifier="io.pasters.nativeqa.pdf"
    native_qa_entry="native-pdf-qa-launcher"
    ;;
  1:--compact)
    native_qa_name="CopyRail Compact QA"
    native_qa_identifier="io.pasters.nativeqa.compact"
    native_qa_entry="native-compact-qa-launcher"
    ;;
  '2:--pdf --compact')
    native_qa_name="CopyRail PDF Compact QA"
    native_qa_identifier="io.pasters.nativeqa.pdfcompact"
    native_qa_entry="native-pdf-compact-qa-launcher"
    ;;
  *) printf '%s\n' 'Usage: prepare-native-qa.sh [--pdf] [--compact]' >&2; exit 2 ;;
esac
native_qa_project="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
native_qa_source="$native_qa_project/target/debug/bundle/macos/CopyRail.app"
test -x "$native_qa_source/Contents/MacOS/pasters-desktop"
native_qa_root="$(mktemp -d /private/tmp/pasters-native-qa-XXXXXX)"
native_qa_app="$native_qa_root/$native_qa_name.app"
cp -R "$native_qa_source" "$native_qa_app"
cp "$native_qa_project/scripts/native-qa-launcher.sh" "$native_qa_app/Contents/MacOS/native-qa-launcher"
chmod 700 "$native_qa_app/Contents/MacOS/native-qa-launcher"
if [ "$native_qa_entry" != 'native-qa-launcher' ]; then
  cp "$native_qa_project/scripts/$native_qa_entry.sh" "$native_qa_app/Contents/MacOS/$native_qa_entry"
  chmod 700 "$native_qa_app/Contents/MacOS/$native_qa_entry"
fi
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable $native_qa_entry" "$native_qa_app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $native_qa_identifier" "$native_qa_app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName $native_qa_name" "$native_qa_app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $native_qa_name" "$native_qa_app/Contents/Info.plist"
cmp "$native_qa_source/Contents/MacOS/pasters-desktop" "$native_qa_app/Contents/MacOS/pasters-desktop"
plutil -lint "$native_qa_app/Contents/Info.plist"
printf '%s\n' "$native_qa_app"
