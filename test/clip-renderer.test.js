"use strict";

const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const zlib = require("zlib");
const sharp = require("sharp");
const initSqlJs = require("sql.js");
const { extractClipData, renderLayeredImage } = require("../src/image-loader");

function u32(value, little = false) {
  const data = Buffer.alloc(4);
  if (little) data.writeUInt32LE(value); else data.writeUInt32BE(value);
  return data;
}
function u64(value) { const data = Buffer.alloc(8); data.writeBigUInt64BE(BigInt(value)); return data; }
function name(value) { return Buffer.concat([u32(value.length), Buffer.from(value, "utf16le").swap16()]); }
function attribute(width, height, mask = false) {
  const packing = Array(16).fill(0);
  packing[1] = 1; packing[2] = mask ? 0 : 4; packing[8] = mask ? 32 : 0;
  return Buffer.concat([u32(16), u32(102), u32(42), u32(0), name("Parameter"),
    ...[width, height, 1, 1, ...packing].map((value) => u32(value)), name("InitColor"),
    ...[20, mask ? 1 : 0, mask ? 0xffffffff : 0, 0, 4].map((value) => u32(value))]);
}
function block(pixels) {
  const compressed = pixels ? zlib.deflateSync(pixels) : null;
  const body = compressed
    ? Buffer.concat([...[0, 0, 0, 0, 1, compressed.length + 4, compressed.length].map((value) => u32(value)), compressed])
    : Buffer.concat([0, 0, 0, 0, 0].map((value) => u32(value)));
  const begin = Buffer.from("BlockDataBeginChunk", "utf16le").swap16();
  const end = Buffer.from("BlockDataEndChunk", "utf16le").swap16();
  return Buffer.concat([u32(8 + begin.length + body.length + 4 + end.length), u32(0), begin, body, u32(17), end]);
}
function pixels(width, height, color) {
  const data = Buffer.alloc(256 * 256 * 5);
  for (let y = 0; y < height; y += 1) for (let x = 0; x < width; x += 1) {
    const index = y * 256 + x;
    data[index] = color[3];
    data[65536 + index * 4] = color[2];
    data[65536 + index * 4 + 1] = color[1];
    data[65536 + index * 4 + 2] = color[0];
  }
  return data;
}

async function createFixture(file, emptyText = false) {
  const SQL = await initSqlJs();
  const db = new SQL.Database();
  const chunks = [];
  let offset = 24;
  db.run("CREATE TABLE Offscreen (MainId INTEGER, LayerId INTEGER, Attribute BLOB, BlockData BLOB)");
  db.run("CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER)");
  db.run("CREATE TABLE Mipmap (MainId INTEGER, BaseMipmapInfo INTEGER)");
  db.run("CREATE TABLE MipmapInfo (MainId INTEGER, ThisScale REAL, Offscreen INTEGER, NextIndex INTEGER)");
  const surface = (id, owner, width, height, data, mask = false, mipmap = true) => {
    const externalId = Buffer.from(`extrnlid${String(id).padStart(32, "0")}`);
    const binary = block(data);
    const body = Buffer.concat([u64(externalId.length), externalId, u64(binary.length), binary]);
    const chunk = Buffer.concat([Buffer.from("CHNKExta"), u64(body.length), body]);
    db.run("INSERT INTO Offscreen VALUES (?, ?, ?, ?)", [id, owner, attribute(width, height, mask), externalId]);
    db.run("INSERT INTO ExternalChunk VALUES (?, ?)", [externalId, offset]);
    if (mipmap) {
      db.run("INSERT INTO Mipmap VALUES (?, ?)", [id, id]);
      db.run("INSERT INTO MipmapInfo VALUES (?, 100, ?, 0)", [id, id]);
    }
    chunks.push(chunk); offset += chunk.length;
  };
  surface(103, 3, 8, 8, pixels(8, 8, [20, 40, 80, 255]));
  surface(105, 5, 8, 8, pixels(8, 8, [0, 255, 0, 255]));
  surface(205, 5, 8, 8, Buffer.alloc(8192), true);
  surface(104, 4, 1, 1, emptyText ? null : pixels(1, 1, [255, 0, 0, 255]), false, false);
  db.run("CREATE TABLE Canvas (CanvasWidth REAL, CanvasHeight REAL, CanvasResolution REAL, CanvasRootFolder INTEGER)");
  db.run("INSERT INTO Canvas VALUES (8, 8, 72, 2)");
  db.run(`CREATE TABLE Layer (_PW_ID INTEGER PRIMARY KEY, MainId INTEGER, LayerName TEXT,
    LayerType INTEGER, LayerFolder INTEGER, LayerVisibility INTEGER, LayerOpacity INTEGER,
    LayerComposite INTEGER, LayerClip INTEGER, LayerFirstChildIndex INTEGER, LayerNextIndex INTEGER,
    LayerRenderMipmap INTEGER, LayerLayerMaskMipmap INTEGER, TextLayerType INTEGER,
    TextLayerString BLOB, TextLayerAttributes BLOB, LayerEffectAttached INTEGER, LayerEffectInfo BLOB)`);
  const add = (id, values = {}) => {
    const row = { _PW_ID: id, MainId: id, LayerName: String(id), LayerType: 1, LayerFolder: 0,
      LayerVisibility: 1, LayerOpacity: 256, LayerComposite: 0, LayerClip: 0,
      LayerFirstChildIndex: 0, LayerNextIndex: 0, LayerRenderMipmap: 0, LayerLayerMaskMipmap: 0, ...values };
    const keys = Object.keys(row);
    db.run(`INSERT INTO Layer (${keys.join(",")}) VALUES (${keys.map(() => "?").join(",")})`, keys.map((key) => row[key]));
  };
  add(2, { LayerType: 256, LayerFolder: 1, LayerFirstChildIndex: 3 });
  add(3, { LayerRenderMipmap: 103, LayerNextIndex: 5 });
  add(5, { LayerType: 3, LayerVisibility: 3, LayerRenderMipmap: 105, LayerLayerMaskMipmap: 205, LayerNextIndex: 4 });
  const section = (id, data) => Buffer.concat([u32(id, true), u32(data.length, true), data]);
  const text = Buffer.concat([section(31, Buffer.from("An uninstalled font")), section(32, u32(9900, true)),
    section(42, Buffer.concat([3, 3, 3, 3].map((value) => u32(value, true)))), section(50, u32(104, true))]);
  const radius = Buffer.alloc(8); radius.writeDoubleBE(1);
  const effect = Buffer.concat([u32(0), name("EffectEdge"), u32(1), radius, u32(0xffffffff), u32(0xffffffff), u32(0xffffffff)]);
  add(4, { LayerType: 0, TextLayerType: 0, TextLayerString: Buffer.from("Not redrawn"), TextLayerAttributes: text,
    LayerEffectAttached: 1, LayerEffectInfo: effect });
  const preview = await sharp({ create: { width: 8, height: 8, channels: 4, background: "black" } }).png().toBuffer();
  db.run("CREATE TABLE CanvasPreview (ImageData BLOB)");
  db.run("INSERT INTO CanvasPreview VALUES (?)", [preview]);
  const sql = Buffer.from(db.export()); db.close();
  const sqlChunk = Buffer.concat([Buffer.from("CHNKSQLi"), u64(sql.length), sql]);
  const length = offset + sqlChunk.length;
  fs.writeFileSync(file, Buffer.concat([Buffer.from("CSFCHUNK"), u64(length), u64(24), ...chunks, sqlChunk]));
}

async function run() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "clip-native-renderer-"));
  try {
    const file = path.join(directory, "native.clip");
    await createFixture(file);
    const before = fs.readFileSync(file);
    const document = await extractClipData(file);
    assert(document.layerDocument.layers.find((layer) => layer.id === "4").textRasterAvailable);
    const item = { kind: "file", path: file, name: "native.clip" };
    const result = await renderLayeredImage(item, { 3: true, 4: true, 5: true }, { preserveOriginal: false });
    const raw = await sharp(Buffer.from(result.dataUrl.split(",")[1], "base64")).ensureAlpha().raw().toBuffer();
    const pixel = (x, y) => [...raw.subarray((y * 8 + x) * 4, (y * 8 + x + 1) * 4)];
    assert.deepEqual(pixel(0, 0), [20, 40, 80, 255], "a black 1-bit mask hides the green layer");
    assert.deepEqual(pixel(3, 3), [255, 0, 0, 255], "native text cache pixels are not redrawn or covered by their outline");
    assert.deepEqual(pixel(2, 3), [255, 255, 255, 255], "a white outline expands the opacity mask");
    const hidden = await renderLayeredImage(item, { 3: true, 4: false, 5: true }, { preserveOriginal: false });
    const stats = await sharp(Buffer.from(hidden.dataUrl.split(",")[1], "base64")).stats();
    assert.deepEqual(stats.channels.slice(0, 3).map((channel) => [channel.min, channel.max]), [[20, 20], [40, 40], [80, 80]]);
    assert(before.equals(fs.readFileSync(file)), "reading and toggling never alter the CLIP file");
    const emptyFile = path.join(directory, "empty-text.clip");
    await createFixture(emptyFile, true);
    const emptyText = await renderLayeredImage({ kind: "file", path: emptyFile, name: "empty-text.clip" },
      { 3: true, 4: true, 5: true }, { preserveOriginal: false });
    const emptyStats = await sharp(Buffer.from(emptyText.dataUrl.split(",")[1], "base64")).stats();
    assert.deepEqual(emptyStats.channels.slice(0, 3).map((channel) => [channel.min, channel.max]), [[20, 20], [40, 40], [80, 80]],
      "an empty native text cache does not trigger font reconstruction");
    console.log("Independent CLIP renderer tests passed");
  } finally {
    assert.equal(path.dirname(path.resolve(directory)), path.resolve(os.tmpdir()));
    assert(path.basename(directory).startsWith("clip-native-renderer-"));
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

run().catch((error) => { console.error(error); process.exitCode = 1; });
