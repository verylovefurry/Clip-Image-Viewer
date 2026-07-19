"use strict";

const assert = require("assert");
const zlib = require("zlib");
const initSqlJs = require("sql.js");
const {
  decodeClipLayer,
  decodeClipVector,
  extractClipBrushStyles,
  extractClipRasterSources,
  extractClipThumbnailSources,
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

function makeEmptyBlockChunk() {
  const block = Buffer.concat([
    u32(0),
    u32(0),
    u32(0),
    u32(0),
    u32(0),
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

function makeVectorRecord(x, y, width = 8, pressure = 0, velocity = 0) {
  const output = Buffer.alloc(88);
  output.writeDoubleBE(x, 0);
  output.writeDoubleBE(y, 8);
  output.writeUInt32BE(Math.floor(x - width / 2), 16);
  output.writeUInt32BE(Math.floor(y - width / 2), 20);
  output.writeUInt32BE(Math.ceil(x + width / 2), 24);
  output.writeUInt32BE(Math.ceil(y + width / 2), 28);
  output.writeUInt32BE(0, 32);
  output.writeFloatBE(pressure, 36);
  output.writeFloatBE(0.75, 40);
  output.writeFloatBE(velocity, 44);
  return output;
}

function makeVectorStroke(records, baseSize = 3.5, styleId = 2, pointStride = 88) {
  const header = Buffer.alloc(92);
  header.writeUInt32BE(92, 0);
  header.writeUInt32BE(76, 4);
  header.writeUInt32BE(pointStride, 8);
  header.writeUInt32BE(88, 12);
  header.writeUInt32BE(records.length, 16);
  header.writeUInt32BE(styleId, 76);
  header.writeDoubleBE(baseSize, 80);
  return Buffer.concat([
    header,
    ...records.map((record) => pointStride === record.length
      ? record
      : Buffer.concat([record, Buffer.alloc(pointStride - record.length)])),
  ]);
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
  const emptySource = makeExternalChunk(externalId, makeEmptyBlockChunk());
  assert.equal(decodeClipLayer(emptySource, [{
    attribute,
    externalId: Buffer.from(externalId).toString("hex"),
    offset: 0,
    scale: 100,
    width: 2,
    height: 1,
  }], 58), null);

  const vectorId = "extrnlid00000000000000000000000000000002";
  const vectorBinary = Buffer.concat([
    makeVectorStroke([
      makeVectorRecord(20, 24),
      makeVectorRecord(26, 30),
    ], 4, 0),
    makeVectorStroke([
      makeVectorRecord(70, 72),
      makeVectorRecord(76, 78),
    ], 4, 0),
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
  assert.equal(vector.segments[0][0].width, 4);
  assert.deepEqual(
    [vector.segments[0][0].x, vector.segments[0][0].y],
    [20, 24],
  );
  const extendedVectorId = "extrnlid00000000000000000000000000000004";
  const extendedVector = decodeClipVector(
    makeExternalChunk(extendedVectorId, makeVectorStroke([
      makeVectorRecord(30, 34),
      makeVectorRecord(38, 42),
    ], 4, 0, 104)),
    [{
      externalId: Buffer.from(extendedVectorId).toString("hex"),
      offset: 0,
    }],
    100,
    100,
  );
  assert.equal(extendedVector.segments.length, 1);
  assert.deepEqual(
    extendedVector.segments[0].map((point) => [point.x, point.y]),
    [[30, 34], [38, 42]],
  );
  const pressureVectorId = "extrnlid00000000000000000000000000000003";
  const pressureVector = decodeClipVector(
    makeExternalChunk(pressureVectorId, makeVectorStroke([
      makeVectorRecord(20, 24, 8, 0.5),
      makeVectorRecord(26, 30, 8, 1),
    ])),
    [{
      externalId: Buffer.from(pressureVectorId).toString("hex"),
      offset: 0,
    }],
    100,
    100,
    {
      2: {
        pressureEnabled: true,
        pressureMinimum: 0,
        pressureGraph: [{ x: 0, y: 0 }, { x: 1, y: 1 }],
        velocityEnabled: false,
        velocityMinimum: 1,
        velocityGraph: null,
        thickness: 0.7,
        rotation: 79,
      },
    },
  );
  assert.equal(pressureVector.segments[0][0].pressure, 0.5);
  assert.equal(pressureVector.segments[0][0].width, 1.75);
  assert.equal(pressureVector.segments[0][1].width, 3.5);
  assert.equal(pressureVector.segments[0][0].thickness, 0.7);
  assert.equal(pressureVector.segments[0][0].rotation, 79);

  const SQL = await initSqlJs({
    locateFile: (file) => require.resolve(`sql.js/dist/${file}`),
  });
  const db = new SQL.Database();
  db.run(`CREATE TABLE Layer (
    MainId INTEGER, LayerRenderMipmap INTEGER, LayerRenderThumbnail INTEGER
  )`);
  db.run("CREATE TABLE LayerThumbnail (MainId INTEGER, ThumbnailOffscreen INTEGER)");
  db.run("CREATE TABLE Mipmap (MainId INTEGER, BaseMipmapInfo INTEGER)");
  db.run(`CREATE TABLE MipmapInfo (
    MainId INTEGER, ThisScale REAL, Offscreen INTEGER, NextIndex INTEGER
  )`);
  db.run("CREATE TABLE Offscreen (MainId INTEGER, Attribute BLOB, BlockData BLOB)");
  db.run("CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER)");
  db.run("CREATE TABLE VectorObjectList (LayerId INTEGER, VectorData BLOB)");
  db.run(`CREATE TABLE BrushEffectorGraphData (
    MainId INTEGER, ControlNumber INTEGER, ControlDataSize INTEGER, ControlPoints BLOB
  )`);
  db.run(`CREATE TABLE BrushStyle (
    MainId INTEGER, SizeEffector BLOB, ThicknessBase REAL, RotationBase REAL,
    AntiAlias INTEGER, Hardness REAL, PatternStyle INTEGER
  )`);
  db.run("INSERT INTO Layer VALUES (7, 10, 40)");
  db.run("INSERT INTO LayerThumbnail VALUES (40, 30)");
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
  const graphPoints = Buffer.alloc(32);
  graphPoints.writeDoubleBE(0, 0);
  graphPoints.writeDoubleBE(0, 8);
  graphPoints.writeDoubleBE(1, 16);
  graphPoints.writeDoubleBE(1, 24);
  const graphInsert = db.prepare(
    "INSERT INTO BrushEffectorGraphData VALUES (3, 2, 16, ?)",
  );
  graphInsert.run([graphPoints]);
  graphInsert.free();
  const sizeEffector = Buffer.alloc(12);
  sizeEffector.writeUInt32BE(1, 0);
  sizeEffector.writeFloatBE(0.2, 4);
  sizeEffector.writeUInt32BE(3, 8);
  const brushInsert = db.prepare(
    "INSERT INTO BrushStyle VALUES (2, ?, 0.7, 79, 1, 1, 0)",
  );
  brushInsert.run([sizeEffector]);
  brushInsert.free();
  const brushStyles = extractClipBrushStyles(db);
  const sources = extractClipRasterSources(db);
  const thumbnailSources = extractClipThumbnailSources(db);
  const vectorSources = extractClipVectorSources(db);
  db.close();
  assert.equal(sources["7"].length, 1);
  assert.equal(sources["7"][0].width, 2);
  assert.equal(thumbnailSources["7"][0].width, 2);
  assert.equal(vectorSources["7"].length, 1);
  assert.equal(vectorSources["7"][0].offset, 100);
  assert.equal(Math.round(brushStyles["2"].pressureMinimum * 10), 2);
  assert.equal(brushStyles["2"].pressureEnabled, true);
  assert.equal(brushStyles["2"].thickness, 0.7);
  assert.equal(brushStyles["2"].rotation, 79);
  assert.deepEqual(brushStyles["2"].pressureGraph, [
    { x: 0, y: 0 },
    { x: 1, y: 1 },
  ]);

  console.log("clip layer reader tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
