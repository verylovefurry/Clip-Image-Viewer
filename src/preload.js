"use strict";

const { contextBridge, ipcRenderer, webUtils } = require("electron");

contextBridge.exposeInMainWorld("clipView", {
  openDialog: (kind) => ipcRenderer.invoke("open-dialog", kind),
  openPath: (targetPath) => ipcRenderer.invoke("open-path", targetPath),
  loadImage: (item, cropMode) => ipcRenderer.invoke("load-image", item, cropMode),
  loadThumbnail: (item) => ipcRenderer.invoke("load-thumbnail", item),
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
  pathForFile: (file) => webUtils.getPathForFile(file),
  onOpenExternalPath: (callback) => {
    ipcRenderer.on("open-external-path", (_event, targetPath) => callback(targetPath));
  },
});
