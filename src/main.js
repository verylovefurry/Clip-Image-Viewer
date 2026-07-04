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
const { pathToFileURL } = require("url");
const {
  ARCHIVE_EXTENSIONS,
  BASIC_ASSOCIATION_EXTENSIONS,
  OPTIONAL_ASSOCIATION_EXTENSIONS,
  OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS,
  OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS,
  PROJECT_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  VIDEO_MIME_TYPES,
  extensionOf,
  findComicProject,
  isSupported,
  listFolder,
  mediaTypeForPath,
  naturalCompare,
} = require("./file-types");
const { constrainBounds, createDefaultBounds } = require("./window-state");

const ALL_EXTENSIONS = [...SUPPORTED_EXTENSIONS].sort();
const OPTIONAL_ASSOCIATION_COUNT = (
  OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS.length +
  OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS.length
);
const SUBTITLE_EXTENSIONS = new Set([".vtt", ".srt", ".ass", ".ssa"]);
const MAX_SUBTITLE_BYTES = 20 * 1024 * 1024;
const PRODUCT_NAME = "Clip Image Viewer";
const PROG_ID_PREFIX = "ClipImageViewer";
const CAPABILITIES_KEY = "HKCU\\Software\\ClipImageViewer\\Capabilities";
const REGISTERED_APPLICATIONS_KEY = "HKCU\\Software\\RegisteredApplications";
const ASSOCIATION_SETTINGS_KEY = "HKCU\\Software\\ClipImageViewer\\Settings";
let mainWindow;
let pendingOpenPath = null;
let imageWorker;
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

function attachPackagedSmokeTest(window, expectedPath) {
  window.webContents.once("did-finish-load", async () => {
    try {
      let result = null;
      for (let attempt = 0; attempt < 300; attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        result = await window.webContents.executeJavaScript(`(() => {
          const image = document.getElementById("viewerImage");
          return {
            title: document.title,
            rendered: image.complete && image.naturalWidth > 0,
            loading: !document.getElementById("loading").classList.contains("hidden"),
          };
        })()`);
        if (
          (!expectedPath || result.title !== PRODUCT_NAME) &&
          result.rendered &&
          !result.loading
        ) break;
      }
      if (expectedPath && (result?.title === PRODUCT_NAME || !result?.rendered)) {
        throw new Error(`Packaged image loading failed: ${JSON.stringify(result)}`);
      }
      if (process.env.CLIPVIEW_SMOKE_OPEN_LAYERS === "1") {
        let layers = null;
        for (let attempt = 0; attempt < 100; attempt += 1) {
          layers = await window.webContents.executeJavaScript(`(() => {
            const button = document.getElementById("layerBtn");
            if (!button.classList.contains("hidden")) button.click();
            return {
              count: document.querySelectorAll(".layer-row").length,
              buttonHidden: button.classList.contains("hidden"),
              thumbnailCount: document.querySelectorAll(".layer-thumbnail").length,
              toast: document.getElementById("toast").textContent,
            };
          })()`);
          if (layers.count) break;
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
        if (!layers?.count) {
          throw new Error(`Packaged layer panel failed: ${JSON.stringify(layers)}`);
        }
        if (layers.thumbnailCount) {
          let thumbnailReady = false;
          for (let attempt = 0; attempt < 100; attempt += 1) {
            thumbnailReady = await window.webContents.executeJavaScript(
              "document.querySelector('.layer-thumbnail')?.naturalWidth > 0",
            );
            if (thumbnailReady) break;
            await new Promise((resolve) => setTimeout(resolve, 100));
          }
          if (!thumbnailReady) throw new Error("Packaged layer thumbnail failed");
        }
      }
      console.log(`${PRODUCT_NAME} packaged smoke test passed: ${result?.title}`);
      app.exit(0);
    } catch (error) {
      console.error(`${PRODUCT_NAME} packaged smoke test failed: ${error.stack || error.message}`);
      app.exit(1);
    }
  });
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
    if (app.isPackaged) {
      attachPackagedSmokeTest(mainWindow, smokeExpectedPath);
    } else {
      const { attachElectronSmoke } = require("../test/electron-smoke");
      attachElectronSmoke({
        app,
        mainWindow,
        expectedPath: smokeExpectedPath,
        productName: PRODUCT_NAME,
        screen,
        smokeListTest,
        smokeNavigationTest,
        smokeSettingsTest,
        smokeUseWindowState,
        optionalAssociationCount: OPTIONAL_ASSOCIATION_COUNT,
        fileAssociationSupported,
      });
    }
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
  const sendFullscreenState = () => {
    if (!mainWindow.isDestroyed()) {
      mainWindow.webContents.send("fullscreen-state", mainWindow.isFullScreen());
    }
  };
  mainWindow.on("enter-full-screen", sendFullscreenState);
  mainWindow.on("leave-full-screen", sendFullscreenState);
}

function itemForPath(filePath) {
  return {
    kind: ARCHIVE_EXTENSIONS.has(extensionOf(filePath)) ? "archive" : "file",
    path: filePath,
    name: path.basename(filePath),
    mediaType: mediaTypeForPath(filePath),
  };
}

function subtitleTimestamp(value) {
  const match = String(value).trim().match(/^(\d+):(\d{1,2}):(\d{1,2})([,.](\d{1,3}))?$/);
  if (!match) return null;
  const [, hours, minutes, seconds, , fraction = "0"] = match;
  return [
    String(Number(hours)).padStart(2, "0"),
    String(Number(minutes)).padStart(2, "0"),
    String(Number(seconds)).padStart(2, "0"),
  ].join(":") + `.${fraction.padEnd(3, "0").slice(0, 3)}`;
}

function srtToVtt(content) {
  return `WEBVTT\n\n${String(content)
    .replace(/^\uFEFF/, "")
    .replace(/\r\n?/g, "\n")
    .replace(/(\d{1,2}:\d{2}:\d{2}),(\d{1,3})/g, "$1.$2")
    .replace(/^\d+\n(?=\d{1,2}:\d{2}:\d{2}\.\d{1,3}\s+-->\s+)/gm, "")}`;
}

function splitAssDialogue(line, fieldCount) {
  const values = [];
  let rest = line;
  for (let index = 1; index < fieldCount; index += 1) {
    const comma = rest.indexOf(",");
    if (comma < 0) break;
    values.push(rest.slice(0, comma));
    rest = rest.slice(comma + 1);
  }
  values.push(rest);
  return values;
}

function assToVtt(content) {
  const lines = String(content).replace(/^\uFEFF/, "").replace(/\r\n?/g, "\n").split("\n");
  let inEvents = false;
  let format = [];
  const cues = [];
  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line || line.startsWith(";")) continue;
    if (/^\[events\]$/i.test(line)) {
      inEvents = true;
      continue;
    }
    if (line.startsWith("[") && !/^\[events\]$/i.test(line)) {
      inEvents = false;
      continue;
    }
    if (!inEvents) continue;
    if (/^format:/i.test(line)) {
      format = line.slice(line.indexOf(":") + 1).split(",").map((value) => value.trim().toLowerCase());
      continue;
    }
    if (!/^dialogue:/i.test(line) || !format.length) continue;
    const values = splitAssDialogue(line.slice(line.indexOf(":") + 1).trim(), format.length);
    const start = subtitleTimestamp(values[format.indexOf("start")]);
    const end = subtitleTimestamp(values[format.indexOf("end")]);
    const textIndex = format.indexOf("text");
    if (!start || !end || textIndex < 0) continue;
    const text = values[textIndex]
      .replace(/\{[^}]*\}/g, "")
      .replace(/\\[Nn]/g, "\n")
      .replace(/\\h/g, " ")
      .trim();
    if (text) cues.push(`${start} --> ${end}\n${text}`);
  }
  return `WEBVTT\n\n${cues.join("\n\n")}`;
}

function subtitleLabel(videoBase, subtitlePath) {
  const parsed = path.parse(subtitlePath);
  const suffix = parsed.name === videoBase
    ? ""
    : parsed.name.slice(videoBase.length).replace(/^\./, "");
  return suffix ? suffix.toUpperCase() : "자막";
}

function subtitleLanguage(label) {
  const normalized = label.toLowerCase().split(/[._-]/)[0];
  if (["ko", "kor", "kr", "korean", "한국어"].includes(normalized)) return "ko";
  if (["ja", "jpn", "jp", "japanese", "日本語"].includes(normalized)) return "ja";
  if (["en", "eng", "english"].includes(normalized)) return "en";
  return "und";
}

async function readSubtitleText(subtitlePath) {
  const stat = await fs.promises.stat(subtitlePath);
  if (stat.size > MAX_SUBTITLE_BYTES) {
    throw new Error("20MB를 넘는 자막 파일은 안전을 위해 불러오지 않습니다.");
  }
  const buffer = await fs.promises.readFile(subtitlePath);
  if (buffer[0] === 0xFF && buffer[1] === 0xFE) {
    return new TextDecoder("utf-16le").decode(buffer);
  }
  if (buffer[0] === 0xFE && buffer[1] === 0xFF) {
    return new TextDecoder("utf-16be").decode(buffer);
  }
  const utf8 = new TextDecoder("utf-8").decode(buffer);
  if (!utf8.includes("\uFFFD")) return utf8;
  try {
    return new TextDecoder("euc-kr").decode(buffer);
  } catch {
    return utf8;
  }
}

async function subtitleToVtt(subtitlePath) {
  const ext = extensionOf(subtitlePath);
  const content = await readSubtitleText(subtitlePath);
  if (ext === ".srt") return srtToVtt(content);
  if (ext === ".ass" || ext === ".ssa") return assToVtt(content);
  return content.replace(/^\uFEFF/, "").startsWith("WEBVTT")
    ? content
    : `WEBVTT\n\n${content}`;
}

async function findSubtitleFiles(videoPath) {
  const folderPath = path.dirname(videoPath);
  const videoBase = path.basename(videoPath, path.extname(videoPath));
  return (await fs.promises.readdir(folderPath, { withFileTypes: true }))
    .filter((entry) => {
      if (!entry.isFile()) return false;
      const ext = extensionOf(entry.name);
      if (!SUBTITLE_EXTENSIONS.has(ext)) return false;
      const subtitleBase = path.basename(entry.name, ext);
      return subtitleBase === videoBase || subtitleBase.startsWith(`${videoBase}.`);
    })
    .map((entry) => path.join(folderPath, entry.name))
    .sort((a, b) => naturalCompare(path.basename(a), path.basename(b)));
}

function getImageWorker() {
  if (!imageWorker) {
    const { ImageWorkerClient } = require("./image-worker-client");
    imageWorker = new ImageWorkerClient();
  }
  return imageWorker;
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
    const items = await getImageWorker().invoke("listArchive", targetPath);
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
    `${PRODUCT_NAME} 미디어`,
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
    "다양한 이미지, 동영상, CLIP STUDIO PAINT 문서를 보는 미디어 뷰어",
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

function readAssociationPreference() {
  if (process.platform !== "win32") return null;
  try {
    const output = execFileSync(
      "reg.exe",
      ["query", ASSOCIATION_SETTINGS_KEY, "/v", "AssociationsEnabled"],
      { windowsHide: true, encoding: "utf8" },
    );
    const match = output.match(/AssociationsEnabled\s+REG_DWORD\s+0x([0-9a-f]+)/i);
    return match ? Number.parseInt(match[1], 16) !== 0 : null;
  } catch {
    return null;
  }
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
    "$TargetFullPath = [IO.Path]::GetFullPath($TargetPath)",
    "$ManagedBackups = @()",
    "foreach ($ManagedName in @('resources', 'locales')) {",
    "  $ManagedPath = [IO.Path]::GetFullPath((Join-Path $TargetFullPath $ManagedName))",
    "  $BackupPath = [IO.Path]::GetFullPath($ManagedPath + '.update-backup')",
    "  if (-not $ManagedPath.StartsWith($TargetFullPath + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {",
    "    exit 90",
    "  }",
    "  Remove-Item -LiteralPath $BackupPath -Recurse -Force -ErrorAction SilentlyContinue",
    "  if (Test-Path -LiteralPath $ManagedPath) {",
    "    Move-Item -LiteralPath $ManagedPath -Destination $BackupPath -Force",
    "    $ManagedBackups += [PSCustomObject]@{ Target = $ManagedPath; Backup = $BackupPath }",
    "  }",
    "}",
    "& robocopy.exe $SourcePath $TargetPath /E /R:10 /W:1 /NFL /NDL /NJH /NJS /NP",
    "$CopyExitCode = $LASTEXITCODE",
    "if ($CopyExitCode -ge 8) {",
    "  foreach ($Item in $ManagedBackups) {",
    "    Remove-Item -LiteralPath $Item.Target -Recurse -Force -ErrorAction SilentlyContinue",
    "    Move-Item -LiteralPath $Item.Backup -Destination $Item.Target -Force -ErrorAction SilentlyContinue",
    "  }",
    "  if (Test-Path -LiteralPath $ExecutablePath) { Start-Process -FilePath $ExecutablePath }",
    "  exit $CopyExitCode",
    "}",
    "foreach ($Item in $ManagedBackups) {",
    "  Remove-Item -LiteralPath $Item.Backup -Recurse -Force -ErrorAction SilentlyContinue",
    "}",
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
          name: "지원 미디어",
          extensions: [...SUPPORTED_EXTENSIONS].map((ext) => ext.slice(1)),
        }, { name: "모든 파일", extensions: ["*"] }],
      });
  return result.canceled ? null : result.filePaths[0];
});

ipcMain.handle("open-path", async (_event, targetPath) => buildCollection(targetPath));
ipcMain.handle("load-image", async (_event, item, cropMode) => (
  getImageWorker().invoke("loadImage", item, cropMode)
));
ipcMain.handle("load-thumbnail", async (_event, item) => (
  getImageWorker().invoke("loadThumbnail", item)
));
ipcMain.handle("load-layer-thumbnail", async (_event, item, id) => (
  getImageWorker().invoke("loadLayerThumbnail", item, id)
));
ipcMain.handle("render-layered-image", async (_event, item, visibility) => (
  getImageWorker().invoke("renderLayeredImage", item, visibility)
));
ipcMain.handle("prepare-layered-image", async (_event, item) => (
  getImageWorker().invoke("prepareLayeredImage", item)
));
ipcMain.handle("pick-layer", async (_event, item, x, y, visibility) => (
  getImageWorker().invoke("pickLayer", item, x, y, visibility)
));

ipcMain.handle("media-file-url", async (_event, item) => {
  if (!item || item.kind !== "file" || !item.path) {
    throw new Error("동영상 파일을 열 수 없습니다.");
  }
  if (mediaTypeForPath(item.path) !== "video") {
    throw new Error("동영상 파일이 아닙니다.");
  }
  const stat = fs.statSync(item.path);
  return {
    url: pathToFileURL(item.path).toString(),
    mime: VIDEO_MIME_TYPES[extensionOf(item.path)] || "",
    metadata: {
      format: extensionOf(item.path).slice(1).toUpperCase(),
      byteSize: stat.size,
      modifiedAt: stat.mtimeMs,
      source: "동영상 파일",
    },
  };
});

ipcMain.handle("find-subtitles", async (_event, item) => {
  if (!item || item.kind !== "file" || !item.path || mediaTypeForPath(item.path) !== "video") {
    return [];
  }
  const videoBase = path.basename(item.path, path.extname(item.path));
  const subtitlePaths = await findSubtitleFiles(item.path);
  return Promise.all(subtitlePaths.map(async (subtitlePath) => {
    const label = subtitleLabel(videoBase, subtitlePath);
    return {
      name: path.basename(subtitlePath),
      label,
      srclang: subtitleLanguage(label),
      vtt: await subtitleToVtt(subtitlePath),
    };
  }));
});

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
  optionalImageAssociations: OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS,
  optionalVideoAssociations: OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS,
  optionalAssociations: OPTIONAL_ASSOCIATION_EXTENSIONS,
  associationsEnabled: readAssociationPreference(),
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
    if (targetPath) {
      if (mainWindow && !mainWindow.webContents.isLoadingMainFrame()) {
        mainWindow.webContents.send("open-external-path", targetPath);
      } else {
        pendingOpenPath = targetPath;
      }
    }
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

app.on("will-quit", () => {
  void imageWorker?.close();
});
