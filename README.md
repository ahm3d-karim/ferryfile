<div align="center">

# ⛵ Skiff

**Drag a file. Drop it on the drawer. Done.**

An offline, drag-and-drop file converter for Windows.
Images, audio, video, PDFs, documents and spreadsheets, all on your machine.

[![CI](https://github.com/ahm3d-karim/skiff/actions/workflows/ci.yml/badge.svg)](https://github.com/ahm3d-karim/skiff/actions/workflows/ci.yml)
![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078d4)
![Built with Tauri + Rust](https://img.shields.io/badge/built%20with-Tauri%202%20%2B%20Rust-2dd4bf)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)

</div>

---

## Contents

- [How it works](#how-it-works)
- [What it can convert](#what-it-can-convert)
- [Recipes](#recipes)
- [File actions and the shelf](#file-actions-and-the-shelf)
- [Settings](#settings)
- [Install](#install)
- [Build from source](#build-from-source)
- [Optional integrations](#optional-integrations)
- [Architecture](#architecture)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [Licenses](#licenses)

## How it works

1. **Start dragging** any file in Explorer or on the desktop.
2. A **drawer slides in** from the edge of the screen.
3. **Drop the file** on it, and pick an action: a conversion, a saved recipe,
   or a file action.

The new file is saved next to the original (or in a folder you choose). Skiff
**never overwrites** anything; a clash gets a name like `photo (2).webp`.
Nothing is uploaded; every conversion runs locally.

Skiff lives in the system tray. Right-click the tray icon for the shelf,
settings, and quit.

## What it can convert

| Category | From | To / actions | Powered by |
| --- | --- | --- | --- |
| **Images** | PNG, JPG, WebP, GIF, BMP, TIFF | PNG, JPG, WebP, GIF, BMP, TIFF, PDF · resize 75/50/25% · compress | [image](https://crates.io/crates/image), PDFium |
| **Audio** | MP3, M4A, AAC, WAV, FLAC, OGG, Opus, AIFF, WMA | MP3, M4A, WAV, FLAC, OGG, Opus | FFmpeg |
| **Video** | MP4, M4V, MOV, MKV, WebM, AVI, WMV | MP4, MOV, MKV, WebM, GIF · extract MP3/M4A · resize 1080/720/480p · compress | FFmpeg |
| **PDF** | PDF | PNG/JPG pages · text · Markdown · one PDF per page | PDFium |
| **Documents** | Markdown, TXT, DOCX, ODT, EPUB, HTML, RTF, RST, Org, LaTeX, Typst, Jupyter, PPTX | DOCX, ODT, Markdown, HTML, EPUB, RTF, text, **PDF** | pandoc + built-in Typst |
| **Spreadsheets** | XLSX, XLSM, XLSB, XLS, ODS, CSV, TSV | CSV, Markdown, HTML, **PDF** · one CSV per sheet | [calamine](https://crates.io/crates/calamine) + built-in Typst |
| **Combine** | several PDFs and/or images | one PDF, in drop order | PDFium |

**PDFs from documents and spreadsheets need no Office suite.** Skiff writes
them with an embedded [Typst](https://typst.app) typesetter. If Microsoft
Office or LibreOffice is installed, Skiff uses it instead for Office files, to
keep their exact layout (see [Optional integrations](#optional-integrations)).

## Recipes

A recipe is a saved chain of actions that runs in one click, for example
*Resize 50% → Convert WebP*. Each step's output feeds the next, and only the
final file is kept.

- **Built in:** Web-ready image, Email-friendly photo, Share-size clip,
  Clip to GIF, Podcast MP3.
- **Your own:** click **+ New** in the drawer, name it, and add steps. The
  editor only offers steps that make sense for the format so far.
- The drawer only shows recipes that fit the file you dropped. Use the search
  box to filter recipes and actions as you type.

## File actions and the shelf

Every drop, including folders, also gets **File** actions:

| Action | What it does |
| --- | --- |
| Copy path | Copies the full path(s) as text |
| Copy | Puts the files on the clipboard, ready to paste in Explorer |
| Show in Explorer | Opens the folder with the item selected |
| Terminal here | Opens Windows Terminal (or PowerShell) in that folder |
| Move to… / Copy to… | Sends the items to a folder you pick |
| Add to shelf | Keeps the items on the shelf |

The **shelf** is a small floating window for files you'll need in a moment.
Drop files on it, then drag them out into any app; dropped items leave the
shelf. Open it from the **Shelf** button in the drawer or the tray menu. Its
contents survive restarts.

## Settings

Open **Settings…** from the tray menu or the ⚙ button in the drawer.

| Setting | Options |
| --- | --- |
| Start with Windows | On / off |
| Drawer edge | Right / left |
| Open the drawer | As soon as you drag, or after a short pause (fewer accidental opens) |
| Save converted files | Next to the original, or in a folder you choose |

## Install

Releases are published on the
[Releases page](https://github.com/ahm3d-karim/skiff/releases) as a per-user
installer (`Skiff_x.y.z_x64-setup.exe`); no administrator rights needed.

> Until releases are code-signed, Windows SmartScreen may warn about an
> unrecognized app. Choose **More info → Run anyway**.

## Build from source

**Requirements**

- Windows 10 or 11
- [Rust](https://rustup.rs) (stable, MSVC toolchain)
- Visual Studio Build Tools with the **Desktop development with C++** workload
- Node.js 20 or newer

**Steps**

```powershell
git clone https://github.com/ahm3d-karim/skiff.git
cd skiff
npm install

# Bundled tools are git-ignored; fetch them once:
./scripts/fetch-ffmpeg.ps1       # FFmpeg (LGPL build)
./scripts/fetch-doc-tools.ps1    # pandoc + PDFium

npx tauri dev      # run with hot reload
npx tauri build    # installer -> src-tauri/target/release/bundle/nsis/
```

**Tests**

```powershell
cd src-tauri
cargo test --lib                 # unit tests
cargo test --lib -- --ignored    # real conversions with FFmpeg, pandoc, PDFium
```

**Code signing**

`npx tauri build` passes every binary to [`scripts/sign.ps1`](scripts/sign.ps1).
It signs when one of these is set, and otherwise leaves the build unsigned:

| Variable | Meaning |
| --- | --- |
| `SKIFF_SIGN_THUMBPRINT` | SHA-1 thumbprint of a code-signing certificate in the Windows certificate store |
| `SKIFF_SIGN_PFX` + `SKIFF_SIGN_PFX_PASSWORD` | Base64-encoded `.pfx` file and its password |

The [release workflow](.github/workflows/release.yml) reads the PFX pair from
repository secrets of the same names. Push a tag like `v0.2.0` to build a
draft release.

## Optional integrations

Skiff detects these at startup and adds their actions only when they're
installed. Nothing is bundled or required.

| Tool | Adds | Install |
| --- | --- | --- |
| Microsoft Office | Highest-fidelity PDF from Word, Excel and PowerPoint files | Office desktop apps |
| LibreOffice | PDF and cross-format Office conversions | [libreoffice.org](https://www.libreoffice.org) |
| [Docling](https://github.com/docling-project/docling) | Layout-aware Markdown from PDFs, Office files and scans (tables, OCR) | `pip install docling` |

When several can make a PDF, Skiff picks in this order: **Microsoft Office →
LibreOffice → built-in Typst**.

## Architecture

```
skiff/
├─ index.html, shelf.html, settings.html   UI pages (drawer, shelf, settings)
├─ src/                                     page scripts and styles
├─ scripts/                                 tool fetchers, signing hook
└─ src-tauri/src/
   ├─ drag.rs          mouse hook: notices drags that start in Explorer
   ├─ dropbridge.rs    receives drops through WebView2 (see below)
   ├─ engine.rs        action registry, recipe chains, output naming
   ├─ engine/          one module per engine (image, ffmpeg, pdf, doc, builtin, …)
   ├─ recipes.rs       built-in and saved recipes
   ├─ files.rs         file actions
   ├─ shelf.rs         the shelf
   └─ settings.rs      settings and start-with-Windows
```

- **Detecting drags.** A low-level mouse hook watches for a press-and-move
  that starts in an Explorer or desktop window, then shows the drawer without
  stealing focus.
- **Receiving drops.** Tauri's built-in drop handler doesn't work with current
  WebView2 runtimes, which host the input window in a separate process. Skiff
  lets WebView2 accept the drop natively; the page passes the dropped `File`
  objects to Rust via `postMessageWithAdditionalObjects`, and Rust reads their
  real paths.
- **Engines.** Each engine declares which actions it offers for a file type and
  what format each produces. That lets the drawer show only valid recipes and
  lets the editor offer only valid next steps, without touching the disk.

## Troubleshooting

| Problem | Fix |
| --- | --- |
| The drawer doesn't appear | Make sure Skiff is running (tray icon). Drags from apps running as administrator can't be seen by a normal-privilege app. |
| "ffmpeg/pandoc/pdfium wasn't found" | In a source build, run the fetch scripts in `scripts/`. |
| A PDF from Word doesn't match the original layout | The built-in typesetter recreates the content, not Word's exact layout. Install Microsoft Office or LibreOffice for layout-exact PDFs. |
| Converted files go somewhere unexpected | Check **Settings → Save converted files**. |

## Contributing

`main` is protected: changes go through pull requests, and CI runs every
test, including real conversions, on each PR. Keep commits focused, and add a
test for new engine behaviour (opt-in with `#[ignore]` if it needs the
bundled tools).

## Licenses

Skiff is released under the [MIT License](LICENSE).

It bundles FFmpeg (LGPL-2.1+), pandoc (GPL-2.0+) and PDFium (BSD-3-Clause /
Apache-2.0) as separate, unmodified programs; their own licenses apply to them.
See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for details and sources.
