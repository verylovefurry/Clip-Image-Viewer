"use strict";

const fs = require("fs");
const path = require("path");
const sharp = require("sharp");
const pngToIco = require("png-to-ico").default;

async function main() {
  const buildDir = path.join(__dirname, "..", "build");
  const svgPath = path.join(buildDir, "icon.svg");
  const sizes = [16, 24, 32, 48, 64, 128, 256];
  const buffers = await Promise.all(
    sizes.map((size) => sharp(svgPath).resize(size, size).png().toBuffer()),
  );
  fs.writeFileSync(path.join(buildDir, "icon.png"), buffers[buffers.length - 1]);
  fs.writeFileSync(path.join(buildDir, "icon.ico"), await pngToIco(buffers));
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
