"use strict";

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");
const packageJson = require("../package.json");

const outputDirectory = path.resolve(__dirname, "..", "..", "..", "outputs");
const version = packageJson.version;
const names = [
  `Clip-Image-Viewer-Setup-${version}-x64.exe`,
  `Clip-Image-Viewer-Setup-${version}-x64.exe.blockmap`,
  `Clip-Image-Viewer-${version}-windows-x64-portable.zip`,
  "latest.yml",
];
const lines = names.map((name) => {
  const filePath = path.join(outputDirectory, name);
  if (!fs.existsSync(filePath)) throw new Error(`체크섬 대상 파일이 없습니다: ${name}`);
  const hash = crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
  return `${hash}  ${name}`;
});
fs.writeFileSync(path.join(outputDirectory, "SHA256SUMS.txt"), `${lines.join("\n")}\n`);
console.log("SHA256SUMS.txt created.");
