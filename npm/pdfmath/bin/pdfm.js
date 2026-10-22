#!/usr/bin/env node
// Launcher: finds the prebuilt pdfm binary for this platform and runs it.
"use strict";

const { spawnSync } = require("child_process");

const exe = process.platform === "win32" ? "pdfm.exe" : "pdfm";
// Windows packages are named "windows" (npm's spam filter rejects the "win32" names).
const platformName = process.platform === "win32" ? "windows" : process.platform;
const pkg = `pdfmath-${platformName}-${process.arch}`;

function findBinary() {
  if (process.env.PDFM_BINARY) return process.env.PDFM_BINARY;
  try {
    return require.resolve(`${pkg}/bin/${exe}`);
  } catch {
    return null;
  }
}

const bin = findBinary();
if (!bin) {
  console.error(
    `pdfmath: no prebuilt binary for ${process.platform}-${process.arch} (expected package "${pkg}").\n` +
      `Reinstall without --no-optional / --omit=optional, or build from source with: cargo install pdfmath`
  );
  process.exit(1);
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`pdfmath: failed to run ${bin}: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
