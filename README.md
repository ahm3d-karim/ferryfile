<div align="center">

# ⛵ Ferryfile

**Drag a file. Drop it on the drawer. Done.**

An offline, drag-and-drop file converter for Windows.
Images, audio, video, PDFs, documents and spreadsheets, all on your machine.

[![CI](https://github.com/ahm3d-karim/ferryfile/actions/workflows/ci.yml/badge.svg)](https://github.com/ahm3d-karim/ferryfile/actions/workflows/ci.yml)
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
- [Code signing policy](#code-signing-policy)
- [Licenses](#licenses)

## How it works

1. **Start dragging** any file in Explorer or on the desktop.
2. A **drawer slides in** from the edge of the screen.
3. **Drop the file** on it, and pick an action: a conversion, a saved recipe,
   or a file action.

The new file is saved next to the original (or in a folder you choose). Ferryfile
**never overwrites** anything; a clash gets a name like `photo (2).webp`.
Nothing is uploaded; every conversion runs locally.

Ferryfile lives in the system tray. Right-click the tray icon for the shelf,
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

**PDFs from documents and spreadsheets need no Office suite.** Ferryfile writes
them with an embedded [Typst](https://typst.app) typesetter. If Microsoft
Office or LibreOffice is installed, Ferryfile uses it instead for Office files, to
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
| Add to shelf | Puts the items on your most recent shelf |

**Shelves** are small floating windows for files you'll need in a moment:

- **Shake to open:** while dragging a file, shake the cursor side to side and a
  new shelf appears right under it. Drop the file in. (Open more from the tray,
  or the **Shelves** button in the drawer.) While a shelf is open, the drawer
  stays out of the way.
- **Drag out** one item or **⇲ Drag all** into any app; items you drag out
  leave the shelf, and an unpinned shelf closes once it's empty. 📌 pins a
  shelf to keep it.
- **Actions** run on everything on the shelf: conversions, recipes, Combine,
  Zip, Copy, Move to… or Copy to…. Results land back on the same shelf, ready
  to drag out.
- Shelves with items survive restarts.

## Settings

Open **Settings…** from the tray menu or the ⚙ button in the drawer.

| Setting | Options |
| --- | --- |
| Start with Windows | On / off |
| Drawer edge | Right / left |
| Open the drawer | As soon as you drag, or after a short pause (fewer accidental opens) |
| Shelves | Shake while dragging to open a shelf on/off, and how firm a shake it takes |
| Save converted files | Next to the original, or in a folder you choose |

## Install

Pick whichever you prefer; all of them install the same release.

| How | Command / link |
| --- | --- |
| **Installer** | Download `Ferryfile_x.y.z_x64-setup.exe` from [Releases](https://github.com/ahm3d-karim/ferryfile/releases). Per-user, no admin rights. |
| **winget** | `winget install ferryfile` |
| **Scoop** | `scoop bucket add ferryfile https://github.com/ahm3d-karim/scoop-ferryfile` then `scoop install ferryfile/ferryfile` |
| **npm** | `npx ferryfile` (or `npm i -g ferryfile`, then `ferryfile`) |
| **Portable** | Unzip `Ferryfile_x.y.z_x64-portable.zip` from [Releases](https://github.com/ahm3d-karim/ferryfile/releases) anywhere and run `ferryfile.exe`. |

> Until releases are code-signed, Windows SmartScreen may warn when you run a
> downloaded installer. Choose **More info → Run anyway**. winget, Scoop and
> npm installs don't show that prompt.

Maintainers: how each channel is published is in
[packaging/README.md](packaging/README.md).

## Build from source

**Requirements**

- Windows 10 or 11
- [Rust](https://rustup.rs) (stable, MSVC toolchain)
- Visual Studio Build Tools with the **Desktop development with C++** workload
- Node.js 20 or newer

**Steps**

```powershell
git clone https://github.com/ahm3d-karim/ferryfile.git
cd ferryfile
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

**Releases**

Bump `version` in `src-tauri/tauri.conf.json`, merge, then push a matching tag
(`v0.2.0`). The [release workflow](.github/workflows/release.yml) builds the
installer, sends it to SignPath for signing once that's configured (see
[Code signing policy](#code-signing-policy)), and drafts a GitHub release.

**Signing local builds (optional)**

`npx tauri build` passes every binary to [`scripts/sign.ps1`](scripts/sign.ps1),
which signs with your own certificate when one of these is set and otherwise
leaves the build unsigned:

| Variable | Meaning |
| --- | --- |
| `FERRYFILE_SIGN_THUMBPRINT` | SHA-1 thumbprint of a code-signing certificate in the Windows certificate store |
| `FERRYFILE_SIGN_PFX` + `FERRYFILE_SIGN_PFX_PASSWORD` | Base64-encoded `.pfx` file and its password |

## Optional integrations

Ferryfile detects these at startup and adds their actions only when they're
installed. Nothing is bundled or required.

| Tool | Adds | Install |
| --- | --- | --- |
| Microsoft Office | Highest-fidelity PDF from Word, Excel and PowerPoint files | Office desktop apps |
| LibreOffice | PDF and cross-format Office conversions | [libreoffice.org](https://www.libreoffice.org) |
| [Docling](https://github.com/docling-project/docling) | Layout-aware Markdown from PDFs, Office files and scans (tables, OCR) | `pip install docling` |

When several can make a PDF, Ferryfile picks in this order: **Microsoft Office →
LibreOffice → built-in Typst**.

## Architecture

```
ferryfile/
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
  WebView2 runtimes, which host the input window in a separate process. Ferryfile
  lets WebView2 accept the drop natively; the page passes the dropped `File`
  objects to Rust via `postMessageWithAdditionalObjects`, and Rust reads their
  real paths.
- **Engines.** Each engine declares which actions it offers for a file type and
  what format each produces. That lets the drawer show only valid recipes and
  lets the editor offer only valid next steps, without touching the disk.

## Troubleshooting

| Problem | Fix |
| --- | --- |
| The drawer doesn't appear | Make sure Ferryfile is running (tray icon). Drags from apps running as administrator can't be seen by a normal-privilege app. |
| "ffmpeg/pandoc/pdfium wasn't found" | In a source build, run the fetch scripts in `scripts/`. |
| A PDF from Word doesn't match the original layout | The built-in typesetter recreates the content, not Word's exact layout. Install Microsoft Office or LibreOffice for layout-exact PDFs. |
| Converted files go somewhere unexpected | Check **Settings → Save converted files**. |

## Contributing

`main` is protected: changes go through pull requests, and CI runs every
test, including real conversions, on each PR. Keep commits focused, and add a
test for new engine behaviour (opt-in with `#[ignore]` if it needs the
bundled tools).

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/),
certificate by [SignPath Foundation](https://signpath.org/).

Release installers are built from this repository by GitHub Actions and signed
only after a maintainer approves each request. Team roles, what is signed, and
Ferryfile's privacy policy are in [CODE_SIGNING.md](CODE_SIGNING.md). In short:
Ferryfile does not transfer any information to other networked systems unless you
ask it to.

## Licenses

Ferryfile is released under the [MIT License](LICENSE).

It bundles FFmpeg (LGPL-2.1+), pandoc (GPL-2.0+) and PDFium (BSD-3-Clause /
Apache-2.0) as separate, unmodified programs; their own licenses apply to them.
See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for details and sources.
