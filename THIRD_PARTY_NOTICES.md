# Third-party notices

Skiff's installer bundles the following programs unmodified, as separate
executables or libraries. Their licenses apply to them; Skiff's own code is
covered by the MIT License (see LICENSE).

## FFmpeg

- Build: [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), `win64-lgpl` variant
  (fetched by `scripts/fetch-ffmpeg.ps1`).
- License: GNU Lesser General Public License v2.1 or later.
- Source: https://ffmpeg.org/download.html; build scripts at
  https://github.com/BtbN/FFmpeg-Builds.

## pandoc

- Build: official Windows release from https://github.com/jgm/pandoc/releases
  (fetched by `scripts/fetch-doc-tools.ps1`).
- License: GNU General Public License v2.0 or later.
- Source: https://github.com/jgm/pandoc (the source for the exact bundled
  version is available from its release page).

## PDFium

- Build: [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries),
  `chromium/7881`.
- License: BSD 3-Clause and Apache License 2.0 (see the build's LICENSE file).
- Source: https://pdfium.googlesource.com/pdfium/

## Rust crates

Skiff is compiled with many crates from crates.io, including Tauri (MIT/Apache-2.0),
Typst (Apache-2.0), calamine (MIT), image (MIT/Apache-2.0) and pdfium-render
(MIT/Apache-2.0). Run `cargo about` or `cargo license` in `src-tauri` for the
complete list.

## Fonts

PDFs typeset by Skiff can embed fonts bundled with Typst (typst-assets), which
are under the SIL Open Font License 1.1, or fonts installed on your system.
