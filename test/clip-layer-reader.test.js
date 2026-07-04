"use strict";

const assert = require("assert");
const zlib = require("zlib");
const initSqlJs = require("sql.js");
const {
  decodeClipLayer,
  decodeClipVector,
  extractClipRasterSources,
  extractClipVectorSources,
  parseChunkBlocks,
  parseOffscreenAttributes,
} = require("../src/clip-layer-reader");

function u32(value) {
  const output = Buffer.alloc(4);
  output.writeUInt32BE(value);
  return output;
}

function u64(value) {
  const output = Buffer.alloc(8);
  output.writeBigUInt64BE(BigInt(value));
  return output;
}

function utf16be(value) {
  return Buffer.from(value, "utf16le").swap16();
}

function text(value) {
  return Buffer.concat([u32(value.length), utf16be(value)]);
}

function makeAttribute() {
  const packing = Array(16).fill(0);
  packing[1] = 1;
  packing[2] = 4;
  return Buffer.concat([
    u32(16),
    u32(102),
    u32(42),
    u32(0),
    text("Parameter"),
    u32(2),
    u32(1),
    u32(1),
    u32(1),
    ...packing.map(u32),
    text("InitColor"),
    u32(0),
    u32(0),
    u32(0),
    u32(0),
    u32(0),
  ]);
}

function makeBlockChunk() {
  const tilePixels = 256 * 256;
  const pixels = Buffer.alloc(tilePixels * 5);
  pixels[0] = 255;
  pixels[tilePixels] = 10;
  pixels[tilePixels + 1] = 20;
  pixels[tilePixels + 2] = 30;
  const compressed = zlib.deflateSync(pixels);
  const block = Buffer.concat([
    u32(0),
    u32(0),
    u32(0),
    u32(0),
    u32(1),
    u32(compressed.length + 4),
    u32(compressed.length),
    compressed,
  ]);
  const begin = utf16be("BlockDataBeginChunk");
  const end = utf16be("BlockDataEndChunk");
  const blockSize = 8 + begin.length + block.length + 4 + end.length;
  return Buffer.concat([
    u32(blockSize),
    u32(0),
    begin,
    block,
    u32(17),
    end,
  ]);
}

function makeExternalChunk(id, binary) {
  const idBytes = Buffer.from(id, "ascii");
  const chunk = Buffer.concat([
    u64(idBytes.length),
    idBytes,
    u64(binary.length),
    binary,
  ]);
  return Buffer.concat([
    Buffer.from("CHNKExta", "ascii"),
    u32(0),
    u32(chunk.length),
    chunk,
  ]);
}

function makeVectorRecord(x, y, width = 8) {
  const output = Buffer.alloc(88);
  output.writeUInt32BE(Math.floor(x - width / 2), 0);
  output.writeUInt32BE(Math.floor(y - width / 2), 4);
  output.writeUInt32BE(Math.ceil(x + width / 2), 8);
  output.writeUInt32BE(Math.ceil(y + width / 2), 12);
  output.writeFloatBE(0, 28);
  output.writeFloatBE(0, 32);
  output.writeFloatBE(0, 36);
  output.writeDoubleBE(x, 72);
  output.writeDoubleBE(y, 80);
  return output;
}

async function run() {
  const attribute = makeAttribute();
  const parsed = parseOffscreenAttributes(attribute);
  assert.deepEqual(
    [parsed.width, parsed.height, parsed.gridWidth, parsed.gridHeight],
    [2, 1, 1, 1],
  );

  const blockChunk = makeBlockChunk();
  const blocks = parseChunkBlocks(blockChunk);
  assert.equal(blocks.length, 1);
  assert(blocks[0].length > 0);

  const externalId = "extrnlid00000000000000000000000000000001";
  const source = makeExternalChunk(externalId, blockChunk);
  const decoded = decodeClipLayer(source, [{
    attribute,
    externalId: Buffer.from(externalId).toString("hex"),
    offset: 0,
    scale: 100,
    width: 2,
    height: 1,
  }], 58);
  assert.equal(decoded.width, 2);
  assert.equal(decoded.height, 1);
  assert.deepEqual([...decoded.data.subarray(0, 4)], [30, 20, 10, 255]);

  const vectorId = "extrnlid00000000000000000000000000000002";
  const vectorBinary = Buffer.concat([
    makeVectorRecord(20, 24),
    makeVectorRecord(26, 30),
    Buffer.alloc(12, 0xff),
    makeVectorRecord(70, 72),
    makeVectorRecord(76, 78),
  ]);
  const vector = decodeClipVector(
    makeExternalChunk(vectorId, vectorBinary),
    [{
      externalId: Buffer.from(vectorId).toString("hex"),
      offset: 0,
    }],
    100,
    100,
  );
  assert.equal(vector.segments.length, 2);
  assert.deepEqual(
    vector.segments.map((segment) => segment.length),
    [2, 2],
  );

  const SQL = await initSqlJs({
    locateFile: (file) => require.resolve(`sql.js/dist/${file}`),
  });
  const db = new SQL.Database();
  db.run("CREATE TABLE Layer (MainId INTEGER, LayerRenderMipmap INTEGER)");
  db.run("CREATE TABLE Mipmap (MainId INTEGER, BaseMipmapInfo INTEGER)");
  db.run(`CREATE TABLE MipmapInfo (
    MainId INTEGER, ThisScale REAL, Offscreen INTEGER, NextIndex INTEGER
  )`);
  db.run("CREATE TABLE Offscreen (MainId INTEGER, Attribute BLOB, BlockData BLOB)");
  db.run("CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER)");
  db.run("CREATE TABLE VectorObjectList (LayerId INTEGER, VectorData BLOB)");
  db.run("INSERT INTO Layer VALUES (7, 10)");
  db.run("INSERT INTO Mipmap VALUES (10, 20)");
  db.run("INSERT INTO MipmapInfo VALUES (20, 100, 30, 0)");
  const offscreenInsert = db.prepare("INSERT INTO Offscreen VALUES (30, ?, ?)");
  offscreenInsert.run([attribute, Buffer.from(externalId)]);
  offscreenInsert.free();
  const externalInsert = db.prepare("INSERT INTO ExternalChunk VALUES (?, 0)");
  externalInsert.run([Buffer.from(externalId)]);
  externalInsert.free();
  const vectorInsert = db.prepare("INSERT INTO VectorObjectList VALUES (7, ?)");
  vectorInsert.run([Buffer.from(vectorId)]);
  vectorInsert.free();
  const vectorExternalInsert = db.prepare("INSERT INTO ExternalChunk VALUES (?, 100)");
  vectorExternalInsert.run([Buffer.from(vectorId)]);
  vectorExternalInsert.free();
  const sources = extractClipRasterSources(db);
  const vectorSources = extractClipVectorSources(db);
  db.close();
  assert.equal(sources["7"].length, 1);
  assert.equal(sources["7"][0].width, 2);
  assert.equal(vectorSources["7"].length, 1);
  assert.equal(vectorSources["7"][0].offset, 100);

  console.log("clip layer reader tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
