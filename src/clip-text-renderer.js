"use strict";

const sharp = require("sharp");
const { decodeClipLayerForCanvas } = require("./clip-layer-reader");

const renderCaches = new WeakMap();
const MAX_OBJECT_PIXELS = 16 * 1024 * 1024;
const MAX_RENDER_CACHE_BYTES = 48 * 1024 * 1024;

function downsampleFactor(document, width, height) {
  const factor = 2 ** Math.max(0, Math.round(Math.log2(document.nativeHeight / height)));
  if (factor === 1) return document.nativeWidth === width && document.nativeHeight === height ? 1 : 0;
  return Math.abs(document.nativeWidth / factor - width) <= 1 &&
    Math.abs(document.nativeHeight / factor - height) <= 1 ? factor : 0;
}

async function objectSprite(clip, layer, entry, width, height) {
  const document = clip.layerDocument;
  const left = entry.left + (layer.offsetX || 0);
  const top = entry.top + (layer.offsetY || 0);
  if (entry.width * entry.height > MAX_OBJECT_PIXELS) {
    throw new Error("CLIP 텍스트 개체의 저장 이미지가 안전 크기를 초과합니다.");
  }
  const decoded = decodeClipLayerForCanvas(clip.source, [entry], entry.width, entry.height, { strictTiles: true });
  // A valid cache with no populated tiles represents an empty object. A bad
  // external chunk still throws in the decoder and is not treated as empty.
  if (!decoded) return null;
  const factor = downsampleFactor(document, width, height);
  let x;
  let y;
  let input = sharp(decoded.data, { raw: { width: entry.width, height: entry.height, channels: 4 } });
  if (factor > 1) {
    // Keep every object on the document's sampling grid. Resizing each tight
    // glyph box independently would shift antialiasing at fractional origins.
    const margin = factor * 3;
    const padLeft = ((left % factor) + factor) % factor + margin;
    const padTop = ((top % factor) + factor) % factor + margin;
    const paddedWidth = Math.ceil((entry.width + padLeft + margin) / factor) * factor;
    const paddedHeight = Math.ceil((entry.height + padTop + margin) / factor) * factor;
    const padded = await input.extend({
      left: padLeft, top: padTop,
      right: paddedWidth - entry.width - padLeft,
      bottom: paddedHeight - entry.height - padTop,
      background: { r: 0, g: 0, b: 0, alpha: 0 },
    }).png({ compressionLevel: 1 }).toBuffer();
    input = sharp(padded).resize(paddedWidth / factor, paddedHeight / factor, { kernel: "lanczos3" });
    x = (left - padLeft) / factor;
    y = (top - padTop) / factor;
  } else {
    const scaleX = width / document.nativeWidth;
    const scaleY = height / document.nativeHeight;
    if (factor !== 1) input = input.resize(Math.max(1, Math.round(entry.width * scaleX)), Math.max(1, Math.round(entry.height * scaleY)));
    x = Math.round(left * scaleX);
    y = Math.round(top * scaleY);
  }
  let { data, info } = await input.raw().toBuffer({ resolveWithObject: true });
  const cropLeft = Math.max(0, -x);
  const cropTop = Math.max(0, -y);
  const cropWidth = Math.min(info.width - cropLeft, width - Math.max(0, x));
  const cropHeight = Math.min(info.height - cropTop, height - Math.max(0, y));
  if (cropWidth <= 0 || cropHeight <= 0) return null;
  if (cropLeft || cropTop || cropWidth !== info.width || cropHeight !== info.height) {
    ({ data, info } = await sharp(data, { raw: info }).extract({ left: cropLeft, top: cropTop, width: cropWidth, height: cropHeight })
      .raw().toBuffer({ resolveWithObject: true }));
  }
  return { input: data, raw: { width: info.width, height: info.height, channels: 4 }, left: Math.max(0, x), top: Math.max(0, y) };
}

async function renderCachedClipText(clip, layer, width, height) {
  const entries = clip.textSources?.[layer.id] || [];
  if (!layer.textRasterAvailable || entries.length !== layer.textObjectCount) return null;
  let cache = renderCaches.get(clip);
  if (!cache) { cache = { entries: new Map(), bytes: 0 }; renderCaches.set(clip, cache); }
  const key = `${layer.id}:${width}x${height}`;
  const cached = cache.entries.get(key);
  if (cached) { cache.entries.delete(key); cache.entries.set(key, cached); return cached.promise; }
  const record = { bytes: 0, promise: null };
  record.promise = (async () => {
    const composites = [];
    // CSP's object caches already contain glyph shape, object styles and
    // transforms. Do not redraw fonts or apply those transforms a second time.
    for (const entry of [...entries].sort((a, b) => a.objectIndex - b.objectIndex)) {
      const sprite = await objectSprite(clip, layer, entry, width, height);
      if (sprite) composites.push(sprite);
    }
    const png = await sharp({ create: { width, height, channels: 4, background: { r: 0, g: 0, b: 0, alpha: 0 } } })
      .composite(composites).png({ compressionLevel: 1 }).toBuffer();
    if (cache.entries.get(key) !== record) return png;
    record.bytes = png.length;
    cache.bytes += record.bytes;
    while ((cache.bytes > MAX_RENDER_CACHE_BYTES || cache.entries.size > 256) && cache.entries.size) {
      const oldest = cache.entries.keys().next().value;
      cache.bytes -= cache.entries.get(oldest).bytes;
      cache.entries.delete(oldest);
    }
    return png;
  })();
  cache.entries.set(key, record);
  record.promise.catch(() => {
    if (cache.entries.get(key) === record) { cache.entries.delete(key); cache.bytes -= record.bytes; }
  });
  return record.promise;
}

module.exports = { renderCachedClipText };
