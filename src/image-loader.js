"use strict";

const fs = require("fs");
const path = require("path");
const {
  ARCHIVE_EXTENSIONS,
  BPG_EXTENSIONS,
  CLIP_EXTENSIONS,
  CONVERT_EXTENSIONS,
  IMAGE_EXTENSIONS,
  JXR_EXTENSIONS,
  MAGICK_EXTENSIONS,
  PSD_EXTENSIONS,
  RAW_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  extensionOf,
  isImage,
  isSupported,
  listFolder,
  naturalCompare,
} = require("./file-types");

const MIME_TYPES = {
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".jfif": "image/jpeg",
  ".png": "image/png",
  ".apng": "image/png",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".bmp": "image/bmp",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".avif": "image/avif",
  ".heic": "image/heic",
  ".heif": "image/heif",
  ".hif": "image/heif",
};

let sqlPromise;
let magickPromise;
let jxrPromise;
let agPsdInitialized = false;
const clipSourceCache = new Map();
const CLIP_SOURCE_CACHE_LIMIT = 5;

function sharp(...args) {
  return require("sharp")(...args);
}

async function getMagick() {
  if (!magickPromise) {
    magickPromise = (async () => {
      const magick = require("@imagemagick/magick-wasm");
      const wasm = fs.readFileSync(
        require.resolve("@imagemagick/magick-wasm/magick.wasm"),
      );
      await magick.initializeImageMagick(wasm);
      return magick;
    })();
  }
  return magickPromise;
}

async function renderMagick(buffer, ext = "") {
  const magick = await getMagick();
  const format = {
    ".dds": magick.MagickFormat.Dds,
    ".j2k": magick.MagickFormat.J2k,
    ".j2c": magick.MagickFormat.J2c,
    ".jpc": magick.MagickFormat.Jpc,
    ".jp2": magick.MagickFormat.Jp2,
    ".jpf": magick.MagickFormat.Jp2,
    ".jpx": magick.MagickFormat.Jp2,
    ".tga": magick.MagickFormat.Tga,
    ".pcx": magick.MagickFormat.Pcx,
    ".pgm": magick.MagickFormat.Pgm,
    ".pnm": magick.MagickFormat.Pnm,
    ".ppm": magick.MagickFormat.Ppm,
    ".pbm": magick.MagickFormat.Pbm,
    ".pam": magick.MagickFormat.Pam,
    ".jxl": magick.MagickFormat.Jxl,
    ".exr": magick.MagickFormat.Exr,
    ".qoi": magick.MagickFormat.Qoi,
  }[ext];
  const convert = (images) => {
    if (!images.length) throw new Error("이미지 프레임을 찾지 못했습니다.");
    const animatedFormat = [".jxl", ".gif", ".webp", ".avif", ".apng"].includes(ext);
    if (images.length > 1 && animatedFormat) {
      images.coalesce();
      return images.write(magick.MagickFormat.WebP, (data) => ({
        buffer: Buffer.from(data),
        mime: "image/webp",
        source: "ImageMagick 애니메이션 변환",
      }));
    }
    return images[0].write(magick.MagickFormat.Png, (data) => ({
      buffer: Buffer.from(data),
      mime: "image/png",
      source: "ImageMagick 변환",
    }));
  };
  return format
    ? magick.ImageMagick.readCollection(buffer, format, convert)
    : magick.ImageMagick.readCollection(buffer, convert);
}

async function getJxrDecoder() {
  if (!jxrPromise) {
    jxrPromise = (async () => {
      if (!globalThis.ImageData) {
        globalThis.ImageData = class ImageData {
          constructor(data, width, height) {
            this.data = data;
            this.width = width;
            this.height = height;
          }
        };
      }
      const codecPath = require.resolve("@discourse/jxr/codec/dec/jxr_dec.js");
      const codecSource = fs.readFileSync(codecPath, "utf8")
        .replace(
          /var ENVIRONMENT_IS_NODE = [^;]+;/,
          "var ENVIRONMENT_IS_NODE = false;",
        )
        .replace(
          "wasmBinaryFile = new URL('jxr_dec.wasm', import.meta.url).href;",
          "wasmBinaryFile = 'jxr_dec.wasm';",
        );
      const dataUrl = `data:text/javascript;base64,${
        Buffer.from(`var window = {};\n${codecSource}`).toString("base64")
      }`;
      const moduleFactory = (await import(dataUrl)).default;
      const wasmBytes = fs.readFileSync(
        require.resolve("@discourse/jxr/codec/dec/jxr_dec.wasm"),
      );
      const wasmModule = await WebAssembly.compile(wasmBytes);
      const module = await moduleFactory({
        noInitialRun: true,
        instantiateWasm: (imports, callback) => {
          const instance = new WebAssembly.Instance(wasmModule, imports);
          callback(instance);
          return instance.exports;
        },
      });
      return async (buffer) => {
        const result = module.decode(buffer);
        if (!result) throw new Error("JPEG XR 디코딩에 실패했습니다.");
        return result;
      };
    })();
  }
  return jxrPromise;
}

async function renderJxr(buffer) {
  const decode = await getJxrDecoder();
  const input = buffer.buffer.slice(
    buffer.byteOffset,
    buffer.byteOffset + buffer.byteLength,
  );
  const image = await decode(input);
  const converted = await sharp(Buffer.from(image.data), {
    raw: {
      width: image.width,
      height: image.height,
      channels: 4,
    },
  }).png().toBuffer();
  return { buffer: converted, mime: "image/png", source: "JPEG XR 변환" };
}

async function renderBpg(buffer) {
  const { BPGDecoder, BPGDecoder8a } = require("bpg-decoder");
  const context = {
    createImageData: (width, height) => ({
      width,
      height,
      data: new Uint8ClampedArray(width * height * 4),
    }),
  };
  const animated = Boolean(buffer[5] & 1);
  const decoder = animated
    ? new BPGDecoder8a(context)
    : new BPGDecoder(context);
  const input = buffer.buffer.slice(
    buffer.byteOffset,
    buffer.byteOffset + buffer.byteLength,
  );
  decoder._onload({ response: input });
  const frames = decoder.frames || [];
  if (!frames.length) throw new Error("BPG 이미지 프레임을 찾지 못했습니다.");

  const width = frames[0].img.width;
  const height = frames[0].img.height;
  const pixels = Buffer.concat(frames.map((frame) => Buffer.from(frame.img.data)));
  const pipeline = sharp(pixels, {
    raw: {
      width,
      height: height * frames.length,
      channels: 4,
      pageHeight: height,
    },
  });
  if (frames.length === 1) {
    return {
      buffer: await pipeline.png().toBuffer(),
      mime: "image/png",
      source: "BPG 변환",
    };
  }
  return {
    buffer: await pipeline.webp({
      loop: decoder.loop_count || 0,
      delay: frames.map((frame) => Math.max(10, Math.round(frame.duration || 100))),
    }).toBuffer(),
    mime: "image/webp",
    source: "BPG 애니메이션 변환",
  };
}

async function renderRawWithLightdrift(item, buffer) {
  const LibRaw = require("lightdrift-libraw");
  const processor = new LibRaw();
  try {
    if (item.kind === "archive-entry") await processor.loadBuffer(buffer);
    else await processor.loadFile(item.path);
    try {
      const thumbnail = await processor.createThumbnailJPEGBuffer({
        maxSize: 4096,
        quality: 92,
      });
      return thumbnail.buffer;
    } catch {
      await processor.processImage();
      const image = await processor.createJPEGBuffer({
        width: 4096,
        quality: 92,
        fastMode: true,
      });
      return image.buffer;
    }
  } finally {
    await processor.close().catch(() => {});
  }
}

async function renderRawWithJulian(item, buffer) {
  const { LibRaw } = require("@julianberger/libraw.js");
  const processor = new LibRaw();
  try {
    if (item.kind === "archive-entry") await processor.readBuffer(buffer);
    else await processor.openFile(item.path);
    const result = await processor.unpackThumb();
    if (result < 0) throw new Error(await processor.strerror(result));
    return await processor.getThumbnail();
  } finally {
    await processor.recycle().catch(() => {});
  }
}

async function renderRaw(item, buffer) {
  let rawBuffer;
  try {
    rawBuffer = process.platform === "linux"
      ? await renderRawWithJulian(item, buffer)
      : await renderRawWithLightdrift(item, buffer);
  } catch (rawError) {
    try {
      const ext = extensionOf(item.kind === "archive-entry" ? item.entryName : item.path);
      return await renderMagick(buffer, ext);
    } catch (magickError) {
      throw new Error(
        `RAW 이미지를 읽지 못했습니다. (${rawError.message}; ${magickError.message})`,
      );
    }
  }

  try {
    const converted = await sharp(rawBuffer).jpeg({ quality: 92 }).toBuffer();
    return { buffer: converted, mime: "image/jpeg", source: "LibRaw 미리보기" };
  } catch {
    const converted = await renderMagick(rawBuffer);
    return { ...converted, source: "LibRaw 미리보기" };
  }
}

function listArchive(archivePath) {
  const AdmZip = require("adm-zip");
  const zip = new AdmZip(archivePath);
  return zip.getEntries()
    .filter((entry) => !entry.isDirectory && isImage(entry.entryName))
    .map((entry) => ({
      kind: "archive-entry",
      archivePath,
      entryName: entry.entryName,
      name: path.basename(entry.entryName),
      displayPath: `${path.basename(archivePath)} / ${entry.entryName}`,
    }))
    .sort((a, b) => naturalCompare(a.entryName, b.entryName));
}

function findSqliteDatabase(buffer) {
  const signature = Buffer.from("SQLite format 3\0", "binary");
  const offset = buffer.indexOf(signature);
  if (offset < 0) {
    throw new Error("CLIP 파일에서 SQLite 미리보기 영역을 찾지 못했습니다.");
  }

  if (buffer.length < offset + 100) {
    throw new Error("CLIP 미리보기 데이터가 손상되었습니다.");
  }

  let pageSize = buffer.readUInt16BE(offset + 16);
  if (pageSize === 1) pageSize = 65536;
  const pageCount = buffer.readUInt32BE(offset + 28);
  const expectedLength = pageSize * pageCount;
  const available = buffer.length - offset;
  const length = expectedLength > 0 && expectedLength <= available
    ? expectedLength
    : available;
  return buffer.subarray(offset, offset + length);
}

function readSqliteDatabaseFromFile(filePath) {
  const signature = Buffer.from("SQLite format 3\0", "binary");
  const stat = fs.statSync(filePath);
  const descriptor = fs.openSync(filePath, "r");
  const chunkSize = 4 * 1024 * 1024;
  let end = stat.size;
  let laterPrefix = Buffer.alloc(0);
  let databaseOffset = -1;

  try {
    while (end > 0) {
      const start = Math.max(0, end - chunkSize);
      const chunk = Buffer.allocUnsafe(end - start);
      fs.readSync(descriptor, chunk, 0, chunk.length, start);
      const searchable = laterPrefix.length
        ? Buffer.concat([chunk, laterPrefix])
        : chunk;
      const offset = searchable.lastIndexOf(signature);
      if (offset >= 0 && offset < chunk.length) {
        databaseOffset = start + offset;
        break;
      }
      laterPrefix = chunk.subarray(0, signature.length - 1);
      end = start;
    }

    if (databaseOffset < 0) {
      throw new Error("CLIP 파일에서 SQLite 미리보기 영역을 찾지 못했습니다.");
    }

    const header = Buffer.alloc(100);
    if (fs.readSync(descriptor, header, 0, header.length, databaseOffset) < header.length) {
      throw new Error("CLIP 미리보기 데이터가 손상되었습니다.");
    }
    let pageSize = header.readUInt16BE(16);
    if (pageSize === 1) pageSize = 65536;
    const pageCount = header.readUInt32BE(28);
    const expectedLength = pageSize * pageCount;
    const available = stat.size - databaseOffset;
    const length = expectedLength > 0 && expectedLength <= available
      ? expectedLength
      : available;
    const database = Buffer.allocUnsafe(length);
    fs.readSync(descriptor, database, 0, length, databaseOffset);
    return database;
  } finally {
    fs.closeSync(descriptor);
  }
}

async function getSql() {
  if (!sqlPromise) {
    const initSqlJs = require("sql.js");
    sqlPromise = initSqlJs({
      locateFile: (file) => require.resolve(`sql.js/dist/${file}`),
    });
  }
  return sqlPromise;
}

function numberOrZero(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : 0;
}

function canvasCrop(row) {
  const canvasWidth = numberOrZero(row.CanvasWidth);
  const canvasHeight = numberOrZero(row.CanvasHeight);
  const trimWidth = numberOrZero(row.CropFrameWidth);
  const trimHeight = numberOrZero(row.CropFrameHeight);
  if (
    !numberOrZero(row.CropFrameShow) ||
    !canvasWidth ||
    !canvasHeight ||
    !trimWidth ||
    !trimHeight
  ) {
    return null;
  }
  return {
    canvasWidth,
    canvasHeight,
    trimWidth,
    trimHeight,
    bleed: Math.max(0, numberOrZero(row.CropFrameDitch)),
    offsetX: numberOrZero(row.CropFrameCropOffsetX ?? row.CropFrameOffsetX),
    offsetY: numberOrZero(row.CropFrameCropOffsetY ?? row.CropFrameOffsetY),
    pageCount: canvasWidth > canvasHeight && trimWidth < trimHeight ? 2 : 1,
  };
}

async function extractClipData(input) {
  const databaseBytes = Buffer.isBuffer(input)
    ? findSqliteDatabase(input)
    : readSqliteDatabaseFromFile(input);
  const SQL = await getSql();
  const db = new SQL.Database(databaseBytes);

  try {
    const previewStatement = db.prepare(
      "SELECT ImageData FROM CanvasPreview WHERE ImageData IS NOT NULL LIMIT 1",
    );
    try {
      if (!previewStatement.step()) {
        throw new Error("CLIP 파일에 저장된 캔버스 미리보기가 없습니다.");
      }
      const row = previewStatement.getAsObject();
      const image = row.ImageData;
      if (!image || !image.length) {
        throw new Error("CLIP 캔버스 미리보기가 비어 있습니다.");
      }
      let crop = null;
      const tableStatement = db.prepare(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='Canvas' LIMIT 1",
      );
      try {
        if (tableStatement.step()) {
          const canvasStatement = db.prepare("SELECT * FROM Canvas LIMIT 1");
          try {
            if (canvasStatement.step()) crop = canvasCrop(canvasStatement.getAsObject());
          } finally {
            canvasStatement.free();
          }
        }
      } finally {
        tableStatement.free();
      }
      return { preview: Buffer.from(image), crop };
    } finally {
      previewStatement.free();
    }
  } finally {
    db.close();
  }
}

async function extractClipPreview(input) {
  return (await extractClipData(input)).preview;
}

async function renderWebtoonPsd(buffer) {
  const module = await import("@webtoon/psd");
  const Psd = module.default;
  const arrayBuffer = buffer.buffer.slice(
    buffer.byteOffset,
    buffer.byteOffset + buffer.byteLength,
  );
  const psd = Psd.parse(arrayBuffer);
  const pixels = await psd.composite();
  if (!pixels) {
    throw new Error("PSD 호환 합성 이미지가 없습니다.");
  }
  return sharp(Buffer.from(pixels), {
    raw: {
      width: psd.width,
      height: psd.height,
      channels: 4,
    },
  }).png().toBuffer();
}

function initializeAgPsd(agPsd) {
  if (agPsdInitialized) return;
  agPsd.initializeCanvas(
    (width, height) => ({
      width,
      height,
      getContext: () => ({
        createImageData: (imageWidth, imageHeight) => ({
          width: imageWidth,
          height: imageHeight,
          data: new Uint8ClampedArray(imageWidth * imageHeight * 4),
        }),
      }),
    }),
    (width, height) => ({
      width,
      height,
      data: new Uint8ClampedArray(width * height * 4),
    }),
  );
  agPsdInitialized = true;
}

async function renderAgPsd(buffer) {
  const agPsd = require("ag-psd");
  initializeAgPsd(agPsd);
  const psd = agPsd.readPsd(buffer, {
    skipLayerImageData: true,
    skipThumbnail: true,
    useImageData: true,
    logMissingFeatures: false,
  });
  if (!psd.imageData?.data) {
    throw new Error("PSD 호환 합성 이미지가 없습니다.");
  }
  return sharp(Buffer.from(psd.imageData.data), {
    raw: {
      width: psd.imageData.width,
      height: psd.imageData.height,
      channels: 4,
    },
  }).png().toBuffer();
}

async function renderPsd(buffer, ext) {
  if (ext === ".psb") {
    return renderWebtoonPsd(buffer);
  }

  try {
    return await renderAgPsd(buffer);
  } catch (agError) {
    try {
      return await renderWebtoonPsd(buffer);
    } catch (webtoonError) {
      throw new Error(
        `PSD 합성 이미지를 읽지 못했습니다. (${agError.message}; ${webtoonError.message})`,
      );
    }
  }
}

function readArchiveEntry(item) {
  const AdmZip = require("adm-zip");
  const zip = new AdmZip(item.archivePath);
  const entry = zip.getEntry(item.entryName);
  if (!entry) throw new Error("압축 파일 안의 이미지를 찾지 못했습니다.");
  return entry.getData();
}

async function cachedClipData(item, input) {
  if (item.kind === "archive-entry") return extractClipData(input);
  const cacheKey = path.resolve(item.path);
  if (clipSourceCache.has(cacheKey)) return clipSourceCache.get(cacheKey);
  const promise = extractClipData(item.path).catch((error) => {
    clipSourceCache.delete(cacheKey);
    throw error;
  });
  clipSourceCache.set(cacheKey, promise);
  while (clipSourceCache.size > CLIP_SOURCE_CACHE_LIMIT) {
    clipSourceCache.delete(clipSourceCache.keys().next().value);
  }
  return promise;
}

async function sourceBuffer(item) {
  const ext = extensionOf(item.kind === "archive-entry" ? item.entryName : item.path);

  if (CLIP_EXTENSIONS.has(ext)) {
    const input = item.kind === "archive-entry" ? readArchiveEntry(item) : null;
    const clip = await cachedClipData(item, input);
    const crop = clip.crop
      ? { ...clip.crop }
      : (item.crop ? { ...item.crop } : null);
    if (clip.crop && item.crop?.canvasWidth) {
      crop.pageCount = Math.max(
        1,
        Math.round(clip.crop.canvasWidth / item.crop.canvasWidth),
      );
    }
    return {
      buffer: clip.preview,
      mime: "image/png",
      source: "CLIP 미리보기",
      crop,
    };
  }

  const input = item.kind === "archive-entry"
    ? readArchiveEntry(item)
    : fs.readFileSync(item.path);

  if (PSD_EXTENSIONS.has(ext)) {
    return {
      buffer: await renderPsd(input, ext),
      mime: "image/png",
      source: "PSD 합성 이미지",
    };
  }

  if (RAW_EXTENSIONS.has(ext)) {
    return renderRaw(item, input);
  }

  if (JXR_EXTENSIONS.has(ext)) {
    return renderJxr(input);
  }

  if (BPG_EXTENSIONS.has(ext)) {
    return renderBpg(input);
  }

  if (MAGICK_EXTENSIONS.has(ext)) {
    return renderMagick(input, ext);
  }

  if (CONVERT_EXTENSIONS.has(ext)) {
    try {
      const converted = await sharp(input, { page: 0 }).png().toBuffer();
      return { buffer: converted, mime: "image/png", source: "변환 이미지" };
    } catch {
      return renderMagick(input, ext);
    }
  }

  return {
    buffer: input,
    mime: MIME_TYPES[ext] || "application/octet-stream",
    source: "원본",
    crop: null,
  };
}

function calculateCropRegion(crop, width, height, mode) {
  if (!crop || !width || !height || !["trim", "bleed"].includes(mode)) return null;
  const pageCount = Math.max(1, numberOrZero(crop.pageCount) || 1);
  const expansion = mode === "bleed" ? crop.bleed * 2 : 0;
  const targetWidth = (crop.trimWidth + expansion) * pageCount;
  const targetHeight = crop.trimHeight + expansion;
  const x = (crop.canvasWidth - targetWidth) / 2 + crop.offsetX;
  const y = (crop.canvasHeight - targetHeight) / 2 + crop.offsetY;
  const left = Math.max(0, Math.round(x / crop.canvasWidth * width));
  const top = Math.max(0, Math.round(y / crop.canvasHeight * height));
  const cropWidth = Math.min(width - left, Math.round(targetWidth / crop.canvasWidth * width));
  const cropHeight = Math.min(height - top, Math.round(targetHeight / crop.canvasHeight * height));
  if (cropWidth <= 0 || cropHeight <= 0) return null;
  return { left, top, width: cropWidth, height: cropHeight };
}

async function imageMetadata(buffer, fallback = {}) {
  try {
    const metadata = await sharp(buffer, { animated: true }).metadata();
    return {
      width: metadata.width || fallback.width || 0,
      height: metadata.height || fallback.height || 0,
      format: (metadata.format || fallback.format || "").toUpperCase(),
      space: metadata.space || "",
      channels: metadata.channels || 0,
      pages: metadata.pages || 1,
      density: metadata.density || 0,
      hasAlpha: Boolean(metadata.hasAlpha),
    };
  } catch {
    return fallback;
  }
}

async function loadImage(item, cropMode = "full") {
  const sourceData = await sourceBuffer(item);
  let { buffer, mime, source } = sourceData;
  const originalMetadata = await imageMetadata(buffer, {
    format: extensionOf(item.name).slice(1).toUpperCase(),
  });
  const cropRegion = calculateCropRegion(
    sourceData.crop,
    originalMetadata.width,
    originalMetadata.height,
    cropMode,
  );
  if (cropRegion) {
    buffer = await sharp(buffer, { page: 0 }).extract(cropRegion).png().toBuffer();
    mime = "image/png";
    source = `${source} · ${cropMode === "trim" ? "재단선" : "재단 여백"}`;
  }
  const metadata = cropRegion
    ? {
        ...originalMetadata,
        width: cropRegion.width,
        height: cropRegion.height,
        format: "PNG",
      }
    : originalMetadata;
  let stat = null;
  const diskPath = item.kind === "archive-entry" ? item.archivePath : item.path;
  try {
    stat = fs.statSync(diskPath);
  } catch {
    stat = null;
  }

  return {
    dataUrl: `data:${mime};base64,${buffer.toString("base64")}`,
    metadata: {
      ...metadata,
      source,
      cropAvailable: Boolean(sourceData.crop),
      byteSize: item.kind === "archive-entry" ? buffer.length : (stat?.size || buffer.length),
      modifiedAt: stat?.mtime?.toISOString() || "",
    },
  };
}

async function loadThumbnail(item, width = 180, height = 110) {
  const { buffer } = await sourceBuffer(item);
  const thumbnail = await sharp(buffer, { page: 0 })
    .resize(width, height, { fit: "inside", withoutEnlargement: true })
    .png({ compressionLevel: 7 })
    .toBuffer();
  return `data:image/png;base64,${thumbnail.toString("base64")}`;
}

module.exports = {
  ARCHIVE_EXTENSIONS,
  IMAGE_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  calculateCropRegion,
  extractClipData,
  extractClipPreview,
  findSqliteDatabase,
  extensionOf,
  isSupported,
  listArchive,
  listFolder,
  loadImage,
  loadThumbnail,
  naturalCompare,
  renderBpg,
  renderJxr,
  renderMagick,
  renderRaw,
};
