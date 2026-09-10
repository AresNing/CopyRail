# CopyRail

**English** · [简体中文](README.zh-CN.md)

**A local-first clipboard workspace for macOS.**

**[Download CopyRail 1.0.0 for Apple Silicon](https://github.com/AresNing/CopyRail/releases/download/v1.0.0/CopyRail_1.0.0_aarch64.dmg)** · [All release files](https://github.com/AresNing/CopyRail/releases/tag/v1.0.0) · [Installation guide](docs/installation.md)

macOS 13+. The download is locally signed, without Apple Developer ID signing or notarization; see the installation guide for first-launch instructions.

CopyRail keeps the things you copy in a searchable card rail. Find a snippet, preview an image, organize reusable content into pinboards, and paste it back into your work. Built with Rust, Tauri and Leptos, it stores history locally in SQLite.

![CopyRail clipboard rail with sample notes, a link, an illustration, a color and code](docs/images/clipboard-dark.png)

*English interface at **50% background transparency**, with synthetic demo content and a synthetic backdrop. [Screenshot details](docs/images/README.md).*

## What you can do

- **Find copied content.** Search your history and filter by content type, source app, device or date.
- **Keep reusable snippets organized.** Create pinboards, rename entries, edit text and reorder saved content.
- **Preview without losing your place.** Open a compact reader above the card rail and move between items with the arrow keys.
- **Reuse more than text.** Work with plain text, HTML, links, images, files, PDFs and colors.
- **Paste one item at a time.** Add cards to a paste queue for filling several fields in a chosen order. Plain-text output is available too.
- **Choose your language.** By default, CopyRail follows your system language: Chinese uses Simplified Chinese; other languages use English. Choose System default, English or Simplified Chinese in Settings → General → Language. Manual choices are saved immediately and preserved on launch.
- **Adjust the glass background.** Settings → General → Background transparency ranges from an opaque surface to clear macOS frosted glass. The default is 50%; the screenshots demonstrate 50%. Changes save automatically across the rail, settings and preview, while text and controls stay solid. System reduced-transparency preferences take priority.
- **Choose where the rail opens.** Settings → General → Selection on open offers Latest item (default) or Last position. Last position remembers the selected card and browsing context across restarts; new captures do not change the remembered card. Missing items return to the latest history. The setting saves automatically and applies on the next invocation.
- **Choose what stays.** Pause capture, exclude apps, set retention limits, and export or restore a local backup.
- **Work from the keyboard.** Open the rail with `⇧⌘V`, navigate with the arrow keys, preview with `Space`, and copy with `⌘C`.

### Preview above the rail

Read a longer snippet while keeping your clipboard history visible below it.

![Compact text preview above the CopyRail clipboard rail](docs/images/preview-dark.png)

### Settings with clear sections

General, Shortcuts, History & privacy, Backup and Advanced keep everyday controls together. Capture controls and keyboard instructions live in Settings. Change the interface language in **General → Language**; it takes effect immediately. Settings save automatically: switches apply when changed, and number fields and app lists save after a brief typing pause. Invalid values keep the previous saved setting and show an error. System default reads the macOS preferred language at launch; change back to it at any time to remove a manual override. Clipboard content and pinboard names stay as you wrote them.

![CopyRail General settings with the English language option](docs/images/language-dark.png)

![CopyRail History and privacy settings with synthetic defaults](docs/images/settings-dark.png)

## A quick workflow

1. Copy something in any app. CopyRail saves new supported clipboard content while it is running.
2. Press `⇧⌘V` to open the rail, then search or select a card. The search icon opens filters; the input has no placeholder text.
3. Press `Space` to preview, or `⌘C` to copy the selected content. Paste it into your destination app as usual.
4. For direct paste, first focus the destination field, open CopyRail with `⇧⌘V`, then press `Return`. macOS Accessibility permission is required.

For a sequence, click **+** on cards in the order you need them. Focus the first destination field, open CopyRail and press `Return`. Repeat for the next field. The queue count decreases after a successful paste request; copy-only results and failed requests leave the item queued. Clicking **Paste queue** clears the queue without deleting history.

Hiding the rail leaves capture running. Pause capture in Settings or the menu bar, or quit CopyRail to stop it.

## Status and platform

CopyRail **1.0.0** is the first downloadable release for Apple Silicon macOS. DMG and ZIP contain the same optimized application; SHA-256 checksums and a release manifest are included. The application retains the existing local data format and identity.

The implemented features above are not a claim of complete daily-use validation. Full cross-app paste and drag/drop coverage, complex rich-text fidelity, full-screen Spaces, multiple displays and VoiceOver still have open acceptance items. Shared pinboards, multi-device sync and mobile clients are outside version 1.0. See the [verification record](docs/verification.md) for the tested scope.

## Run from source

Requires macOS, Xcode Command Line Tools, [Rustup](https://rustup.rs/) and Trunk. The repository pins Rust **1.98.0**; the validated Trunk version is **0.21.14**. The initial build downloads public dependencies.

```sh
git clone https://github.com/AresNing/CopyRail.git
cd CopyRail
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
cd apps/desktop
../../scripts/trunk.sh build --config Trunk.toml
cd ../..
cargo run -p pasters-desktop --no-default-features --locked
```

Keep `--no-default-features` for version 1.0: it excludes the experimental CloudKit transport. The legacy `scripts/dev.sh` enables default features and is not the recommended entry point.

## Local data and privacy

- History is stored at `~/Library/Application Support/io.pasters.desktop/history.db`. The internal app identifier remains unchanged for compatibility with earlier PasteRS data.
- History and backups are **not encrypted by the app**. Quit the app and back up your data before an upgrade; restoring a backup replaces existing data.
- Confidential and transient clipboard types are skipped by default. Excluded-app rules and pause controls let you limit capture further.
- A local stdio MCP interface is available for explicitly authorized clients and is disabled by default. It does not expose a network listener.

Read [SECURITY.md](SECURITY.md) for the security model and reporting process. Copying content and confirming that another app accepted a direct paste are separate outcomes.

## Development and verification

```sh
./scripts/check.sh
node --test scripts/maintain-artifacts.test.mjs scripts/check-local-qa-preparation.test.mjs scripts/check-native-tab-trace.test.mjs
```

The checks cover formatting, Rust tests, strict host/WASM Clippy and the frontend build. Script tests require Node.js 22 or newer. Compiled-UI checks use an isolated headless Chrome profile and synthetic IPC; they require Google Chrome in its standard macOS location.

```sh
node scripts/check-copyrail-design-ui.mjs
node scripts/check-workspace-interactions-ui.mjs
node scripts/check-toolbar-settings-ui.mjs
node scripts/check-language-ui.mjs
node scripts/capture-readme-ui.mjs
```

The screenshot command renders documentation fixtures without reading real clipboard history or changing compiled application assets. See [how the screenshots are made](docs/images/README.md).

For native testing with a temporary synthetic database, after building the frontend:

```sh
cargo run -p pasters-desktop --no-default-features --locked -- --native-ui-test
```

This debug-only mode refuses real capture, clipboard writes, cloud and account operations. Browser tests do not replace native acceptance. The embedded PDF viewer has its own [fixture and validation notes](apps/desktop/src-tauri/fixtures/README.md).

Local delivery maintenance keeps three versions in total, including the current version. Cleanup previews changes before removing old delivery groups or rebuildable outputs; it preserves source, fixtures, user data and verification evidence.

## Project layout

| Path | Purpose |
| --- | --- |
| `apps/desktop` | Leptos/WASM interface and styles |
| `apps/desktop/src-tauri` | Desktop shell, native windows, preview, editing and MCP |
| `crates/paste-domain`, `paste-core`, `paste-storage` | Domain types, use cases, SQLite history and search |
| `crates/paste-platform` | macOS clipboard, permissions and window integration |
| `crates/paste-sync`, `paste-cloudkit` | Experimental sync components, outside version 1.0 |
| `scripts` | Build helpers, synthetic UI checks and artifact maintenance |

[Architecture](docs/architecture.md) · [Verification](docs/verification.md) · [Dependencies](docs/dependencies.md)

## License and independence

Copyright 2026 CopyRail contributors. Project-owned code, documentation and original graphics are licensed under **[Apache-2.0](LICENSE)**, unless otherwise stated. See [NOTICE](NOTICE) for attribution and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for third-party terms.

Early research explored existing clipboard tools, including Paste. CopyRail now uses its own card-and-rail identity and design baseline. It is an independent project and is not affiliated with or endorsed by Paste Team ApS. This repository's license does not grant rights to third-party names, trademarks or assets.

## Build the downloadable release

On an Apple Silicon Mac with Cargo Tauri and Python 3.11+ installed, run `./scripts/check.sh`, then `./scripts/build-release.sh`. Check the optimized frontend, then run `python3 scripts/package-release.py`. The packager validates the version, architecture and build flags, signs locally, and creates DMG, ZIP and checksums under `output/CopyRail-1.0.0/downloads/`. It does not install or publish. Public builds disable experimental CloudKit and do not request iCloud entitlements.
