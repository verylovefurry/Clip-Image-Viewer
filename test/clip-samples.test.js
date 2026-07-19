"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const sharp = require("sharp");
const {
  decodeClipLayerForCanvas,
  decodeClipVector,
} = require("../src/clip-layer-reader");
const {
  extractClipData,
  loadLayerThumbnail,
  renderLayeredImage,
} = require("../src/image-loader");

const SAMPLE_ROOT = path.join(__dirname, "samples", "RecWild1_KR");
if (!fs.existsSync(SAMPLE_ROOT)) {
  console.log("CLIP sample tests skipped: local sample files are not available");
  process.exit(0);
}

const EXPECTED_TEXT_OBJECTS = {
  "page0006.clip": 34,
  "page0007.clip": 29,
  "page0008.clip": 29,
  "page0009.clip": 43,
  "page0010.clip": 39,
  "page0011.clip": 27,
  "page0012.clip": 25,
  "page0013.clip": 57,
  "page0014.clip": 16,
  "page0015.clip": 8,
  "page0016.clip": 32,
  "page0017.clip": 29,
  "page0018.clip": 24,
  "page0019.clip": 23,
};

function flattenLayers(layers, output = []) {
  for (const layer of layers || []) {
    output.push(layer);
    flattenLayers(layer.children, output);
  }
  return output;
}

function isolatedVisibility(layers, targetId) {
  const visibility = {};
  const visit = (items) => {
    let containsTarget = false;
    for (const layer of items || []) {
      const childContainsTarget = visit(layer.children);
      const selected = layer.id === targetId || childContainsTarget;
      visibility[layer.id] = selected;
      containsTarget ||= selected;
    }
    return containsTarget;
  };
  visit(layers);
  return visibility;
}

function documentVisibility(layers, output = {}) {
  for (const layer of layers || []) {
    output[layer.id] = layer.visible !== false;
    documentVisibility(layer.children, output);
  }
  return output;
}

function imageBuffer(result) {
  return Buffer.from(result.dataUrl.split(",")[1], "base64");
}

async function run() {
  if (!fs.existsSync(SAMPLE_ROOT)) {
    console.log("CLIP sample tests skipped: local samples are not installed");
    return;
  }
  const files = Object.keys(EXPECTED_TEXT_OBJECTS);
  for (const file of files) {
    assert(fs.existsSync(path.join(SAMPLE_ROOT, file)), `missing sample: ${file}`);
  }

  const clips = new Map();
  let totalTextObjects = 0;
  let totalLeaves = 0;
  let transformedObjects = 0;
  for (const file of files) {
    const clip = await extractClipData(path.join(SAMPLE_ROOT, file));
    clips.set(file, clip);
    const layers = flattenLayers(clip.layerDocument.layers);
    const layerById = new Map(layers.map((layer) => [layer.id, layer]));
    const textObjects = layers
      .filter((layer) => layer.type === "text")
      .reduce((sum, layer) => sum + layer.textObjects.length, 0);
    assert.equal(textObjects, EXPECTED_TEXT_OBJECTS[file], `${file}: text object count`);
    totalTextObjects += textObjects;
    assert.deepEqual(
      [clip.layerDocument.nativeWidth, clip.layerDocument.nativeHeight],
      [4961, 7016],
      `${file}: physical canvas size`,
    );
    for (const layer of layers) {
      if (layer.folderKind) {
        assert.equal(layer.type, "group", `${file}:${layer.id} empty folder classification`);
      }
      if (layer.type === "text") {
        assert(layer.textObjects.length > 0, `${file}:${layer.id} empty text object list`);
      }
      if (layer.type !== "group") {
        totalLeaves += 1;
        const directlyRenderable = layer.rasterAvailable ||
          layer.vectorAvailable ||
          layer.type === "text" ||
          layer.type === "paper" ||
          Boolean(layer.gradient) ||
          layer.objectRasterAvailable && Boolean(layer.resizable);
        assert(directlyRenderable, `${file}:${layer.id} context-thumbnail-only layer`);
      }
      if (layer.objectRasterAvailable) {
        assert(layer.resizable, `${file}:${layer.id} missing object transform`);
        transformedObjects += 1;
      }
    }
    for (const vectorLayerId of Object.keys(clip.vectorSources)) {
      const owner = layerById.get(vectorLayerId);
      assert(owner, `${file}:${vectorLayerId} orphan vector data`);
      assert.notEqual(owner.type, "text", `${file}:${vectorLayerId} vector/text ownership overlap`);
      assert(decodeClipVector(
        clip.source,
        clip.vectorSources[vectorLayerId],
        clip.layerDocument.nativeWidth,
        clip.layerDocument.nativeHeight,
        clip.brushStyles,
      ), `${file}:${vectorLayerId} vector decoding`);
    }
  }
  assert.equal(totalTextObjects, 415);
  assert.equal(totalLeaves, 482);
  assert.equal(transformedObjects, 19);

  const rasterCases = [
    ["page0006.clip", "9"],
    ["page0006.clip", "303"],
    ["page0007.clip", "98"],
    ["page0009.clip", "52"],
    ["page0015.clip", "75"],
  ];
  for (const [file, layerId] of rasterCases) {
    const clip = clips.get(file);
    const layer = flattenLayers(clip.layerDocument.layers)
      .find((candidate) => candidate.id === layerId);
    const decoded = decodeClipLayerForCanvas(
      clip.source,
      clip.rasterSources[layerId],
      64,
      91,
      {
        canvasWidth: clip.layerDocument.nativeWidth,
        canvasHeight: clip.layerDocument.nativeHeight,
        offsetX: layer.offsetX + layer.renderOffsetX,
        offsetY: layer.offsetY + layer.renderOffsetY,
        color: layer.drawColor,
      },
    );
    assert(decoded, `${file}:${layerId} raster format decoding`);
    assert.equal(decoded.width, 64);
    assert.equal(decoded.height, 91);
  }

  const page = clips.get("page0006.clip");
  const item = {
    kind: "file",
    path: path.join(SAMPLE_ROOT, "page0006.clip"),
    name: "page0006.clip",
  };
  const textOnly = await renderLayeredImage(
    item,
    isolatedVisibility(page.layerDocument.layers, "308"),
    { preserveOriginal: false },
  );
  const textMetadata = await sharp(imageBuffer(textOnly))
    .trim({ background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .metadata();
  assert(textMetadata.width > page.layerDocument.width * 0.55, "multi-object text width");
  assert(textMetadata.height > page.layerDocument.height * 0.55, "multi-object text height");

  const frameOnly = await renderLayeredImage(
    item,
    isolatedVisibility(page.layerDocument.layers, "10"),
    { preserveOriginal: false },
  );
  const frameStats = await sharp(imageBuffer(frameOnly)).ensureAlpha().stats();
  assert(frameStats.channels[3].max > 0, "folder-owned frame vector rendering");

  const recomposedVisibility = documentVisibility(page.layerDocument.layers);
  recomposedVisibility["175"] = true;
  const recomposed = await sharp(imageBuffer(await renderLayeredImage(
    item,
    recomposedVisibility,
    { preserveOriginal: false },
  ))).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  assert.deepEqual(
    [...recomposed.data.subarray(0, 4)],
    [255, 255, 255, 255],
    "frame folder clips child pixels outside panels",
  );

  const page14 = clips.get("page0014.clip");
  const page14Item = {
    kind: "file",
    path: path.join(SAMPLE_ROOT, "page0014.clip"),
    name: "page0014.clip",
  };
  const maskedObject = await sharp(imageBuffer(await renderLayeredImage(
    page14Item,
    isolatedVisibility(page14.layerDocument.layers, "92"),
    { preserveOriginal: false },
  ))).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  assert.equal(maskedObject.data[(700 * maskedObject.info.width + 500) * 4 + 3], 0);
  assert(maskedObject.data[(200 * maskedObject.info.width + 500) * 4 + 3] > 0);

  const isolatedThumbnail = await loadLayerThumbnail(item, "332", 160);
  const thumbnailAlpha = await sharp(Buffer.from(
    isolatedThumbnail.split(",")[1],
    "base64",
  )).ensureAlpha().stats();
  assert(thumbnailAlpha.channels[3].mean < 100, "layer thumbnail is isolated");

  const originalVisibility = documentVisibility(page.layerDocument.layers);
  const hiddenTextLayer = { ...originalVisibility, 308: false };
  const [originalPreview, originalPreservingToggle] = await Promise.all([
    sharp(page.preview).ensureAlpha().raw().toBuffer({ resolveWithObject: true }),
    sharp(imageBuffer(await renderLayeredImage(item, hiddenTextLayer)))
      .ensureAlpha()
      .raw()
      .toBuffer({ resolveWithObject: true }),
  ]);
  let changedPixels = 0;
  for (let offset = 0; offset < originalPreview.data.length; offset += 4) {
    if (
      originalPreview.data[offset] !== originalPreservingToggle.data[offset] ||
      originalPreview.data[offset + 1] !== originalPreservingToggle.data[offset + 1] ||
      originalPreview.data[offset + 2] !== originalPreservingToggle.data[offset + 2] ||
      originalPreview.data[offset + 3] !== originalPreservingToggle.data[offset + 3]
    ) changedPixels += 1;
  }
  assert(changedPixels > 0, "visibility change affects the requested layer");
  assert(
    changedPixels / (page.layerDocument.width * page.layerDocument.height) < 0.08,
    "visibility change preserves unrelated original pixels",
  );

  console.log("CLIP sample tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
