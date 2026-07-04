"use strict";

const fs = require("fs");
const path = require("path");
const AdmZip = require("adm-zip");
const packageJson = require("../package.json");

const outputDirectory = path.resolve(__dirname, "..", "..", "..", "outputs");
const unpackedDirectory = path.join(outputDirectory, "win-unpacked");
const artifactPath = path.join(
  outputDirectory,
  `Clip-Image-Viewer-${packageJson.version}-windows-x64-portable.zip`,
);

if (!fs.existsSync(path.join(unpackedDirectory, "ClipImageViewer.exe"))) {
  throw new Error("win-unpacked 빌드가 없습니다. 먼저 npm run build:win을 실행하세요.");
}

const zip = new AdmZip();
zip.addLocalFolder(unpackedDirectory);
zip.addFile("portable.flag", Buffer.alloc(0));
zip.writeZip(artifactPath);
console.log(`Portable package created: ${artifactPath}`);
