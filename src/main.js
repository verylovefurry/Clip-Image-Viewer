"use strict";

const {
  app,
  BrowserWindow,
  clipboard,
  dialog,
  ipcMain,
  Menu,
  nativeImage,
  screen,
  shell,
} = require("electron");
const fs = require("fs");
const path = require("path");
const { execFileSync, spawn } = require("child_process");
const {
  ARCHIVE_EXTENSIONS,
  BASIC_ASSOCIATION_EXTENSIONS,
  OPTIONAL_ASSOCIATION_EXTENSIONS,
  PROJECT_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  extensionOf,
  findComicProject,
  isSupported,
  listFolder,
} = require("./file-types");
const { constrainBounds, createDefaultBounds } = require("./window-state");

const ALL_EXTENSIONS = [...SUPPORTED_EXTENSIONS].sort();
const PRODUCT_NAME = "Clip Image Viewer";
const PROG_ID_PREFIX = "ClipImageViewer";
const CAPABILITIES_KEY = "HKCU\\Software\\ClipImageViewer\\Capabilities";
const REGISTERED_APPLICATIONS_KEY = "HKCU\\Software\\RegisteredApplications";
const ASSOCIATION_SETTINGS_KEY = "HKCU\\Software\\ClipImageViewer\\Settings";
let mainWindow;
let pendingOpenPath = null;
let imageLoader;
let installedAutoUpdater;
let updaterInitialization;
let updateCheckPromise;
let portableUpdateStagingPath = "";
let updatePromptVersion = "";
let updateState = {
  status: "idle",
  currentVersion: app.getVersion(),
  version: "",
  percent: 0,
  message: "업데이트 확인 대기 중",
};
const smokeTest = process.argv.includes("--smoke-test");
const smokeListTest = process.argv.includes("--smoke-list-test");
const smokeNavigationTest = process.argv.includes("--smoke-navigation-test");
const smokeSettingsTest = process.argv.includes("--smoke-settings-test");
const updateSmokeTest = process.argv.includes("--update-smoke-test");
const smokeUseWindowState = process.argv.includes("--smoke-use-window-state");
const windowStateSaveTest = process.argv.includes("--window-state-save-test");
const WINDOW_STATE_FILE = "window-state.json";
const userDataOverride = process.env.CLIPVIEW_USER_DATA_DIR;

if (userDataOverride) {
  fs.mkdirSync(userDataOverride, { recursive: true });
  app.setPath("userData", userDataOverride);
}

function parseStartupPath(argv) {
  const startIndex = app.isPackaged ? 1 : 2;
  return argv.find((value, index) => (
    index >= startIndex &&
    !value.startsWith("--") &&
    fs.existsSync(value)
  )) || null;
}

function readWindowState() {
  if (smokeTest && !smokeUseWindowState) return null;
  try {
    return JSON.parse(
      fs.readFileSync(path.join(app.getPath("userData"), WINDOW_STATE_FILE), "utf8"),
    );
  } catch {
    return null;
  }
}

function getInitialWindowState() {
  const saved = readWindowState();
  if (!saved?.bounds) {
    return {
      bounds: createDefaultBounds(screen.getPrimaryDisplay().workArea),
      isMaximized: false,
    };
  }

  const hasSavedPosition = (
    Number.isFinite(saved.bounds.x) &&
    Number.isFinite(saved.bounds.y) &&
    Number.isFinite(saved.bounds.width) &&
    Number.isFinite(saved.bounds.height)
  );
  const display = hasSavedPosition
    ? screen.getDisplayNearestPoint({
        x: Math.round(saved.bounds.x + saved.bounds.width / 2),
        y: Math.round(saved.bounds.y + saved.bounds.height / 2),
      })
    : screen.getPrimaryDisplay();
  return {
    bounds: constrainBounds(saved.bounds, display.workArea),
    isMaximized: Boolean(saved.isMaximized),
  };
}

function saveWindowState() {
  if ((smokeTest && !smokeUseWindowState) || !mainWindow || mainWindow.isDestroyed()) return;
  const statePath = path.join(app.getPath("userData"), WINDOW_STATE_FILE);
  fs.mkdirSync(path.dirname(statePath), { recursive: true });
  fs.writeFileSync(statePath, JSON.stringify({
    bounds: mainWindow.getNormalBounds(),
    isMaximized: mainWindow.isMaximized(),
  }));
}

function createWindow() {
  const initialState = getInitialWindowState();
  const smokeExpectedPath = pendingOpenPath;
  mainWindow = new BrowserWindow({
    ...initialState.bounds,
    minWidth: 320,
    minHeight: 320,
    backgroundColor: "#111318",
    title: PRODUCT_NAME,
    icon: path.join(__dirname, "..", "build", "icon.png"),
    show: false,
    autoHideMenuBar: true,
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });

  Menu.setApplicationMenu(null);
  mainWindow.loadFile(path.join(__dirname, "renderer", "index.html"));
  if (windowStateSaveTest) {
    mainWindow.webContents.once("did-finish-load", () => {
      const workArea = screen.getPrimaryDisplay().workArea;
      mainWindow.setBounds(constrainBounds({
        x: workArea.x + 80,
        y: workArea.y + 60,
        width: 900,
        height: 700,
      }, workArea), false);
      mainWindow.close();
    });
  }
  if (smokeTest) {
    mainWindow.webContents.once("did-finish-load", async () => {
      try {
        await new Promise((resolve) => setTimeout(resolve, 500));
        let title = await mainWindow.webContents.executeJavaScript("document.title");
        if (smokeExpectedPath) {
          for (let attempt = 0; attempt < 100; attempt += 1) {
            const rendered = await mainWindow.webContents.executeJavaScript(`(() => {
              const layer = document.getElementById("imageLayer");
              const loading = document.getElementById("loading");
              const image = document.getElementById("viewerImage");
              return (
                !layer.classList.contains("hidden") &&
                loading.classList.contains("hidden") &&
                image.complete &&
                image.naturalWidth > 0
              );
            })()`);
            title = await mainWindow.webContents.executeJavaScript("document.title");
            if (title !== PRODUCT_NAME && rendered) break;
            await new Promise((resolve) => setTimeout(resolve, 100));
          }
          if (title === PRODUCT_NAME) {
            throw new Error("Image did not finish loading");
          }
        }
        await mainWindow.webContents.executeJavaScript(
          "new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))",
        );
        await new Promise((resolve) => setTimeout(resolve, 250));
        if (smokeListTest) {
          const itemCount = await mainWindow.webContents.executeJavaScript(`(() => {
            document.getElementById("thumbBtn").click();
            const items = document.querySelectorAll(".thumb-item");
            if (items.length > 1) items[1].click();
            return items.length;
          })()`);
          if (itemCount < 2) throw new Error("Thumbnail smoke test needs at least two items");

          const previousTitle = title;
          for (let attempt = 0; attempt < 40; attempt += 1) {
            await new Promise((resolve) => setTimeout(resolve, 100));
            title = await mainWindow.webContents.executeJavaScript("document.title");
            const visible = await mainWindow.webContents.executeJavaScript(
              "!document.getElementById('imageLayer').classList.contains('hidden')",
            );
            if (title !== previousTitle && visible) break;
          }
          if (title === previousTitle) {
            throw new Error("Thumbnail selection did not change the displayed image");
          }
        }
        if (smokeNavigationTest) {
          await mainWindow.webContents.executeJavaScript(`(() => {
            const select = document.getElementById("cropModeSelect");
            select.value = "trim";
            select.dispatchEvent(new Event("change", { bubbles: true }));
          })()`);
          await new Promise((resolve) => setTimeout(resolve, 800));
          const before = await mainWindow.webContents.executeJavaScript(`(() => {
            const select = document.getElementById("cropModeSelect");
            select.focus();
            return { mode: select.value, title: document.title };
          })()`);
          mainWindow.webContents.sendInputEvent({ type: "keyDown", keyCode: "LEFT" });
          mainWindow.webContents.sendInputEvent({ type: "keyUp", keyCode: "LEFT" });
          await new Promise((resolve) => setTimeout(resolve, 30));
          const immediate = await mainWindow.webContents.executeJavaScript(`(() => ({
            loadingHidden: document.getElementById("loading").classList.contains("hidden"),
            mode: document.getElementById("cropModeSelect").value,
            title: document.title,
          }))()`);
          if (immediate.mode !== before.mode) {
            throw new Error(`Arrow key changed crop mode: ${JSON.stringify({ before, immediate })}`);
          }
          if (immediate.title === before.title) {
            throw new Error("Arrow key did not navigate while crop mode was focused");
          }
          if (!immediate.loadingHidden) {
            throw new Error("Adjacent page was not served from the preload cache");
          }
          title = immediate.title;
        }
        if (smokeSettingsTest) {
          await mainWindow.webContents.executeJavaScript(
            "document.getElementById('settingsBtn').click()",
          );
          let settings;
          for (let attempt = 0; attempt < 40; attempt += 1) {
            await new Promise((resolve) => setTimeout(resolve, 50));
            settings = await mainWindow.webContents.executeJavaScript(`(() => ({
              open: document.getElementById("settingsDialog").open,
              count: document.querySelectorAll(".association-extension").length,
              controlsHidden: document.getElementById("associationControls")
                .classList.contains("hidden"),
            }))()`);
            if (settings.count) break;
          }
          if (!settings?.open || settings.count !== OPTIONAL_ASSOCIATION_EXTENSIONS.length) {
            throw new Error(`Association settings failed: ${JSON.stringify(settings)}`);
          }
          const associationControlsExpectedHidden = !fileAssociationSupported();
          if (settings.controlsHidden !== associationControlsExpectedHidden) {
            throw new Error(`Portable association UI failed: ${JSON.stringify(settings)}`);
          }
          await mainWindow.webContents.executeJavaScript(
            "document.getElementById('settingsDialog').close()",
          );
        }
        await mainWindow.webContents.executeJavaScript(
          "new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))",
        );
        const geometry = await mainWindow.webContents.executeJavaScript(`(() => {
          const stage = document.getElementById("stage").getBoundingClientRect();
          const image = document.getElementById("viewerImage");
          const topbar = document.querySelector(".topbar").getBoundingClientRect();
          const bottombar = document.querySelector(".bottombar").getBoundingClientRect();
          if (!image.naturalWidth) return null;
          const imageRect = image.getBoundingClientRect();
          return {
            deltaX: (imageRect.left + imageRect.width / 2) - (stage.left + stage.width / 2),
            deltaY: (imageRect.top + imageRect.height / 2) - (stage.top + stage.height / 2),
            gapX: stage.width - imageRect.width,
            gapY: stage.height - imageRect.height,
            overflowLeft: stage.left - imageRect.left,
            overflowTop: stage.top - imageRect.top,
            overflowRight: imageRect.right - stage.right,
            overflowBottom: imageRect.bottom - stage.bottom,
            topbarGap: stage.top - topbar.bottom,
            bottombarGap: bottombar.top - stage.bottom,
          };
        })()`);
        if (geometry && (Math.abs(geometry.deltaX) > 1 || Math.abs(geometry.deltaY) > 1)) {
          throw new Error(`Image is not centered: ${JSON.stringify(geometry)}`);
        }
        if (geometry && (
          geometry.overflowLeft > 1 ||
          geometry.overflowTop > 1 ||
          geometry.overflowRight > 1 ||
          geometry.overflowBottom > 1
        )) {
          throw new Error(`Image exceeds the viewer area: ${JSON.stringify(geometry)}`);
        }
        if (geometry && Math.min(Math.abs(geometry.gapX), Math.abs(geometry.gapY)) > 2) {
          throw new Error(`Image is not fitted to the viewer area: ${JSON.stringify(geometry)}`);
        }
        if (geometry && (
          Math.abs(geometry.topbarGap) > 1 ||
          Math.abs(geometry.bottombarGap) > 1
        )) {
          throw new Error(`Viewer area does not match the bars: ${JSON.stringify(geometry)}`);
        }
        const navigationGeometry = await mainWindow.webContents.executeJavaScript(`(() => {
          return ["prevOverlay", "nextOverlay", "prevBtn", "nextBtn"].flatMap((id) => {
            const button = document.getElementById(id);
            if (button.classList.contains("hidden")) return [];
            const icon = button.querySelector(".nav-icon");
            const buttonRect = button.getBoundingClientRect();
            const iconRect = icon.getBoundingClientRect();
            const pathBox = icon.querySelector("path").getBBox();
            return [{
              id,
              deltaX: (iconRect.left + iconRect.width / 2) -
                (buttonRect.left + buttonRect.width / 2),
              deltaY: (iconRect.top + iconRect.height / 2) -
                (buttonRect.top + buttonRect.height / 2),
              pathDeltaX: pathBox.x + pathBox.width / 2 - 10,
              pathDeltaY: pathBox.y + pathBox.height / 2 - 10,
            }];
          });
        })()`);
        const misalignedNavigation = navigationGeometry.find((item) => (
          Math.abs(item.deltaX) > 0.1 ||
          Math.abs(item.deltaY) > 0.1 ||
          Math.abs(item.pathDeltaX) > 0.1 ||
          Math.abs(item.pathDeltaY) > 0.1
        ));
        if (misalignedNavigation) {
          throw new Error(
            `Navigation icon is not centered: ${JSON.stringify(misalignedNavigation)}`,
          );
        }
        const fullscreenBeforeNavigationDoubleClick = mainWindow.isFullScreen();
        await mainWindow.webContents.executeJavaScript(`(() => {
          document.getElementById("prevOverlay").dispatchEvent(new MouseEvent("dblclick", {
            bubbles: true,
            cancelable: true,
          }));
        })()`);
        await new Promise((resolve) => setTimeout(resolve, 100));
        if (mainWindow.isFullScreen() !== fullscreenBeforeNavigationDoubleClick) {
          throw new Error("Navigation button double click toggled fullscreen");
        }
        const screenshotPath = process.env.CLIPVIEW_SMOKE_SCREENSHOT;
        if (screenshotPath) {
          const image = await mainWindow.webContents.capturePage();
          fs.writeFileSync(screenshotPath, image.toPNG());
        }
        const bounds = mainWindow.getBounds();
        if (!smokeUseWindowState && Math.abs(bounds.width / bounds.height - 3 / 4) > 0.002) {
          throw new Error(`Initial window is not 3:4: ${JSON.stringify(bounds)}`);
        }
        console.log(
          `${PRODUCT_NAME} smoke test passed: ${title} (${bounds.width}x${bounds.height})`,
        );
        app.exit(0);
      } catch (error) {
        console.error(`${PRODUCT_NAME} smoke test failed: ${error.stack || error.message}`);
        app.exit(1);
      }
    });
  }
  mainWindow.once("ready-to-show", () => {
    if (initialState.isMaximized) mainWindow.maximize();
    mainWindow.show();
    if (pendingOpenPath) {
      mainWindow.webContents.send("open-external-path", pendingOpenPath);
      pendingOpenPath = null;
    }
    if (updateSmokeTest) {
      void checkForUpdates(true).then((result) => {
        if (result.status === "error") {
          console.error(`${PRODUCT_NAME} update smoke test failed: ${result.message}`);
          app.exit(1);
          return;
        }
        console.log(
          `${PRODUCT_NAME} update smoke test passed: ${result.status} (${result.message})`,
        );
        app.exit(0);
      });
    } else if (!smokeTest) {
      setTimeout(() => void checkForUpdates(false), 1800);
    }
  });
  mainWindow.on("close", saveWindowState);
}

function itemForPath(filePath) {
  return {
    kind: ARCHIVE_EXTENSIONS.has(extensionOf(filePath)) ? "archive" : "file",
    path: filePath,
    name: path.basename(filePath),
  };
}

function getImageLoader() {
  if (!imageLoader) imageLoader = require("./image-loader");
  return imageLoader;
}

async function comicCollection(cmcPath, targetPath = null) {
  const { loadComicProject } = require("./comic-loader");
  const collection = await loadComicProject(cmcPath);
  let index = 0;
  if (targetPath) {
    index = collection.items.findIndex(
      (item) => path.resolve(item.path) === path.resolve(targetPath),
    );
  }
  return {
    ...collection,
    index,
    basePath: path.dirname(cmcPath),
  };
}

async function buildCollection(targetPath) {
  const stat = fs.statSync(targetPath);
  if (stat.isDirectory()) {
    const cmcPath = findComicProject(targetPath);
    if (cmcPath) {
      try {
        return await comicCollection(cmcPath);
      } catch {
        // A damaged project must not block opening ordinary images in the folder.
      }
    }
    const items = listFolder(targetPath);
    return { items, index: items.length ? 0 : -1, basePath: targetPath };
  }

  if (PROJECT_EXTENSIONS.has(extensionOf(targetPath))) {
    return comicCollection(targetPath);
  }

  const target = itemForPath(targetPath);
  if (target.kind === "archive") {
    const items = getImageLoader().listArchive(targetPath);
    return { items, index: items.length ? 0 : -1, basePath: targetPath };
  }

  if (!isSupported(targetPath)) {
    throw new Error("지원하지 않는 파일 형식입니다.");
  }

  const folderPath = path.dirname(targetPath);
  const cmcPath = findComicProject(folderPath);
  if (cmcPath) {
    try {
      const collection = await comicCollection(cmcPath, targetPath);
      if (collection.index >= 0) return collection;
    } catch {
      // Fall back to the folder's natural image order.
    }
  }

  const items = listFolder(folderPath).filter((item) => item.kind !== "archive");
  let index = items.findIndex((item) => path.resolve(item.path) === path.resolve(targetPath));
  if (index < 0) {
    items.push(target);
    index = items.length - 1;
  }
  return { items, index, basePath: folderPath };
}

function runReg(args) {
  execFileSync("reg.exe", args, { windowsHide: true, stdio: "ignore" });
}

function unregisterExtension(ext) {
  for (const prefix of [PROG_ID_PREFIX, "ClipView"]) {
    const progId = `${prefix}${ext}`;
    try {
      runReg(["delete", `HKCU\\Software\\Classes\\${ext}\\OpenWithProgids`, "/v", progId, "/f"]);
    } catch {
      // Missing keys are expected.
    }
    try {
      const current = execFileSync(
        "reg.exe",
        ["query", `HKCU\\Software\\Classes\\${ext}`, "/ve"],
        { windowsHide: true, encoding: "utf8" },
      );
      if (current.includes(progId)) {
        runReg(["delete", `HKCU\\Software\\Classes\\${ext}`, "/ve", "/f"]);
      }
    } catch {
      // Missing keys are expected.
    }
    try {
      runReg(["delete", `HKCU\\Software\\Classes\\${progId}`, "/f"]);
    } catch {
      // Missing keys are expected.
    }
  }
}

function registerExtension(ext) {
  const progId = `${PROG_ID_PREFIX}${ext}`;
  const exePath = process.execPath;
  runReg([
    "add",
    `HKCU\\Software\\Classes\\${progId}`,
    "/ve",
    "/d",
    `${PRODUCT_NAME} 이미지`,
    "/f",
  ]);
  runReg([
    "add",
    `HKCU\\Software\\Classes\\${progId}\\DefaultIcon`,
    "/ve",
    "/d",
    `${exePath},0`,
    "/f",
  ]);
  runReg([
    "add",
    `HKCU\\Software\\Classes\\${progId}\\shell\\open\\command`,
    "/ve",
    "/d",
    `"${exePath}" "%1"`,
    "/f",
  ]);
  runReg([
    "add",
    `HKCU\\Software\\Classes\\${ext}\\OpenWithProgids`,
    "/v",
    progId,
    "/d",
    "",
    "/f",
  ]);
}

function registerApplicationCapabilities(extensions) {
  runReg([
    "add",
    REGISTERED_APPLICATIONS_KEY,
    "/v",
    PRODUCT_NAME,
    "/d",
    "Software\\ClipImageViewer\\Capabilities",
    "/f",
  ]);
  runReg(["add", CAPABILITIES_KEY, "/v", "ApplicationName", "/d", PRODUCT_NAME, "/f"]);
  runReg([
    "add",
    CAPABILITIES_KEY,
    "/v",
    "ApplicationDescription",
    "/d",
    "다양한 이미지와 CLIP STUDIO PAINT 문서를 보는 이미지 뷰어",
    "/f",
  ]);
  runReg([
    "add",
    CAPABILITIES_KEY,
    "/v",
    "ApplicationIcon",
    "/d",
    `${process.execPath},0`,
    "/f",
  ]);
  try {
    runReg(["delete", `${CAPABILITIES_KEY}\\FileAssociations`, "/f"]);
  } catch {
    // The key may not exist on the first registration.
  }
  for (const ext of extensions) {
    runReg([
      "add",
      `${CAPABILITIES_KEY}\\FileAssociations`,
      "/v",
      ext,
      "/d",
      `${PROG_ID_PREFIX}${ext}`,
      "/f",
    ]);
  }
}

function unregisterApplicationCapabilities() {
  try {
    runReg(["delete", REGISTERED_APPLICATIONS_KEY, "/v", PRODUCT_NAME, "/f"]);
  } catch {
    // Missing registration is expected.
  }
  try {
    runReg(["delete", CAPABILITIES_KEY, "/f"]);
  } catch {
    // Missing registration is expected.
  }
}

function saveAssociationPreference(enabled) {
  runReg([
    "add",
    ASSOCIATION_SETTINGS_KEY,
    "/v",
    "AssociationsEnabled",
    "/t",
    "REG_DWORD",
    "/d",
    enabled ? "1" : "0",
    "/f",
  ]);
}

function isPortableBuild() {
  return Boolean(process.env.PORTABLE_EXECUTABLE_FILE) ||
    fs.existsSync(path.join(path.dirname(process.execPath), "portable.flag"));
}

function setUpdateState(patch) {
  updateState = { ...updateState, ...patch };
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send("update-state", updateState);
  }
  return updateState;
}

function updateSupported() {
  return app.isPackaged && process.platform === "win32" && (!smokeTest || updateSmokeTest);
}

function fileAssociationSupported() {
  return (
    app.isPackaged &&
    process.platform === "win32" &&
    !isPortableBuild() &&
    !smokeTest &&
    !updateSmokeTest
  );
}

async function promptForUpdateRestart(version) {
  if (!mainWindow || mainWindow.isDestroyed() || updatePromptVersion === version) return;
  updatePromptVersion = version;
  const result = await dialog.showMessageBox(mainWindow, {
    type: "info",
    title: `${PRODUCT_NAME} 업데이트`,
    message: `새 버전 ${version} 다운로드가 완료되었습니다.`,
    detail: "지금 재시작하면 자동으로 새 버전으로 교체됩니다.",
    buttons: ["지금 재시작", "나중에"],
    defaultId: 0,
    cancelId: 1,
    noLink: true,
  });
  if (result.response === 0) installDownloadedUpdate();
}

async function initializeInstalledUpdater() {
  if (installedAutoUpdater) return installedAutoUpdater;
  if (!updaterInitialization) {
    updaterInitialization = Promise.resolve().then(() => {
      const { autoUpdater } = require("electron-updater");
      autoUpdater.autoDownload = true;
      autoUpdater.autoInstallOnAppQuit = true;
      autoUpdater.allowPrerelease = false;
      autoUpdater.on("checking-for-update", () => {
        setUpdateState({
          status: "checking",
          percent: 0,
          message: "새 버전을 확인하는 중...",
        });
      });
      autoUpdater.on("update-available", (info) => {
        setUpdateState({
          status: "available",
          version: info.version,
          percent: 0,
          message: `새 버전 ${info.version}을 다운로드합니다.`,
        });
      });
      autoUpdater.on("update-not-available", () => {
        setUpdateState({
          status: "up-to-date",
          version: "",
          percent: 0,
          message: `최신 버전 ${app.getVersion()}을 사용 중입니다.`,
        });
      });
      autoUpdater.on("download-progress", (progress) => {
        const percent = Math.max(0, Math.min(100, progress.percent || 0));
        setUpdateState({
          status: "downloading",
          percent,
          message: `업데이트 다운로드 중 ${Math.round(percent)}%`,
        });
      });
      autoUpdater.on("update-downloaded", (info) => {
        setUpdateState({
          status: "downloaded",
          version: info.version,
          percent: 100,
          message: `버전 ${info.version} 준비 완료. 재시작하면 업데이트됩니다.`,
        });
        void promptForUpdateRestart(info.version);
      });
      autoUpdater.on("error", (error) => {
        setUpdateState({
          status: "error",
          percent: 0,
          message: `업데이트 실패: ${error?.message || "알 수 없는 오류"}`,
        });
      });
      installedAutoUpdater = autoUpdater;
      return autoUpdater;
    });
  }
  return updaterInitialization;
}

async function checkPortableUpdate() {
  const {
    downloadPortableUpdate,
    getLatestPortableRelease,
    isNewerVersion,
  } = require("./updater");
  setUpdateState({
    status: "checking",
    percent: 0,
    message: "GitHub에서 새 버전을 확인하는 중...",
  });
  const release = await getLatestPortableRelease();
  if (!release.version || !isNewerVersion(release.version, app.getVersion())) {
    return setUpdateState({
      status: "up-to-date",
      version: "",
      percent: 0,
      message: `최신 버전 ${app.getVersion()}을 사용 중입니다.`,
    });
  }

  setUpdateState({
    status: "downloading",
    version: release.version,
    percent: 0,
    message: `포터블 버전 ${release.version} 다운로드 중 0%`,
  });
  portableUpdateStagingPath = await downloadPortableUpdate({
    release,
    destinationRoot: app.getPath("temp"),
    onProgress: (percent) => setUpdateState({
      status: "downloading",
      percent,
      message: percent
        ? `포터블 버전 ${release.version} 다운로드 중 ${Math.round(percent)}%`
        : `포터블 버전 ${release.version} 다운로드 중`,
    }),
  });
  setUpdateState({
    status: "downloaded",
    version: release.version,
    percent: 100,
    message: `버전 ${release.version} 준비 완료. 재시작하면 업데이트됩니다.`,
  });
  void promptForUpdateRestart(release.version);
  return updateState;
}

async function checkForUpdates(manual = false) {
  if (!updateSupported()) {
    return setUpdateState({
      status: "disabled",
      message: app.isPackaged
        ? "자동 업데이트는 Windows 버전에서만 지원합니다."
        : "개발 모드에서는 자동 업데이트를 확인하지 않습니다.",
    });
  }
  if (updateState.status === "downloaded") {
    if (manual) void promptForUpdateRestart(updateState.version);
    return updateState;
  }
  if (updateCheckPromise) return updateCheckPromise;

  updateCheckPromise = (async () => {
    try {
      if (isPortableBuild()) return await checkPortableUpdate();
      const { getLatestPortableRelease, isNewerVersion } = require("./updater");
      setUpdateState({
        status: "checking",
        percent: 0,
        message: "GitHub에서 새 버전을 확인하는 중...",
      });
      const release = await getLatestPortableRelease();
      if (!release.version || !isNewerVersion(release.version, app.getVersion())) {
        return setUpdateState({
          status: "up-to-date",
          version: "",
          percent: 0,
          message: `최신 버전 ${app.getVersion()}을 사용 중입니다.`,
        });
      }
      const updater = await initializeInstalledUpdater();
      await updater.checkForUpdates();
      return updateState;
    } catch (error) {
      return setUpdateState({
        status: "error",
        percent: 0,
        message: `업데이트 실패: ${error?.message || "알 수 없는 오류"}`,
      });
    } finally {
      updateCheckPromise = null;
    }
  })();
  return updateCheckPromise;
}

function launchPortableUpdate() {
  if (!portableUpdateStagingPath) return false;
  const targetPath = path.dirname(process.execPath);
  const executablePath = path.join(targetPath, path.basename(process.execPath));
  const scriptPath = path.join(
    app.getPath("temp"),
    `clip-image-viewer-update-${Date.now()}.ps1`,
  );
  const script = [
    "param(",
    "  [int]$AppProcessId,",
    "  [string]$SourcePath,",
    "  [string]$TargetPath,",
    "  [string]$ExecutablePath,",
    "  [string]$ScriptPath",
    ")",
    "Wait-Process -Id $AppProcessId -ErrorAction SilentlyContinue",
    "& robocopy.exe $SourcePath $TargetPath /E /R:10 /W:1 /NFL /NDL /NJH /NJS /NP",
    "if ($LASTEXITCODE -ge 8) { exit $LASTEXITCODE }",
    "Start-Process -FilePath $ExecutablePath",
    "Remove-Item -LiteralPath (Split-Path $SourcePath) -Recurse -Force -ErrorAction SilentlyContinue",
    "Remove-Item -LiteralPath $ScriptPath -Force -ErrorAction SilentlyContinue",
  ].join("\r\n");
  fs.writeFileSync(scriptPath, script, "utf8");
  const helper = spawn("powershell.exe", [
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-WindowStyle",
    "Hidden",
    "-File",
    scriptPath,
    String(process.pid),
    portableUpdateStagingPath,
    targetPath,
    executablePath,
    scriptPath,
  ], {
    detached: true,
    stdio: "ignore",
    windowsHide: true,
  });
  helper.unref();
  app.quit();
  return true;
}

function installDownloadedUpdate() {
  if (updateState.status !== "downloaded") return false;
  if (isPortableBuild()) return launchPortableUpdate();
  if (!installedAutoUpdater) return false;
  installedAutoUpdater.quitAndInstall(true, true);
  return true;
}

function registerAssociations(requestedExtensions, removeUnselected = true) {
  if (process.platform !== "win32") {
    throw new Error("파일 연결 설정은 Windows 설치형에서만 지원합니다.");
  }
  if (!app.isPackaged) {
    throw new Error("파일 연결 설정은 설치된 앱에서만 지원합니다.");
  }
  if (smokeTest || updateSmokeTest) {
    throw new Error("테스트 모드에서는 파일 연결 설정을 변경하지 않습니다.");
  }
  if (isPortableBuild()) {
    throw new Error("포터블 버전에서는 파일 연결을 지원하지 않습니다.");
  }

  const requested = new Set(
    Array.isArray(requestedExtensions)
      ? requestedExtensions.map((ext) => String(ext).toLowerCase())
      : [],
  );
  const extensions = ALL_EXTENSIONS.filter((ext) => requested.has(ext));
  if (removeUnselected) {
    ALL_EXTENSIONS
      .filter((ext) => !requested.has(ext))
      .forEach(unregisterExtension);
  }
  for (const ext of extensions) {
    registerExtension(ext);
  }
  if (extensions.length) {
    registerApplicationCapabilities(extensions);
  } else {
    unregisterApplicationCapabilities();
  }
  saveAssociationPreference(Boolean(extensions.length));
  try {
    execFileSync("ie4uinit.exe", ["-show"], { windowsHide: true, stdio: "ignore" });
  } catch {
    // Association registration itself already succeeded.
  }
}

ipcMain.handle("open-dialog", async (_event, kind) => {
  const result = await dialog.showOpenDialog(mainWindow, kind === "folder"
    ? { properties: ["openDirectory"] }
    : {
        properties: ["openFile"],
        filters: [{
          name: "지원 이미지",
          extensions: [...SUPPORTED_EXTENSIONS].map((ext) => ext.slice(1)),
        }, { name: "모든 파일", extensions: ["*"] }],
      });
  return result.canceled ? null : result.filePaths[0];
});

ipcMain.handle("open-path", async (_event, targetPath) => buildCollection(targetPath));
ipcMain.handle("load-image", async (_event, item, cropMode) => (
  getImageLoader().loadImage(item, cropMode)
));
ipcMain.handle("load-thumbnail", async (_event, item) => getImageLoader().loadThumbnail(item));

ipcMain.handle("copy-image", async (_event, dataUrl) => {
  const image = nativeImage.createFromDataURL(dataUrl);
  clipboard.writeImage(image);
  return true;
});

ipcMain.handle("save-image-copy", async (_event, dataUrl, suggestedName) => {
  const result = await dialog.showSaveDialog(mainWindow, {
    defaultPath: suggestedName,
    filters: [{ name: "PNG 이미지", extensions: ["png"] }],
  });
  if (result.canceled || !result.filePath) return false;
  const base64 = dataUrl.replace(/^data:image\/png;base64,/, "");
  fs.writeFileSync(result.filePath, Buffer.from(base64, "base64"));
  return true;
});

ipcMain.handle("show-in-folder", async (_event, item) => {
  shell.showItemInFolder(item.kind === "archive-entry" ? item.archivePath : item.path);
});

ipcMain.handle("open-original", async (_event, item) => {
  return shell.openPath(item.kind === "archive-entry" ? item.archivePath : item.path);
});

ipcMain.handle("toggle-fullscreen", () => {
  mainWindow.setFullScreen(!mainWindow.isFullScreen());
  return mainWindow.isFullScreen();
});

ipcMain.handle("set-always-on-top", (_event, enabled) => {
  mainWindow.setAlwaysOnTop(Boolean(enabled));
  return mainWindow.isAlwaysOnTop();
});

ipcMain.handle("get-runtime-info", () => ({
  productName: PRODUCT_NAME,
  version: app.getVersion(),
  platform: process.platform,
  isPortable: isPortableBuild(),
  associationSupported: fileAssociationSupported(),
  basicAssociations: [...BASIC_ASSOCIATION_EXTENSIONS].sort(),
  optionalAssociations: OPTIONAL_ASSOCIATION_EXTENSIONS,
  updateSupported: updateSupported(),
  updateState,
}));

ipcMain.handle("register-associations", async (_event, extensions, openSettings) => {
  registerAssociations(extensions);
  if (openSettings) {
    try {
      await shell.openExternal(
        `ms-settings:defaultapps?registeredAppUser=${encodeURIComponent(PRODUCT_NAME)}`,
      );
    } catch {
      await shell.openExternal("ms-settings:defaultapps");
    }
  }
  return true;
});

ipcMain.handle("sync-associations", async (_event, extensions) => {
  registerAssociations(extensions, false);
  return true;
});

ipcMain.handle("check-for-updates", () => checkForUpdates(true));
ipcMain.handle("restart-and-update", () => installDownloadedUpdate());

const singleInstance = app.requestSingleInstanceLock();
if (!singleInstance) {
  app.quit();
} else {
  app.on("second-instance", (_event, argv) => {
    const targetPath = parseStartupPath(argv);
    if (targetPath) mainWindow?.webContents.send("open-external-path", targetPath);
    if (mainWindow) {
      if (mainWindow.isMinimized()) mainWindow.restore();
      mainWindow.focus();
    }
  });

  app.whenReady().then(() => {
    app.setAppUserModelId("com.clipimageviewer.app");
    pendingOpenPath = parseStartupPath(process.argv);
    createWindow();
  });
}

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
