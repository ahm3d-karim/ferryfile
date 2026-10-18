# skiff-app

[Skiff](https://github.com/ahm3d-karim/skiff) is an offline, drag-and-drop file
converter for Windows: start dragging a file in Explorer, drop it on the drawer
that slides in, and convert images, audio, video, PDFs, documents and
spreadsheets locally.

```sh
npx skiff-app        # run once
npm i -g skiff-app   # or install the `skiff` command
skiff
```

On first run the launcher downloads the portable build for its version from
[GitHub Releases](https://github.com/ahm3d-karim/skiff/releases) (about 120 MB,
including FFmpeg, pandoc and PDFium), verifies its SHA-256 checksum, and unpacks
it to `%LOCALAPPDATA%\skiff-app\<version>`. Nothing runs during `npm install`.

- `skiff --path` prints the install folder; delete it to remove Skiff.
- Windows 10/11 x64 only. Skiff lives in the system tray.
- For "Start with Windows", prefer `npm i -g` over `npx`, and re-enable it in
  Settings after upgrading (each version installs to its own folder).

Other ways to install: the installer on the
[Releases page](https://github.com/ahm3d-karim/skiff/releases),
`winget install AhmadKarim.Skiff`, or Scoop. MIT licensed; bundled tools keep
their own licenses (see THIRD_PARTY_NOTICES.md in the download).
