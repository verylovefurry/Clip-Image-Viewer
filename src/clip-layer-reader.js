"use strict";

const fs = require("fs");
const zlib = require("zlib");

// Adapted from the CLIP chunk analysis in dobrokot/clip_to_psd (MIT).
const BLOCK_BEGIN = utf16be("BlockDataBeginChunk");
const BLOCK_END = utf16be("BlockDataEndChunk");
const BLOCK_STATUS = utf16be("BlockStatus");
const BLOCK_CHECKSUM = utf16be("BlockCheckSum");
const TILE_SIZE = 256;
const TILE_PIXELS = TILE_SIZE * TILE_SIZE;
const MAX_EXTERNAL_CHUNK_BYTES = 256 * 1024 * 1024;
const MAX_DECODE_PIXELS = 64 * 1024 * 1024;
const MAX_VECTOR_POINTS = 200000;

function utf16be(value) {
  return Buffer.from(value, "utf16le").swap16();
}

function queryRows(db, sql) {
  const statement = db.prepare(sql);
  const rows = [];
  try {
    while (statement.step()) rows.push(statement.getAsObject());
  } finally {
    statement.free();
  }
  return rows;
}

function blobKey(value) {
  if (!value) return "";
  return Buffer.from(value).toString("hex");
}

function extractClipMipmapSources(db, column) {
  if (!["LayerRenderMipmap", "LayerLayerMaskMipmap"].includes(column)) return {};
  try {
    const layers = queryRows(
      db,
      `SELECT MainId, "${column}" AS SourceMipmap FROM Layer`,
    );
    const mipmaps = new Map(queryRows(
      db,
      "SELECT MainId, BaseMipmapInfo FROM Mipmap",
    ).map((row) => [Number(row.MainId), row]));
    const mipmapInfo = new Map(queryRows(
      db,
      "SELECT MainId, ThisScale, Offscreen, NextIndex FROM MipmapInfo",
    ).map((row) => [Number(row.MainId), row]));
    const offscreens = new Map(queryRows(
      db,
      "SELECT MainId, Attribute, BlockData FROM Offscreen",
    ).map((row) => [Number(row.MainId), row]));
    const externalOffsets = new Map(queryRows(
      db,
      "SELECT ExternalID, Offset FROM ExternalChunk",
    ).map((row) => [blobKey(row.ExternalID), Number(row.Offset)]));
    const sources = {};

    for (const layer of layers) {
      const mipmap = mipmaps.get(Number(layer.SourceMipmap));
      let infoId = Number(mipmap?.BaseMipmapInfo);
      const visited = new Set();
      const levels = [];
      while (infoId && !visited.has(infoId) && visited.size < 32) {
        visited.add(infoId);
        const info = mipmapInfo.get(infoId);
        if (!info) break;
        const offscreen = offscreens.get(Number(info.Offscreen));
        const externalId = blobKey(offscreen?.BlockData);
        const offset = externalOffsets.get(externalId);
        if (
          offscreen?.Attribute &&
          externalId &&
          Number.isSafeInteger(offset) &&
          offset >= 0
        ) {
          try {
            const attributes = parseOffscreenAttributes(Buffer.from(offscreen.Attribute));
            levels.push({
              attribute: Buffer.from(offscreen.Attribute),
              externalId,
              offset,
              scale: Number(info.ThisScale) || 0,
              width: attributes.width,
              height: attributes.height,
            });
          } catch {
            // Unsupported offscreen attributes are ignored per mipmap level.
          }
        }
        infoId = Number(info.NextIndex);
      }
      if (levels.length) {
        sources[String(Number(layer.MainId))] = levels;
      }
    }
    return sources;
  } catch {
    return {};
  }
}

function extractClipRasterSources(db) {
  return extractClipMipmapSources(db, "LayerRenderMipmap");
}

function extractClipMaskSources(db) {
  return extractClipMipmapSources(db, "LayerLayerMaskMipmap");
}

function extractClipVectorSources(db) {
  try {
    const table = queryRows(
      db,
      "SELECT 1 FROM sqlite_master WHERE type='table' AND name='VectorObjectList' LIMIT 1",
    );
    if (!table.length) return {};
    const externalOffsets = new Map(queryRows(
      db,
      "SELECT ExternalID, Offset FROM ExternalChunk",
    ).map((row) => [blobKey(row.ExternalID), Number(row.Offset)]));
    const sources = {};
    for (const row of queryRows(db, "SELECT LayerId, VectorData FROM VectorObjectList")) {
      const externalId = blobKey(row.VectorData);
      const offset = externalOffsets.get(externalId);
      const layerId = String(Number(row.LayerId));
      if (!externalId || !Number.isSafeInteger(offset) || offset < 0 || layerId === "0") {
        continue;
      }
      if (!sources[layerId]) sources[layerId] = [];
      sources[layerId].push({ externalId, offset });
    }
    return sources;
  } catch {
    return {};
  }
}

function readUtf16be(buffer, cursor) {
  if (cursor.offset + 4 > buffer.length) throw new Error("CLIP 속성이 손상되었습니다.");
  const length = buffer.readUInt32BE(cursor.offset);
  cursor.offset += 4;
  const byteLength = length * 2;
  if (cursor.offset + byteLength > buffer.length) {
    throw new Error("CLIP 속성 문자열이 손상되었습니다.");
  }
  const value = Buffer.from(buffer.subarray(cursor.offset, cursor.offset + byteLength))
    .swap16()
    .toString("utf16le");
  cursor.offset += byteLength;
  return value;
}

function parseOffscreenAttributes(attribute) {
  const cursor = { offset: 0 };
  const readInt = () => {
    if (cursor.offset + 4 > attribute.length) throw new Error("CLIP 속성이 손상되었습니다.");
    const value = attribute.readUInt32BE(cursor.offset);
    cursor.offset += 4;
    return value;
  };
  const headerSize = readInt();
  const infoSize = readInt();
  const extraInfoSize = readInt();
  readInt();
  if (headerSize !== 16 || infoSize !== 102 || ![42, 58].includes(extraInfoSize)) {
    throw new Error("지원하지 않는 CLIP 오프스크린 속성입니다.");
  }
  if (readUtf16be(attribute, cursor) !== "Parameter") {
    throw new Error("CLIP 오프스크린 매개변수가 없습니다.");
  }
  const width = readInt();
  const height = readInt();
  const gridWidth = readInt();
  const gridHeight = readInt();
  const packing = Array.from({ length: 16 }, readInt);
  if (readUtf16be(attribute, cursor) !== "InitColor") {
    throw new Error("CLIP 초기 색상 정보가 없습니다.");
  }
  readInt();
  const defaultFill = readInt();
  readInt();
  readInt();
  readInt();
  if (extraInfoSize === 58) {
    for (let index = 0; index < 4; index += 1) readInt();
  }
  if (
    !width ||
    !height ||
    !gridWidth ||
    !gridHeight ||
    gridWidth * gridHeight > 100000
  ) {
    throw new Error("CLIP 레이어 크기가 올바르지 않습니다.");
  }
  return { width, height, gridWidth, gridHeight, packing, defaultFill };
}

function parseChunkBlocks(data) {
  const blocks = [];
  let offset = 0;
  while (offset < data.length) {
    if (
      offset + 8 + BLOCK_BEGIN.length <= data.length &&
      data.subarray(offset + 8, offset + 8 + BLOCK_BEGIN.length).equals(BLOCK_BEGIN)
    ) {
      const blockSize = data.readUInt32BE(offset);
      const endMarkerSize = 4 + BLOCK_END.length;
      if (blockSize < 8 + BLOCK_BEGIN.length + endMarkerSize || offset + blockSize > data.length) {
        throw new Error("CLIP 레이어 블록 크기가 올바르지 않습니다.");
      }
      const expectedEnd = Buffer.concat([
        Buffer.from([0, 0, 0, 17]),
        BLOCK_END,
      ]);
      if (!data.subarray(offset + blockSize - endMarkerSize, offset + blockSize).equals(expectedEnd)) {
        throw new Error("CLIP 레이어 블록 종료 표식이 없습니다.");
      }
      const block = data.subarray(
        offset + 8 + BLOCK_BEGIN.length,
        offset + blockSize - endMarkerSize,
      );
      if (block.length < 20) throw new Error("CLIP 레이어 블록이 손상되었습니다.");
      const hasData = block.readUInt32BE(16);
      if (hasData > 1) throw new Error("CLIP 레이어 블록 상태가 올바르지 않습니다.");
      if (hasData) {
        if (block.length < 28) throw new Error("CLIP 레이어 타일이 손상되었습니다.");
        const subblockLength = block.readUInt32BE(20);
        if (block.length !== subblockLength + 24) {
          throw new Error("CLIP 레이어 타일 길이가 올바르지 않습니다.");
        }
        blocks.push(block.subarray(28));
      } else {
        blocks.push(null);
      }
      offset += blockSize;
      continue;
    }

    const nameLength = offset + 4 <= data.length ? data.readUInt32BE(offset) : -1;
    if (
      nameLength === 11 &&
      data.subarray(offset + 4, offset + 4 + BLOCK_STATUS.length).equals(BLOCK_STATUS)
    ) {
      if (offset + 34 > data.length) throw new Error("CLIP 블록 상태가 손상되었습니다.");
      const count = data.readUInt32BE(offset + 30);
      const blockSize = count * 4 + 12 + 4 + BLOCK_STATUS.length;
      if (offset + blockSize > data.length) throw new Error("CLIP 블록 상태 범위를 벗어났습니다.");
      offset += blockSize;
      continue;
    }
    if (
      nameLength === 13 &&
      data.subarray(offset + 4, offset + 4 + BLOCK_CHECKSUM.length).equals(BLOCK_CHECKSUM)
    ) {
      const blockSize = 4 + BLOCK_CHECKSUM.length + 12 + blocks.length * 4;
      if (offset + blockSize > data.length) throw new Error("CLIP 체크섬 범위를 벗어났습니다.");
      offset += blockSize;
      continue;
    }
    throw new Error("인식할 수 없는 CLIP 레이어 블록입니다.");
  }
  return blocks;
}

function readExternalChunk(source, offset, expectedId) {
  let descriptor = null;
  try {
    if (!Buffer.isBuffer(source)) descriptor = fs.openSync(source, "r");
    const readRange = (position, length) => {
      if (Buffer.isBuffer(source)) {
        if (position < 0 || position + length > source.length) {
          throw new Error("CLIP 외부 청크 범위를 벗어났습니다.");
        }
        return source.subarray(position, position + length);
      }
      const output = Buffer.allocUnsafe(length);
      let read = 0;
      while (read < length) {
        const count = fs.readSync(descriptor, output, read, length - read, position + read);
        if (!count) throw new Error("CLIP 외부 청크를 끝까지 읽지 못했습니다.");
        read += count;
      }
      return output;
    };
    const header = readRange(offset, 16);
    if (
      header.toString("ascii", 0, 4) !== "CHNK" ||
      header.toString("ascii", 4, 8) !== "Exta"
    ) {
      throw new Error("CLIP 외부 청크 헤더가 올바르지 않습니다.");
    }
    const chunkLength = header.readUInt32BE(12);
    if (!chunkLength || chunkLength > MAX_EXTERNAL_CHUNK_BYTES) {
      throw new Error("CLIP 외부 청크 크기가 안전 제한을 초과합니다.");
    }
    const chunk = readRange(offset + 16, chunkLength);
    const idLength = Number(chunk.readBigUInt64BE(0));
    if (idLength <= 0 || idLength > 1024 || 16 + idLength > chunk.length) {
      throw new Error("CLIP 외부 청크 식별자가 손상되었습니다.");
    }
    const id = chunk.subarray(8, 8 + idLength);
    if (expectedId && id.toString("hex") !== expectedId) {
      throw new Error("CLIP 외부 청크 식별자가 일치하지 않습니다.");
    }
    const binaryLength = Number(chunk.readBigUInt64BE(8 + idLength));
    const binary = chunk.subarray(16 + idLength);
    if (binaryLength !== binary.length) {
      throw new Error("CLIP 외부 청크 데이터 길이가 일치하지 않습니다.");
    }
    return binary;
  } finally {
    if (descriptor !== null) fs.closeSync(descriptor);
  }
}

function chooseThumbnailLevel(levels, size) {
  const sorted = [...levels].sort((a, b) => (
    a.width * a.height - b.width * b.height
  ));
  return sorted.find((level) => Math.max(level.width, level.height) >= size * 2)
    || sorted.at(-1)
    || null;
}

function chooseCanvasLevel(levels, targetWidth, targetHeight) {
  return [...levels].sort((a, b) => {
    const aDistance = Math.abs(a.width - targetWidth) + Math.abs(a.height - targetHeight);
    const bDistance = Math.abs(b.width - targetWidth) + Math.abs(b.height - targetHeight);
    return aDistance - bDistance;
  })[0] || null;
}

function decodeClipLayerLevel(source, level) {
  if (!level) return null;
  const attributes = parseOffscreenAttributes(Buffer.from(level.attribute));
  const {
    width,
    height,
    gridWidth,
    gridHeight,
    packing,
    defaultFill,
  } = attributes;
  if (width * height > MAX_DECODE_PIXELS) {
    throw new Error("CLIP 레이어 미리보기 크기가 안전 제한을 초과합니다.");
  }
  if (packing[1] !== 1 || packing[2] !== 4 || packing[8] === 32) {
    return null;
  }
  const blocks = parseChunkBlocks(
    readExternalChunk(source, level.offset, level.externalId),
  );
  if (blocks.length !== gridWidth * gridHeight) {
    throw new Error("CLIP 레이어 타일 수가 일치하지 않습니다.");
  }

  const output = Buffer.alloc(width * height * 4, defaultFill ? 255 : 0);
  for (let tileY = 0; tileY < gridHeight; tileY += 1) {
    for (let tileX = 0; tileX < gridWidth; tileX += 1) {
      const compressed = blocks[tileY * gridWidth + tileX];
      if (!compressed) continue;
      let pixels;
      try {
        pixels = zlib.inflateSync(compressed, { maxOutputLength: TILE_PIXELS * 5 });
      } catch {
        continue;
      }
      if (pixels.length !== TILE_PIXELS * 5) continue;
      const copyWidth = Math.min(TILE_SIZE, width - tileX * TILE_SIZE);
      const copyHeight = Math.min(TILE_SIZE, height - tileY * TILE_SIZE);
      for (let y = 0; y < copyHeight; y += 1) {
        for (let x = 0; x < copyWidth; x += 1) {
          const tilePixel = y * TILE_SIZE + x;
          const rgbx = TILE_PIXELS + tilePixel * 4;
          const outputPixel = ((tileY * TILE_SIZE + y) * width + tileX * TILE_SIZE + x) * 4;
          output[outputPixel] = pixels[rgbx + 2];
          output[outputPixel + 1] = pixels[rgbx + 1];
          output[outputPixel + 2] = pixels[rgbx];
          output[outputPixel + 3] = pixels[tilePixel];
        }
      }
    }
  }
  return { data: output, width, height };
}

function decodeClipLayer(source, levels, size = 58) {
  return decodeClipLayerLevel(source, chooseThumbnailLevel(levels, size));
}

function decodeClipLayerForCanvas(source, levels, targetWidth, targetHeight) {
  return decodeClipLayerLevel(
    source,
    chooseCanvasLevel(levels, targetWidth, targetHeight),
  );
}

function decodeClipMaskForCanvas(source, levels, targetWidth, targetHeight) {
  const level = chooseCanvasLevel(levels, targetWidth, targetHeight);
  if (!level) return null;
  const attributes = parseOffscreenAttributes(Buffer.from(level.attribute));
  const {
    width,
    height,
    gridWidth,
    gridHeight,
    packing,
    defaultFill,
  } = attributes;
  if (width * height > MAX_DECODE_PIXELS || packing[1] + packing[2] !== 1) {
    return null;
  }
  const blocks = parseChunkBlocks(
    readExternalChunk(source, level.offset, level.externalId),
  );
  if (blocks.length !== gridWidth * gridHeight) {
    throw new Error("CLIP 레이어 마스크 타일 수가 일치하지 않습니다.");
  }
  const output = Buffer.alloc(width * height, defaultFill ? 255 : 0);
  for (let tileY = 0; tileY < gridHeight; tileY += 1) {
    for (let tileX = 0; tileX < gridWidth; tileX += 1) {
      const compressed = blocks[tileY * gridWidth + tileX];
      if (!compressed) continue;
      let pixels;
      try {
        pixels = zlib.inflateSync(compressed, { maxOutputLength: TILE_PIXELS });
      } catch {
        continue;
      }
      if (pixels.length !== TILE_PIXELS) continue;
      const copyWidth = Math.min(TILE_SIZE, width - tileX * TILE_SIZE);
      const copyHeight = Math.min(TILE_SIZE, height - tileY * TILE_SIZE);
      for (let y = 0; y < copyHeight; y += 1) {
        const sourceStart = y * TILE_SIZE;
        const outputStart = (tileY * TILE_SIZE + y) * width + tileX * TILE_SIZE;
        pixels.copy(output, outputStart, sourceStart, sourceStart + copyWidth);
      }
    }
  }
  return { data: output, width, height };
}

function vectorRecord(data, offset, maximumX, maximumY) {
  if (offset < 0 || offset + 88 > data.length) return null;
  const left = data.readUInt32BE(offset);
  const top = data.readUInt32BE(offset + 4);
  const right = data.readUInt32BE(offset + 8);
  const bottom = data.readUInt32BE(offset + 12);
  const x = data.readDoubleBE(offset + 72);
  const y = data.readDoubleBE(offset + 80);
  const red = data.readFloatBE(offset + 28);
  const green = data.readFloatBE(offset + 32);
  const blue = data.readFloatBE(offset + 36);
  const width = Math.max(right - left, bottom - top);
  if (
    left > right ||
    top > bottom ||
    right > maximumX ||
    bottom > maximumY ||
    width > Math.max(512, Math.min(maximumX, maximumY) / 2) ||
    !Number.isFinite(x) ||
    !Number.isFinite(y) ||
    x < left - 2 ||
    x > right + 2 ||
    y < top - 2 ||
    y > bottom + 2 ||
    ![red, green, blue].every((value) => Number.isFinite(value) && value >= 0 && value <= 1)
  ) {
    return null;
  }
  return {
    x,
    y,
    width: Math.max(1, width),
    color: [
      Math.round(red * 255),
      Math.round(green * 255),
      Math.round(blue * 255),
    ],
  };
}

function decodeClipVector(source, entries, canvasWidth, canvasHeight) {
  const maximumX = Math.max(4096, Math.ceil(canvasWidth * 2));
  const maximumY = Math.max(4096, Math.ceil(canvasHeight * 2));
  const segments = [];
  let pointCount = 0;
  for (const entry of entries || []) {
    const data = readExternalChunk(source, entry.offset, entry.externalId);
    let segment = [];
    let offset = 0;
    while (offset + 88 <= data.length && pointCount < MAX_VECTOR_POINTS) {
      const point = vectorRecord(data, offset, maximumX, maximumY);
      if (!point) {
        if (segment.length > 1) segments.push(segment);
        segment = [];
        offset += 4;
        continue;
      }
      const previous = segment.at(-1);
      if (
        previous &&
        Math.hypot(point.x - previous.x, point.y - previous.y) >
          Math.max(30, previous.width * 8)
      ) {
        if (segment.length > 1) segments.push(segment);
        segment = [];
      }
      segment.push(point);
      pointCount += 1;
      offset += 88;
    }
    if (segment.length > 1) segments.push(segment);
    if (pointCount >= MAX_VECTOR_POINTS) break;
  }
  if (!segments.length) return null;
  const points = segments.flat();
  return {
    segments,
    bounds: {
      left: Math.min(...points.map((point) => point.x - point.width / 2)),
      top: Math.min(...points.map((point) => point.y - point.width / 2)),
      right: Math.max(...points.map((point) => point.x + point.width / 2)),
      bottom: Math.max(...points.map((point) => point.y + point.width / 2)),
    },
  };
}

module.exports = {
  decodeClipLayer,
  decodeClipLayerForCanvas,
  decodeClipMaskForCanvas,
  decodeClipVector,
  extractClipMaskSources,
  extractClipRasterSources,
  extractClipVectorSources,
  parseChunkBlocks,
  parseOffscreenAttributes,
};
