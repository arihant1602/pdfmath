#!/usr/bin/env node
// Create the platform npm package for one prebuilt binary.
// usage: node scripts/npm-platform-package.mjs <platform> <arch> <path/to/pdfm[.exe]> [outdir]
// e.g.   node scripts/npm-platform-package.mjs linux x64 target/x86_64-unknown-linux-musl/release/pdfm
import fs from "node:fs";
import path from "node:path";

const [platform, arch, binary, outRoot = "npm/dist"] = process.argv.slice(2);
if (!platform || !arch || !binary) {
  console.error("usage: npm-platform-package.mjs <platform> <arch> <binary> [outdir]");
  process.exit(2);
}

const main = JSON.parse(fs.readFileSync("npm/pdfmath/package.json", "utf8"));
const name = `pdfmath-${platform}-${arch}`;
const exe = platform === "win32" ? "pdfm.exe" : "pdfm";
const dir = path.join(outRoot, name);
fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
fs.copyFileSync(binary, path.join(dir, "bin", exe));
fs.chmodSync(path.join(dir, "bin", exe), 0o755);
fs.writeFileSync(
  path.join(dir, "package.json"),
  JSON.stringify(
    {
      name,
      version: main.version,
      description: `The ${platform}-${arch} binary for pdfmath`,
      license: main.license,
      repository: main.repository,
      os: [platform],
      cpu: [arch],
      files: [`bin/${exe}`],
      preferUnplugged: true,
    },
    null,
    2
  ) + "\n"
);
console.log(dir);
