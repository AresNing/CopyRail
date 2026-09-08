# MVP verification

Snapshot: **2026-09-08**, source release **v0.1.0-mvp.1**. Platform checked locally: Apple Silicon macOS, Rust 1.98.0, Trunk 0.21.14. This record separates automated checks from native acceptance and distribution readiness.

## Final source checks

- `./scripts/check.sh`: passed. **357 Rust tests passed, 4 intentionally ignored**; formatting, host/WASM Clippy with warnings denied, and compiled frontend passed.
- `cargo check -p pasters-desktop --no-default-features --locked`: passed with the pinned toolchain, checking the MVP configuration without CloudKit. All eight workspace manifests also resolve from the staged source export.
- Script tests: **28 passed**, covering artifact maintenance, isolated QA preparation and native trace parsing.
- Compiled-WASM interface regression: **12 groups passed**, covering CopyRail design, visual styles, content dialogs, pinboards, shortcut fallback, accessibility semantics, focus, card gestures, drag placement, source icons, paste outcomes and window grid. These use synthetic data and mock IPC, not AppKit end-to-end interaction.
- License: root Apache-2.0 text matches the official text (SHA-256 `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30`). All eight workspace packages inherit Apache-2.0. The DejaVu font notice accompanying the PDF fixtures is included.
- Gitleaks 8.30.1 scanned the staged source export with redacted output: no findings. Its executable was checked against the official release checksum. A separate scan found no local user paths or employer identifiers in the publication files; relative documentation links resolved.
- Publication excludes local histories, databases, logs, output packages, build caches, private QA records and machine configuration. Only original brand graphics and synthetic fixtures accompany the code.

Three ignored tests require the macOS pasteboard server and fresh private synthetic pasteboards. The fourth is a helper entry point invoked explicitly by the cross-process test. None of these four is added to the ordinary passed-test count. CI repeats the core source checks on a clean GitHub macOS runner; its result is visible under Actions, independently of these local results.

## Native evidence and remaining acceptance

The previous local beta.5 used this same application implementation. Its AppKit drag-preview render check passed 26 synthetic cases. Native startup from its delivery bundle, a single ordinary instance, history readability, the new card layout, empty-state action and shortcut registration were observed locally.

**Direct-paste permission on that build was pending, and actual cross-application paste was not performed.** Complete capture → search → restart → safe paste, permission recovery, full drag/drop, complex rich text/PDF, multi-display/window recovery, VoiceOver and long-running/performance matrices remain open. Browser results and native render tests do not close those items. Shared/multi-device sync remains out of scope.

This MVP publishes source only. No Developer ID signing, notarization or public application binary is claimed. Internal storage/application identifiers remain unchanged for compatibility. The source publication does not rebuild or modify previous local delivery packages.

## Dependency advisory snapshot

Queried the public [OSV API](https://google.github.io/osv.dev/api/) for **589 crates.io package versions** resolved by Cargo.lock across all platforms and optional features. Deduplicating the glib GHSA/RustSec aliases yields **19 advisories: 18 unmaintained-package notices and 1 unsoundness advisory**. This is a dated known-advisory lookup, not a complete vulnerability or reachability audit.

The glib/GTK dependencies are absent from `cargo tree -p pasters-desktop --no-default-features --target aarch64-apple-darwin --locked` and belong to other-platform paths. Linux is not supported by this MVP. The glib issue is documented in [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html); glib >= 0.20.0 is patched upstream. Do not extend support to Linux without resolving that dependency path.

The `unic-*` maintenance notices apply to dependencies in the macOS graph. `proc-macro-error2` is also present in the UI build chain and produces a future-Rust compatibility warning. The release retains the tested lockfile, pins Rust 1.98.0 and records these upstream maintenance risks; it does not suppress advisories or claim zero findings. Dependency replacement/update remains follow-up work.

| Package/version | Advisory | Finding |
| --- | --- | --- |
| `atk 0.18.2` | [RUSTSEC-2024-0413](https://rustsec.org/advisories/RUSTSEC-2024-0413.html) | Unmaintained |
| `atk-sys 0.18.2` | [RUSTSEC-2024-0416](https://rustsec.org/advisories/RUSTSEC-2024-0416.html) | Unmaintained |
| `gdk 0.18.2` | [RUSTSEC-2024-0412](https://rustsec.org/advisories/RUSTSEC-2024-0412.html) | Unmaintained |
| `gdk-sys 0.18.2` | [RUSTSEC-2024-0418](https://rustsec.org/advisories/RUSTSEC-2024-0418.html) | Unmaintained |
| `gdkwayland-sys 0.18.2` | [RUSTSEC-2024-0411](https://rustsec.org/advisories/RUSTSEC-2024-0411.html) | Unmaintained |
| `gdkx11 0.18.2` | [RUSTSEC-2024-0417](https://rustsec.org/advisories/RUSTSEC-2024-0417.html) | Unmaintained |
| `gdkx11-sys 0.18.2` | [RUSTSEC-2024-0414](https://rustsec.org/advisories/RUSTSEC-2024-0414.html) | Unmaintained |
| `glib 0.18.5` | [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html) | Unsoundness |
| `gtk 0.18.2` | [RUSTSEC-2024-0415](https://rustsec.org/advisories/RUSTSEC-2024-0415.html) | Unmaintained |
| `gtk-sys 0.18.2` | [RUSTSEC-2024-0420](https://rustsec.org/advisories/RUSTSEC-2024-0420.html) | Unmaintained |
| `gtk3-macros 0.18.2` | [RUSTSEC-2024-0419](https://rustsec.org/advisories/RUSTSEC-2024-0419.html) | Unmaintained |
| `paste 1.0.15` | [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html) | Unmaintained |
| `proc-macro-error 1.0.4` | [RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370.html) | Unmaintained |
| `proc-macro-error2 2.0.1` | [RUSTSEC-2026-0173](https://rustsec.org/advisories/RUSTSEC-2026-0173.html) | Unmaintained |
| `unic-char-property 0.9.0` | [RUSTSEC-2025-0081](https://rustsec.org/advisories/RUSTSEC-2025-0081.html) | Unmaintained |
| `unic-char-range 0.9.0` | [RUSTSEC-2025-0075](https://rustsec.org/advisories/RUSTSEC-2025-0075.html) | Unmaintained |
| `unic-common 0.9.0` | [RUSTSEC-2025-0080](https://rustsec.org/advisories/RUSTSEC-2025-0080.html) | Unmaintained |
| `unic-ucd-ident 0.9.0` | [RUSTSEC-2025-0100](https://rustsec.org/advisories/RUSTSEC-2025-0100.html) | Unmaintained |
| `unic-ucd-version 0.9.0` | [RUSTSEC-2025-0098](https://rustsec.org/advisories/RUSTSEC-2025-0098.html) | Unmaintained |

## Publication and rights boundary

Project-owned code/documentation/brand graphics use Apache-2.0; external dependencies and embedded fonts retain their original terms. [Third-party notices](../THIRD_PARTY_NOTICES.md) and the [dependency inventory](dependencies.md) document the source release scope. The project is independently branded and disclaims affiliation with Paste Team ApS. These checks cannot establish ownership of every contribution or provide trademark/legal clearance.
