"use strict";

const assert = require("assert");
const { createPatternStroke, stampPattern, mergePatternStroke } = require("../src/clip-brush-renderer");

const surface = createPatternStroke([{ x: 4, y: 4, width: 4 }], 8, 8, 1, 0, 0);
const pattern = { width: 2, height: 2, pixels: new Float32Array([
  255, 0, 0, 255, 255, 0, 0, 255,
  255, 0, 0, 255, 255, 0, 0, 255,
]) };
const budget = { visits: 0 };
stampPattern(surface, pattern, 4, 4, 4, 4, 0, budget);
stampPattern(surface, pattern, 4, 4, 4, 4, 0, budget);
const output = Buffer.alloc(8 * 8 * 4);
mergePatternStroke(output, surface, 8, 0.5);
const pixel = (x, y) => [...output.subarray((y * 8 + x) * 4, (y * 8 + x + 1) * 4)];
assert.deepEqual(pixel(4, 4), [255, 0, 0, 128], "stroke opacity is applied once after overlapping stamps");
assert.deepEqual(pixel(0, 0), [0, 0, 0, 0], "brush pixels stay inside their local bounds");
assert.equal(pixel(2, 2)[0], 255, "bilinear alpha edges keep their original ink colour");
assert.throws(() => stampPattern(surface, pattern, 4, 4, 4, 4, 0, { visits: 100 * 1024 * 1024 }), /안전 처리 한도/);
const offCanvas = createPatternStroke([{ x: -100, y: -100, width: 4 }], 8, 8, 1, 0, 0);
assert.equal(offCanvas.data.length, 0);
stampPattern(offCanvas, pattern, -100, -100, 4, 4, 0, { visits: 0 });
console.log("CLIP brush renderer tests passed");
