"use strict";

const assert = require("assert");
const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");
const AdmZip = require("adm-zip");
const {
  downloadPortableUpdate,
  getLatestPortableRelease,
  isNewerVersion,
} = require("../src/updater");

assert.strictEqual(isNewerVersion("1.2.0", "1.1.0"), true);
assert.strictEqual(isNewerVersion("v2.0.0", "1.99.99"), true);
assert.strictEqual(isNewerVersion("1.1.0", "1.1.0"), false);
assert.strictEqual(isNewerVersion("1.0.9", "1.1.0"), false);

async function run() {
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
      {
        name: "SHA256SUMS.txt",
        browser_download_url: "https://example.test/SHA256SUMS.txt",
      },
    ],
    }),
  };

  const release = await getLatestPortableRelease(async () => response);
  assert.deepStrictEqual(release, {
    version: "1.2.0",
    assetName: "Clip Image Viewer-1.2.0-windows-x64-portable.zip",
    downloadUrl: "https://example.test/portable.zip",
    checksumUrl: "https://example.test/SHA256SUMS.txt",
    releaseUrl: "https://example.test/release",
  });

  const zip = new AdmZip();
  zip.addFile("ClipImageViewer.exe", Buffer.from("test executable"));
  zip.addFile("resources/app.asar", Buffer.from("test app"));
  const archive = zip.toBuffer();
  const checksum = crypto.createHash("sha256").update(archive).digest("hex");
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "clipview-update-test-"));
  const fetchImpl = async (url) => {
    if (url.endsWith("portable.zip")) {
      return new Response(archive, {
        status: 200,
        headers: { "content-length": String(archive.length) },
      });
    }
    return new Response(
      `${checksum}  Clip Image Viewer-1.2.0-windows-x64-portable.zip\n`,
      { status: 200 },
    );
  };
  const staging = await downloadPortableUpdate({
    release,
    destinationRoot: temp,
    fetchImpl,
  });
  assert(fs.existsSync(path.join(staging, "ClipImageViewer.exe")));
  await assert.rejects(
    downloadPortableUpdate({
      release: { ...release, version: "../unsafe" },
      destinationRoot: temp,
      fetchImpl,
    }),
    /버전 문자열/,
  );
  await assert.rejects(
    downloadPortableUpdate({
      release,
      destinationRoot: path.join(temp, "tampered"),
      fetchImpl: async (url) => (
        url.endsWith("portable.zip")
          ? new Response(archive, { status: 200 })
          : new Response(`${"0".repeat(64)}  ${release.assetName}\n`, { status: 200 })
      ),
    }),
    /SHA-256/,
  );
  fs.rmSync(temp, { recursive: true, force: true });
  console.log("Updater tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
