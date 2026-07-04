"use strict";

const path = require("path");
const { Worker } = require("worker_threads");

class ImageWorkerClient {
  constructor() {
    this.nextId = 1;
    this.pending = new Map();
    this.worker = null;
  }

  ensureWorker() {
    if (this.worker) return;
    const worker = new Worker(path.join(__dirname, "image-worker.js"), {
      resourceLimits: {
        maxOldGenerationSizeMb: 1536,
        maxYoungGenerationSizeMb: 128,
      },
    });
    worker.on("message", (message) => {
      const request = this.pending.get(message.id);
      if (!request) return;
      this.pending.delete(message.id);
      if (message.error) {
        const error = new Error(message.error);
        if (message.stack) error.stack = message.stack;
        request.reject(error);
      } else {
        request.resolve(message.result);
      }
    });
    worker.on("error", (error) => this.handleExit(error));
    worker.on("exit", (code) => {
      if (code !== 0) this.handleExit(new Error(`이미지 워커가 종료되었습니다. (${code})`));
      else this.worker = null;
    });
    this.worker = worker;
  }

  handleExit(error) {
    const requests = [...this.pending.values()];
    this.pending.clear();
    this.worker = null;
    requests.forEach(({ reject }) => reject(error));
  }

  invoke(method, ...args) {
    this.ensureWorker();
    const id = this.nextId;
    this.nextId += 1;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, method, args });
    });
  }

  async close() {
    const worker = this.worker;
    this.worker = null;
    if (worker) await worker.terminate();
  }
}

module.exports = { ImageWorkerClient };
