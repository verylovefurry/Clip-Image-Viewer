"use strict";

const fs = require("fs");
const path = require("path");
const sharp = require("sharp");
const { extractClipData, renderLayeredImage } = require("../src/image-loader");

function flatten(layers) { return (layers || []).flatMap((layer) => [layer, ...flatten(layer.children)]); }

async function run() {
  const directory = path.resolve(process.argv[2] || path.join(__dirname, "..", "test", "samples", "RecWild1_KR"));
  const output = path.resolve(process.argv[3] || path.join(__dirname, "..", "..", "..", "outputs", "clip-renderer-research"));
  fs.mkdirSync(output, { recursive: true });
  const report = { runtime: "Independent CLIP decoding; no CSP executable/DLL", files: [] };
  for (const file of fs.readdirSync(directory).filter((file) => /^page00(0[6-9]|1[0-9])\.clip$/i.test(file))) {
    const filePath = path.join(directory, file);
    const clip = await extractClipData(filePath);
    const layers = flatten(clip.layerDocument.layers);
    const visibility = Object.fromEntries(layers.map((layer) => [layer.id, layer.visible]));
    const start = performance.now();
    const result = await renderLayeredImage({ kind: "file", path: filePath, name: file }, visibility, { preserveOriginal: false });
    const png = Buffer.from(result.dataUrl.split(",")[1], "base64");
    const [original, rendered] = await Promise.all([sharp(clip.preview).ensureAlpha().raw().toBuffer(), sharp(png).ensureAlpha().raw().toBuffer()]);
    if (original.length !== rendered.length) throw new Error("Reference dimensions differ");
    let changed = 0; let squared = 0; let max = 0;
    for (let offset = 0; offset < original.length; offset += 4) {
      let pixelMax = 0;
      for (let channel = 0; channel < 4; channel += 1) {
        const difference = Math.abs(original[offset + channel] - rendered[offset + channel]);
        squared += difference * difference; pixelMax = Math.max(pixelMax, difference);
      }
      max = Math.max(max, pixelMax);
      if (pixelMax > 2) changed += 1;
    }
    const row = {
      file, layers: layers.length, textObjects: layers.reduce((count, layer) => count + (layer.textObjectCount || 0), 0),
      nativeTextObjects: layers.reduce((count, layer) => count + (layer.textRasterCount || 0), 0),
      pixelExact: max === 0, maxDifference: max, rmse: Number(Math.sqrt(squared / original.length).toFixed(3)),
      changedPixelPercent: Number((changed / (original.length / 4) * 100).toFixed(3)), renderMs: Math.round(performance.now() - start),
    };
    report.files.push(row);
    fs.writeFileSync(path.join(output, file.replace(/\.clip$/i, "-independent.png")), png);
    console.log(JSON.stringify(row));
  }
  fs.writeFileSync(path.join(output, "rendering-report.json"), `${JSON.stringify(report, null, 2)}\n`);
}

run().catch((error) => { console.error(error); process.exitCode = 1; });
