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
const MAX_SOURCE_PIXELS = 512 * 1024 * 1024;
const MAX_VECTOR_POINTS = 200000;
const MIN_VECTOR_HEADER_SIZE = 92;
const MIN_VECTOR_POINT_SIZE = 88;

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
  if (![
    "LayerRenderMipmap",
    "LayerLayerMaskMipmap",
    "ResizableOriginalMipmap",
    "ComicFrameLineMipmap",
  ].includes(column)) return {};
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

function extractClipObjectSources(db) {
  return extractClipMipmapSources(db, "ResizableOriginalMipmap");
}

function extractClipBorderSources(db) {
  return extractClipMipmapSources(db, "ComicFrameLineMipmap");
}

function extractClipThumbnailSources(db) {
  try {
    const columns = new Set(queryRows(db, "PRAGMA table_info(LayerThumbnail)")
      .map((row) => String(row.name)));
    const optional = (name) => columns.has(name)
      ? `LayerThumbnail.${name}`
      : `0 AS ${name}`;
    const rows = queryRows(db, `
      SELECT Layer.MainId AS LayerId, Offscreen.Attribute, Offscreen.BlockData,
        ${optional("ThumbnailCanvasWidth")},
        ${optional("ThumbnailCanvasHeight")},
        ${optional("ThumbnailDrewUseCanvasAspect0")},
        ${optional("ThumbnailDrewUseCanvasAspect1")}
      FROM Layer
      JOIN LayerThumbnail
        ON LayerThumbnail.MainId = Layer.LayerRenderThumbnail
      JOIN Offscreen
        ON Offscreen.MainId = LayerThumbnail.ThumbnailOffscreen
      WHERE Offscreen.Attribute IS NOT NULL
        AND Offscreen.BlockData IS NOT NULL
    `);
    const externalOffsets = new Map(queryRows(
      db,
      "SELECT ExternalID, Offset FROM ExternalChunk",
    ).map((row) => [blobKey(row.ExternalID), Number(row.Offset)]));
    const sources = {};
    for (const row of rows) {
      const externalId = blobKey(row.BlockData);
      const offset = externalOffsets.get(externalId);
      if (!externalId || !Number.isSafeInteger(offset) || offset < 0) continue;
      try {
        const attribute = Buffer.from(row.Attribute);
        const attributes = parseOffscreenAttributes(attribute);
        sources[String(Number(row.LayerId))] = [{
          attribute,
          externalId,
          offset,
          scale: 0,
          width: attributes.width,
          height: attributes.height,
          canvasWidth: Number(row.ThumbnailCanvasWidth) || 0,
          canvasHeight: Number(row.ThumbnailCanvasHeight) || 0,
          useCanvasAspect: Boolean(
            Number(row.ThumbnailDrewUseCanvasAspect0) ||
            Number(row.ThumbnailDrewUseCanvasAspect1)
          ),
        }];
      } catch {
        // Some CLIP versions use thumbnail encodings that are not tile-compatible.
      }
    }
    return sources;
  } catch {
    return {};
  }
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

function extractClipBrushStyles(db) {
  try {
    const graphs = new Map();
    for (const row of queryRows(
      db,
      "SELECT MainId, ControlNumber, ControlDataSize, ControlPoints FROM BrushEffectorGraphData",
    )) {
      const data = row.ControlPoints ? Buffer.from(row.ControlPoints) : null;
      const count = Number(row.ControlNumber);
      if (!data || Number(row.ControlDataSize) !== 16 || data.length < count * 16) continue;
      const points = [];
      for (let index = 0; index < count; index += 1) {
        const x = data.readDoubleBE(index * 16);
        const y = data.readDoubleBE(index * 16 + 8);
        if (Number.isFinite(x) && Number.isFinite(y)) points.push({ x, y });
      }
      if (points.length) {
        points.sort((left, right) => left.x - right.x);
        graphs.set(Number(row.MainId), points);
      }
    }
    const columns = new Set(queryRows(db, "PRAGMA table_info(BrushStyle)")
      .map((row) => String(row.name)));
    const optionalColumn = (name) => (
      columns.has(name) ? name : `NULL AS ${name}`
    );
    const styles = {};
    for (const row of queryRows(db, `
      SELECT MainId, SizeEffector,
        ${optionalColumn("ThicknessBase")},
        ${optionalColumn("RotationBase")},
        ${optionalColumn("AntiAlias")},
        ${optionalColumn("Hardness")},
        ${optionalColumn("PatternStyle")}
      FROM BrushStyle
    `)) {
      const data = row.SizeEffector ? Buffer.from(row.SizeEffector) : null;
      const pressureMinimum = data?.length >= 12 ? data.readFloatBE(4) : 0;
      const pressureGraph = data?.length >= 12
        ? graphs.get(data.readUInt32BE(8)) || null
        : null;
      let velocityMinimum = 1;
      let velocityGraph = null;
      if (data?.length >= 20) {
        velocityMinimum = data.readFloatBE(12);
        velocityGraph = graphs.get(data.readUInt32BE(16)) || null;
      }
      const thickness = Number(row.ThicknessBase);
      const rotation = Number(row.RotationBase);
      styles[String(Number(row.MainId))] = {
        pressureEnabled: Boolean(pressureGraph),
        pressureMinimum: Number.isFinite(pressureMinimum) ? pressureMinimum : 0,
        pressureGraph,
        velocityEnabled: Boolean(velocityGraph),
        velocityMinimum: Number.isFinite(velocityMinimum) ? velocityMinimum : 1,
        velocityGraph,
        thickness: Number.isFinite(thickness)
          ? Math.max(0.01, Math.min(1, thickness))
          : 1,
        rotation: Number.isFinite(rotation) ? rotation : 0,
        antiAlias: Number(row.AntiAlias) || 0,
        hardness: Number.isFinite(Number(row.Hardness)) ? Number(row.Hardness) : 1,
        patternStyle: Number(row.PatternStyle) || 0,
      };
    }
    return styles;
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

function clipLayerPixelFormat(packing) {
  if (packing[1] === 1 && packing[2] === 4 && packing[8] !== 32) return "rgba";
  if (packing[1] === 1 && packing[2] === 1 && packing[8] === 32) {
    return "bit-gray-alpha";
  }
  if (packing[1] === 1 && packing[2] === 1) return "gray-alpha";
  if (packing[1] + packing[2] === 1) return "alpha";
  return "";
}

function decodeClipLayerLevelScaled(
  source,
  level,
  targetWidth,
  targetHeight,
  options = {},
) {
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
  const outputWidth = Math.max(1, Math.round(targetWidth));
  const outputHeight = Math.max(1, Math.round(targetHeight));
  if (
    width * height > MAX_SOURCE_PIXELS ||
    outputWidth * outputHeight > MAX_DECODE_PIXELS
  ) {
    throw new Error("CLIP 레이어 미리보기 크기가 안전 제한을 초과합니다.");
  }
  const format = clipLayerPixelFormat(packing);
  if (!format) return null;
  const blocks = parseChunkBlocks(
    readExternalChunk(source, level.offset, level.externalId),
  );
  if (blocks.length !== gridWidth * gridHeight) {
    throw new Error("CLIP 레이어 타일 수가 일치하지 않습니다.");
  }

  const canvasWidth = Math.max(1, Number(options.canvasWidth) || width);
  const canvasHeight = Math.max(1, Number(options.canvasHeight) || height);
  const offsetX = Number(options.offsetX) || 0;
  const offsetY = Number(options.offsetY) || 0;
  const scaleX = outputWidth / canvasWidth;
  const scaleY = outputHeight / canvasHeight;
  const color = Array.isArray(options.color) && options.color.length >= 3
    ? options.color.slice(0, 3).map((value) => (
      Math.max(0, Math.min(255, Math.round(Number(value) || 0)))
    ))
    : [0, 0, 0];
  const output = Buffer.alloc(outputWidth * outputHeight * 4);
  const regionLeft = Math.max(0, Math.floor(offsetX * scaleX));
  const regionTop = Math.max(0, Math.floor(offsetY * scaleY));
  const regionRight = Math.min(outputWidth, Math.ceil((offsetX + width) * scaleX));
  const regionBottom = Math.min(outputHeight, Math.ceil((offsetY + height) * scaleY));
  if (defaultFill) {
    const fillColor = format === "alpha" ? color : [255, 255, 255];
    for (let y = regionTop; y < regionBottom; y += 1) {
      for (let x = regionLeft; x < regionRight; x += 1) {
        const outputPixel = (y * outputWidth + x) * 4;
        output[outputPixel] = fillColor[0];
        output[outputPixel + 1] = fillColor[1];
        output[outputPixel + 2] = fillColor[2];
        output[outputPixel + 3] = 255;
      }
    }
  }

  let decodedTileCount = 0;
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
      const expectedLength = {
        rgba: TILE_PIXELS * 5,
        "gray-alpha": TILE_PIXELS * 2,
        "bit-gray-alpha": TILE_PIXELS / 4,
        alpha: TILE_PIXELS,
      }[format];
      if (pixels.length !== expectedLength) continue;
      decodedTileCount += 1;
      const sourceLeft = tileX * TILE_SIZE;
      const sourceTop = tileY * TILE_SIZE;
      const sourceRight = Math.min(width, sourceLeft + TILE_SIZE);
      const sourceBottom = Math.min(height, sourceTop + TILE_SIZE);
      const destinationLeft = Math.max(0, Math.floor((offsetX + sourceLeft) * scaleX));
      const destinationTop = Math.max(0, Math.floor((offsetY + sourceTop) * scaleY));
      const destinationRight = Math.min(
        outputWidth,
        Math.ceil((offsetX + sourceRight) * scaleX),
      );
      const destinationBottom = Math.min(
        outputHeight,
        Math.ceil((offsetY + sourceBottom) * scaleY),
      );
      for (let y = destinationTop; y < destinationBottom; y += 1) {
        const sourceY = Math.max(sourceTop, Math.min(
          sourceBottom - 1,
          Math.floor((y + 0.5) / scaleY - offsetY),
        ));
        const tilePixelY = sourceY - sourceTop;
        for (let x = destinationLeft; x < destinationRight; x += 1) {
          const sourceX = Math.max(sourceLeft, Math.min(
            sourceRight - 1,
            Math.floor((x + 0.5) / scaleX - offsetX),
          ));
          const tilePixel = tilePixelY * TILE_SIZE + sourceX - sourceLeft;
          const outputPixel = (y * outputWidth + x) * 4;
          let red;
          let green;
          let blue;
          let alpha;
          if (format === "rgba") {
            const rgbx = TILE_PIXELS + tilePixel * 4;
            red = pixels[rgbx + 2];
            green = pixels[rgbx + 1];
            blue = pixels[rgbx];
            alpha = pixels[tilePixel];
          } else if (format === "gray-alpha") {
            red = pixels[TILE_PIXELS + tilePixel];
            green = red;
            blue = red;
            alpha = pixels[tilePixel];
          } else if (format === "bit-gray-alpha") {
            const byte = tilePixel >> 3;
            const bit = 7 - (tilePixel & 7);
            const planeSize = TILE_PIXELS / 8;
            alpha = (pixels[byte] & (1 << bit)) ? 255 : 0;
            red = (pixels[planeSize + byte] & (1 << bit)) ? 255 : 0;
            green = red;
            blue = red;
          } else {
            [red, green, blue] = color;
            alpha = pixels[tilePixel];
          }
          output[outputPixel] = red;
          output[outputPixel + 1] = green;
          output[outputPixel + 2] = blue;
          output[outputPixel + 3] = alpha;
        }
      }
    }
  }
  if (!decodedTileCount && !defaultFill) return null;
  return { data: output, width: outputWidth, height: outputHeight };
}

function decodeClipLayer(source, levels, size = 58, options = {}) {
  const level = chooseThumbnailLevel(levels, size);
  if (!level) return null;
  const scale = Math.min(1, size / Math.max(1, level.width, level.height));
  return decodeClipLayerLevelScaled(
    source,
    level,
    Math.max(1, Math.round(level.width * scale)),
    Math.max(1, Math.round(level.height * scale)),
    { ...options, canvasWidth: level.width, canvasHeight: level.height },
  );
}

function decodeClipLayerForCanvas(
  source,
  levels,
  targetWidth,
  targetHeight,
  options = {},
) {
  return decodeClipLayerLevelScaled(
    source,
    chooseCanvasLevel(levels, targetWidth, targetHeight),
    targetWidth,
    targetHeight,
    options,
  );
}

function decodeClipMaskForCanvas(
  source,
  levels,
  targetWidth,
  targetHeight,
  options = {},
) {
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
  const outputWidth = Math.max(1, Math.round(targetWidth));
  const outputHeight = Math.max(1, Math.round(targetHeight));
  if (
    width * height > MAX_SOURCE_PIXELS ||
    outputWidth * outputHeight > MAX_DECODE_PIXELS ||
    packing[1] + packing[2] !== 1
  ) return null;
  const blocks = parseChunkBlocks(
    readExternalChunk(source, level.offset, level.externalId),
  );
  if (blocks.length !== gridWidth * gridHeight) {
    throw new Error("CLIP 레이어 마스크 타일 수가 일치하지 않습니다.");
  }
  const canvasWidth = Math.max(1, Number(options.canvasWidth) || width);
  const canvasHeight = Math.max(1, Number(options.canvasHeight) || height);
  const offsetX = Number(options.offsetX) || 0;
  const offsetY = Number(options.offsetY) || 0;
  const scaleX = outputWidth / canvasWidth;
  const scaleY = outputHeight / canvasHeight;
  const output = Buffer.alloc(outputWidth * outputHeight, defaultFill ? 255 : 0);
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
      const sourceLeft = tileX * TILE_SIZE;
      const sourceTop = tileY * TILE_SIZE;
      const sourceRight = Math.min(width, sourceLeft + TILE_SIZE);
      const sourceBottom = Math.min(height, sourceTop + TILE_SIZE);
      const destinationLeft = Math.max(0, Math.floor((offsetX + sourceLeft) * scaleX));
      const destinationTop = Math.max(0, Math.floor((offsetY + sourceTop) * scaleY));
      const destinationRight = Math.min(
        outputWidth,
        Math.ceil((offsetX + sourceRight) * scaleX),
      );
      const destinationBottom = Math.min(
        outputHeight,
        Math.ceil((offsetY + sourceBottom) * scaleY),
      );
      for (let y = destinationTop; y < destinationBottom; y += 1) {
        const sourceY = Math.max(sourceTop, Math.min(
          sourceBottom - 1,
          Math.floor((y + 0.5) / scaleY - offsetY),
        ));
        for (let x = destinationLeft; x < destinationRight; x += 1) {
          const sourceX = Math.max(sourceLeft, Math.min(
            sourceRight - 1,
            Math.floor((x + 0.5) / scaleX - offsetX),
          ));
          output[y * outputWidth + x] = pixels[
            (sourceY - sourceTop) * TILE_SIZE + sourceX - sourceLeft
          ];
        }
      }
    }
  }
  return { data: output, width: outputWidth, height: outputHeight };
}

function evaluateBrushGraph(points, input) {
  const value = Math.max(0, Math.min(1, input));
  if (!points?.length) return value;
  if (value <= points[0].x) return points[0].y;
  for (let index = 1; index < points.length; index += 1) {
    const right = points[index];
    const left = points[index - 1];
    if (value <= right.x) {
      const distance = right.x - left.x;
      const ratio = distance > 0 ? (value - left.x) / distance : 0;
      return left.y + (right.y - left.y) * ratio;
    }
  }
  return points.at(-1).y;
}

function vectorStrokeInfo(data, offset, brushStyles) {
  if (
    offset < 0 ||
    offset + MIN_VECTOR_HEADER_SIZE > data.length
  ) {
    return null;
  }
  const headerSize = data.readUInt32BE(offset);
  const styleOffset = data.readUInt32BE(offset + 4);
  const pointStride = data.readUInt32BE(offset + 8);
  const pointDataSize = data.readUInt32BE(offset + 12);
  const pointCount = data.readUInt32BE(offset + 16);
  if (
    headerSize < MIN_VECTOR_HEADER_SIZE ||
    headerSize > 4096 ||
    styleOffset + 12 > headerSize ||
    pointStride < MIN_VECTOR_POINT_SIZE ||
    pointStride > 1024 ||
    pointDataSize < MIN_VECTOR_POINT_SIZE ||
    pointDataSize > pointStride ||
    pointCount > MAX_VECTOR_POINTS ||
    offset + headerSize + pointCount * pointStride > data.length
  ) {
    return null;
  }
  const styleId = String(data.readUInt32BE(offset + styleOffset));
  const baseSize = data.readDoubleBE(offset + styleOffset + 4);
  if (!Number.isFinite(baseSize) || baseSize <= 0 || baseSize > 10000) return null;
  return {
    baseSize,
    headerSize,
    pointCount,
    pointStride,
    styleId,
    style: brushStyles?.[styleId] || null,
  };
}

function vectorRecord(data, offset, maximumX, maximumY, strokeInfo = null) {
  if (offset < 0 || offset + MIN_VECTOR_POINT_SIZE > data.length) return null;
  const x = data.readDoubleBE(offset);
  const y = data.readDoubleBE(offset + 8);
  const left = data.readUInt32BE(offset + 16);
  const top = data.readUInt32BE(offset + 20);
  const right = data.readUInt32BE(offset + 24);
  const bottom = data.readUInt32BE(offset + 28);
  const pressure = data.readFloatBE(offset + 36);
  const tilt = data.readFloatBE(offset + 40);
  const velocity = data.readFloatBE(offset + 44);
  const envelopeWidth = Math.max(right - left, bottom - top);
  if (
    left > right ||
    top > bottom ||
    envelopeWidth < 1 ||
    right > maximumX ||
    bottom > maximumY ||
    envelopeWidth > Math.max(512, Math.min(maximumX, maximumY) / 2) ||
    !Number.isFinite(x) ||
    !Number.isFinite(y) ||
    !Number.isFinite(pressure) ||
    pressure < 0 ||
    pressure > 1 ||
    !Number.isFinite(velocity) ||
    velocity < 0 ||
    velocity > 1 ||
    x < left - 2 ||
    x > right + 2 ||
    y < top - 2 ||
    y > bottom + 2
  ) {
    return null;
  }
  let width = Math.max(1, envelopeWidth - 4);
  let thickness = 1;
  let rotation = 0;
  if (strokeInfo) {
    const style = strokeInfo.style;
    let pressureFactor = 1;
    if (style?.pressureEnabled) {
      const pressureCurve = evaluateBrushGraph(style.pressureGraph, pressure);
      const pressureMinimum = Math.max(0, Math.min(1, style.pressureMinimum || 0));
      pressureFactor = pressureMinimum + (1 - pressureMinimum) * pressureCurve;
    }
    let velocityFactor = 1;
    if (style?.velocityEnabled) {
      const velocityCurve = evaluateBrushGraph(style.velocityGraph, velocity);
      const velocityMinimum = Math.max(0, Math.min(1, style.velocityMinimum || 0));
      velocityFactor = velocityMinimum + (1 - velocityMinimum) * velocityCurve;
    }
    width = Math.max(0.01, strokeInfo.baseSize * pressureFactor * velocityFactor);
    thickness = style?.thickness || 1;
    rotation = style?.rotation || 0;
  }
  return {
    x,
    y,
    width,
    pressure,
    velocity,
    tilt: Number.isFinite(tilt) ? tilt : 0,
    thickness,
    rotation,
  };
}

function decodeClipVector(source, entries, canvasWidth, canvasHeight, brushStyles = {}) {
  const maximumX = Math.max(4096, Math.ceil(canvasWidth * 2));
  const maximumY = Math.max(4096, Math.ceil(canvasHeight * 2));
  const segments = [];
  let pointCount = 0;
  for (const entry of entries || []) {
    const data = readExternalChunk(source, entry.offset, entry.externalId);
    let offset = 0;
    while (offset + MIN_VECTOR_HEADER_SIZE <= data.length && pointCount < MAX_VECTOR_POINTS) {
      const strokeInfo = vectorStrokeInfo(data, offset, brushStyles);
      if (!strokeInfo) {
        offset += 4;
        continue;
      }
      const segment = [];
      const available = Math.min(
        strokeInfo.pointCount,
        MAX_VECTOR_POINTS - pointCount,
      );
      for (let index = 0; index < available; index += 1) {
        const pointOffset = offset + strokeInfo.headerSize +
          index * strokeInfo.pointStride;
        const point = vectorRecord(
          data,
          pointOffset,
          maximumX,
          maximumY,
          strokeInfo,
        );
        if (point) segment.push(point);
        pointCount += 1;
      }
      if (segment.length) segments.push(segment);
      offset += strokeInfo.headerSize +
        strokeInfo.pointCount * strokeInfo.pointStride;
    }
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
  extractClipBorderSources,
  extractClipBrushStyles,
  extractClipMaskSources,
  extractClipObjectSources,
  extractClipRasterSources,
  extractClipThumbnailSources,
  extractClipVectorSources,
  parseChunkBlocks,
  parseOffscreenAttributes,
};
