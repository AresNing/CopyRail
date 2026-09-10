# Third-party notices

CopyRail's original code, documentation and original brand graphics are licensed under Apache-2.0. Third-party works retain their own terms; the root LICENSE does not replace them.

## Embedded font in synthetic PDF fixtures

`apps/desktop/src-tauri/fixtures/native-preview-acceptance.pdf` and `native-preview-locked.pdf` are generated synthetic documents containing an embedded subset of **DejaVu Sans**. The font is based on Bitstream Vera; DejaVu changes are in the public domain. The full upstream notice, including the Bitstream Vera and Arev terms, is distributed in [licenses/DejaVu-Fonts.txt](licenses/DejaVu-Fonts.txt).

- Upstream: https://dejavu-fonts.github.io/
- License source: https://github.com/dejavu-fonts/dejavu-fonts/blob/master/LICENSE
- Fixture generator: [scripts/create-native-pdf-fixture.py](scripts/create-native-pdf-fixture.py)

The PDFs contain synthetic test content and a documented synthetic test password. ReportLab and pypdf are optional generator tools, not bundled application code.

## Rust dependencies

[docs/dependencies.md](docs/dependencies.md) inventories the registry package versions and declared SPDX license expressions resolved by Cargo.lock. This includes optional, build, test and other-platform dependencies; it is not a list of everything linked into the macOS application. Registry dependencies are fetched from crates.io; the macOS panel adapter is fetched from the pinned Git revision below. Dependencies are not vendored into this repository; MPL source archives accompany the binary release. Their original license and notice files remain applicable.

Five packages declare MPL-2.0: `cssparser`, `cssparser-macros`, `dtoa-short`, `option-ext` and `selectors`. A distributor of a compiled application must satisfy the applicable dependency terms, including MPL-covered source availability. For version 1.0.0, the DMG and ZIP include upstream license/notice files and an exact-version dependency inventory under `licenses/dependencies/` and `licenses/dependency-inventory.json`. The release also includes `CopyRail_1.0.0_third-party-sources.zip`, containing the checksum-verified, unmodified upstream source archives for these five MPL packages. Each `.crate` file is a gzip-compressed tar archive with its original license notices. No local modifications are made to those sources.

## macOS panel adapter

`tauri-nspanel` 2.1.0 is pinned to upstream commit [c9ec2130422200f0863b23dfdad02b133a529b07](https://github.com/ahkohd/tauri-nspanel/tree/c9ec2130422200f0863b23dfdad02b133a529b07). Its manifest does not declare an SPDX expression; the repository supplies MIT and Apache-2.0 license files. CopyRail uses it under the MIT license and includes the original [MIT notice](licenses/tauri-nspanel-MIT.txt), including Victor Aremu's copyright. No floating branch is used.

## Graphics and platform resources

CopyRail's brand SVG and derived app/tray icons are project-created graphics. Source-application icons are read from installed applications at runtime and belong to their respective owners; this release does not redistribute a collection of those application icons. The visual test landscape is synthetic.

CopyRail is independent and is not affiliated with, sponsored by, or endorsed by Paste Team ApS. References to other products do not grant rights to their trademarks, code or assets.
