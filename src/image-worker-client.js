"use strict";

const path = require("path");
const { Worker } = require("worker_threads");

const MAX_QUEUED_REQUESTS = 32;
const LAYER_METHODS = new Set([
  "loadLayerThumbnail", "prepareLayeredImage", "renderLayeredImage", "pickLayer",
]);

function itemIdentity(item) {
  if (!item) return "";
  return item.kind === "archive-entry"
    ? JSON.stringify([item.archivePath, item.entryName])
    : item.path || "";
}

function requestError(message, code = "ERR_IMAGE_REQUEST_CANCELLED") {
  return Object.assign(new Error(message), { code });
}

class ImageWorkerClient {
  constructor({ WorkerClass = Worker } = {}) {
    this.WorkerClass = WorkerClass;
    this.nextId = 1;
    this.pending = new Map();
    this.queue = [];
    this.running = null;
    this.worker = null;
    this.pumpScheduled = false;
    this.closed = false;
  }

  setActiveItem(item) {
    const identity = itemIdentity(item);
    this.cancelQueued((request) => (
      (request.method === "loadImage" || LAYER_METHODS.has(request.method)) &&
      request.itemIdentity !== identity
    ));
  }

  cancelQueued(predicate) {
    for (const request of [...this.queue]) {
      if (!predicate(request)) continue;
      this.queue.splice(this.queue.indexOf(request), 1);
      this.pending.delete(request.key);
      request.reject(requestError("더 최신 이미지 작업으로 대체되었습니다."));
    }
  }

  ensureWorker() {
    if (this.worker) return;
    const worker = new this.WorkerClass(path.join(__dirname, "image-worker.js"), {
      resourceLimits: {
        maxOldGenerationSizeMb: 1536,
        maxYoungGenerationSizeMb: 128,
      },
    });
    worker.on("message", (message) => {
      if (this.worker !== worker || this.running?.id !== message.id) return;
      const request = this.running;
      this.running = null;
      this.pending.delete(request.key);
      if (message.error) {
        const error = new Error(message.error);
        if (message.stack) error.stack = message.stack;
        request.reject(error);
      } else {
        request.resolve(message.result);
      }
      this.schedulePump();
    });
    worker.on("error", (error) => this.handleExit(worker, error));
    worker.on("exit", (code) => {
      this.handleExit(worker, new Error(`이미지 워커가 종료되었습니다. (${code})`));
    });
    this.worker = worker;
  }

  handleExit(worker, error) {
    if (this.worker !== worker) return;
    const requests = [...this.pending.values()];
    this.pending.clear();
    this.queue = [];
    this.running = null;
    this.worker = null;
    requests.forEach(({ reject }) => reject(error));
  }

  invoke(method, ...args) {
    return this.enqueue(method, args, false);
  }

  invokeBackground(method, ...args) {
    return this.enqueue(method, args, true);
  }

  enqueue(method, args, background) {
    if (this.closed) {
      return Promise.reject(requestError("이미지 워커가 닫혔습니다.", "ERR_IMAGE_WORKER_CLOSED"));
    }
    let key;
    try {
      key = JSON.stringify([method, args]);
    } catch (error) {
      return Promise.reject(error);
    }
    const priority = background || method === "prepareLayeredImage" ? 2
      : method === "loadThumbnail" || method === "loadLayerThumbnail" ? 3 : 0;
    if (method === "loadImage" && !background) this.setActiveItem(args[0]);
    if ((method === "loadImage" && !background) || method === "renderLayeredImage") {
      this.cancelQueued((request) => (
        request.method === method && request.priority === 0 && request.key !== key
      ));
    }
    const existing = this.pending.get(key);
    if (existing) {
      existing.priority = Math.min(existing.priority, priority);
      return existing.promise;
    }
    if (this.queue.length >= MAX_QUEUED_REQUESTS) {
      const replaceable = [...this.queue].reverse().find((request) => request.priority > priority);
      if (replaceable) this.cancelQueued((request) => request === replaceable);
      else return Promise.reject(requestError(
        "이미지 작업 대기열이 가득 찼습니다.", "ERR_IMAGE_QUEUE_FULL",
      ));
    }
    const request = {
      id: this.nextId++, key, method, args, priority, itemIdentity: itemIdentity(args[0]),
    };
    request.promise = new Promise((resolve, reject) => {
      Object.assign(request, { resolve, reject });
    });
    this.pending.set(key, request);
    this.queue.push(request);
    this.schedulePump();
    return request.promise;
  }

  schedulePump() {
    if (this.pumpScheduled || this.closed) return;
    this.pumpScheduled = true;
    queueMicrotask(() => {
      this.pumpScheduled = false;
      this.pump();
    });
  }

  pump() {
    if (this.closed || this.running || !this.queue.length) return;
    // Dispatch only one job so foreground work can bypass queued previews.
    const next = this.queue.reduce((best, request) => (
      request.priority < best.priority ? request : best
    ));
    this.queue.splice(this.queue.indexOf(next), 1);
    this.running = next;
    try {
      this.ensureWorker();
      this.worker.postMessage({ id: next.id, method: next.method, args: next.args });
    } catch (error) {
      this.running = null;
      this.pending.delete(next.key);
      next.reject(error);
      this.schedulePump();
    }
  }

  async close() {
    this.closed = true;
    const worker = this.worker;
    this.handleExit(worker, requestError("이미지 워커가 닫혔습니다.", "ERR_IMAGE_WORKER_CLOSED"));
    if (worker) await worker.terminate();
  }
}

module.exports = { ImageWorkerClient };
