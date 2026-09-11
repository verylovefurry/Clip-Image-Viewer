"use strict";

const assert = require("assert");
const path = require("path");
const { EventEmitter } = require("events");
const { ImageWorkerClient } = require("../src/image-worker-client");

class FakeWorker extends EventEmitter {
  static instances = [];
  constructor() {
    super();
    this.messages = [];
    FakeWorker.instances.push(this);
  }
  postMessage(message) {
    if (message.method === "uncloneable") throw new Error("DataCloneError");
    this.messages.push(message);
  }
  complete(result = true) {
    this.emit("message", { id: this.messages.at(-1).id, result });
  }
  async terminate() {
    this.emit("exit", 1);
  }
}

const tick = () => new Promise((resolve) => setImmediate(resolve));
const item = (name) => ({ kind: "file", path: name, name });

async function testScheduling() {
  const client = new ImageWorkerClient({ WorkerClass: FakeWorker });
  const results = [];
  const watch = (promise) => {
    results.push(promise.catch((error) => error.code));
    return promise;
  };
  try {
    watch(client.invoke("loadImage", item("first.png"), "full"));
    await tick();
    const worker = FakeWorker.instances.at(-1);
    for (let index = 0; index < 32; index += 1) {
      watch(client.invoke("loadThumbnail", item(`thumb${index}.png`)));
    }
    watch(client.invoke("loadImage", item("last.png"), "full"));
    assert.equal(client.queue.length, 32, "queue stays bounded");
    assert.equal(worker.messages.length, 1, "only one request is in flight");
    worker.complete();
    await tick();
    assert.equal(worker.messages.at(-1).args[0].path, "last.png", "foreground bypasses thumbnails");
    while (client.running) {
      worker.complete();
      await tick();
    }
    assert.equal((await Promise.all(results)).filter((result) => result === true).length, 33);
    assert.equal(client.pending.size, 0);
  } finally {
    await client.close();
  }
}

async function testPrefetchPromotion() {
  const client = new ImageWorkerClient({ WorkerClass: FakeWorker });
  const results = [];
  const watch = (promise) => {
    results.push(promise.catch((error) => error.code));
    return promise;
  };
  try {
    watch(client.invoke("loadImage", item("first.png"), "full"));
    await tick();
    const worker = FakeWorker.instances.at(-1);
    watch(client.invoke("prepareLayeredImage", item("first.png")));
    watch(client.invokeBackground("loadImage", item("unused.png"), "full"));
    const prefetch = watch(client.invokeBackground("loadImage", item("next.png"), "full"));
    const foreground = client.invoke("loadImage", item("next.png"), "full");
    assert.strictEqual(prefetch, foreground, "prefetch is reused, not decoded twice");
    assert.equal(client.queue.length, 1, "obsolete prefetch and layer preparation are cancelled");
    assert.equal(client.queue[0].priority, 0, "prefetch becomes foreground work");
    worker.complete();
    await tick();
    worker.complete();
    const values = await Promise.all(results);
    assert.equal(values.filter((value) => value === "ERR_IMAGE_REQUEST_CANCELLED").length, 2);
  } finally {
    await client.close();
  }
}

async function testLifecycle() {
  const client = new ImageWorkerClient({ WorkerClass: FakeWorker });
  try {
    const first = assert.rejects(client.invoke("loadImage", item("first.png")), /crashed/);
    await tick();
    const oldWorker = FakeWorker.instances.at(-1);
    oldWorker.emit("error", new Error("crashed"));
    await first;
    const restarted = client.invoke("loadImage", item("second.png"));
    await tick();
    const newWorker = FakeWorker.instances.at(-1);
    oldWorker.emit("exit", 1);
    assert.strictEqual(client.worker, newWorker, "old exit does not clear the replacement worker");
    newWorker.complete();
    assert.equal(await restarted, true);

    await assert.rejects(client.invoke("uncloneable"), /DataCloneError/);
    assert.equal(client.pending.size, 0, "postMessage failures do not leak requests");

    const running = assert.rejects(client.invoke("loadImage", item("closing.png")), {
      code: "ERR_IMAGE_WORKER_CLOSED",
    });
    await tick();
    const queued = assert.rejects(client.invoke("loadThumbnail", item("queued.png")), {
      code: "ERR_IMAGE_WORKER_CLOSED",
    });
    await client.close();
    await Promise.all([running, queued]);
    await assert.rejects(client.invoke("loadImage", item("closed.png")), {
      code: "ERR_IMAGE_WORKER_CLOSED",
    });
    assert.equal(client.pending.size, 0);
  } finally {
    await client.close();
  }

  const exiting = new ImageWorkerClient({ WorkerClass: FakeWorker });
  const result = assert.rejects(exiting.invoke("loadImage", item("exit.png")), /0/);
  await tick();
  FakeWorker.instances.at(-1).emit("exit", 0);
  await result;
  await exiting.close();
}

async function run() {
  await testScheduling();
  await testPrefetchPromotion();
  await testLifecycle();
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
    const sample = { kind: "file", path: imagePath, name: "sample.png", mediaType: "image" };
    const identical = await Promise.all(Array.from({ length: 64 }, () => (
      client.invoke("loadImage", sample, "full")
    )));
    assert(identical.every((image) => image.dataUrl === result.dataUrl));

    const rapid = await Promise.allSettled(Array.from({ length: 64 }, (_, index) => (
      client.invoke("loadImage", { ...sample, name: `page${index}.png` }, "full")
    )));
    assert.equal(rapid.at(-1).status, "fulfilled", "last selected image always completes");
    assert(rapid.filter((entry) => entry.status === "rejected").every((entry) => (
      entry.reason.code === "ERR_IMAGE_REQUEST_CANCELLED"
    )), "superseded requests are cancelled, not failed by queue overflow");
    assert.equal(client.pending.size, 0);
  } finally {
    await client.close();
  }
  console.log("image worker tests passed");
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
