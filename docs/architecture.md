# CopyRail architecture

CopyRail is a Rust workspace with a Leptos CSR/WASM interface and a Tauri 2 desktop shell. The supported MVP target is Apple Silicon macOS. Node and Chrome are development/test tools, not application runtimes.

## Boundaries

| Component | Responsibility |
| --- | --- |
| `paste-domain` | Content types, identifiers, policies and invariants |
| `paste-storage` | SQLite WAL, FTS5, migrations, content-addressed blobs, backup and restore |
| `paste-core` | Use-case orchestration, capture policy, events and undo |
| `paste-platform` | NSPasteboard, source identity, Accessibility permission, window integration and OCR |
| `paste-sync` | Experimental operation envelopes, clocks and conflict model |
| `paste-cloudkit` | Experimental native CloudKit adapter |
| `apps/desktop` | Leptos interface, layout, selection and interaction |
| `apps/desktop/src-tauri` | IPC, native preview/edit/drag, application lifecycle and local stdio MCP |

The domain does not depend on Tauri, AppKit or SQLite. Platform-specific code uses objc2 bindings and explicit native adapters. Mobile clients are not implemented in this MVP.

## Capture and storage

Normal startup observes later NSPasteboard changes. Capture evaluates application exclusions and concealed/transient markers before importing data. Original representations retain native types and bytes; BLAKE3 addresses blob content. Search/preview normalization does not overwrite the source representation.

SQLite transactions keep records, pinboard membership, search documents and related operation queues consistent. The current schema is v19. FTS5 provides text search, with structured filters and newest-first ordering; pinboards preserve explicit item order. History retention protects pinned items. Backups use SQLite's online backup API, and restore validates application identity, schema and integrity before replacement.

Stored history and backups are not encrypted by the application. See [SECURITY.md](../SECURITY.md) for the privacy boundaries.

## Presentation and native operations

The interface uses a neutral light/dark palette, two-row navigation in normal layout and a compact fallback for shorter windows. Cards keep colored type chips and foreground content; source and metadata appear in their footer. The brand SVG is project-created.

Image decoding is bounded; native PDF rendering produces thumbnails and drag previews. Rich-text editing uses AppKit text controls with guarded import and concurrent-change checks. Drag export preserves source files or prepares temporary representations; the app tracks its own sessions before accepting a drop back onto a pinboard.

Direct paste validates permission, the intended target application, focus and clipboard changes. Failure can leave a successful copy without a successful paste, and the interface reports the distinction. Registering a shortcut or compiling the bridge does not establish full cross-application reliability.

## Experimental capabilities

Build the MVP with `--no-default-features` to exclude the optional CloudKit adapter. Sync/shared-board models, queues and conflict handling remain in the source tree, but production account setup, multi-device convergence and shared transmission are outside release acceptance. Do not treat the presence of this code or a setting as a supported cloud service.

The local stdio MCP server is off by default. Client authorization is checked during initialization and tool calls. It is not a public network service.

## Validation layers

Rust unit/integration tests cover policy, state, persistence, migration and native helper behavior. Compiled-WASM browser tests use synthetic fixtures and mock IPC. Native debug QA uses independent temporary data and restricted commands. Each layer has a different scope; none substitutes for complete permission, cross-application, multi-display or accessibility acceptance. Current evidence is summarized in [verification.md](verification.md).
