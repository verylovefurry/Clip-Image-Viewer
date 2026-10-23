"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const sharp = require("sharp");
const { decodeClipLayerForCanvas, decodeClipVector } = require("../src/clip-layer-reader");
const { extractClipData, renderLayeredImage, loadLayerThumbnail } = require("../src/image-loader");

const ROOT = path.join(__dirname, "samples", "Tos_Tos_F_ski");
const EXPECTED_OBJECTS = [38, 22, 30, 21, 16, 27, 16, 28, 26, 14];
const flatten = (layers) => (layers || []).flatMap((layer) => [layer, ...flatten(layer.children)]);
const png = (result) => Buffer.from(result.dataUrl.split(",")[1], "base64");
const hash = (file) => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");

function isolatedVisibility(layers, target) {
  const visibility = {};
  const visit = (nodes) => {
    let found = false;
    for (const layer of nodes || []) {
      const nested = visit(layer.children);
      const selected = layer.id === target || nested;
      visibility[layer.id] = selected;
      found ||= selected;
    }
    return found;
  };
  visit(layers);
  return visibility;
}

async function run() {
  if (!fs.existsSync(ROOT)) {
    console.log("Multi-document CLIP tests skipped: private sample copies are not available");
    return;
  }
  let totalObjects = 0; let totalLayers = 0; let totalBalloons = 0; let checkedLayers = 0;
  for (let index = 0; index < EXPECTED_OBJECTS.length; index += 1) {
    const name = `page${String(index + 1).padStart(4, "0")}.clip`;
    const file = path.join(ROOT, name);
    const before = hash(file);
    const clip = await extractClipData(file);
    const layers = flatten(clip.layerDocument.layers);
    const byId = new Map(layers.map((layer) => [layer.id, layer]));
    const visibility = Object.fromEntries(layers.map((layer) => [layer.id, layer.visible]));
    const item = { kind: "file", path: file, name };
    const count = layers.reduce((total, layer) => total + layer.textObjectCount, 0);
    assert.equal(count, EXPECTED_OBJECTS[index], `${name}: all text objects, including compound layers`);
    totalObjects += count; totalLayers += layers.length;
    for (const [owner, entries] of Object.entries(clip.vectorSources)) {
      const layer = byId.get(owner);
      assert(layer, `${name}: vector has an owner`);
      if (layer.type === "text") assert.equal(layer.kind, "balloon", `${name}: balloon/text overlap is intentional`);
      const decoded = decodeClipVector(clip.source, entries, clip.layerDocument.nativeWidth, clip.layerDocument.nativeHeight, clip.brushStyles);
      assert(decoded?.segments.length, `${name}:${owner}: vector decoding`);
      if (layer.kind === "balloon") {
        totalBalloons += 1;
        assert(layer.vectorInfo, `${name}:${owner}: balloon geometry metadata`);
        assert(decoded.strokes.some((stroke) => stroke.closed && stroke.fillColor), `${name}:${owner}: native balloon fills`);
      }
    }
    for (const layer of layers) for (const object of layer.textObjects) {
      const cache = clip.textSources[layer.id]?.find((entry) => entry.objectIndex === object.index);
      assert(cache, `${name}:${layer.id}:${object.index}: owned native text cache`);
      if (object.text) assert(decodeClipLayerForCanvas(clip.source, [cache], cache.width, cache.height), `${name}: native glyph decoding`);
    }
    const baseline = await renderLayeredImage(item, visibility, { preserveOriginal: false });
    assert.equal((await sharp(png(baseline)).metadata()).width, clip.layerDocument.width);
    let thumbnailChecked = false;
    for (const layer of layers) {
      if (layer.type === "group") continue;
      const isolated = await renderLayeredImage(item, isolatedVisibility(clip.layerDocument.layers, layer.id), { preserveOriginal: false });
      const mask = await sharp(png(isolated)).ensureAlpha().extractChannel(3).raw().toBuffer();
      const toggled = await renderLayeredImage(item, { ...visibility, [layer.id]: !layer.visible }, { preserveOriginal: false });
      assert.equal((await sharp(png(toggled)).metadata()).height, clip.layerDocument.height);
      if (layer.visible && !layer.clipping && layer.blendMode === "normal") {
        const [base, changed] = await Promise.all([
          sharp(png(baseline)).ensureAlpha().raw().toBuffer(), sharp(png(toggled)).ensureAlpha().raw().toBuffer(),
        ]);
        for (let pixel = 0; pixel < mask.length; pixel += 1) {
          if (mask[pixel]) continue;
          const offset = pixel * 4;
          if (base[offset] !== changed[offset] || base[offset + 1] !== changed[offset + 1] ||
            base[offset + 2] !== changed[offset + 2] || base[offset + 3] !== changed[offset + 3]) {
            assert.fail(`${name}:${layer.id}: toggle changed pixels outside its own layer`);
          }
        }
      }
      checkedLayers += 1;
      if (!thumbnailChecked && layer.vectorAvailable) {
        const thumbnail = await loadLayerThumbnail(item, layer.id, 58);
        assert.equal((await sharp(Buffer.from(thumbnail.split(",")[1], "base64")).metadata()).width, 58, "vector thumbnails contain PNG bytes, not an unresolved Promise");
        thumbnailChecked = true;
      }
    }
    const restored = await renderLayeredImage(item, visibility);
    assert((await sharp(png(restored)).ensureAlpha().raw().toBuffer()).equals(await sharp(clip.preview).ensureAlpha().raw().toBuffer()), `${name}: restoring saved visibility restores CSP preview`);
    const group = layers.find((layer) => layer.type === "group" && layer.visible && layer.children.length);
    const target = group || layers.find((layer) => layer.visible && layer.type !== "group");
    const hidden = { ...visibility, [target.id]: false };
    const toggle = await renderLayeredImage(item, hidden);
    const started = performance.now();
    for (let repeat = 0; repeat < 10; repeat += 1) assert.strictEqual(await renderLayeredImage(item, hidden), toggle);
    console.log(`${name}: ${layers.length} layers checked; warm toggle ${((performance.now() - started) / 10).toFixed(2)}ms`);
    assert.equal(hash(file), before, `${name}: original sample bytes remain unchanged`);
  }
  assert.equal(totalObjects, 238);
  assert.equal(totalLayers, 96);
  assert(totalBalloons > 10, "coverage includes multiple balloon compositions");
  console.log(`Multi-document CLIP tests passed: 10 files, ${checkedLayers} leaves, ${totalBalloons} balloon layers, ${totalObjects} native text objects`);
}

run().catch((error) => { console.error(error); process.exitCode = 1; });
