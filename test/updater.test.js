"use strict";

const assert = require("assert");
const { getLatestPortableRelease, isNewerVersion } = require("../src/updater");

assert.strictEqual(isNewerVersion("1.2.0", "1.1.0"), true);
assert.strictEqual(isNewerVersion("v2.0.0", "1.99.99"), true);
assert.strictEqual(isNewerVersion("1.1.0", "1.1.0"), false);
assert.strictEqual(isNewerVersion("1.0.9", "1.1.0"), false);

const response = {
  ok: true,
  json: async () => ({
    tag_name: "v1.2.0",
    html_url: "https://example.test/release",
    assets: [
      {
        name: "Clip Image Viewer-1.2.0-windows-x64-portable.zip",
        browser_download_url: "https://example.test/portable.zip",
      },
    ],
  }),
};

getLatestPortableRelease(async () => response).then((release) => {
  assert.deepStrictEqual(release, {
    version: "1.2.0",
    assetName: "Clip Image Viewer-1.2.0-windows-x64-portable.zip",
    downloadUrl: "https://example.test/portable.zip",
    releaseUrl: "https://example.test/release",
  });
  console.log("Updater tests passed");
});
