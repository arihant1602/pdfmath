#!/bin/sh
# Copy the version from Cargo.toml into the npm package (and its optionalDependencies).
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
node -e '
const fs = require("fs");
const f = "npm/pdfmath/package.json";
const p = JSON.parse(fs.readFileSync(f, "utf8"));
p.version = process.argv[1];
for (const k of Object.keys(p.optionalDependencies)) p.optionalDependencies[k] = process.argv[1];
fs.writeFileSync(f, JSON.stringify(p, null, 2) + "\n");
' "$version"
echo "npm package set to $version"
