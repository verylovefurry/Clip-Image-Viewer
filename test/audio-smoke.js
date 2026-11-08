"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

async function run() {
  const root = path.resolve(os.tmpdir());
  const directory = fs.mkdtempSync(path.join(root, "clipview-audio-"));
  try {
    const media = process.argv[2] ? path.resolve(process.argv[2]) : path.join(directory, "media");
    if (!process.argv[2]) {
      fs.mkdirSync(media);
      const samples = 44100 * 8;
      const wav = Buffer.alloc(44 + samples * 2);
      wav.write("RIFF"); wav.writeUInt32LE(wav.length - 8, 4); wav.write("WAVEfmt ", 8);
      wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22);
      wav.writeUInt32LE(44100, 24); wav.writeUInt32LE(88200, 28);
      wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34);
      wav.write("data", 36); wav.writeUInt32LE(samples * 2, 40);
      for (let i = 0; i < samples; i += 1) wav.writeInt16LE(Math.round(Math.sin(i * Math.PI * 880 / 44100) * 500), 44 + i * 2);
      fs.writeFileSync(path.join(media, "track1.wav"), wav);
      fs.writeFileSync(path.join(media, "track2.wave"), wav);
    }
    const { listFolder } = require("../src/file-types");
    const first = listFolder(media).find(item => item.mediaType === "audio");
    if (!first) throw new Error("No audio fixtures found");
    const env = { ...process.env, CLIPVIEW_USER_DATA_DIR: path.join(directory, "profile"),
      CLIPVIEW_SMOKE_AUDIO: "1", CLIPVIEW_SMOKE_EXPECT_VIDEO: "1", CLIPVIEW_SMOKE_VIDEO_SHORTCUT: "1" };
    delete env.ELECTRON_RUN_AS_NODE;
    await new Promise((resolve, reject) => {
      const child = spawn(require("electron"), [".", first.path, "--smoke-test",
        "--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding"], { cwd: path.join(__dirname, ".."), env, stdio: "inherit", windowsHide: true });
      let timedOut = false;
      const timeout = setTimeout(() => { timedOut = true; child.kill(); }, 120000);
      child.once("error", error => { clearTimeout(timeout); reject(error); });
      child.once("close", code => {
        clearTimeout(timeout);
        if (timedOut || code !== 0) reject(new Error(`Audio smoke exited with ${code}`));
        else resolve();
      });
    });
  } finally {
    const resolved = path.resolve(directory);
    if (resolved.startsWith(root + path.sep) && path.basename(resolved).startsWith("clipview-audio-")) {
      fs.rmSync(resolved, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    }
  }
}
run().catch(error => { console.error(error); process.exitCode = 1; });
