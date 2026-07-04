"use strict";

const assert = require("assert");
const path = require("path");
const { ImageWorkerClient } = require("../src/image-worker-client");

async function run() {
  const client = new ImageWorkerClient();
  try {
    const imagePath = path.join(__dirname, "sample.png");
    const result = await client.invoke("loadImage", {
      kind: "file",
      path: imagePath,
      name: "sample.png",
      mediaType: "image",
    }, "full");
    assert(result.dataUrl.startsWith("data:image/png;base64,"));
    assert(result.metadata.width > 0);
  } finally {
    await client.close();
  }
  console.log("image worker tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
