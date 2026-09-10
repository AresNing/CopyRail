# README screenshots

These images are rendered from CopyRail's compiled Leptos interface using the repository's isolated browser fixture. All cards, board names and source labels are synthetic. The mountain illustration comes from the existing canvas fixture; no personal images, clipboard history, native desktop screenshots or third-party artwork are included.

The capture script selects **English using the production Settings → General → Language control**. It does not translate DOM nodes or replace interface labels. The same saved language preference is available in the shipping desktop app. Layout, styles and interactions come from the compiled UI. Capture does not edit generated application assets, write to the system clipboard, register shortcuts or launch a collecting desktop instance. The Demo badge reflects this isolation.

After building the frontend, with Node.js 22+ and Google Chrome in its standard macOS location:

```sh
node scripts/capture-readme-ui.mjs
```

The script checks that visible app copy is English, screenshots the rail, compact preview, language settings and history settings, and verifies that compiled asset hashes are unchanged. Its report is written to `target/ui-verification/readme-screenshots-report.json`. Browser screenshots do not establish native paste, permissions, window behavior or VoiceOver acceptance.

Native frosted-glass compositing is checked in the macOS app separately. These browser captures show the interface and synthetic content, not a captured desktop backdrop.
