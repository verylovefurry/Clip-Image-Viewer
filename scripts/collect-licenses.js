"use strict";

const fs = require("fs");
const path = require("path");
const packageLock = require("../package-lock.json");

const projectRoot = path.resolve(__dirname, "..");
const outputRoot = path.join(projectRoot, "licenses");
const npmOutputRoot = path.join(outputRoot, "npm");
const licenseNamePattern = /^(license|licence|copying|notice)(\..*)?$/i;

function read(relativePath) {
  return fs.readFileSync(path.join(projectRoot, relativePath), "utf8");
}

function write(relativePath, content) {
  const targetPath = path.join(outputRoot, relativePath);
  fs.mkdirSync(path.dirname(targetPath), { recursive: true });
  fs.writeFileSync(targetPath, content.replace(/\r?\n/g, "\r\n").trimEnd() + "\r\n");
}

function safeName(value) {
  return value.replace(/^@/, "").replace(/[\\/:*?"<>|@]/g, "--");
}

function mitFallback(author) {
  return `MIT License

Copyright (c) ${author || "the package contributors"}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.`;
}

function iscFallback(author) {
  return `ISC License

Copyright (c) ${author || "the package contributors"}

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY
AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR
OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THIS SOFTWARE.`;
}

function fallbackLicense(packageJson) {
  const author = typeof packageJson.author === "string"
    ? packageJson.author
    : packageJson.author?.name;
  if (packageJson.license === "MIT") return mitFallback(author);
  if (packageJson.license === "ISC") return iscFallback(author);
  return `Package: ${packageJson.name}@${packageJson.version}
Declared license: ${packageJson.license || "Not declared"}
Repository: ${typeof packageJson.repository === "string"
    ? packageJson.repository
    : packageJson.repository?.url || "Not declared"}

The installed npm package did not include a standalone license file.
Consult the package repository for the authoritative license terms.`;
}

function collectPackageLicenses() {
  const seen = new Set();
  for (const [relativePath, metadata] of Object.entries(packageLock.packages || {})) {
    if (!relativePath.includes("node_modules/") || metadata.dev === true) continue;
    const packageDirectory = path.join(projectRoot, relativePath);
    const packageJsonPath = path.join(packageDirectory, "package.json");
    if (!fs.existsSync(packageJsonPath)) continue;

    const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
    const identity = `${packageJson.name}@${packageJson.version}`;
    if (seen.has(identity)) continue;
    seen.add(identity);

    const matchingFiles = fs.readdirSync(packageDirectory)
      .filter((name) => licenseNamePattern.test(name))
      .sort();
    const sections = matchingFiles.length
      ? matchingFiles.map((name) => (
          `===== ${name} =====\n\n${fs.readFileSync(path.join(packageDirectory, name), "utf8")}`
        ))
      : [fallbackLicense(packageJson)];
    const header = [
      `Package: ${identity}`,
      `Declared license: ${packageJson.license || metadata.license || "Not declared"}`,
      "",
    ].join("\n");
    write(path.join("npm", `${safeName(identity)}.txt`), header + sections.join("\n\n"));
  }
  return seen.size;
}

fs.rmSync(outputRoot, { recursive: true, force: true });
fs.mkdirSync(npmOutputRoot, { recursive: true });

const electronLicense = read("node_modules/electron/LICENSE");
const sharpLicense = read("node_modules/sharp/LICENSE");
const imageMagickNotice = read("node_modules/@imagemagick/magick-wasm/NOTICE");
const imageMagickEnd = imageMagickNotice.indexOf("\n[ ", 2);
const imageMagickLicense = imageMagickEnd > 0
  ? imageMagickNotice.slice(0, imageMagickEnd)
  : imageMagickNotice;
const libRawBase =
  "node_modules/lightdrift-libraw/deps/LibRaw-Win64/LibRaw-0.21.4";
const libRawLgpl = read(`${libRawBase}/LICENSE.LGPL`);
const libRawCddl = read(`${libRawBase}/LICENSE.CDDL`);

write("MIT.txt", electronLicense);
write("Apache-2.0.txt", sharpLicense);
write("LGPL-2.1.txt", libRawLgpl);
write("ImageMagick-License.txt", imageMagickLicense);
write("ImageMagick-NOTICE.txt", imageMagickNotice);
write("Electron-LICENSE.txt", electronLicense);
write("Sharp-LICENSE.txt", sharpLicense);
write(
  "LibRaw-LICENSE.txt",
  `LibRaw is dual-licensed under LGPL 2.1 or CDDL 1.0.\n\n` +
  `===== LGPL 2.1 =====\n\n${libRawLgpl}\n\n` +
  `===== CDDL 1.0 =====\n\n${libRawCddl}`,
);
write("Clip-Image-Viewer-LICENSE.txt", read("LICENSE"));
write("THIRD_PARTY_NOTICES.md", read("THIRD_PARTY_NOTICES.md"));

const packageCount = collectPackageLicenses();
console.log(`Collected licenses for ${packageCount} runtime npm packages.`);
