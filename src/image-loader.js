"use strict";

const fs = require("fs");
const path = require("path");
const {
  decodeClipLayer,
  decodeClipLayerForCanvas,
  decodeClipMaskForCanvas,
  decodeClipVector,
  extractClipMaskSources,
  extractClipRasterSources,
  extractClipVectorSources,
} = require("./clip-layer-reader");
const {
  BPG_EXTENSIONS,
  CLIP_EXTENSIONS,
  CONVERT_EXTENSIONS,
  JXR_EXTENSIONS,
  MAGICK_EXTENSIONS,
  PSD_EXTENSIONS,
  RAW_EXTENSIONS,
  extensionOf,
  isImage,
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
const clipSourceCacheSizes = new Map();
const CLIP_SOURCE_CACHE_LIMIT = 5;
const MAX_CLIP_SOURCE_CACHE_BYTES = 512 * 1024 * 1024;
const psdDocumentCache = new Map();
const PSD_DOCUMENT_CACHE_LIMIT = 1;
const layerThumbnailCache = new Map();
const LAYER_THUMBNAIL_CACHE_LIMIT = 256;
const layeredRenderCache = new Map();
const LAYERED_RENDER_CACHE_LIMIT = 12;
const psdLeafCompositeCache = new WeakMap();
const clipLayerDecodeCache = new Map();
const CLIP_LAYER_DECODE_CACHE_LIMIT = 12;
const clipVectorDecodeCache = new Map();
const CLIP_VECTOR_DECODE_CACHE_LIMIT = 32;
const clipMaskDecodeCache = new Map();
const CLIP_MASK_DECODE_CACHE_LIMIT = 12;
const psdNodeRenderCache = new WeakMap();
const clipNodeRenderCache = new WeakMap();
const MAX_SOURCE_BYTES = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRY_BYTES = 1024 * 1024 * 1024;
const MAX_ARCHIVE_TOTAL_BYTES = 4 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES = 10000;
const MAX_IMAGE_PIXELS = 300 * 1000 * 1000;
const MAX_CLIP_DATABASE_BYTES = 1536 * 1024 * 1024;

const PSD_BLEND_LABELS = {
  "pass through": "통과",
  normal: "표준",
  dissolve: "디졸브",
  darken: "어둡게",
  multiply: "곱하기",
  "color burn": "색상 번",
  "linear burn": "선형 번",
  "darker color": "어두운 색상",
  lighten: "밝게",
  screen: "스크린",
  "color dodge": "색상 닷지",
  "linear dodge": "선형 닷지(추가)",
  "lighter color": "밝은 색상",
  overlay: "오버레이",
  "soft light": "소프트 라이트",
  "hard light": "하드 라이트",
  "vivid light": "선명한 라이트",
  "linear light": "선형 라이트",
  "pin light": "핀 라이트",
  "hard mix": "하드 혼합",
  difference: "차이",
  exclusion: "제외",
  subtract: "빼기",
  divide: "나누기",
  hue: "색조",
  saturation: "채도",
  color: "색상",
  luminosity: "광도",
};

const SHARP_BLEND_MODES = {
  normal: "over",
  "pass through": "over",
  darken: "darken",
  multiply: "multiply",
  "color burn": "colour-burn",
  lighten: "lighten",
  screen: "screen",
  "color dodge": "colour-dodge",
  "linear dodge": "add",
  overlay: "overlay",
  "soft light": "soft-light",
  "hard light": "hard-light",
  difference: "difference",
  exclusion: "exclusion",
};

const PSD_EFFECT_LABELS = {
  dropShadow: "그림자",
  innerShadow: "내부 그림자",
  outerGlow: "외부 광선",
  innerGlow: "내부 광선",
  bevel: "경사와 엠보스",
  solidFill: "색상 오버레이",
  satin: "새틴",
  stroke: "획",
  gradientOverlay: "그레이디언트 오버레이",
  patternOverlay: "패턴 오버레이",
};

function sharp(...args) {
  return require("sharp")(...args);
}

function assertSourceSize(filePath) {
  const stat = fs.statSync(filePath);
  if (stat.size > MAX_SOURCE_BYTES) {
    throw new Error("2GB를 넘는 파일은 안전을 위해 열 수 없습니다.");
  }
  return stat;
}

function readFileChecked(filePath) {
  assertSourceSize(filePath);
  return fs.readFileSync(filePath);
}

function assertPsdDimensions(buffer) {
  if (buffer.length < 26 || buffer.toString("ascii", 0, 4) !== "8BPS") return;
  const height = buffer.readUInt32BE(14);
  const width = buffer.readUInt32BE(18);
  if (!width || !height || width * height > MAX_IMAGE_PIXELS) {
    throw new Error("PSD 이미지 크기가 안전 제한을 초과합니다.");
  }
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

async function renderRaw(item, buffer) {
  let rawBuffer;
  try {
    rawBuffer = await renderRawWithLightdrift(item, buffer);
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
  assertSourceSize(archivePath);
  const zip = new AdmZip(archivePath);
  const entries = zip.getEntries();
  if (entries.length > MAX_ARCHIVE_ENTRIES) {
    throw new Error("압축 파일의 항목 수가 안전 제한을 초과합니다.");
  }
  const totalSize = entries.reduce((sum, entry) => sum + Number(entry.header?.size || 0), 0);
  if (totalSize > MAX_ARCHIVE_TOTAL_BYTES) {
    throw new Error("압축 해제 예상 크기가 안전 제한을 초과합니다.");
  }
  return entries
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
  if (length > MAX_CLIP_DATABASE_BYTES) {
    throw new Error("CLIP 미리보기 데이터가 안전 제한을 초과합니다.");
  }
  return buffer.subarray(offset, offset + length);
}

function readSqliteDatabaseFromFile(filePath) {
  const signature = Buffer.from("SQLite format 3\0", "binary");
  const stat = assertSourceSize(filePath);
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
    if (length > MAX_CLIP_DATABASE_BYTES) {
      throw new Error("CLIP 미리보기 데이터가 안전 제한을 초과합니다.");
    }
    const database = Buffer.allocUnsafe(length);
    let read = 0;
    while (read < length) {
      const count = fs.readSync(
        descriptor,
        database,
        read,
        length - read,
        databaseOffset + read,
      );
      if (!count) throw new Error("CLIP 미리보기 데이터를 끝까지 읽지 못했습니다.");
      read += count;
    }
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

function clipBlendMode(value) {
  const numeric = numberOrZero(value);
  const labels = [
    "표준", "어둡게", "곱하기", "색상 번", "선형 번", "빼기",
    "밝게", "스크린", "색상 닷지", "발광 닷지", "더하기", "더하기(발광)",
    "오버레이", "소프트 라이트", "하드 라이트", "차이", "선명한 라이트",
    "선형 라이트", "핀 라이트", "하드 혼합", "제외", "어두운 색상",
    "밝은 색상", "나누기", "색조", "채도", "색상", "명도",
  ];
  return {
    value: numeric === 0 ? "normal" : `clip-${numeric}`,
    label: labels[numeric] || `CLIP 합성 모드 ${numeric}`,
  };
}

function clipLayerType(row, hasChildren, hintedType = "") {
  if (hasChildren) return "group";
  if (numberOrZero(row.TextLayerType) || row.TextLayerString?.length) return "text";
  if (numberOrZero(row.LayerType) === 1) return "raster";
  if (numberOrZero(row.LayerType) === 1584) return "paper";
  if (row.GradationFillInfo?.length) return "gradient";
  if (hintedType) return hintedType;
  if (
    numberOrZero(row.LayerEffectAttached) ||
    row.FilterLayerInfo?.length ||
    row.FilterLayerV132?.length
  ) {
    return "effect";
  }
  const types = {
    800: "special",
  };
  if (numberOrZero(row.ComicFrameLineMipmap)) return "border";
  if (numberOrZero(row.ResizableOriginalMipmap)) return "vector";
  return types[numberOrZero(row.LayerType)] || "layer";
}

function queryClipRows(db, sql) {
  const statement = db.prepare(sql);
  const rows = [];
  try {
    while (statement.step()) rows.push(statement.getAsObject());
  } finally {
    statement.free();
  }
  return rows;
}

function extractClipLayerTypeHints(db) {
  const hints = new Map();
  const priorities = { effect: 1, border: 2, tone: 3, "3d": 4, vector: 5 };
  let tableRows = [];
  try {
    const statement = db.prepare(
      "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
    );
    try {
      while (statement.step()) tableRows.push(statement.getAsObject());
    } finally {
      statement.free();
    }
  } catch {
    return hints;
  }
  for (const row of tableRows) {
    const table = String(row.name || "");
    if (!/^[A-Za-z0-9_]+$/.test(table)) continue;
    let type = "";
    if (/3d/i.test(table)) type = "3d";
    else if (/tone/i.test(table)) type = "tone";
    else if (/comic|frame/i.test(table)) type = "border";
    else if (/vector/i.test(table)) type = "vector";
    else if (/effect|filter/i.test(table)) type = "effect";
    if (!type) continue;
    try {
      const columns = queryClipRows(db, `PRAGMA table_info("${table}")`)
        .map((column) => String(column.name || ""));
      const layerColumn = columns.find((column) => /^LayerId$/i.test(column));
      if (!layerColumn) continue;
      for (const linked of queryClipRows(
        db,
        `SELECT "${layerColumn}" AS LayerId FROM "${table}"`,
      )) {
        const id = numberOrZero(linked.LayerId);
        const existing = hints.get(id);
        if (id && (!existing || priorities[type] > priorities[existing])) {
          hints.set(id, type);
        }
      }
    } catch {
      // Optional feature tables vary between CLIP format versions.
    }
  }
  return hints;
}

function decodeClipText(value) {
  if (!value?.length) return "";
  return Buffer.from(value).toString("utf8").replace(/\0/g, "").trim();
}

function parseClipTextAttributes(value) {
  const data = value?.length ? Buffer.from(value) : null;
  if (!data) return null;
  const result = {};
  let offset = 0;
  while (offset + 8 <= data.length) {
    const id = data.readUInt32LE(offset);
    const length = data.readUInt32LE(offset + 4);
    offset += 8;
    if (length > data.length - offset) break;
    if (id === 31) result.font = data.subarray(offset, offset + length).toString("utf8");
    if (id === 32 && length >= 4) result.fontSize = data.readUInt32LE(offset);
    if (id === 34 && length >= 12) {
      result.color = [0, 4, 8].map((delta) => data.readUInt32LE(offset + delta));
    }
    if (id === 42 && length >= 16) {
      result.bounds = [0, 4, 8, 12].map((delta) => data.readInt32LE(offset + delta));
    }
    offset += length;
  }
  return Object.keys(result).length ? result : null;
}

function parseClipGradient(value) {
  const data = value?.length ? Buffer.from(value) : null;
  if (!data) return null;
  let offset = 0;
  const readInt = () => {
    if (offset + 4 > data.length) throw new Error("CLIP 그레이디언트 데이터가 손상되었습니다.");
    const result = data.readUInt32BE(offset);
    offset += 4;
    return result;
  };
  const readDouble = () => {
    if (offset + 8 > data.length) throw new Error("CLIP 그레이디언트 좌표가 손상되었습니다.");
    const result = data.readDoubleBE(offset);
    offset += 8;
    return result;
  };
  const readName = () => {
    if (offset + 4 > data.length) return null;
    const length = readInt();
    const byteLength = length * 2;
    if (offset + byteLength > data.length) {
      throw new Error("CLIP 그레이디언트 항목 이름이 손상되었습니다.");
    }
    const result = Buffer.from(data.subarray(offset, offset + byteLength))
      .swap16()
      .toString("utf16le");
    offset += byteLength;
    return result;
  };
  try {
    readInt();
    readInt();
    let stops = null;
    let geometry = null;
    let flatColor = null;
    while (offset < data.length) {
      const name = readName();
      if (!name) break;
      if (name === "GradationData") {
        const sectionSize = readInt();
        const sectionEnd = offset + sectionSize;
        readInt();
        readInt();
        const count = readInt();
        readInt();
        if (count > 256 || sectionEnd > data.length) break;
        if (sectionEnd - offset === 8 + count * 28) {
          readInt();
          readInt();
        }
        stops = [];
        for (let index = 0; index < count; index += 1) {
          const color = [readInt() >>> 24, readInt() >>> 24, readInt() >>> 24];
          const opacity = readInt() >>> 24;
          readInt();
          const position = Math.max(0, Math.min(1, readInt() / 32768));
          const curveCount = readInt();
          const curveBytes = curveCount * 16;
          if (curveCount > 4096 || offset + curveBytes > sectionEnd) break;
          offset += curveBytes;
          stops.push({ color, opacity, position });
        }
        offset = Math.max(offset, sectionEnd);
      } else if (name === "GradationSettingAdd0001") {
        const sectionSize = readInt();
        const sectionEnd = offset + sectionSize;
        if (readInt()) flatColor = [readInt() >>> 24, readInt() >>> 24, readInt() >>> 24];
        offset = Math.max(offset, sectionEnd);
      } else if (name === "GradationSetting") {
        geometry = {
          repeat: readInt(),
          shape: readInt(),
          antiAlias: Boolean(readInt()),
        };
        readDouble();
        geometry.ellipseDiameter = readDouble();
        readDouble();
        geometry.startX = readDouble();
        geometry.startY = readDouble();
        geometry.endX = readDouble();
        geometry.endY = readDouble();
      } else {
        if (offset + 4 > data.length) break;
        const size = readInt();
        if (size > data.length - offset) break;
        offset += size;
      }
    }
    if (flatColor) return { flatColor };
    if (stops?.length && geometry) return { stops, geometry };
  } catch {
    return null;
  }
  return null;
}

function clipColorChannel(value) {
  const numeric = numberOrZero(value);
  return Math.max(0, Math.min(255, Math.round(numeric / 0xffffffff * 255)));
}

function clipLayerEffects(row) {
  const effects = [];
  if (numberOrZero(row.LayerEffectAttached) || row.LayerEffectInfo?.length) {
    effects.push("테두리/레이어 효과");
  }
  const filterData = row.FilterLayerInfo?.length
    ? Buffer.from(row.FilterLayerInfo)
    : row.FilterLayerV132?.length
      ? Buffer.from(row.FilterLayerV132)
      : null;
  if (filterData) {
    const filterNames = {
      1: "밝기/대비",
      2: "레벨 보정",
      3: "톤 커브",
      4: "색조/채도/명도",
      5: "컬러 밸런스",
      6: "계조 반전",
      9: "그레이디언트 맵",
    };
    const filterId = filterData.length >= 4 ? filterData.readUInt32BE(0) : 0;
    effects.push(filterNames[filterId] || "필터 효과");
  }
  return effects;
}

function clipLayerDefinition(row, nested, hintedType = "") {
  const blend = clipBlendMode(row.LayerComposite);
  const type = clipLayerType(row, nested.length > 0, hintedType);
  const drawColor = [
    clipColorChannel(row.DrawColorMainRed),
    clipColorChannel(row.DrawColorMainGreen),
    clipColorChannel(row.DrawColorMainBlue),
  ];
  return {
    id: String(numberOrZero(row.MainId)),
    name: String(row.LayerName || "").trim() || (nested.length ? "그룹" : "이름 없는 레이어"),
    type,
    typeLabel: {
      group: "그룹",
      raster: "래스터",
      vector: "벡터/오브젝트",
      text: "텍스트",
      border: "테두리",
      tone: "톤",
      "3d": "3D",
      gradient: "그레이디언트/채우기",
      effect: "효과",
      paper: "용지",
      special: "CSP 전용",
      layer: "CSP 레이어",
    }[type] || type,
    visible: numberOrZero(row.LayerVisibility) !== 0,
    opacity: Math.round(Math.max(0, Math.min(1, numberOrZero(row.LayerOpacity) / 256)) * 100),
    blendMode: blend.value,
    blendModeLabel: blend.label,
    clipping: numberOrZero(row.LayerClip) !== 0,
    mask: Boolean(
      numberOrZero(row.LayerLayerMaskMipmap) ||
      numberOrZero(row.LayerLayerMaskThumbnail),
    ),
    effects: clipLayerEffects(row),
    textPreview: decodeClipText(row.TextLayerString),
    textAttributes: parseClipTextAttributes(row.TextLayerAttributes),
    gradient: parseClipGradient(row.GradationFillInfo),
    drawColor,
    paperColor: type === "paper" ? drawColor : null,
    children: nested,
  };
}

function extractClipLayerDocument(db, canvasRow) {
  const tableStatement = db.prepare(
    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='Layer' LIMIT 1",
  );
  try {
    if (!tableStatement.step()) return null;
  } finally {
    tableStatement.free();
  }

  const statement = db.prepare("SELECT * FROM Layer ORDER BY _PW_ID");
  const rows = [];
  try {
    while (statement.step()) rows.push(statement.getAsObject());
  } finally {
    statement.free();
  }
  if (!rows.length) return null;

  const typeHints = extractClipLayerTypeHints(db);
  const byId = new Map(rows.map((row) => [numberOrZero(row.MainId), row]));
  const rootId = numberOrZero(canvasRow?.CanvasRootFolder);
  const root = byId.get(rootId);
  const visited = new Set();
  const buildChildren = (parent) => {
    const children = [];
    let childId = numberOrZero(parent?.LayerFirstChildIndex);
    while (childId && !visited.has(childId)) {
      visited.add(childId);
      const row = byId.get(childId);
      if (!row) break;
      const nested = buildChildren(row);
      children.push(clipLayerDefinition(
        row,
        nested,
        typeHints.get(numberOrZero(row.MainId)),
      ));
      childId = numberOrZero(row.LayerNextIndex);
    }
    return children.reverse();
  };

  let layers = root ? buildChildren(root) : [];
  if (!layers.length) {
    layers = rows
      .filter((row) => numberOrZero(row.MainId) !== rootId)
      .map((row) => clipLayerDefinition(
        row,
        [],
        typeHints.get(numberOrZero(row.MainId)),
      ))
      .reverse();
  }

  return {
    format: "CLIP",
    width: 0,
    height: 0,
    nativeWidth: 0,
    nativeHeight: 0,
    resolution: numberOrZero(canvasRow?.CanvasResolution) || 72,
    toggleSupported: false,
    pickSupported: false,
    thumbnailSupported: false,
    approximateRendering: true,
    note: "",
    layers,
  };
}

function applyClipLayerAvailability(layerDocument, rasterSources, vectorSources) {
  if (!layerDocument) return;
  let available = 0;
  let toggleable = 0;
  const visit = (layers) => {
    for (const layer of layers || []) {
      if (layer.type !== "group" && !layer.children?.length) {
        layer.rasterAvailable = Boolean(rasterSources[layer.id]?.length);
        layer.vectorAvailable = Boolean(vectorSources[layer.id]?.length);
        layer.thumbnailAvailable = true;
        layer.toggleAvailable = layer.rasterAvailable ||
          layer.vectorAvailable ||
          layer.type === "paper" ||
          layer.type === "text" ||
          Boolean(layer.gradient);
        layer.previewAccuracy = layer.rasterAvailable
          ? layer.visible && layer.effects?.length
            ? "원본 합성 문맥 래스터(정확)"
            : "저장 레이어 래스터(정확)"
          : layer.visible && (layer.vectorAvailable || layer.type === "text")
            ? "원본 합성 문맥 래스터(정확)"
            : layer.vectorAvailable
              ? "벡터 경로 재구성(근사)"
            : layer.type === "paper"
              ? "용지 색상"
              : layer.gradient
                ? "그레이디언트 재구성(근사)"
                : layer.visible
                  ? "원본 합성 문맥 래스터(정확)"
                  : "레이어 메타데이터";
        available += 1;
        if (layer.toggleAvailable) toggleable += 1;
      }
      visit(layer.children);
      if (layer.type === "group" || layer.children?.length) {
        layer.toggleAvailable = layer.children.some((child) => child.toggleAvailable);
      }
    }
  };
  visit(layerDocument.layers);
  layerDocument.thumbnailSupported = available > 0;
  layerDocument.toggleSupported = toggleable > 0;
  layerDocument.note = toggleable
    ? "레이어 목록은 원본 구조를 유지하며, CSP 전용 표현은 저장된 합성본에서 비파괴 문맥 래스터로 표시합니다. ON/OFF 재합성은 지원 데이터 범위에서 처리합니다."
    : "레이어 구조와 종류별 미리보기만 제공합니다. 이 파일에는 독립 합성 가능한 레이어 데이터가 없습니다.";
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
      let canvasRow = null;
      const tableStatement = db.prepare(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='Canvas' LIMIT 1",
      );
      try {
        if (tableStatement.step()) {
          const canvasStatement = db.prepare("SELECT * FROM Canvas LIMIT 1");
          try {
            if (canvasStatement.step()) {
              canvasRow = canvasStatement.getAsObject();
              crop = canvasCrop(canvasRow);
            }
          } finally {
            canvasStatement.free();
          }
        }
      } finally {
        tableStatement.free();
      }
      const layerDocument = extractClipLayerDocument(db, canvasRow);
      const rasterSources = extractClipRasterSources(db);
      const maskSources = extractClipMaskSources(db);
      const vectorSources = extractClipVectorSources(db);
      applyClipLayerAvailability(layerDocument, rasterSources, vectorSources);
      const preview = Buffer.from(image);
      const previewMetadata = await imageMetadata(preview);
      const rasterLevels = Object.values(rasterSources).flat();
      const largestLevel = rasterLevels.reduce((largest, level) => (
        !largest || level.width * level.height > largest.width * largest.height
          ? level
          : largest
      ), null);
      if (layerDocument) {
        layerDocument.width = previewMetadata.width || largestLevel?.width || 0;
        layerDocument.height = previewMetadata.height || largestLevel?.height || 0;
        layerDocument.nativeWidth = largestLevel?.width || layerDocument.width;
        layerDocument.nativeHeight = largestLevel?.height || layerDocument.height;
      }
      return {
        preview,
        crop,
        layerDocument,
        rasterSources,
        maskSources,
        vectorSources,
        source: input,
      };
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

function readAgPsd(buffer, includeLayers = true) {
  assertPsdDimensions(buffer);
  const agPsd = require("ag-psd");
  initializeAgPsd(agPsd);
  return agPsd.readPsd(buffer, {
    skipLayerImageData: !includeLayers,
    skipThumbnail: true,
    useImageData: true,
    logMissingFeatures: false,
  });
}

function renderAgPsdComposite(psd) {
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

function cacheIdentity(item) {
  if (item.kind === "archive-entry") {
    const stat = fs.statSync(item.archivePath);
    return `${path.resolve(item.archivePath)}::${item.entryName}:${stat.size}:${stat.mtimeMs}`;
  }
  const resolvedPath = path.resolve(item.path);
  const stat = fs.statSync(resolvedPath);
  return `${resolvedPath}:${stat.size}:${stat.mtimeMs}`;
}

function effectNames(layer) {
  if (!layer.effects || layer.effects.disabled) return [];
  return Object.entries(PSD_EFFECT_LABELS)
    .filter(([key]) => {
      const value = layer.effects[key];
      return Array.isArray(value) ? value.length > 0 : Boolean(value);
    })
    .map(([, label]) => label);
}

function psdLayerTree(layers, parentId = "") {
  return (layers || []).map((layer, index) => {
    const id = parentId ? `${parentId}.${index}` : String(index);
    const children = psdLayerTree(layer.children, id);
    const contextRaster = !layer.hidden && psdLayerNeedsContextRaster(layer);
    return {
      id,
      name: layer.name || (children.length ? "그룹" : "이름 없는 레이어"),
      type: children.length ? "group" : "layer",
      visible: !layer.hidden,
      opacity: Math.round((layer.opacity ?? 1) * 100),
      blendMode: layer.blendMode || "normal",
      blendModeLabel: PSD_BLEND_LABELS[layer.blendMode] || layer.blendMode || "표준",
      clipping: Boolean(layer.clipping),
      mask: Boolean(layer.mask || layer.vectorMask),
      effects: effectNames(layer),
      previewAccuracy: children.length
        ? "레이어 그룹"
        : contextRaster
          ? "원본 합성 문맥 래스터(정확)"
          : "저장 레이어 래스터(정확)",
      children,
    };
  }).reverse();
}

function psdLayerDocument(psd) {
  return {
    format: "PSD",
    width: psd.width,
    height: psd.height,
    toggleSupported: true,
    pickSupported: true,
    thumbnailSupported: true,
    approximateRendering: true,
    note: "레이어 ON/OFF 합성은 지원합니다. 일부 고급 효과와 특수 혼합은 원본 합성과 다를 수 있습니다.",
    layers: psdLayerTree(psd.children),
  };
}

async function getPsdDocument(item, input = null) {
  const key = cacheIdentity(item);
  if (psdDocumentCache.has(key)) return psdDocumentCache.get(key);
  const identityPrefix = item.kind === "archive-entry"
    ? `${path.resolve(item.archivePath)}::${item.entryName}:`
    : `${path.resolve(item.path)}:`;
  for (const existingKey of psdDocumentCache.keys()) {
    if (existingKey.startsWith(identityPrefix)) psdDocumentCache.delete(existingKey);
  }
  const source = input || (item.kind === "archive-entry"
    ? readArchiveEntry(item)
    : fs.readFileSync(item.path));
  const promise = Promise.resolve().then(() => {
    const psd = readAgPsd(source, true);
    return { psd, layerDocument: psdLayerDocument(psd) };
  }).catch((error) => {
    psdDocumentCache.delete(key);
    throw error;
  });
  psdDocumentCache.set(key, promise);
  while (psdDocumentCache.size > PSD_DOCUMENT_CACHE_LIMIT) {
    psdDocumentCache.delete(psdDocumentCache.keys().next().value);
  }
  return promise;
}

async function renderPsd(buffer, ext, item) {
  if (ext === ".psb") {
    return renderWebtoonPsd(buffer);
  }

  try {
    const document = await getPsdDocument(item, buffer);
    return await psdCompositeBuffer(document);
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
  assertSourceSize(item.archivePath);
  const zip = new AdmZip(item.archivePath);
  const entry = zip.getEntry(item.entryName);
  if (!entry) throw new Error("압축 파일 안의 이미지를 찾지 못했습니다.");
  const size = Number(entry.header?.size || 0);
  const compressedSize = Number(entry.header?.compressedSize || 0);
  if (
    size > MAX_ARCHIVE_ENTRY_BYTES ||
    (compressedSize > 0 && size / compressedSize > 500)
  ) {
    throw new Error("압축 항목의 해제 크기 또는 압축률이 안전 제한을 초과합니다.");
  }
  return entry.getData();
}

async function cachedClipData(item, input) {
  const cacheKey = cacheIdentity(item);
  if (clipSourceCache.has(cacheKey)) return clipSourceCache.get(cacheKey);
  const identityPrefix = item.kind === "archive-entry"
    ? `${path.resolve(item.archivePath)}::${item.entryName}:`
    : `${path.resolve(item.path)}:`;
  for (const existingKey of clipSourceCache.keys()) {
    if (existingKey.startsWith(identityPrefix)) {
      clipSourceCache.delete(existingKey);
      clipSourceCacheSizes.delete(existingKey);
    }
  }
  const source = input || (item.kind === "archive-entry"
    ? readArchiveEntry(item)
    : item.path);
  const promise = extractClipData(source).catch((error) => {
    clipSourceCache.delete(cacheKey);
    clipSourceCacheSizes.delete(cacheKey);
    throw error;
  });
  clipSourceCache.set(cacheKey, promise);
  clipSourceCacheSizes.set(cacheKey, Buffer.isBuffer(source) ? source.length : 0);
  const cachedBytes = () => [...clipSourceCacheSizes.values()]
    .reduce((total, size) => total + size, 0);
  while (
    clipSourceCache.size > CLIP_SOURCE_CACHE_LIMIT ||
    (clipSourceCache.size > 1 && cachedBytes() > MAX_CLIP_SOURCE_CACHE_BYTES)
  ) {
    const oldest = clipSourceCache.keys().next().value;
    clipSourceCache.delete(oldest);
    clipSourceCacheSizes.delete(oldest);
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
      layerDocument: clip.layerDocument,
    };
  }

  const input = item.kind === "archive-entry"
    ? readArchiveEntry(item)
    : readFileChecked(item.path);

  if (PSD_EXTENSIONS.has(ext)) {
    let document = null;
    if (ext === ".psd") {
      try {
        document = await getPsdDocument(item, input);
      } catch (error) {
        if (process.env.CLIPVIEW_DEBUG_PSD === "1") {
          console.error(`PSD layer parsing failed: ${error?.stack || error}`);
        }
        // The flattened-image fallback below can still open some unsupported PSD variants.
      }
    }
    return {
      buffer: await renderPsd(input, ext, item),
      mime: "image/png",
      source: "PSD 합성 이미지",
      layerDocument: document?.layerDocument || null,
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
    layerDocument: null,
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
  const right = Math.min(crop.canvasWidth, x + targetWidth);
  const bottom = Math.min(crop.canvasHeight, y + targetHeight);
  const clippedX = Math.max(0, x);
  const clippedY = Math.max(0, y);
  const left = Math.round(clippedX / crop.canvasWidth * width);
  const top = Math.round(clippedY / crop.canvasHeight * height);
  const cropWidth = Math.round((right - clippedX) / crop.canvasWidth * width);
  const cropHeight = Math.round((bottom - clippedY) / crop.canvasHeight * height);
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
      pageHeight: metadata.pageHeight || metadata.height || fallback.height || 0,
      density: metadata.density || 0,
      hasAlpha: Boolean(metadata.hasAlpha),
    };
  } catch {
    return fallback;
  }
}

function layerVisibility(layer, id, visibility) {
  if (visibility && Object.prototype.hasOwnProperty.call(visibility, id)) {
    return Boolean(visibility[id]);
  }
  return !layer.hidden;
}

function rgbaBytes(imageData) {
  if (!imageData?.data || !imageData.width || !imageData.height) return null;
  const source = imageData.data;
  if (source.BYTES_PER_ELEMENT === 1) {
    return Buffer.from(source.buffer, source.byteOffset, source.byteLength);
  }
  const output = Buffer.alloc(imageData.width * imageData.height * 4);
  const maximum = source.BYTES_PER_ELEMENT === 2 ? 65535 : 1;
  for (let index = 0; index < output.length; index += 1) {
    output[index] = Math.max(0, Math.min(255, Math.round(source[index] / maximum * 255)));
  }
  return output;
}

function maskValue(mask, documentX, documentY) {
  if (!mask || mask.disabled || !mask.imageData?.data) return 255;
  const left = numberOrZero(mask.left);
  const top = numberOrZero(mask.top);
  const x = Math.floor(documentX - left);
  const y = Math.floor(documentY - top);
  const { width, height, data } = mask.imageData;
  if (x < 0 || y < 0 || x >= width || y >= height) {
    return numberOrZero(mask.defaultColor) ? 255 : 0;
  }
  const pixelCount = width * height;
  const channels = Math.max(1, Math.round(data.length / pixelCount));
  const value = data[(y * width + x) * channels];
  if (data.BYTES_PER_ELEMENT === 2) return Math.round(value / 65535 * 255);
  if (data instanceof Float32Array) return Math.round(Math.max(0, Math.min(1, value)) * 255);
  return value;
}

function psdLayerById(psd, id) {
  let layers = psd.children || [];
  let layer = null;
  for (const part of String(id).split(".")) {
    const index = Number.parseInt(part, 10);
    if (!Number.isInteger(index) || index < 0 || index >= layers.length) return null;
    layer = layers[index];
    layers = layer.children || [];
  }
  return layer;
}

function clipLayerById(layers, id) {
  for (const layer of layers || []) {
    if (layer.id === String(id)) return layer;
    const nested = clipLayerById(layer.children, id);
    if (nested) return nested;
  }
  return null;
}

function escapeXml(value) {
  return String(value || "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&apos;");
}

function cachedClipVector(item, clip, layer) {
  const key = `${cacheIdentity(item)}:${layer.id}`;
  if (clipVectorDecodeCache.has(key)) return clipVectorDecodeCache.get(key);
  const decoded = decodeClipVector(
    clip.source,
    clip.vectorSources?.[layer.id] || [],
    clip.layerDocument.nativeWidth,
    clip.layerDocument.nativeHeight,
  );
  clipVectorDecodeCache.set(key, decoded);
  while (clipVectorDecodeCache.size > CLIP_VECTOR_DECODE_CACHE_LIMIT) {
    clipVectorDecodeCache.delete(clipVectorDecodeCache.keys().next().value);
  }
  return decoded;
}

function clipVectorSvg(
  layer,
  decoded,
  width,
  height,
  thumbnail = false,
  nativeWidth = width,
  nativeHeight = height,
) {
  const bounds = decoded.bounds;
  const padding = thumbnail ? 5 : 0;
  const sourceWidth = Math.max(1, bounds.right - bounds.left);
  const sourceHeight = Math.max(1, bounds.bottom - bounds.top);
  const scale = thumbnail
    ? Math.min((width - padding * 2) / sourceWidth, (height - padding * 2) / sourceHeight)
    : Math.min(
        width / Math.max(1, nativeWidth),
        height / Math.max(1, nativeHeight),
      );
  const offsetX = thumbnail
    ? (width - sourceWidth * scale) / 2 - bounds.left * scale
    : 0;
  const offsetY = thumbnail
    ? (height - sourceHeight * scale) / 2 - bounds.top * scale
    : 0;
  const color = layer.drawColor?.some((channel) => channel > 0)
    ? layer.drawColor
    : [28, 32, 38];
  const thumbnailBackground =
    (color[0] * 0.299 + color[1] * 0.587 + color[2] * 0.114) > 210
      ? "#343a45"
      : "#f3f5f8";
  const paths = decoded.segments.map((segment) => {
    const points = segment.map((point) => (
      `${point.x * scale + offsetX},${point.y * scale + offsetY}`
    )).join(" ");
    const widthValues = segment.map((point) => point.width).sort((a, b) => a - b);
    const strokeWidth = Math.max(
      thumbnail ? 1.2 : 0.5,
      widthValues[Math.floor(widthValues.length / 2)] * scale,
    );
    const outline = layer.effects?.length
      ? `<polyline points="${points}" fill="none" stroke="#ffffff"
          stroke-width="${strokeWidth + Math.max(1.5, scale * 3)}"
          stroke-linecap="round" stroke-linejoin="round"/>`
      : "";
    return `${outline}<polyline points="${points}" fill="none"
      stroke="rgb(${color.join(",")})" stroke-width="${strokeWidth}"
      stroke-linecap="round" stroke-linejoin="round"/>`;
  }).join("");
  return Buffer.from(`
    <svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">
      ${thumbnail ? `<rect width="${width}" height="${height}" fill="${thumbnailBackground}"/>` : ""}
      ${paths}
    </svg>
  `);
}

async function clipMetadataThumbnail(layer, size, preview) {
  const color = layer.paperColor || [39, 45, 55];
  const text = layer.type === "text"
    ? (layer.textPreview || layer.name)
    : layer.type === "effect" && layer.effects?.length
      ? layer.effects[0]
      : (layer.typeLabel || layer.type || "CLIP");
  const shortText = [...String(text)].slice(0, 16).join("");
  const fontSize = layer.type === "text" ? 10 : 9;
  const background = layer.type === "paper"
    ? `rgb(${color[0]},${color[1]},${color[2]})`
    : "#272d37";
  const foreground = layer.type === "paper" &&
    (color[0] * 0.299 + color[1] * 0.587 + color[2] * 0.114) > 150
    ? "#20242b"
    : "#f3f6fb";
  const effectBadge = layer.effects?.length
    ? `<circle cx="${size - 10}" cy="10" r="8" fill="#6ca8ff"/>
       <text x="${size - 10}" y="13" text-anchor="middle" font-size="8"
         font-family="Segoe UI, sans-serif" font-weight="700" fill="#10151d">FX</text>`
    : "";
  if (preview && layer.visible && !["paper", "text"].includes(layer.type)) {
    const overlay = Buffer.from(`
      <svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}">
        <rect width="${size}" height="${size}" fill="#111722" fill-opacity="0.3"/>
        <rect x="3" y="${size - 18}" width="${size - 6}" height="15" rx="4"
          fill="#171d27" fill-opacity="0.9"/>
        <text x="${size / 2}" y="${size - 7}" text-anchor="middle"
          font-family="Segoe UI, Malgun Gothic, sans-serif" font-size="8"
          font-weight="700" fill="#f3f6fb">${escapeXml(shortText)}</text>
        ${effectBadge}
      </svg>
    `);
    return sharp(preview)
      .resize(size, size, { fit: "cover", position: "centre" })
      .modulate({ brightness: 0.82, saturation: 0.55 })
      .composite([{ input: overlay }])
      .png({ compressionLevel: 7 })
      .toBuffer();
  }
  const svg = Buffer.from(`
    <svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}">
      <defs>
        <pattern id="grid" width="8" height="8" patternUnits="userSpaceOnUse">
          <rect width="8" height="8" fill="#e9edf3"/>
          <rect width="4" height="4" fill="#cfd5df"/>
          <rect x="4" y="4" width="4" height="4" fill="#cfd5df"/>
        </pattern>
      </defs>
      <rect width="${size}" height="${size}" fill="url(#grid)"/>
      <rect x="3" y="3" width="${size - 6}" height="${size - 6}" rx="5"
        fill="${background}" fill-opacity="${layer.type === "paper" ? 1 : 0.94}"/>
      <text x="${size / 2}" y="${size / 2 + 3}" text-anchor="middle"
        font-family="Segoe UI, Malgun Gothic, sans-serif" font-size="${fontSize}"
        font-weight="${layer.type === "text" ? 500 : 700}" fill="${foreground}">
        ${escapeXml(shortText)}
      </text>
      ${effectBadge}
    </svg>
  `);
  return sharp(svg).png({ compressionLevel: 7 }).toBuffer();
}

function psdLayerNeedsContextRaster(layer) {
  return Boolean(
    !layer.imageData?.data ||
    layer.text ||
    layer.vectorMask ||
    layer.placedLayer ||
    layer.linkedFile ||
    layer.adjustment ||
    layer.clipping ||
    (layer.effects && !layer.effects.disabled),
  );
}

function psdCompositeBuffer(document) {
  if (!document.composite) {
    document.composite = renderAgPsdComposite(document.psd).catch((error) => {
      document.composite = null;
      throw error;
    });
  }
  return document.composite;
}

function clipContextBounds(layer, vector) {
  if (vector?.bounds) return vector.bounds;
  const bounds = layer.textAttributes?.bounds;
  if (Array.isArray(bounds) && bounds.length >= 4) {
    return {
      left: bounds[0],
      top: bounds[1],
      right: bounds[2],
      bottom: bounds[3],
    };
  }
  return null;
}

function psdContextBounds(layer) {
  const left = numberOrZero(layer.left);
  const top = numberOrZero(layer.top);
  const right = Number.isFinite(layer.right)
    ? layer.right
    : left + numberOrZero(layer.imageData?.width);
  const bottom = Number.isFinite(layer.bottom)
    ? layer.bottom
    : top + numberOrZero(layer.imageData?.height);
  if (right <= left || bottom <= top) return null;
  return { left, top, right, bottom };
}

async function contextRasterThumbnail(
  composite,
  size,
  bounds = null,
  coordinateWidth = 0,
  coordinateHeight = 0,
) {
  const metadata = await sharp(composite).metadata();
  const imageWidth = metadata.width || size;
  const imageHeight = metadata.height || size;
  let pipeline = sharp(composite);
  if (
    bounds &&
    coordinateWidth > 0 &&
    coordinateHeight > 0 &&
    [bounds.left, bounds.top, bounds.right, bounds.bottom].every(Number.isFinite)
  ) {
    const scaleX = imageWidth / coordinateWidth;
    const scaleY = imageHeight / coordinateHeight;
    const left = bounds.left * scaleX;
    const top = bounds.top * scaleY;
    const right = bounds.right * scaleX;
    const bottom = bounds.bottom * scaleY;
    const contentWidth = Math.max(1, right - left);
    const contentHeight = Math.max(1, bottom - top);
    const side = Math.min(
      Math.max(imageWidth, imageHeight),
      Math.max(24, Math.max(contentWidth, contentHeight) * 1.4),
    );
    const cropWidth = Math.max(1, Math.min(imageWidth, Math.round(side)));
    const cropHeight = Math.max(1, Math.min(imageHeight, Math.round(side)));
    const centerX = (left + right) / 2;
    const centerY = (top + bottom) / 2;
    const cropLeft = Math.max(
      0,
      Math.min(imageWidth - cropWidth, Math.round(centerX - cropWidth / 2)),
    );
    const cropTop = Math.max(
      0,
      Math.min(imageHeight - cropHeight, Math.round(centerY - cropHeight / 2)),
    );
    pipeline = pipeline.extract({
      left: cropLeft,
      top: cropTop,
      width: cropWidth,
      height: cropHeight,
    });
  }
  return pipeline
    .resize(size, size, {
      fit: "contain",
      background: { r: 0, g: 0, b: 0, alpha: 0 },
    })
    .png({ compressionLevel: 7 })
    .toBuffer();
}

async function loadLayerThumbnail(item, id, size = 58) {
  const ext = extensionOf(item.kind === "archive-entry" ? item.entryName : item.path);
  if (ext !== ".psd" && !CLIP_EXTENSIONS.has(ext)) return null;
  const thumbnailSize = Math.max(24, Math.min(160, Math.round(Number(size) || 58)));
  const cacheKey = `${cacheIdentity(item)}:${id}:${thumbnailSize}`;
  if (layerThumbnailCache.has(cacheKey)) return layerThumbnailCache.get(cacheKey);

  if (CLIP_EXTENSIONS.has(ext)) {
    const clip = await cachedClipData(item);
    const layer = clipLayerById(clip.layerDocument?.layers, id);
    if (!layer || layer.children?.length) {
      layerThumbnailCache.set(cacheKey, null);
      return null;
    }
    const decoded = decodeClipLayer(
      clip.source,
      clip.rasterSources?.[String(id)] || [],
      thumbnailSize,
    );
    const vector = !decoded && layer.vectorAvailable
      ? cachedClipVector(item, clip, layer)
      : null;
    const contextRaster = layer.visible && Boolean(
      vector ||
      layer.type === "text" ||
      layer.effects?.length ||
      (!decoded && !layer.gradient && layer.type !== "paper"),
    );
    let reconstructed = null;
    if (!decoded && !vector && !contextRaster && layer.gradient) {
      const documentWidth = Math.max(1, clip.layerDocument.width);
      const documentHeight = Math.max(1, clip.layerDocument.height);
      const scale = Math.min(1, 320 / Math.max(documentWidth, documentHeight));
      reconstructed = await clipLeafComposite(
        item,
        clip,
        layer,
        Math.max(1, Math.round(documentWidth * scale)),
        Math.max(1, Math.round(documentHeight * scale)),
      );
    }
    const thumbnail = contextRaster
      ? await contextRasterThumbnail(
          clip.preview,
          thumbnailSize,
          clipContextBounds(layer, vector),
          clip.layerDocument.nativeWidth,
          clip.layerDocument.nativeHeight,
        )
      : decoded
      ? await sharp(decoded.data, {
          raw: { width: decoded.width, height: decoded.height, channels: 4 },
        })
          .resize(thumbnailSize, thumbnailSize, {
            fit: "contain",
            background: { r: 0, g: 0, b: 0, alpha: 0 },
          })
          .png({ compressionLevel: 7 })
          .toBuffer()
      : vector
        ? await sharp(clipVectorSvg(layer, vector, thumbnailSize, thumbnailSize, true))
            .png({ compressionLevel: 7 })
            .toBuffer()
        : reconstructed
          ? await sharp(reconstructed.input)
              .trim({ background: { r: 0, g: 0, b: 0, alpha: 0 } })
              .resize(thumbnailSize, thumbnailSize, {
                fit: "contain",
                background: { r: 0, g: 0, b: 0, alpha: 0 },
              })
              .png({ compressionLevel: 7 })
              .toBuffer()
          : await clipMetadataThumbnail(layer, thumbnailSize, clip.preview);
    const dataUrl = `data:image/png;base64,${thumbnail.toString("base64")}`;
    layerThumbnailCache.set(cacheKey, dataUrl);
    while (layerThumbnailCache.size > LAYER_THUMBNAIL_CACHE_LIMIT) {
      layerThumbnailCache.delete(layerThumbnailCache.keys().next().value);
    }
    return dataUrl;
  }

  const document = await getPsdDocument(item);
  const { psd } = document;
  const layer = psdLayerById(psd, id);
  if (!layer || layer.children?.length) return null;
  if (!layer.hidden && psdLayerNeedsContextRaster(layer)) {
    const thumbnail = await contextRasterThumbnail(
      await psdCompositeBuffer(document),
      thumbnailSize,
      psdContextBounds(layer),
      psd.width,
      psd.height,
    );
    const dataUrl = `data:image/png;base64,${thumbnail.toString("base64")}`;
    layerThumbnailCache.set(cacheKey, dataUrl);
    while (layerThumbnailCache.size > LAYER_THUMBNAIL_CACHE_LIMIT) {
      layerThumbnailCache.delete(layerThumbnailCache.keys().next().value);
    }
    return dataUrl;
  }
  const bytes = rgbaBytes(layer.imageData);
  if (!bytes) return null;

  const width = layer.imageData.width;
  const height = layer.imageData.height;
  const left = numberOrZero(layer.left);
  const top = numberOrZero(layer.top);
  const output = Buffer.from(bytes);
  if (layer.mask && !layer.mask.disabled) {
    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const alphaIndex = (y * width + x) * 4 + 3;
        output[alphaIndex] = Math.round(
          output[alphaIndex] * maskValue(layer.mask, left + x, top + y) / 255,
        );
      }
    }
  }

  const thumbnail = await sharp(output, { raw: { width, height, channels: 4 } })
    .resize(thumbnailSize, thumbnailSize, {
      fit: "contain",
      background: { r: 0, g: 0, b: 0, alpha: 0 },
    })
    .png({ compressionLevel: 7 })
    .toBuffer();
  const dataUrl = `data:image/png;base64,${thumbnail.toString("base64")}`;
  layerThumbnailCache.set(cacheKey, dataUrl);
  while (layerThumbnailCache.size > LAYER_THUMBNAIL_CACHE_LIMIT) {
    layerThumbnailCache.delete(layerThumbnailCache.keys().next().value);
  }
  return dataUrl;
}

async function psdLeafComposite(layer, documentWidth, documentHeight, opacity = 1) {
  const bytes = rgbaBytes(layer.imageData);
  if (!bytes) return null;
  const width = layer.imageData.width;
  const height = layer.imageData.height;
  const left = numberOrZero(layer.left);
  const top = numberOrZero(layer.top);
  const effectiveOpacity = Math.max(0, Math.min(1, (layer.opacity ?? 1) * opacity));
  const output = Buffer.from(bytes);
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const alphaIndex = (y * width + x) * 4 + 3;
      const mask = maskValue(layer.mask, left + x, top + y);
      output[alphaIndex] = Math.round(output[alphaIndex] * effectiveOpacity * mask / 255);
    }
  }

  const outputLeft = Math.max(0, left);
  const outputTop = Math.max(0, top);
  const sourceLeft = Math.max(0, -left);
  const sourceTop = Math.max(0, -top);
  const outputWidth = Math.min(width - sourceLeft, documentWidth - outputLeft);
  const outputHeight = Math.min(height - sourceTop, documentHeight - outputTop);
  if (outputWidth <= 0 || outputHeight <= 0) return null;

  let pipeline = sharp(output, { raw: { width, height, channels: 4 } });
  if (
    sourceLeft ||
    sourceTop ||
    outputWidth !== width ||
    outputHeight !== height
  ) {
    pipeline = pipeline.extract({
      left: sourceLeft,
      top: sourceTop,
      width: outputWidth,
      height: outputHeight,
    });
  }
  return {
    input: await pipeline.png({ compressionLevel: 1 }).toBuffer(),
    left: outputLeft,
    top: outputTop,
    blend: SHARP_BLEND_MODES[layer.blendMode] || "over",
  };
}

function cachedPsdLeafComposite(layer, documentWidth, documentHeight) {
  let cached = psdLeafCompositeCache.get(layer);
  const key = `${documentWidth}x${documentHeight}`;
  if (!cached) {
    cached = new Map();
    psdLeafCompositeCache.set(layer, cached);
  }
  if (!cached.has(key)) {
    cached.set(key, psdLeafComposite(layer, documentWidth, documentHeight));
  }
  return cached.get(key);
}

async function applyPngOpacity(buffer, opacity) {
  if (opacity >= 0.999) return buffer;
  const { data, info } = await sharp(buffer).ensureAlpha().raw().toBuffer({
    resolveWithObject: true,
  });
  for (let index = 3; index < data.length; index += info.channels) {
    data[index] = Math.round(data[index] * opacity);
  }
  return sharp(data, {
    raw: { width: info.width, height: info.height, channels: info.channels },
  }).png({ compressionLevel: 1 }).toBuffer();
}

function cacheNodeRender(cache, key, promise, limit = 3) {
  cache.set(key, promise);
  while (cache.size > limit) cache.delete(cache.keys().next().value);
  promise.catch(() => cache.delete(key));
  return promise;
}

function psdSubtreeVisibilityKey(layers, visibility, parentId = "", output = []) {
  for (let index = 0; index < (layers || []).length; index += 1) {
    const layer = layers[index];
    const id = parentId ? `${parentId}.${index}` : String(index);
    output.push(`${id}:${layerVisibility(layer, id, visibility) ? 1 : 0}`);
    psdSubtreeVisibilityKey(layer.children, visibility, id, output);
  }
  return output.join(",");
}

async function renderPsdNodeList(layers, documentWidth, documentHeight, visibility, parentId = "") {
  let cache = psdNodeRenderCache.get(layers);
  if (!cache) {
    cache = new Map();
    psdNodeRenderCache.set(layers, cache);
  }
  const key = `${documentWidth}x${documentHeight}:${psdSubtreeVisibilityKey(
    layers,
    visibility,
    parentId,
  )}`;
  if (cache.has(key)) return cache.get(key);
  return cacheNodeRender(cache, key, (async () => {
    const composites = (await Promise.all((layers || []).map(async (layer, index) => {
      const id = parentId ? `${parentId}.${index}` : String(index);
      if (!layerVisibility(layer, id, visibility)) return null;
      if (layer.children?.length) {
        let input = await renderPsdNodeList(
          layer.children,
          documentWidth,
          documentHeight,
          visibility,
          id,
        );
        input = await applyPngOpacity(input, Math.max(0, Math.min(1, layer.opacity ?? 1)));
        return {
          input,
          left: 0,
          top: 0,
          blend: SHARP_BLEND_MODES[layer.blendMode] || "over",
        };
      }
      return cachedPsdLeafComposite(layer, documentWidth, documentHeight);
    }))).filter(Boolean);
    return sharp({
      create: {
        width: documentWidth,
        height: documentHeight,
        channels: 4,
        background: { r: 0, g: 0, b: 0, alpha: 0 },
      },
    }).composite(composites).png({ compressionLevel: 1 }).toBuffer();
  })());
}

function visibilityCacheKey(item, visibility) {
  const entries = Object.entries(visibility || {})
    .sort(([left], [right]) => left.localeCompare(right));
  return `${cacheIdentity(item)}:${JSON.stringify(entries)}`;
}

function documentVisibility(layers, output = {}) {
  for (const layer of layers || []) {
    output[layer.id] = Boolean(layer.visible);
    documentVisibility(layer.children, output);
  }
  return output;
}

function isOriginalVisibility(layerDocument, visibility) {
  const original = documentVisibility(layerDocument?.layers);
  return Object.entries(original).every(([id, visible]) => (
    !Object.prototype.hasOwnProperty.call(visibility || {}, id) ||
    Boolean(visibility[id]) === visible
  ));
}

function cacheLayeredRender(key, promise) {
  layeredRenderCache.set(key, promise);
  while (layeredRenderCache.size > LAYERED_RENDER_CACHE_LIMIT) {
    layeredRenderCache.delete(layeredRenderCache.keys().next().value);
  }
  promise.catch(() => layeredRenderCache.delete(key));
  return promise;
}

function clipLayerVisibility(layer, visibility) {
  if (visibility && Object.prototype.hasOwnProperty.call(visibility, layer.id)) {
    return Boolean(visibility[layer.id]);
  }
  return Boolean(layer.visible);
}

function clipSubtreeVisibilityKey(layers, visibility, output = []) {
  for (const layer of layers || []) {
    output.push(`${layer.id}:${clipLayerVisibility(layer, visibility) ? 1 : 0}`);
    clipSubtreeVisibilityKey(layer.children, visibility, output);
  }
  return output.join(",");
}

function clipBlendModeForSharp(value) {
  const modes = {
    "clip-1": "darken",
    "clip-2": "multiply",
    "clip-3": "colour-burn",
    "clip-6": "lighten",
    "clip-7": "screen",
    "clip-8": "colour-dodge",
    "clip-10": "add",
    "clip-12": "overlay",
    "clip-13": "soft-light",
    "clip-14": "hard-light",
    "clip-15": "difference",
    "clip-20": "exclusion",
  };
  return modes[value] || "over";
}

async function decodedClipLayerPng(item, clip, layer, width, height) {
  const key = `${cacheIdentity(item)}:${layer.id}:${width}x${height}`;
  if (clipLayerDecodeCache.has(key)) return clipLayerDecodeCache.get(key);
  const promise = (async () => {
    const decoded = decodeClipLayerForCanvas(
      clip.source,
      clip.rasterSources?.[layer.id] || [],
      width,
      height,
    );
    if (!decoded) return null;
    let pipeline = sharp(decoded.data, {
      raw: { width: decoded.width, height: decoded.height, channels: 4 },
    });
    if (decoded.width !== width || decoded.height !== height) {
      pipeline = pipeline.resize(width, height, { fit: "fill" });
    }
    return pipeline.png({ compressionLevel: 1 }).toBuffer();
  })();
  clipLayerDecodeCache.set(key, promise);
  while (clipLayerDecodeCache.size > CLIP_LAYER_DECODE_CACHE_LIMIT) {
    clipLayerDecodeCache.delete(clipLayerDecodeCache.keys().next().value);
  }
  promise.catch(() => clipLayerDecodeCache.delete(key));
  return promise;
}

function clipTextSvg(layer, layerDocument, width, height) {
  const attributes = layer.textAttributes || {};
  const bounds = attributes.bounds || [0, 0, layerDocument.nativeWidth, layerDocument.nativeHeight];
  const scaleX = width / Math.max(1, layerDocument.nativeWidth || width);
  const scaleY = height / Math.max(1, layerDocument.nativeHeight || height);
  const x = Math.max(0, bounds[0] * scaleX);
  const y = Math.max(0, bounds[1] * scaleY);
  const defaultSize = Math.max(10, Math.abs(bounds[3] - bounds[1]) * scaleY);
  const fontSize = attributes.fontSize
    ? Math.max(6, attributes.fontSize / 100 * layerDocument.resolution / 72 * scaleY)
    : defaultSize;
  const colorValues = attributes.color || [0, 0, 0];
  const color = colorValues.map(clipColorChannel);
  const lines = String(layer.textPreview || layer.name).split(/\r\n|\r|\n/);
  const stroke = layer.effects?.length
    ? 'paint-order="stroke" stroke="#ffffff" stroke-width="2" stroke-linejoin="round"'
    : "";
  const tspans = lines.map((line, index) => (
    `<tspan x="${x}" dy="${index ? fontSize * 1.2 : 0}">${escapeXml(line)}</tspan>`
  )).join("");
  return Buffer.from(`
    <svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">
      <text x="${x}" y="${y + fontSize}" font-family="${escapeXml(attributes.font || "sans-serif")}"
        font-size="${fontSize}" fill="rgb(${color.join(",")})" ${stroke}>${tspans}</text>
    </svg>
  `);
}

function clipGradientSvg(layer, layerDocument, width, height) {
  const gradient = layer.gradient;
  if (gradient?.flatColor) {
    return Buffer.from(`
      <svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">
        <rect width="${width}" height="${height}" fill="rgb(${gradient.flatColor.join(",")})"/>
      </svg>
    `);
  }
  const geometry = gradient?.geometry;
  const stops = gradient?.stops;
  if (!geometry || !stops?.length) return null;
  const scaleX = width / Math.max(1, layerDocument.nativeWidth || width);
  const scaleY = height / Math.max(1, layerDocument.nativeHeight || height);
  const startX = geometry.startX * scaleX;
  const startY = geometry.startY * scaleY;
  const endX = geometry.endX * scaleX;
  const endY = geometry.endY * scaleY;
  const spreadMethod = geometry.repeat === 1
    ? "repeat"
    : geometry.repeat === 2
      ? "reflect"
      : "pad";
  const stopTags = stops.map((stop) => (
    `<stop offset="${stop.position * 100}%" stop-color="rgb(${stop.color.join(",")})"
      stop-opacity="${stop.opacity / 255}"/>`
  )).join("");
  const definition = geometry.shape === 0
    ? `<linearGradient id="fill" gradientUnits="userSpaceOnUse"
        x1="${startX}" y1="${startY}" x2="${endX}" y2="${endY}"
        spreadMethod="${spreadMethod}">${stopTags}</linearGradient>`
    : `<radialGradient id="fill" gradientUnits="userSpaceOnUse"
        cx="${startX}" cy="${startY}"
        r="${Math.max(1, Math.hypot(endX - startX, endY - startY))}"
        spreadMethod="${spreadMethod}">${stopTags}</radialGradient>`;
  return Buffer.from(`
    <svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">
      <defs>${definition}</defs>
      <rect width="${width}" height="${height}" fill="url(#fill)"/>
    </svg>
  `);
}

function decodedClipMask(item, clip, layer, width, height) {
  const key = `${cacheIdentity(item)}:${layer.id}:${width}x${height}`;
  if (clipMaskDecodeCache.has(key)) return clipMaskDecodeCache.get(key);
  const promise = (async () => {
    const decoded = decodeClipMaskForCanvas(
      clip.source,
      clip.maskSources?.[layer.id] || [],
      width,
      height,
    );
    if (!decoded) return null;
    let pipeline = sharp(decoded.data, {
      raw: { width: decoded.width, height: decoded.height, channels: 1 },
    });
    if (decoded.width !== width || decoded.height !== height) {
      pipeline = pipeline.resize(width, height, { fit: "fill" });
    }
    return pipeline.raw().toBuffer();
  })();
  clipMaskDecodeCache.set(key, promise);
  while (clipMaskDecodeCache.size > CLIP_MASK_DECODE_CACHE_LIMIT) {
    clipMaskDecodeCache.delete(clipMaskDecodeCache.keys().next().value);
  }
  promise.catch(() => clipMaskDecodeCache.delete(key));
  return promise;
}

async function applyClipLayerMask(item, clip, layer, input, width, height) {
  if (!layer.mask || !clip.maskSources?.[layer.id]?.length) return input;
  const mask = await decodedClipMask(item, clip, layer, width, height);
  if (!mask) return input;
  const { data, info } = await sharp(input).ensureAlpha().raw().toBuffer({
    resolveWithObject: true,
  });
  for (let pixel = 0; pixel < width * height; pixel += 1) {
    const alpha = pixel * info.channels + 3;
    data[alpha] = Math.round(data[alpha] * mask[pixel] / 255);
  }
  return sharp(data, {
    raw: { width, height, channels: info.channels },
  }).png({ compressionLevel: 1 }).toBuffer();
}

async function clipLeafComposite(item, clip, layer, width, height) {
  let input = null;
  if (layer.rasterAvailable) {
    input = await decodedClipLayerPng(item, clip, layer, width, height);
  } else if (layer.vectorAvailable) {
    const vector = cachedClipVector(item, clip, layer);
    if (vector) {
      input = await sharp(clipVectorSvg(
        layer,
        vector,
        width,
        height,
        false,
        clip.layerDocument.nativeWidth,
        clip.layerDocument.nativeHeight,
      )).png({ compressionLevel: 1 }).toBuffer();
    }
  } else if (layer.gradient) {
    const svg = clipGradientSvg(layer, clip.layerDocument, width, height);
    if (svg) input = await sharp(svg).png({ compressionLevel: 1 }).toBuffer();
  } else if (layer.type === "paper") {
    const color = layer.paperColor || [255, 255, 255];
    input = await sharp({
      create: {
        width,
        height,
        channels: 4,
        background: { r: color[0], g: color[1], b: color[2], alpha: 1 },
      },
    }).png({ compressionLevel: 1 }).toBuffer();
  } else if (layer.type === "text" && layer.textPreview) {
    input = await sharp(clipTextSvg(layer, clip.layerDocument, width, height))
      .png({ compressionLevel: 1 })
      .toBuffer();
  }
  if (!input) return null;
  input = await applyClipLayerMask(item, clip, layer, input, width, height);
  input = await applyPngOpacity(input, Math.max(0, Math.min(1, layer.opacity / 100)));
  return {
    input,
    left: 0,
    top: 0,
    blend: clipBlendModeForSharp(layer.blendMode),
  };
}

async function renderClipNodeList(item, clip, layers, visibility, width, height) {
  let cache = clipNodeRenderCache.get(layers);
  if (!cache) {
    cache = new Map();
    clipNodeRenderCache.set(layers, cache);
  }
  const key = `${width}x${height}:${clipSubtreeVisibilityKey(layers, visibility)}`;
  if (cache.has(key)) return cache.get(key);
  return cacheNodeRender(cache, key, (async () => {
    const orderedLayers = [...(layers || [])].reverse();
    const composites = (await Promise.all(orderedLayers.map(async (layer) => {
      if (!clipLayerVisibility(layer, visibility)) return null;
      if (layer.children?.length) {
        let input = await renderClipNodeList(
          item,
          clip,
          layer.children,
          visibility,
          width,
          height,
        );
        input = await applyPngOpacity(input, Math.max(0, Math.min(1, layer.opacity / 100)));
        return {
          input,
          left: 0,
          top: 0,
          blend: clipBlendModeForSharp(layer.blendMode),
        };
      }
      return clipLeafComposite(item, clip, layer, width, height);
    }))).filter(Boolean);
    return sharp({
      create: {
        width,
        height,
        channels: 4,
        background: { r: 0, g: 0, b: 0, alpha: 0 },
      },
    }).composite(composites).png({ compressionLevel: 1 }).toBuffer();
  })());
}

async function renderLayeredImage(item, visibility = {}) {
  const ext = extensionOf(item.kind === "archive-entry" ? item.entryName : item.path);
  if (ext !== ".psd" && !CLIP_EXTENSIONS.has(ext)) {
    throw new Error("이 파일 형식은 레이어 합성을 지원하지 않습니다.");
  }
  const key = visibilityCacheKey(item, visibility);
  if (layeredRenderCache.has(key)) return layeredRenderCache.get(key);
  return cacheLayeredRender(key, (async () => {
    let buffer;
    let layerDocument;
    let width;
    let height;
    let source;
    if (ext === ".psd") {
      const document = await getPsdDocument(item);
      ({ layerDocument } = document);
      width = document.psd.width;
      height = document.psd.height;
      source = "PSD 레이어 합성";
      buffer = await renderPsdNodeList(
        document.psd.children,
        width,
        height,
        visibility,
      );
    } else {
      const clip = await cachedClipData(item);
      ({ layerDocument } = clip);
      width = layerDocument.width;
      height = layerDocument.height;
      if (isOriginalVisibility(layerDocument, visibility)) {
        source = "CLIP 저장 미리보기";
        buffer = clip.preview;
      } else {
        source = "CLIP 레이어 합성";
        buffer = await renderClipNodeList(
          item,
          clip,
          layerDocument.layers,
          visibility,
          width,
          height,
        );
      }
    }
    const stat = fs.statSync(item.kind === "archive-entry" ? item.archivePath : item.path);
    return {
      dataUrl: `data:image/png;base64,${buffer.toString("base64")}`,
      layerDocument,
      metadata: {
        ...(await imageMetadata(buffer, { width, height, format: "PNG" })),
        source,
        cropAvailable: false,
        byteSize: item.kind === "archive-entry" ? buffer.length : stat.size,
        modifiedAt: stat.mtime.toISOString(),
      },
    };
  })());
}

async function prepareLayeredImage(item) {
  const ext = extensionOf(item.kind === "archive-entry" ? item.entryName : item.path);
  if (ext === ".psd") {
    const { layerDocument } = await getPsdDocument(item);
    await renderLayeredImage(item, documentVisibility(layerDocument.layers));
    return true;
  }
  if (CLIP_EXTENSIONS.has(ext)) {
    const clip = await cachedClipData(item);
    const visibility = documentVisibility(clip.layerDocument.layers);
    await renderClipNodeList(
      item,
      clip,
      clip.layerDocument.layers,
      visibility,
      clip.layerDocument.width,
      clip.layerDocument.height,
    );
    await renderLayeredImage(item, visibility);
    return true;
  }
  return false;
}

function layerAlphaAt(layer, documentX, documentY) {
  const bytes = rgbaBytes(layer.imageData);
  if (!bytes) return 0;
  const x = Math.floor(documentX - numberOrZero(layer.left));
  const y = Math.floor(documentY - numberOrZero(layer.top));
  if (x < 0 || y < 0 || x >= layer.imageData.width || y >= layer.imageData.height) return 0;
  const alpha = bytes[(y * layer.imageData.width + x) * 4 + 3];
  return alpha * (layer.opacity ?? 1) *
    maskValue(layer.mask, documentX, documentY) / 255;
}

function pickPsdNode(layers, x, y, visibility, parentId = "") {
  for (let index = (layers || []).length - 1; index >= 0; index -= 1) {
    const layer = layers[index];
    const id = parentId ? `${parentId}.${index}` : String(index);
    if (!layerVisibility(layer, id, visibility)) continue;
    if (layer.children?.length) {
      const nested = pickPsdNode(layer.children, x, y, visibility, id);
      if (nested) return nested;
    } else if (layerAlphaAt(layer, x, y) > 2) {
      return id;
    }
  }
  return null;
}

async function pickLayer(item, x, y, visibility = {}) {
  const ext = extensionOf(item.kind === "archive-entry" ? item.entryName : item.path);
  if (ext !== ".psd") return null;
  const { psd } = await getPsdDocument(item);
  return pickPsdNode(psd.children, Number(x), Number(y), visibility);
}

async function loadImage(item, cropMode = "full") {
  const sourceData = await sourceBuffer(item);
  let { buffer, mime, source } = sourceData;
  const originalMetadata = await imageMetadata(buffer, {
    format: extensionOf(item.name).slice(1).toUpperCase(),
  });
  const pixelHeight = originalMetadata.pageHeight || originalMetadata.height;
  if (
    originalMetadata.width &&
    pixelHeight &&
    originalMetadata.width * pixelHeight > MAX_IMAGE_PIXELS
  ) {
    throw new Error("이미지 크기가 안전 제한을 초과합니다.");
  }
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
    layerDocument: sourceData.layerDocument || null,
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
  calculateCropRegion,
  extractClipData,
  extractClipPreview,
  findSqliteDatabase,
  extensionOf,
  listArchive,
  loadImage,
  loadLayerThumbnail,
  loadThumbnail,
  naturalCompare,
  pickLayer,
  prepareLayeredImage,
  renderBpg,
  renderJxr,
  renderLayeredImage,
  renderMagick,
  renderRaw,
};
