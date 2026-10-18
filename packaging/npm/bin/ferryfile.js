#!/usr/bin/env node
// Launcher for Ferryfile. On first run it downloads the portable build for this
// package's version from GitHub Releases, checks its SHA-256, unpacks it to
// %LOCALAPPDATA%\ferryfile\<version>, then starts Ferryfile in the background.
// No install scripts run during `npm install`; everything happens here.

import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { createWriteStream, existsSync, readFileSync, mkdirSync, renameSync, rmSync } from "node:fs";
import { join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

const release = JSON.parse(readFileSync(new URL("../release.json", import.meta.url), "utf8"));
const base = join(process.env.LOCALAPPDATA ?? join(process.env.USERPROFILE ?? ".", "AppData", "Local"), "ferryfile");
const dir = join(base, release.version);
const exe = join(dir, "ferryfile.exe");

const args = process.argv.slice(2);
if (args.includes("--help") || args.includes("-h")) {
  console.log(`ferryfile ${release.version}: offline drag-and-drop file converter for Windows

Usage:
  ferryfile              download Ferryfile if needed, then start it (it lives in the tray)
  ferryfile --install    download Ferryfile without starting it
  ferryfile --path       print where Ferryfile is installed
  ferryfile --version    print the version

Ferryfile is ~120 MB on first run (it includes FFmpeg, pandoc and PDFium).
More: https://github.com/ahm3d-karim/ferryfile`);
  process.exit(0);
}
if (args.includes("--version") || args.includes("-v")) {
  console.log(release.version);
  process.exit(0);
}
if (process.platform !== "win32" || process.arch !== "x64") {
  console.error("Ferryfile runs on 64-bit Windows only.");
  process.exit(1);
}

async function download() {
  mkdirSync(base, { recursive: true });
  const tmp = join(base, `.download-${process.pid}`);
  rmSync(tmp, { recursive: true, force: true });
  mkdirSync(tmp);
  const zip = join(tmp, "ferryfile.zip");

  console.error(`Downloading Ferryfile ${release.version} (first run only)…`);
  const res = await fetch(release.url);
  if (!res.ok || !res.body) throw new Error(`Download failed: HTTP ${res.status} for ${release.url}`);
  const total = Number(res.headers.get("content-length")) || 0;
  const hash = createHash("sha256");
  let received = 0;
  let lastShown = -1;
  const body = Readable.fromWeb(res.body);
  body.on("data", (chunk) => {
    hash.update(chunk);
    received += chunk.length;
    const pct = total ? Math.floor((received / total) * 100) : -1;
    if (process.stderr.isTTY && pct !== lastShown && pct % 5 === 0) {
      lastShown = pct;
      process.stderr.write(`\r  ${pct}%  (${(received / 1e6).toFixed(0)} MB)`);
    }
  });
  await pipeline(body, createWriteStream(zip));
  if (process.stderr.isTTY) process.stderr.write("\n");

  const digest = hash.digest("hex");
  if (digest !== release.sha256) {
    rmSync(tmp, { recursive: true, force: true });
    throw new Error(`Checksum mismatch: expected ${release.sha256}, got ${digest}. Not installing.`);
  }

  // Windows' own tar.exe (10 1803+) unpacks zip files. Use its full path: a
  // GNU tar from Git for Windows earlier on PATH would read "C:" as a host.
  const tar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
  const unpack = spawnSync(tar, ["-xf", zip, "-C", tmp], { stdio: "inherit" });
  if (unpack.status !== 0) throw new Error("Couldn't unpack the download (tar.exe failed).");
  rmSync(zip);
  rmSync(dir, { recursive: true, force: true });
  renameSync(tmp, dir);
}

try {
  if (args.includes("--path")) {
    console.log(dir);
    process.exit(0);
  }
  if (!existsSync(exe)) await download();
  if (args.includes("--install")) {
    console.error(`Ferryfile ${release.version} is installed in ${dir}`);
    process.exit(0);
  }
  // Detached so Ferryfile keeps running in the tray after this terminal closes.
  spawn(exe, [], { detached: true, stdio: "ignore", cwd: dir }).unref();
  console.error("Ferryfile is running in the tray. Drag a file from Explorer to use it.");
} catch (err) {
  console.error(`ferryfile: ${err.message}`);
  process.exit(1);
}
