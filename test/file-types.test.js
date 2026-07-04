"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const {
  BASIC_ASSOCIATION_EXTENSIONS,
  OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS,
  OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  VIDEO_EXTENSIONS,
  VIDEO_MIME_TYPES,
} = require("../src/file-types");

const installer = fs.readFileSync(
  path.join(__dirname, "..", "build", "installer.nsh"),
  "utf8",
);
const registered = new Set(
  [...installer.matchAll(/UnregisterClipImageViewerExtension "([^"]+)"/g)]
    .map((match) => match[1]),
);
assert.deepStrictEqual(
  [...registered].sort(),
  [...SUPPORTED_EXTENSIONS].sort(),
  "installer extension list is out of sync",
);
for (const extension of OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS) {
  assert(!BASIC_ASSOCIATION_EXTENSIONS.has(extension));
}
for (const extension of OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS) {
  assert(VIDEO_EXTENSIONS.has(extension));
}
for (const extension of VIDEO_EXTENSIONS) {
  assert(VIDEO_MIME_TYPES[extension], `missing video MIME type: ${extension}`);
}

console.log("file type tests passed");
