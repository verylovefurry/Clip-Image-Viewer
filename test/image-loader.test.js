"use strict";

const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const sharp = require("sharp");
const initSqlJs = require("sql.js");
const { writePsdBuffer } = require("ag-psd");
const { loadComicProject } = require("../src/comic-loader");
const {
  calculateCropRegion,
  extractClipPreview,
  listFolder,
  loadImage,
  naturalCompare,
  renderMagick,
} = require("../src/image-loader");
const { SUPPORTED_EXTENSIONS } = require("../src/file-types");

async function createSyntheticClip(outputPath) {
  const png = await sharp({
    create: {
      width: 96,
      height: 64,
      channels: 4,
      background: { r: 70, g: 120, b: 230, alpha: 1 },
    },
  }).png().toBuffer();
  const SQL = await initSqlJs({
    locateFile: (file) => require.resolve(`sql.js/dist/${file}`),
  });
  const db = new SQL.Database();
  db.run("CREATE TABLE CanvasPreview (ImageData BLOB)");
  db.run(`CREATE TABLE Canvas (
    CanvasWidth REAL,
    CanvasHeight REAL,
    CropFrameWidth REAL,
    CropFrameHeight REAL,
    CropFrameDitch REAL,
    CropFrameCropOffsetX REAL,
    CropFrameCropOffsetY REAL,
    CropFrameShow INTEGER
  )`);
  db.run("INSERT INTO Canvas VALUES (96, 64, 48, 32, 4, 0, 0, 1)");
  const statement = db.prepare("INSERT INTO CanvasPreview VALUES (?)");
  statement.run([png]);
  statement.free();
  const database = Buffer.from(db.export());
  db.close();
  fs.writeFileSync(outputPath, Buffer.concat([Buffer.from("CSFCHUNK_TEST_DATA"), database]));
}

async function createSyntheticCmc(outputPath) {
  const SQL = await initSqlJs({
    locateFile: (file) => require.resolve(`sql.js/dist/${file}`),
  });
  const db = new SQL.Database();
  db.run(`CREATE TABLE Project (
    ProjectRootCanvasNode INTEGER,
    DefaultPageUseCropFrame INTEGER,
    DefaultPageWidth REAL,
    DefaultPageHeight REAL,
    DefaultPageCropWidth REAL,
    DefaultPageCropHeight REAL,
    DefaultPageCropDitch REAL,
    DefaultPageCropOffsetX REAL,
    DefaultPageCropOffsetY REAL
  )`);
  db.run("INSERT INTO Project VALUES (1, 1, 96, 64, 48, 32, 4, 0, 0)");
  db.run(`CREATE TABLE CanvasNode (
    _PW_ID INTEGER,
    MainId INTEGER,
    Type INTEGER,
    NextIndex INTEGER,
    FirstChildIndex INTEGER,
    CanvasIndex INTEGER,
    PageFlag INTEGER,
    LinkPath TEXT
  )`);
  db.run("INSERT INTO CanvasNode VALUES (1, 1, 1, 0, 2, 0, 0, NULL)");
  db.run("INSERT INTO CanvasNode VALUES (2, 2, 2, 3, 0, 2, 0, '.:page2.clip')");
  db.run("INSERT INTO CanvasNode VALUES (3, 3, 2, 0, 0, 1, 0, '.:page1.clip')");
  fs.writeFileSync(outputPath, Buffer.from(db.export()));
  db.close();
}

async function run() {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "clipview-test-"));
  const clipPath = path.join(temp, "sample.clip");
  await createSyntheticClip(clipPath);

  const extracted = await extractClipPreview(clipPath);
  const metadata = await sharp(extracted).metadata();
  assert.equal(metadata.width, 96);
  assert.equal(metadata.height, 64);
  assert.deepEqual(calculateCropRegion({
    canvasWidth: 96,
    canvasHeight: 64,
    trimWidth: 48,
    trimHeight: 32,
    bleed: 4,
    offsetX: 0,
    offsetY: 0,
  }, 96, 64, "trim"), {
    left: 24,
    top: 16,
    width: 48,
    height: 32,
  });

  const cropped = await loadImage({
    kind: "file",
    path: clipPath,
    name: "sample.clip",
  }, "bleed");
  assert.equal(cropped.metadata.width, 56);
  assert.equal(cropped.metadata.height, 40);

  fs.writeFileSync(path.join(temp, "image10.png"), extracted);
  fs.writeFileSync(path.join(temp, "image2.png"), extracted);
  fs.writeFileSync(path.join(temp, "ignore.txt"), "x");
  const files = listFolder(temp).map((item) => item.name);
  assert.deepEqual(files, ["image2.png", "image10.png", "sample.clip"]);
  assert(naturalCompare("2.png", "10.png") < 0);

  const page1 = path.join(temp, "page1.clip");
  const page2 = path.join(temp, "page2.clip");
  fs.copyFileSync(clipPath, page1);
  fs.copyFileSync(clipPath, page2);
  const cmcPath = path.join(temp, "sample.cmc");
  await createSyntheticCmc(cmcPath);
  const comic = await loadComicProject(cmcPath);
  assert.deepEqual(comic.items.map((item) => item.name), ["page2.clip", "page1.clip"]);
  assert.equal(comic.comic.cropAvailable, true);

  const psdPath = path.join(temp, "sample.psd");
  const psdPixels = new Uint8ClampedArray(32 * 24 * 4);
  for (let index = 0; index < psdPixels.length; index += 4) {
    psdPixels[index] = 40;
    psdPixels[index + 1] = 120;
    psdPixels[index + 2] = 220;
    psdPixels[index + 3] = 255;
  }
  fs.writeFileSync(psdPath, writePsdBuffer({
    width: 32,
    height: 24,
    imageData: { width: 32, height: 24, data: psdPixels },
  }));
  const loadedPsd = await loadImage({
    kind: "file",
    path: psdPath,
    name: "sample.psd",
  });
  assert.equal(loadedPsd.metadata.width, 32);
  assert.equal(loadedPsd.metadata.height, 24);
  assert(loadedPsd.dataUrl.startsWith("data:image/png;base64,"));

  const requiredExtensions = [
    ".bmp", ".jpg", ".gif", ".png", ".psd", ".dds", ".jxr", ".webp",
    ".j2k", ".jp2", ".tga", ".tiff", ".pcx", ".pgm", ".pnm", ".ppm",
    ".bpg", ".dng", ".cr2", ".crw", ".nef", ".nrw", ".orf", ".rw2",
    ".pef", ".sr2", ".raf", ".avif", ".jxl", ".exr", ".qoi", ".ico",
    ".svg", ".heic", ".heif", ".hif", ".clip",
  ];
  requiredExtensions.forEach((ext) => assert(SUPPORTED_EXTENSIONS.has(ext), ext));

  const sourcePng = await sharp({
    create: {
      width: 20,
      height: 12,
      channels: 4,
      background: { r: 80, g: 160, b: 220, alpha: 1 },
    },
  }).png().toBuffer();
  await renderMagick(sourcePng);
  const magick = require("@imagemagick/magick-wasm");
  for (const [extension, format] of [
    [".dds", magick.MagickFormat.Dds],
    [".jp2", magick.MagickFormat.Jp2],
    [".jxl", magick.MagickFormat.Jxl],
    [".exr", magick.MagickFormat.Exr],
    [".qoi", magick.MagickFormat.Qoi],
    [".pcx", magick.MagickFormat.Pcx],
    [".ppm", magick.MagickFormat.Ppm],
  ]) {
    const encoded = magick.ImageMagick.read(sourcePng, (image) => (
      image.write(format, (data) => Buffer.from(data))
    ));
    const formatPath = path.join(temp, `sample${extension}`);
    fs.writeFileSync(formatPath, encoded);
    const loaded = await loadImage({
      kind: "file",
      path: formatPath,
      name: path.basename(formatPath),
    });
    assert.equal(loaded.metadata.width, 20, extension);
    assert.equal(loaded.metadata.height, 12, extension);
  }

  fs.rmSync(temp, { recursive: true, force: true });
  console.log("image-loader tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
