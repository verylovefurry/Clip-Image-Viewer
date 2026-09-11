"use strict";

const { contextBridge, ipcRenderer, webUtils } = require("electron");

async function invokeImageTask(channel, ...args) {
  const result = await ipcRenderer.invoke(channel, ...args);
  if (result?.cancelled) {
    throw Object.assign(new Error("더 최신 이미지 작업으로 대체되었습니다."), {
      code: "ERR_IMAGE_REQUEST_CANCELLED",
    });
  }
  return result;
}

contextBridge.exposeInMainWorld("clipView", {
  openDialog: (kind) => ipcRenderer.invoke("open-dialog", kind),
  openPath: (targetPath) => ipcRenderer.invoke("open-path", targetPath),
  setActiveItem: (item) => ipcRenderer.invoke("set-active-item", item),
  loadImage: (item, cropMode, prefetch = false) => (
    invokeImageTask("load-image", item, cropMode, prefetch)
  ),
  loadThumbnail: (item) => invokeImageTask("load-thumbnail", item),
  loadLayerThumbnail: (item, id) => invokeImageTask("load-layer-thumbnail", item, id),
  renderLayeredImage: (item, visibility) => (
    invokeImageTask("render-layered-image", item, visibility)
  ),
  prepareLayeredImage: (item) => invokeImageTask("prepare-layered-image", item),
  pickLayer: (item, x, y, visibility) => (
    invokeImageTask("pick-layer", item, x, y, visibility)
  ),
  mediaFileUrl: (item) => ipcRenderer.invoke("media-file-url", item),
  findSubtitles: (item) => ipcRenderer.invoke("find-subtitles", item),
  copyImage: (dataUrl) => ipcRenderer.invoke("copy-image", dataUrl),
  saveImageCopy: (dataUrl, name) => ipcRenderer.invoke("save-image-copy", dataUrl, name),
  showInFolder: (item) => ipcRenderer.invoke("show-in-folder", item),
  openOriginal: (item) => ipcRenderer.invoke("open-original", item),
  toggleFullscreen: () => ipcRenderer.invoke("toggle-fullscreen"),
  setAlwaysOnTop: (enabled) => ipcRenderer.invoke("set-always-on-top", enabled),
  getRuntimeInfo: () => ipcRenderer.invoke("get-runtime-info"),
  registerAssociations: (extensions, openSettings) => (
    ipcRenderer.invoke("register-associations", extensions, openSettings)
  ),
  syncAssociations: (extensions) => ipcRenderer.invoke("sync-associations", extensions),
  checkForUpdates: () => ipcRenderer.invoke("check-for-updates"),
  restartAndUpdate: () => ipcRenderer.invoke("restart-and-update"),
  onUpdateState: (callback) => {
    ipcRenderer.on("update-state", (_event, updateState) => callback(updateState));
  },
  onFullscreenState: (callback) => {
    ipcRenderer.on("fullscreen-state", (_event, enabled) => callback(Boolean(enabled)));
  },
  pathForFile: (file) => webUtils.getPathForFile(file),
  onOpenExternalPath: (callback) => {
    ipcRenderer.on("open-external-path", (_event, targetPath) => callback(targetPath));
  },
});
