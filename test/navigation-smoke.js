"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");
const sharp = require("sharp");
const { writePsdBuffer } = require("ag-psd");

async function run() {
  const temporaryRoot = path.resolve(os.tmpdir());
  const directory = fs.mkdtempSync(path.join(temporaryRoot, "clipview-navigation-"));
  try {
    const media = path.join(directory, "media");
    fs.mkdirSync(media);
    for (let index = 0; index < 64; index += 1) {
      const ext = ["png", "jpeg", "webp", "psd"][index % 4];
      const output = path.join(media, `page${String(index).padStart(3, "0")}.${ext}`);
      const pixels = sharp({
        create: {
          width: 256 + index, height: 192 + index, channels: 4,
          background: { r: index * 3, g: 180, b: 255 - index * 3, alpha: 1 },
        },
      });
      if (ext === "psd") {
        const data = await pixels.raw().toBuffer();
        const imageData = { width: 256 + index, height: 192 + index, data };
        fs.writeFileSync(output, writePsdBuffer({
          width: imageData.width, height: imageData.height, imageData,
          children: [{ name: "Sample layer", left: 0, top: 0, imageData }],
        }));
      } else {
        await pixels.toFormat(ext).toFile(output);
      }
    }
    const env = {
      ...process.env,
      CLIPVIEW_USER_DATA_DIR: path.join(directory, "profile"),
      CLIPVIEW_SMOKE_RAPID_NAVIGATION: "1",
      CLIPVIEW_SMOKE_THUMBNAIL_WINDOW: "1",
    };
    delete env.ELECTRON_RUN_AS_NODE;
    await new Promise((resolve, reject) => {
      const child = spawn(require("electron"), [
        ".", path.join(media, "page000.png"), "--smoke-test",
      ], { cwd: path.join(__dirname, ".."), env, windowsHide: true, stdio: "inherit" });
      let timedOut = false;
      const timeout = setTimeout(() => {
        timedOut = true;
        child.kill();
      }, 120000);
      child.once("error", (error) => {
        clearTimeout(timeout);
        reject(error);
      });
      child.once("close", (code) => {
        clearTimeout(timeout);
        if (timedOut || code !== 0) reject(new Error(`Navigation smoke exited with ${code}`));
        else resolve();
      });
    });
  } finally {
    const resolved = path.resolve(directory);
    if (resolved.startsWith(`${temporaryRoot}${path.sep}`) &&
        path.basename(resolved).startsWith("clipview-navigation-")) {
      fs.rmSync(resolved, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    }
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
