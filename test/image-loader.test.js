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
  loadImage,
  loadLayerThumbnail,
  pickLayer,
  prepareLayeredImage,
  renderMagick,
  renderLayeredImage,
} = require("../src/image-loader");
const {
  SUPPORTED_EXTENSIONS,
  VIDEO_EXTENSIONS,
  VIDEO_MIME_TYPES,
  listFolder,
  naturalCompare,
} = require("../src/file-types");

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
    CropFrameShow INTEGER,
    CanvasRootFolder INTEGER
  )`);
  db.run("INSERT INTO Canvas VALUES (96, 64, 48, 32, 4, 0, 0, 1, 1)");
  db.run(`CREATE TABLE Layer (
    _PW_ID INTEGER PRIMARY KEY,
    MainId INTEGER,
    LayerName TEXT,
    LayerType INTEGER,
    LayerClip INTEGER,
    LayerMasking INTEGER,
    LayerOpacity INTEGER,
    LayerComposite INTEGER,
    LayerFolder INTEGER,
    LayerVisibility INTEGER,
    LayerNextIndex INTEGER,
    LayerFirstChildIndex INTEGER,
    LayerLayerMaskMipmap INTEGER,
    LayerLayerMaskThumbnail INTEGER
  )`);
  db.run("INSERT INTO Layer VALUES (1, 1, '', 256, 0, 0, 256, 0, 1, 1, 0, 2, 0, 0)");
  db.run("INSERT INTO Layer VALUES (2, 2, '테스트 레이어', 1, 0, 0, 192, 0, 0, 1, 0, 0, 0, 0)");
  db.run("ALTER TABLE Layer ADD COLUMN TextLayerType INTEGER");
  db.run("ALTER TABLE Layer ADD COLUMN TextLayerString BLOB");
  const textStatement = db.prepare(
    "UPDATE Layer SET LayerType=800, TextLayerType=2, TextLayerString=? WHERE MainId=2",
  );
  textStatement.run([Buffer.from("테스트 텍스트", "utf8")]);
  textStatement.free();
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
  assert.equal(cropped.layerDocument.format, "CLIP");
  assert.equal(cropped.layerDocument.layers[0].name, "테스트 레이어");
  assert.equal(cropped.layerDocument.layers[0].opacity, 75);
  assert.equal(cropped.layerDocument.toggleSupported, true);
  assert.equal(cropped.layerDocument.thumbnailSupported, true);
  assert.equal(
    cropped.layerDocument.layers[0].previewAccuracy,
    "원본 합성 문맥 래스터(정확)",
  );
  const clipLayerThumbnail = await loadLayerThumbnail({
    kind: "file",
    path: clipPath,
    name: "sample.clip",
  }, "2");
  assert(clipLayerThumbnail.startsWith("data:image/png;base64,"));
  const clipThumbnailPixel = await sharp(
    Buffer.from(clipLayerThumbnail.split(",")[1], "base64"),
  ).ensureAlpha().raw().toBuffer();
  const clipThumbnailCenter = (29 * 58 + 29) * 4;
  assert.deepEqual(
    [...clipThumbnailPixel.subarray(clipThumbnailCenter, clipThumbnailCenter + 4)],
    [70, 120, 230, 255],
  );
  const hiddenClip = await renderLayeredImage({
    kind: "file",
    path: clipPath,
    name: "sample.clip",
  }, { "2": false });
  const hiddenClipStats = await sharp(
    Buffer.from(hiddenClip.dataUrl.split(",")[1], "base64"),
  ).stats();
  assert.equal(hiddenClipStats.isOpaque, false);
  assert.deepEqual(calculateCropRegion({
    canvasWidth: 100,
    canvasHeight: 100,
    trimWidth: 80,
    trimHeight: 80,
    bleed: 0,
    offsetX: -30,
    offsetY: -30,
  }, 100, 100, "trim"), {
    left: 0,
    top: 0,
    width: 60,
    height: 60,
  });

  fs.writeFileSync(path.join(temp, "image10.png"), extracted);
  fs.writeFileSync(path.join(temp, "image2.png"), extracted);
  fs.writeFileSync(path.join(temp, "sample.mp4"), "not a real video");
  fs.writeFileSync(path.join(temp, "ignore.txt"), "x");
  const folderItems = listFolder(temp);
  const files = folderItems.map((item) => item.name);
  assert.deepEqual(files, ["image2.png", "image10.png", "sample.clip", "sample.mp4"]);
  assert.equal(folderItems.find((item) => item.name === "sample.mp4").mediaType, "video");
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
    children: [{
      name: "테스트 레이어",
      left: 0,
      top: 0,
      imageData: { width: 32, height: 24, data: psdPixels },
    }],
  }));
  const loadedPsd = await loadImage({
    kind: "file",
    path: psdPath,
    name: "sample.psd",
  });
  assert.equal(loadedPsd.metadata.width, 32);
  assert.equal(loadedPsd.metadata.height, 24);
  assert(loadedPsd.dataUrl.startsWith("data:image/png;base64,"));
  assert.equal(loadedPsd.layerDocument.format, "PSD");
  assert.equal(loadedPsd.layerDocument.layers.length, 1);
  assert.equal(loadedPsd.layerDocument.thumbnailSupported, true);
  assert.equal(
    loadedPsd.layerDocument.layers[0].previewAccuracy,
    "저장 레이어 래스터(정확)",
  );
  assert.equal(await prepareLayeredImage({
    kind: "file",
    path: psdPath,
    name: "sample.psd",
  }), true);
  const layerThumbnail = await loadLayerThumbnail({
    kind: "file",
    path: psdPath,
    name: "sample.psd",
  }, "0");
  assert(layerThumbnail.startsWith("data:image/png;base64,"));
  const layerThumbnailMetadata = await sharp(
    Buffer.from(layerThumbnail.split(",")[1], "base64"),
  ).metadata();
  assert.equal(layerThumbnailMetadata.width, 58);
  assert.equal(layerThumbnailMetadata.height, 58);
  const hiddenPsd = await renderLayeredImage({
    kind: "file",
    path: psdPath,
    name: "sample.psd",
  }, { "0": false });
  const hiddenStats = await sharp(Buffer.from(hiddenPsd.dataUrl.split(",")[1], "base64"))
    .stats();
  assert.equal(hiddenStats.isOpaque, false);
  assert.equal(await pickLayer({
    kind: "file",
    path: psdPath,
    name: "sample.psd",
  }, 4, 4, { "0": false }), null);

  const requiredExtensions = [
    ".bmp", ".jpg", ".gif", ".png", ".psd", ".dds", ".jxr", ".webp",
    ".j2k", ".jp2", ".tga", ".tiff", ".pcx", ".pgm", ".pnm", ".ppm",
    ".bpg", ".dng", ".cr2", ".crw", ".nef", ".nrw", ".orf", ".rw2",
    ".pef", ".sr2", ".raf", ".avif", ".jxl", ".exr", ".qoi", ".ico",
    ".svg", ".heic", ".heif", ".hif", ".clip",
    ".mp4", ".mkv", ".webm", ".avi", ".mov", ".wmv", ".m2ts", ".ogv",
  ];
  requiredExtensions.forEach((ext) => assert(SUPPORTED_EXTENSIONS.has(ext), ext));
  VIDEO_EXTENSIONS.forEach((ext) => assert(VIDEO_MIME_TYPES[ext], `missing MIME: ${ext}`));

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
