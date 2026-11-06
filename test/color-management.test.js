"use strict";

const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const AdmZip = require("adm-zip");
const sharp = require("sharp");
const { loadImage, loadThumbnail } = require("../src/image-loader");

function imageBuffer(dataUrl) {
  return Buffer.from(dataUrl.split(",")[1], "base64");
}

async function run() {
  const temporaryRoot = path.resolve(os.tmpdir());
  const directory = fs.mkdtempSync(path.join(temporaryRoot, "clipview-color-"));
  try {
    const width = 48;
    const height = 32;
    const pixels = Buffer.alloc(width * height * 3);
    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const offset = (y * width + x) * 3;
        pixels[offset] = Math.round(x / (width - 1) * 255);
        pixels[offset + 1] = Math.round(y / (height - 1) * 255);
        pixels[offset + 2] = (x + y) % 2 ? 32 : 192;
      }
    }
    const input = () => sharp(pixels, { raw: { width, height, channels: 3 } });

    for (const options of [
      { name: "profiled.jpg", profile: true, orientation: 1 },
      { name: "unprofiled.jpg", profile: false, orientation: 1 },
      { name: "rotated.jpeg", profile: true, orientation: 6 },
      { name: "mirrored.jfif", profile: true, orientation: 2 },
    ]) {
      let encoder = input();
      if (options.orientation !== 1) encoder = encoder.withMetadata({ orientation: options.orientation });
      const original = await encoder.withIccProfile("cmyk", { attach: options.profile })
        .jpeg({ quality: 100, chromaSubsampling: "4:4:4" }).toBuffer();
      const originalMetadata = await sharp(original).metadata();
      assert.equal(originalMetadata.space, "cmyk");
      assert.equal(originalMetadata.hasProfile, options.profile);
      const filePath = path.join(directory, options.name);
      fs.writeFileSync(filePath, original);
      const item = { kind: "file", path: filePath, name: options.name };
      const loaded = await loadImage(item);
      assert(loaded.dataUrl.startsWith("data:image/png;base64,"), options.name);
      const rendered = imageBuffer(loaded.dataUrl);
      const metadata = await sharp(rendered).metadata();
      assert.equal(metadata.space, "srgb");
      assert.equal(metadata.channels, 3);
      assert.equal(metadata.icc.toString("ascii", 16, 20), "RGB ");
      assert.equal(metadata.orientation, undefined, "orientation must be applied exactly once");
      assert.equal(loaded.metadata.space, "cmyk", "file info must describe the original image");
      assert.equal(loaded.metadata.format, "JPEG");
      assert.equal(loaded.metadata.byteSize, original.length);
      assert.equal(loaded.metadata.width, options.orientation === 6 ? height : width);
      assert.equal(loaded.metadata.height, options.orientation === 6 ? width : height);

      const expected = await sharp(original).autoOrient().png().toBuffer();
      assert.deepEqual(await sharp(rendered).raw().toBuffer(), await sharp(expected).raw().toBuffer());
      const thumbnail = imageBuffer(await loadThumbnail(item, 64, 64));
      assert.deepEqual(await sharp(thumbnail).raw().toBuffer(), await sharp(rendered).raw().toBuffer());
      assert.deepEqual(fs.readFileSync(filePath), original, "opening must not alter the original file");

      const archive = new AdmZip();
      archive.addFile(options.name, original);
      const archivePath = path.join(directory, `${options.name}.zip`);
      archive.writeZip(archivePath);
      const archiveItem = {
        kind: "archive-entry", archivePath, entryName: options.name, name: options.name,
      };
      const archived = await loadImage(archiveItem);
      assert.equal(archived.dataUrl, loaded.dataUrl, "archives must use the same color conversion");
      assert.equal(await loadThumbnail(archiveItem, 64, 64), await loadThumbnail(item, 64, 64));
    }

    for (const format of ["jpeg", "png", "gif", "webp"]) {
      let encoder = input();
      if (format === "jpeg") encoder = encoder.withIccProfile("p3");
      const original = await encoder[format]().toBuffer();
      const name = `unchanged.${format}`;
      const filePath = path.join(directory, name);
      fs.writeFileSync(filePath, original);
      const loaded = await loadImage({ kind: "file", path: filePath, name });
      assert.deepEqual(imageBuffer(loaded.dataUrl), original, `${format} must retain the original fast path`);
    }
  } finally {
    const resolved = path.resolve(directory);
    if (resolved.startsWith(`${temporaryRoot}${path.sep}`) &&
        path.basename(resolved).startsWith("clipview-color-")) {
      fs.rmSync(resolved, { recursive: true, force: true });
    }
  }
  console.log("color-management tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
