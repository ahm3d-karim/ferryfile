# ferryfile

[Ferryfile](https://github.com/ahm3d-karim/ferryfile) is an offline, drag-and-drop file
converter for Windows: start dragging a file in Explorer, drop it on the drawer
that slides in, and convert images, audio, video, PDFs, documents and
spreadsheets locally.

```sh
npx ferryfile        # run once
npm i -g ferryfile   # or install the `ferryfile` command
ferryfile
```

On first run the launcher downloads the portable build for its version from
[GitHub Releases](https://github.com/ahm3d-karim/ferryfile/releases) (about 120 MB,
including FFmpeg, pandoc and PDFium), verifies its SHA-256 checksum, and unpacks
it to `%LOCALAPPDATA%\ferryfile\<version>`. Nothing runs during `npm install`.

- `ferryfile --path` prints the install folder; delete it to remove Ferryfile.
- Windows 10/11 x64 only. Ferryfile lives in the system tray.
- For "Start with Windows", prefer `npm i -g` over `npx`, and re-enable it in
  Settings after upgrading (each version installs to its own folder).

Other ways to install: the installer on the
[Releases page](https://github.com/ahm3d-karim/ferryfile/releases),
`winget install ferryfile`, or Scoop. MIT licensed; bundled tools keep
their own licenses (see THIRD_PARTY_NOTICES.md in the download).
