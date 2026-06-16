"use strict";

const assert = require("assert");
const {
  DEFAULT_HEIGHT,
  DEFAULT_WIDTH,
  constrainBounds,
  createDefaultBounds,
} = require("../src/window-state");

const workArea = { x: 0, y: 0, width: 1920, height: 1040 };
const defaults = createDefaultBounds(workArea);
assert.equal(defaults.width, DEFAULT_WIDTH);
assert.equal(defaults.height, DEFAULT_HEIGHT);
assert.equal(defaults.width / defaults.height, 3 / 4);
assert.equal(defaults.x, 600);
assert.equal(defaults.y, 40);

const restored = constrainBounds(
  { x: 140, y: 90, width: 1100, height: 760 },
  workArea,
);
assert.deepEqual(restored, { x: 140, y: 90, width: 1100, height: 760 });

const offscreen = constrainBounds(
  { x: 5000, y: -900, width: 2600, height: 1500 },
  workArea,
);
assert.deepEqual(offscreen, { x: 0, y: 0, width: 1920, height: 1040 });

const smallWorkArea = { x: -1280, y: 0, width: 1280, height: 720 };
const smallDefaults = createDefaultBounds(smallWorkArea);
assert.equal(smallDefaults.width / smallDefaults.height, 3 / 4);
assert(smallDefaults.width <= smallWorkArea.width);
assert(smallDefaults.height <= smallWorkArea.height);

console.log("window-state tests passed");
