"use strict";

const assert = require("assert");
const { blendColor, compositeRgba, modeCode } = require("../src/clip-compositor");

const backdrop = [64, 128, 192, 255];
const source = Buffer.from([128, 192, 64, 255]);
for (const [mode, expected] of [
  [0, [128, 192, 64]], [1, [64, 128, 64]], [2, [32, 96, 48]],
  [4, [0, 65, 1]], [5, [0, 0, 128]], [7, [128, 192, 192]],
  [8, [160, 224, 208]], [11, [192, 255, 255]], [12, [192, 255, 255]],
  [21, [64, 64, 128]], [30, [128, 192, 64]],
]) {
  const output = compositeRgba(Buffer.from(backdrop), source, mode);
  assert.deepEqual([...output], [...expected, 255], `blend mode ${mode}`);
}
for (const mode of [...Array(27).keys(), 30, 36]) {
  assert.equal(modeCode(`clip-${mode}`), mode);
  const color = blendColor(mode, [0.2, 0.5, 0.8], [0.8, 0.1, 0.4]);
  assert(color.every((channel) => Number.isFinite(channel) && channel >= -1e-9 && channel <= 1 + 1e-9));
  const transparent = compositeRgba(Buffer.from([0, 0, 0, 0]), source, mode);
  assert.deepEqual([...transparent], [...source], `transparent backdrop mode ${mode}`);
  assert.deepEqual([...compositeRgba(Buffer.from(backdrop), Buffer.from([255, 0, 0, 0]), mode)], backdrop);
}
assert.deepEqual([...source], [128, 192, 64, 255], "source buffer is immutable");
assert.deepEqual([...compositeRgba(Buffer.from([0, 64, 0, 255]), Buffer.from([0, 0, 128, 255]), 36)], [0, 255, 0, 255], "divide keeps a zero backdrop channel black");
assert.deepEqual([...compositeRgba(Buffer.from([0, 0, 0, 0]), Buffer.from([200, 100, 20, 255]), 0, 127 / 256)], [200, 100, 20, 127], "CLIP opacity uses 256 units");
assert.deepEqual([...compositeRgba(Buffer.from([0, 0, 0, 0]), Buffer.from([200, 100, 20, 253]), 0, 128 / 256)], [200, 100, 20, 126], "alpha rounding uses ties-to-even");
assert.deepEqual([...compositeRgba(Buffer.from([10, 20, 30, 128]), Buffer.from([100, 200, 100, 255]), 0, 1, true)], [100, 200, 100, 128], "clipping does not increase the base alpha");
assert.deepEqual([...compositeRgba(Buffer.from([10, 20, 30, 0]), source, 0, 1, true)], [10, 20, 30, 0]);
assert.deepEqual([...compositeRgba(Buffer.from([64, 64, 64, 255]), Buffer.from([255, 255, 255, 128]), 11)], [160, 160, 160, 255]);
assert.deepEqual([...compositeRgba(Buffer.from([64, 64, 64, 255]), Buffer.from([255, 255, 255, 128]), 12)], [192, 192, 192, 255], "add glow uses the premultiplied source, not ordinary add");
assert.throws(() => compositeRgba(Buffer.alloc(4), source, 999), /지원하지 않는/);
assert.throws(() => compositeRgba(Buffer.alloc(8), source), /크기가/);
console.log("CLIP compositor tests passed");
