"use strict";

// Collects license texts for every crate compiled into the Rust build:
// the native host for x86_64-pc-windows-msvc and the WebAssembly UI.
// Development and build-script-only dependencies are not shipped and are skipped.
// Output: licenses/rust/<crate>-<version>.txt and licenses/RUST_THIRD_PARTY_NOTICES.md.

const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const projectRoot = path.resolve(__dirname, "..");
const outputRoot = path.join(projectRoot, "licenses");
const rustOutput = path.join(outputRoot, "rust");
const licenseFilePattern = /^(licen[cs]e|copying|notice|copyright|unlicense|authors)([-._].*)?$/i;
const roots = [
  { name: "clip-image-viewer-native", target: "x86_64-pc-windows-msvc" },
  { name: "clipview-ui", target: "wasm32-unknown-unknown" },
];

function metadata(target) {
  const output = execFileSync("cargo", [
    "metadata", "--format-version", "1", "--locked",
    "--manifest-path", path.join(projectRoot, "rust", "Cargo.toml"),
    "--filter-platform", target,
  ], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  return JSON.parse(output);
}

function shippedPackages() {
  const shipped = new Map();
  for (const root of roots) {
    const data = metadata(root.target);
    const packages = new Map(data.packages.map((pkg) => [pkg.id, pkg]));
    const nodes = new Map(data.resolve.nodes.map((node) => [node.id, node]));
    const members = new Set(data.workspace_members);
    const start = data.packages.find((pkg) => pkg.name === root.name && members.has(pkg.id));
    if (!start) throw new Error(`Workspace package not found: ${root.name}`);
    const pending = [start.id];
    const seen = new Set();
    while (pending.length) {
      const id = pending.pop();
      if (seen.has(id)) continue;
      seen.add(id);
      if (!members.has(id)) shipped.set(id, packages.get(id));
      for (const dep of nodes.get(id)?.deps || []) {
        // dep_kinds kind is null for normal dependencies, "dev" or "build" otherwise.
        if (dep.dep_kinds.some((kind) => kind.kind === null)) pending.push(dep.pkg);
      }
    }
  }
  return [...shipped.values()].sort((a, b) => (
    a.name.localeCompare(b.name) || a.version.localeCompare(b.version)
  ));
}

function licenseTexts(pkg) {
  const directory = path.dirname(pkg.manifest_path);
  const files = new Set();
  if (pkg.license_file) files.add(path.resolve(directory, pkg.license_file));
  for (const name of fs.readdirSync(directory)) {
    const file = path.join(directory, name);
    if (licenseFilePattern.test(name) && fs.statSync(file).isFile()) files.add(file);
  }
  return [...files].sort().map((file) => ({
    name: path.basename(file),
    text: fs.readFileSync(file, "utf8").trim(),
  }));
}

function standardText(pkg) {
  // Crates that ship no license file still name SPDX licenses; attach the standard texts
  // (SPDX license-list-data). BSD-3-Clause names the crate authors as copyright holders.
  const expression = pkg.license || "";
  const standard = [
    [/\bMIT\b/, "MIT.txt"], [/Apache-2\.0/, "Apache-2.0.txt"], [/BSD-3-Clause/, "BSD-3-Clause.txt"],
    [/BSL-1\.0/, "BSL-1.0.txt"], [/MPL-2\.0/, "MPL-2.0.txt"],
  ];
  return standard.filter(([pattern]) => pattern.test(expression)).map(([, file]) => {
    let text = fs.readFileSync(path.join(outputRoot, file), "utf8").trim();
    if (file === "BSD-3-Clause.txt") {
      const owners = (pkg.authors || []).join(", ") || "the crate authors";
      text = text.replace("Copyright (c) <year> <owner>.", `Copyright (c) ${owners}.`);
    }
    return { name: `${file.replace(/\.txt$/, "")} (standard text; the crate ships no license file)`, text };
  });
}

function write(file, content) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content.replace(/\r?\n/g, "\r\n").trimEnd() + "\r\n");
}

const packages = shippedPackages();
fs.rmSync(rustOutput, { recursive: true, force: true });
const rows = [];
const missing = [];
for (const pkg of packages) {
  let texts = licenseTexts(pkg);
  if (!texts.length) {
    texts = standardText(pkg);
    if (!texts.length) missing.push(`${pkg.name} ${pkg.version} (${pkg.license || "no license declared"})`);
  }
  const fileName = `${pkg.name}-${pkg.version}.txt`;
  const header = [
    `Crate: ${pkg.name} ${pkg.version}`,
    `License: ${pkg.license || "Not declared"}`,
    `Repository: ${pkg.repository || "Not declared"}`,
    pkg.authors?.length ? `Authors: ${pkg.authors.join(", ")}` : null,
  ].filter(Boolean).join("\n");
  write(path.join(rustOutput, fileName), [header, ...texts.map((t) => `===== ${t.name} =====\n\n${t.text}`)].join("\n\n"));
  rows.push(`| ${pkg.name} | ${pkg.version} | ${pkg.license || "Not declared"} | rust/${fileName} |`);
}

write(path.join(outputRoot, "RUST_THIRD_PARTY_NOTICES.md"), `# Third-Party Notices (Rust build)

Clip Image Viewer's Rust build (native host and WebAssembly UI) includes the
following Rust crates. Each crate's complete license texts are in
\`licenses/rust/\`. Generated by \`scripts/collect-rust-licenses.js\` from
\`rust/Cargo.lock\`; development and build-script-only crates are not shipped
and are not listed.

Statically linked C code arrives through these crates: SQLite (public domain,
via libsqlite3-sys), Little CMS 2 (MIT, via lcms2-sys) and the WebView2 loader
(via webview2-com-sys). The app needs the Microsoft Edge WebView2 Runtime,
which is part of Windows and is not redistributed in this package.

Crates under MPL-2.0 (for example \`selectors\`) are used unmodified. Their
source code is available from crates.io and from the repository named in each
crate's notice file.

## Code adapted from other projects

- CLIP layer chunk decoding is adapted from dobrokot/clip_to_psd (MIT); see
  \`licenses/clip_to_psd-LICENSE.txt\`.
- LibRaw 0.22.3 (https://www.libraw.org), Copyright (C) 2008-2025 LibRaw LLC,
  is linked statically and used under the COMMON DEVELOPMENT AND DISTRIBUTION
  LICENSE Version 1.0 (CDDL-1.0), chosen from its LGPL-2.1/CDDL-1.0 dual license.
  It runs only in the isolated RAW worker process. The LibRaw source is
  unmodified; it is included under \`third_party/LibRaw-0.22.3\` in the
  Clip Image Viewer source repository and is available from libraw.org. See
  \`licenses/LibRaw-0.22.3-LICENSE.CDDL.txt\` and
  \`licenses/LibRaw-0.22.3-COPYRIGHT.txt\` (which also lists components LibRaw
  includes under other licenses).
- The BPG header parsing, chroma interpolation filters and colour conversion
  in \`rust/native/src/bpg.rs\` follow libbpg 0.9.8 \`libbpg.c\`,
  Copyright (c) 2014 Fabrice Bellard, MIT license; see
  \`licenses/libbpg-LICENSE.txt\`. No libbpg or FFmpeg code is linked.

## Crates (${packages.length})

| Crate | Version | License | Text |
|---|---|---|---|
${rows.join("\n")}
`);

const libbpg = `libbpg (libbpg.c), Copyright (c) 2014 Fabrice Bellard

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.`;
write(path.join(outputRoot, "libbpg-LICENSE.txt"), libbpg);
const libraw = path.join(projectRoot, "third_party", "LibRaw-0.22.3");
for (const [source, target] of [["LICENSE.CDDL", "LibRaw-0.22.3-LICENSE.CDDL.txt"], ["COPYRIGHT", "LibRaw-0.22.3-COPYRIGHT.txt"]]) {
  write(path.join(outputRoot, target), fs.readFileSync(path.join(libraw, source), "utf8"));
}

console.log(`Rust license notices: ${packages.length} crates`);
if (missing.length) {
  console.error(`No license text found for:\n${missing.join("\n")}`);
  process.exitCode = 1;
}
