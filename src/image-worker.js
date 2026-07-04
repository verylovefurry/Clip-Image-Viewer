"use strict";

const { parentPort } = require("worker_threads");
const imageLoader = require("./image-loader");

const ALLOWED_METHODS = new Set([
  "listArchive",
  "loadImage",
  "loadLayerThumbnail",
  "loadThumbnail",
  "pickLayer",
  "prepareLayeredImage",
  "renderLayeredImage",
]);
const MAX_PENDING_REQUESTS = 16;
let pending = 0;
let queue = Promise.resolve();
let latestRenderRequestId = 0;

parentPort.on("message", (message) => {
  const { id, method, args = [] } = message || {};
  if (!ALLOWED_METHODS.has(method)) {
    parentPort.postMessage({ id, error: `허용되지 않은 이미지 작업입니다: ${method}` });
    return;
  }
  if (pending >= MAX_PENDING_REQUESTS) {
    parentPort.postMessage({
      id,
      error: "이미지 작업 요청이 너무 많습니다. 잠시 후 다시 시도하세요.",
    });
    return;
  }

  pending += 1;
  if (method === "renderLayeredImage") latestRenderRequestId = id;
  queue = queue
    .then(async () => {
      try {
        if (method === "renderLayeredImage" && id !== latestRenderRequestId) {
          parentPort.postMessage({ id, error: "더 최신 레이어 표시 요청으로 대체되었습니다." });
          return;
        }
        const result = await imageLoader[method](...args);
        parentPort.postMessage({ id, result });
      } catch (error) {
        parentPort.postMessage({
          id,
          error: error?.message || "이미지 작업에 실패했습니다.",
          stack: error?.stack || "",
        });
      } finally {
        pending -= 1;
      }
    })
    .catch(() => {});
});
