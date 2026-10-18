# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/),
certificate by [SignPath Foundation](https://signpath.org/).

## What is signed

Only Skiff's own release installer (`Skiff_<version>_x64-setup.exe`) is
signed. It is built from this repository's source by GitHub Actions
([`.github/workflows/release.yml`](.github/workflows/release.yml)) when a
version tag (`v*`) is pushed, then submitted to SignPath for signing. Nothing
built on a personal machine is signed.

The installer bundles unmodified third-party programs (FFmpeg, pandoc, PDFium;
see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)). Those binaries are
**not** signed under this policy; they are included as published by their
upstream projects.

## Team roles

| Role | Members |
| --- | --- |
| Committers and reviewers | [Ahmad Karim (@ahm3d-karim)](https://github.com/ahm3d-karim) |
| Approvers | [Ahmad Karim (@ahm3d-karim)](https://github.com/ahm3d-karim) |

- **Committers** may change the source. Every change reaches `main` through a
  pull request, and the CI test suite must pass before it can merge.
- **Reviewers** review every contribution from anyone who is not a committer.
- **Approvers** manually approve each signing request in SignPath before a
  release is signed.

All team members use multi-factor authentication for GitHub and SignPath.

## Privacy policy

This program will not transfer any information to other networked systems
unless specifically requested by the user or the person installing or
operating it.

Skiff converts files entirely on your computer; it has no telemetry, update
checks or accounts. Optional tools that Skiff can use when you have installed
them yourself (Microsoft Office, LibreOffice, Docling) are governed by their
own privacy policies. Skiff's interface runs in Microsoft Edge WebView2, a
Windows component whose diagnostic data follows your Windows privacy
settings.

## Uninstalling

Skiff installs per user. Remove it from **Settings → Apps → Installed apps →
Skiff → Uninstall**, or run `uninstall.exe` in its install folder.
