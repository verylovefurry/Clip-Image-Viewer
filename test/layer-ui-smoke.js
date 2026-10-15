"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

async function run() {
  const sample = path.join(__dirname, "samples", "RecWild1_KR", "page0006.clip");
  if (!fs.existsSync(sample)) {
    console.log("CLIP layer UI smoke skipped: local samples are not present");
    return;
  }
  const temporaryRoot = path.resolve(os.tmpdir());
  const directory = fs.mkdtempSync(path.join(temporaryRoot, "clipview-layer-ui-"));
  try {
    const env = {
      ...process.env,
      CLIPVIEW_USER_DATA_DIR: directory,
      CLIPVIEW_SMOKE_OPEN_LAYERS: "1",
      CLIPVIEW_SMOKE_LAYER_DETAILS: "1",
    };
    delete env.ELECTRON_RUN_AS_NODE;
    await new Promise((resolve, reject) => {
      const child = spawn(require("electron"), [".", sample, "--smoke-test"], {
        cwd: path.join(__dirname, ".."), env, windowsHide: true, stdio: "inherit",
      });
      let timedOut = false;
      const timeout = setTimeout(() => { timedOut = true; child.kill(); }, 120000);
      child.once("error", (error) => { clearTimeout(timeout); reject(error); });
      child.once("close", (code) => {
        clearTimeout(timeout);
        if (timedOut || code !== 0) reject(new Error(`Layer UI smoke exited with ${code}`));
        else resolve();
      });
    });
  } finally {
    const resolved = path.resolve(directory);
    if (resolved.startsWith(`${temporaryRoot}${path.sep}`) && path.basename(resolved).startsWith("clipview-layer-ui-")) {
      fs.rmSync(resolved, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    }
  }
}

run().catch((error) => { console.error(error); process.exitCode = 1; });
