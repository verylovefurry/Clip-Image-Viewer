"use strict";

const assert = require("assert");
const sharp = require("sharp");
const initSqlJs = require("sql.js");
const { extractClipData } = require("../src/image-loader");

function int(value, bigEndian = false) {
  const data = Buffer.alloc(4);
  if (bigEndian) data.writeUInt32BE(value);
  else data.writeUInt32LE(value);
  return data;
}

function parameter(id, data) {
  return Buffer.concat([int(id), int(data.length), data]);
}

function prefixed(entries) {
  return Buffer.concat(entries.map((data) => Buffer.concat([int(data.length), data])));
}

async function run() {
  const SQL = await initSqlJs({ locateFile: (file) => require.resolve(`sql.js/dist/${file}`) });
  const db = new SQL.Database();
  try {
    db.run("CREATE TABLE CanvasPreview (ImageData BLOB)");
    db.run("CREATE TABLE Canvas (CanvasRootFolder INTEGER, CanvasResolution REAL)");
    db.run("INSERT INTO Canvas VALUES (1, 300)");
    const preview = await sharp({ create: { width: 96, height: 64, channels: 4, background: "white" } }).png().toBuffer();
    db.run("INSERT INTO CanvasPreview VALUES (?)", [preview]);
    const columns = [
      "_PW_ID", "MainId", "LayerName", "LayerType", "LayerFolder",
      "LayerFirstChildIndex", "LayerNextIndex", "LayerVisibility", "LayerOpacity",
      "LayerComposite", "LayerLock", "LayerClip", "DraftLayer", "LayerColorTypeIndex",
      "LayerColorTypeBlackChecked", "LayerColorTypeWhiteChecked", "LayerUsePaletteColor",
      "LayerPaletteRed", "LayerPaletteGreen", "LayerPaletteBlue", "TextLayerType",
      "ComicStoryInfoType", "TextLayerString", "TextLayerAttributes", "TextLayerStringArray",
      "TextLayerAttributesArray", "TextLayerAddAttributesV01", "VectorNormalType",
      "VectorNormalStrokeIndex", "VectorNormalFillIndex", "VectorNormalBalloonIndex",
      "ComicFrameLineMipmap", "ResizableOriginalMipmap", "LayerLayerMaskMipmap",
      "LayerEffectAttached", "LayerEffectInfo", "FilterLayerV132", "SpecialRenderType",
    ];
    db.run(`CREATE TABLE Layer (${columns.map((name) => `"${name}"`).join(", ")})`);

    const textRun = Buffer.alloc(34);
    textRun.writeUInt32LE(1, 0);
    textRun.writeInt32LE(3, 8);
    textRun.writeInt32LE(18, 12);
    textRun.writeUInt8(1, 16);
    textRun.writeDoubleLE(100, 24);
    const textAttributes = Buffer.concat([
      parameter(11, textRun),
      parameter(31, Buffer.from("Test Font", "utf8")),
      parameter(32, int(1800)),
      parameter(33, int(16)),
      parameter(35, int(2)),
      parameter(42, Buffer.concat([1, 2, 30, 40].map((value) => int(value)))),
    ]);
    const edgeWidth = Buffer.alloc(8);
    edgeWidth.writeDoubleBE(3);
    const edge = Buffer.concat([
      int(10, true), Buffer.from("EffectEdge", "utf16le").swap16(),
      int(1, true), edgeWidth, ...[0xffffffff, 0xffffffff, 0xffffffff].map((value) => int(value, true)),
    ]);
    const layers = [
      { MainId: 1, LayerType: 256, LayerFolder: 1, LayerFirstChildIndex: 2 },
      { MainId: 2, LayerType: 1, LayerLock: 16, LayerOpacity: 255, LayerColorTypeIndex: 2, LayerColorTypeBlackChecked: 1, LayerColorTypeWhiteChecked: 1 },
      { MainId: 3, VectorNormalType: 0 },
      { MainId: 4 },
      { MainId: 5, TextLayerType: 0, TextLayerString: Buffer.from("abc"), TextLayerAttributes: textAttributes,
        TextLayerStringArray: prefixed([Buffer.from("def")]), TextLayerAttributesArray: prefixed([textAttributes]),
        TextLayerAddAttributesV01: prefixed([textAttributes, textAttributes]) },
      { MainId: 6, LayerType: 1, TextLayerType: 0 },
      { MainId: 7, LayerType: 800, TextLayerType: 2, ComicStoryInfoType: 2 },
      { MainId: 8, LayerType: 800, TextLayerType: 2, ComicStoryInfoType: 1 },
      { MainId: 9, LayerFolder: 17 },
      { MainId: 10, LayerFolder: 1, VectorNormalType: 3, VectorNormalBalloonIndex: 7, ComicFrameLineMipmap: 7 },
      { MainId: 11, LayerType: 2, ResizableOriginalMipmap: 7 },
      { MainId: 12, LayerType: 4098, SpecialRenderType: 13, FilterLayerV132: int(1, true) },
      { MainId: 13, LayerType: 1, VectorNormalType: 0, LayerEffectAttached: 1, LayerEffectInfo: edge, LayerLock: 1,
        LayerLayerMaskMipmap: 7, LayerVisibility: 1, LayerUsePaletteColor: 1, LayerPaletteRed: 0xffffffff },
      { MainId: 14, LayerType: 999 },
      { MainId: 15, LayerEffectAttached: 1, LayerEffectInfo: edge },
      { MainId: 16 },
      { MainId: 17, LayerType: 1584 },
      { MainId: 18, VectorNormalType: 1, VectorNormalFillIndex: 9 },
    ];
    const insert = db.prepare(`INSERT INTO Layer VALUES (${columns.map(() => "?").join(", ")})`);
    try {
      for (const [index, layer] of layers.entries()) {
        const row = {
          _PW_ID: index + 1, LayerName: `Layer ${layer.MainId}`, LayerType: 0,
          LayerFolder: 0, LayerOpacity: 256, LayerVisibility: 1, LayerComposite: 0,
          LayerNextIndex: index && index < layers.length - 1 ? layers[index + 1].MainId : 0,
          ...layer,
        };
        insert.run(columns.map((name) => row[name] ?? null));
      }
    } finally {
      insert.free();
    }
    db.run("CREATE TABLE VectorObjectList (LayerId INTEGER, VectorData BLOB)");
    db.run("CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER)");
    db.run("INSERT INTO VectorObjectList VALUES (4, ?), (5, ?), (16, ?)", [Buffer.from("v"), Buffer.from("v"), Buffer.from("v")]);
    db.run("INSERT INTO ExternalChunk VALUES (?, 0)", [Buffer.from("v")]);
    db.run("CREATE TABLE Manager3D (LayerId INTEGER)");
    db.run("INSERT INTO Manager3D VALUES (16)");
    db.run("CREATE TABLE RulerSymmetry (LayerId INTEGER)");
    db.run("INSERT INTO RulerSymmetry VALUES (2)");
    db.run("CREATE TABLE Ruler3D (LayerId INTEGER)");
    db.run("INSERT INTO Ruler3D VALUES (2)");
    db.run("CREATE TABLE VectorRulerOptions (LayerId INTEGER)");
    db.run("INSERT INTO VectorRulerOptions VALUES (14)");
    db.run("CREATE TABLE ToneSettings (LayerId INTEGER)");
    db.run("INSERT INTO ToneSettings VALUES (14)");
    const file = Buffer.concat([Buffer.from("CSFCHUNK_TEST_DATA"), Buffer.from(db.export())]);
    const { layerDocument } = await extractClipData(file);
    const byId = new Map(layerDocument.layers.map((layer) => [layer.id, layer]));

    assert.equal(byId.get("2").kind, "raster");
    assert.equal(byId.get("2").locked, false, "alpha lock is not edit lock");
    assert.equal(byId.get("2").alphaLocked, true);
    assert.equal(byId.get("2").opacityExact, 255 / 256 * 100);
    assert.equal(byId.get("2").colorMode.label, "모노크롬");
    assert.equal(byId.get("2").hasRuler, true);
    assert.deepEqual(byId.get("2").rulerTables, ["Ruler3D", "RulerSymmetry"]);
    assert.equal(byId.get("3").kind, "vector", "empty vectors retain their native kind");
    assert.equal(byId.get("3").vectorInfo.kindCode, 0);
    assert.equal(byId.get("4").kind, "vector", "owner links identify older vectors");
    assert.equal(byId.get("4").vectorInfo.dataCount, 1);
    assert.equal(byId.get("5").kind, "text", "text fields take priority over vector preview data");
    assert.equal(byId.get("5").textObjectCount, 2);
    assert.equal(byId.get("5").textObjects[0].attributes.font, "Test Font");
    assert.equal(byId.get("5").textObjects[0].attributes.vertical, true);
    assert.equal(byId.get("5").textObjects[0].attributes.align, 2);
    assert.equal(byId.get("5").textObjects[0].attributes.runs.length, 1);
    assert.equal(byId.get("5").textObjects[0].extendedAttributes.runs.length, 0, "V01 records must not be interpreted as normal character runs");
    assert.equal(byId.get("6").kind, "text");
    assert.equal(byId.get("7").kind, "page-number");
    assert.equal(byId.get("8").kind, "story-information");
    assert.equal(byId.get("9").type, "group", "empty folders remain folders");
    assert.equal(byId.get("10").type, "group", "a frame folder is still a render group");
    assert.equal(byId.get("10").kind, "frame-folder");
    assert.equal(byId.get("11").kind, "image-material");
    assert.equal(byId.get("12").kind, "correction");
    assert.equal(byId.get("13").kind, "raster", "edge effects do not change the base layer kind");
    assert.equal(byId.get("13").vectorInfo, null, "a leftover zero-valued vector field does not change a raster layer");
    assert.equal(byId.get("13").locked, true);
    assert.equal(byId.get("13").mask, true);
    assert.equal(byId.get("13").maskVisible, false);
    assert.deepEqual(byId.get("13").paletteColor, [255, 0, 0]);
    assert.deepEqual(byId.get("13").edgeEffect, { width: 3, color: [255, 255, 255] });
    assert.equal(byId.get("14").kind, "unknown", "similarly named tables are not layer-type evidence");
    assert.equal(byId.get("15").kind, "unknown", "attached effects alone do not identify a correction layer");
    assert.equal(byId.get("16").kind, "3d");
    assert.equal(byId.get("17").kind, "paper");
    assert.equal(byId.get("18").kind, "vector-object");
    console.log("CLIP layer metadata tests passed");
  } finally {
    db.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
