# Security and privacy

## Supported scope

The current release is a **source MVP for macOS on Apple Silicon**. It has not completed independent security review or the full native acceptance matrix. Windows, Linux, mobile and production CloudKit/shared sync are outside the supported MVP scope.

The dependency advisory snapshot and current limitations are recorded in [docs/verification.md](docs/verification.md). A passing test suite does not mean there are no vulnerabilities.

## Reporting

Use this repository's **Security → Report a vulnerability** for a private report. Do not include real clipboard content, credentials, tokens or personal databases in public issues. Include the affected revision, macOS version and a minimal synthetic reproduction.

## Local data and permissions

- Normal application startup begins observing subsequent clipboard changes. Hiding the panel does not stop collection; pause capture or quit to stop it.
- History, original content representations, search/OCR text and settings are stored locally under `~/Library/Application Support/io.pasters.desktop/`. History and exported backups are **not encrypted by CopyRail**. Protect them with your account and device security controls.
- Concealed/transient pasteboard markers and app exclusions reduce collection; they cannot identify every secret copied by every application.
- Direct paste requires macOS Accessibility permission and is distinct from copying an item. Only grant permission to a build you trust. The application does not silently grant permission or retry a previous paste after authorization.
- Build the MVP with `--no-default-features` to exclude the experimental CloudKit adapter. Full sync and shared boards are not supported for this release.
- MCP access is off by default and uses local stdio with per-client authorization. Enabling a client grants it access within the configured tool scope; treat its token as a secret.
- Synthetic browser tests use a fresh temporary profile and mock IPC. Native debug QA has a restricted command surface; native embedded viewers and system menus still require separate validation.

Source publication does not include user histories, backups, local logs, signing credentials or private QA records.
