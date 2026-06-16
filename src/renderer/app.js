"use strict";

const state = {
  items: [],
  index: -1,
  imageDataUrl: "",
  naturalWidth: 0,
  naturalHeight: 0,
  metadata: null,
  comic: null,
  cropMode: localStorage.getItem("cropMode") || "full",
  zoom: 1,
  fitZoom: 1,
  rotation: 0,
  flipX: 1,
  panX: 0,
  panY: 0,
  dragging: false,
  dragStartX: 0,
  dragStartY: 0,
  panStartX: 0,
  panStartY: 0,
  fitMode: true,
  slideshowTimer: null,
  fullscreen: false,
  thumbsVisible: false,
  infoVisible: false,
  runtime: null,
  update: null,
  settings: {
    background: localStorage.getItem("background") || "dark",
    slideInterval: Number(localStorage.getItem("slideInterval") || 3000),
    loop: localStorage.getItem("loop") !== "false",
  },
};

const $ = (id) => document.getElementById(id);
const stage = $("stage");
const imageLayer = $("imageLayer");
const image = $("viewerImage");
const emptyState = $("emptyState");
const loading = $("loading");
const toast = $("toast");
let toastTimer;
let loadSequence = 0;
const imageCache = new Map();

function currentItem() {
  return state.items[state.index] || null;
}

function imageKey(index, cropMode = state.cropMode) {
  const item = state.items[index];
  if (!item) return "";
  const identity = item.kind === "archive-entry"
    ? `${item.archivePath}::${item.entryName}`
    : item.path;
  return `${identity}::${cropMode}`;
}

function adjacentIndices(index = state.index) {
  if (state.items.length < 2) return [];
  const indices = [];
  for (const delta of [-1, 1]) {
    let adjacent = index + delta;
    if (state.settings.loop) {
      adjacent = (adjacent + state.items.length) % state.items.length;
    }
    if (adjacent >= 0 && adjacent < state.items.length && adjacent !== index) {
      indices.push(adjacent);
    }
  }
  return [...new Set(indices)];
}

async function decodeImage(dataUrl) {
  const preloadImage = new Image();
  preloadImage.src = dataUrl;
  if (preloadImage.decode) {
    await preloadImage.decode();
    return;
  }
  await new Promise((resolve, reject) => {
    preloadImage.onload = resolve;
    preloadImage.onerror = reject;
  });
}

function loadImageCached(index) {
  const key = imageKey(index);
  if (imageCache.has(key)) return imageCache.get(key);
  const item = state.items[index];
  const promise = window.clipView.loadImage(item, state.cropMode)
    .then(async (result) => {
      await decodeImage(result.dataUrl);
      return result;
    })
    .catch((error) => {
      imageCache.delete(key);
      throw error;
    });
  imageCache.set(key, promise);
  return promise;
}

function pruneImageCache() {
  const keep = new Set(
    [state.index, ...adjacentIndices()].map((index) => imageKey(index)),
  );
  for (const key of imageCache.keys()) {
    if (!keep.has(key)) imageCache.delete(key);
  }
}

function preloadAdjacentImages() {
  pruneImageCache();
  for (const index of adjacentIndices()) {
    void loadImageCached(index).catch(() => {});
  }
}

function formatBytes(bytes) {
  if (!bytes) return "-";
  const units = ["B", "KB", "MB", "GB"];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / (1024 ** exponent)).toFixed(exponent ? 1 : 0)} ${units[exponent]}`;
}

function showToast(message, isError = false) {
  clearTimeout(toastTimer);
  toast.textContent = message;
  toast.style.borderColor = isError ? "#8c4545" : "";
  toast.classList.remove("hidden");
  toastTimer = setTimeout(() => toast.classList.add("hidden"), 2400);
}

function applyTransform() {
  image.style.transform = [
    `translate(${state.panX}px, ${state.panY}px)`,
    `rotate(${state.rotation}deg)`,
    `scale(${state.zoom * state.flipX}, ${state.zoom})`,
    "translate(-50%, -50%)",
  ].join(" ");
  $("zoomLabel").textContent = `${Math.round(state.zoom * 100)}%`;
}

function calculateFitZoom() {
  if (!state.naturalWidth || !state.naturalHeight) return 1;
  const rect = stage.getBoundingClientRect();
  const quarterTurn = Math.abs(state.rotation % 180) === 90;
  const width = quarterTurn ? state.naturalHeight : state.naturalWidth;
  const height = quarterTurn ? state.naturalWidth : state.naturalHeight;
  const availableWidth = Math.max(1, rect.width);
  const availableHeight = Math.max(1, rect.height);
  return Math.min(availableWidth / width, availableHeight / height);
}

function fitImage() {
  state.fitMode = true;
  state.fitZoom = calculateFitZoom();
  state.zoom = state.fitZoom;
  state.panX = 0;
  state.panY = 0;
  applyTransform();
}

function actualSize() {
  state.fitMode = false;
  state.zoom = 1;
  state.panX = 0;
  state.panY = 0;
  applyTransform();
}

function zoomBy(factor) {
  if (!state.imageDataUrl) return;
  state.fitMode = false;
  state.zoom = Math.min(32, Math.max(0.02, state.zoom * factor));
  applyTransform();
}

function rotate(amount) {
  if (!state.imageDataUrl) return;
  state.rotation = (state.rotation + amount + 360) % 360;
  if (state.fitMode) fitImage();
  else applyTransform();
}

function updateInfoPanel() {
  const item = currentItem();
  const meta = state.metadata || {};
  const values = [
    ["파일", item?.name || "-"],
    ["경로", item?.displayPath || item?.path || "-"],
    ["크기", `${meta.width || 0} × ${meta.height || 0}`],
    ["형식", meta.format || "-"],
    ["파일 용량", formatBytes(meta.byteSize)],
    ["색 공간", meta.space || "-"],
    ["채널", meta.channels || "-"],
    ["페이지/프레임", meta.pages || 1],
    ["투명도", meta.hasAlpha ? "있음" : "없음"],
    ["불러오기", meta.source || "-"],
    ["수정 시각", meta.modifiedAt ? new Date(meta.modifiedAt).toLocaleString() : "-"],
  ];
  $("infoList").innerHTML = values
    .map(([key, value]) => `<dt>${key}</dt><dd>${escapeHtml(String(value))}</dd>`)
    .join("");
}

function escapeHtml(value) {
  return value.replace(/[&<>"']/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    "\"": "&quot;",
    "'": "&#39;",
  }[char]));
}

function updateUi() {
  const item = currentItem();
  $("fileTitle").textContent = item?.name || "이미지를 열어주세요";
  document.title = item ? `${item.name} - Clip Image Viewer` : "Clip Image Viewer";
  $("counter").textContent = state.items.length ? `${state.index + 1} / ${state.items.length}` : "0 / 0";
  $("dimensionLabel").textContent = state.metadata?.width
    ? `${state.metadata.width} × ${state.metadata.height} · ${state.metadata.format}`
    : "-";
  $("prevOverlay").classList.toggle("hidden", state.items.length < 2);
  $("nextOverlay").classList.toggle("hidden", state.items.length < 2);
  const showCropModes = Boolean(state.comic?.cropAvailable);
  $("cropModeSelect").classList.toggle("hidden", !showCropModes);
  $("cropModeSelect").value = state.cropMode;
  updateInfoPanel();

  document.querySelectorAll(".thumb-item").forEach((element, index) => {
    element.classList.toggle("active", index === state.index);
  });
  const activeThumb = document.querySelector(".thumb-item.active");
  activeThumb?.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "center" });
}

async function loadCurrent() {
  const item = currentItem();
  if (!item) return;
  const sequence = ++loadSequence;
  const cached = imageCache.has(imageKey(state.index));
  loading.classList.toggle("hidden", cached);
  emptyState.classList.add("hidden");
  if (!state.imageDataUrl) imageLayer.classList.add("hidden");

  try {
    const result = await loadImageCached(state.index);
    if (sequence !== loadSequence) return;
    state.imageDataUrl = result.dataUrl;
    state.metadata = result.metadata;
    state.rotation = 0;
    state.flipX = 1;
    state.panX = 0;
    state.panY = 0;
    state.fitMode = true;

    await new Promise((resolve, reject) => {
      image.onload = resolve;
      image.onerror = reject;
      image.src = result.dataUrl;
    });
    if (sequence !== loadSequence) return;
    state.naturalWidth = image.naturalWidth;
    state.naturalHeight = image.naturalHeight;
    image.alt = item.name;
    imageLayer.classList.remove("hidden");
    fitImage();
    updateUi();
    preloadAdjacentImages();
  } catch (error) {
    state.imageDataUrl = "";
    emptyState.classList.remove("hidden");
    showToast(error?.message || "이미지를 열지 못했습니다.", true);
  } finally {
    loading.classList.add("hidden");
  }
}

async function openPath(targetPath) {
  if (!targetPath) return;
  stopSlideshow();
  try {
    const collection = await window.clipView.openPath(targetPath);
    if (!collection.items.length) {
      showToast("지원되는 이미지가 없습니다.", true);
      return;
    }
    state.items = collection.items;
    state.index = collection.index;
    state.comic = collection.comic || null;
    imageCache.clear();
    updateUi();
    await loadCurrent();
    if (state.thumbsVisible) renderThumbnails();
  } catch (error) {
    showToast(error?.message || "경로를 열지 못했습니다.", true);
  }
}

async function selectItem(index) {
  if (!Number.isInteger(index) || index < 0 || index >= state.items.length) return;
  stopSlideshow();
  state.index = index;
  updateUi();
  await loadCurrent();
}

function moveTo(delta) {
  if (state.items.length < 2) return;
  let next = state.index + delta;
  if (state.settings.loop) {
    next = (next + state.items.length) % state.items.length;
  } else {
    next = Math.max(0, Math.min(state.items.length - 1, next));
  }
  if (next !== state.index) {
    void selectItem(next);
  }
}

function toggleInfo(force) {
  state.infoVisible = force ?? !state.infoVisible;
  $("infoPanel").classList.toggle("hidden", !state.infoVisible);
  $("infoBtn").classList.toggle("active", state.infoVisible);
  setTimeout(() => state.fitMode && fitImage(), 0);
}

function toggleThumbnails(force) {
  state.thumbsVisible = force ?? !state.thumbsVisible;
  $("thumbnailPanel").classList.toggle("hidden", !state.thumbsVisible);
  $("thumbBtn").classList.toggle("active", state.thumbsVisible);
  if (state.thumbsVisible) renderThumbnails();
  setTimeout(() => state.fitMode && fitImage(), 0);
}

async function renderThumbnails() {
  const strip = $("thumbnailStrip");
  strip.innerHTML = "";
  state.items.forEach((item, index) => {
    const element = document.createElement("div");
    element.className = `thumb-item${index === state.index ? " active" : ""}`;
    element.title = item.displayPath || item.path;
    element.dataset.index = String(index);
    element.innerHTML = `<img alt=""><span>${escapeHtml(item.name)}</span>`;
    element.addEventListener("click", () => {
      void selectItem(Number(element.dataset.index));
    });
    strip.appendChild(element);
  });

  const queue = [...strip.children].map((element) => ({
    element,
    index: Number(element.dataset.index),
  }));
  const workers = Array.from({ length: Math.min(4, queue.length) }, async () => {
    while (queue.length) {
      const { element, index } = queue.shift();
      try {
        element.querySelector("img").src = await window.clipView.loadThumbnail(state.items[index]);
      } catch {
        element.querySelector("span").textContent = `미리보기 없음 · ${state.items[index].name}`;
      }
    }
  });
  await Promise.all(workers);
}

function startSlideshow() {
  if (state.items.length < 2) return;
  stopSlideshow();
  $("slideshowBtn").classList.add("active");
  $("slideshowBtn").textContent = "정지";
  state.slideshowTimer = setInterval(() => moveTo(1), state.settings.slideInterval);
}

function stopSlideshow() {
  clearInterval(state.slideshowTimer);
  state.slideshowTimer = null;
  $("slideshowBtn")?.classList.remove("active");
  if ($("slideshowBtn")) $("slideshowBtn").textContent = "재생";
}

async function toggleFullscreen() {
  state.fullscreen = await window.clipView.toggleFullscreen();
  document.body.classList.toggle("fullscreen-ui", state.fullscreen);
  setTimeout(() => state.fitMode && fitImage(), 0);
}

function applyBackground(value) {
  stage.classList.remove("bg-black", "bg-light", "bg-checker");
  if (value !== "dark") stage.classList.add(`bg-${value}`);
}

function transformedPngDataUrl() {
  if (!state.imageDataUrl) return "";
  const rotation = state.rotation % 360;
  const swap = rotation === 90 || rotation === 270;
  const canvas = document.createElement("canvas");
  canvas.width = swap ? image.naturalHeight : image.naturalWidth;
  canvas.height = swap ? image.naturalWidth : image.naturalHeight;
  const context = canvas.getContext("2d");
  context.translate(canvas.width / 2, canvas.height / 2);
  context.rotate(rotation * Math.PI / 180);
  context.scale(state.flipX, 1);
  context.drawImage(image, -image.naturalWidth / 2, -image.naturalHeight / 2);
  return canvas.toDataURL("image/png");
}

function selectedOptionalAssociations() {
  return [...document.querySelectorAll(".association-extension:checked")]
    .map((input) => input.value);
}

function savedOptionalAssociations() {
  try {
    return JSON.parse(localStorage.getItem("associationExtensions") || "[]");
  } catch {
    return [];
  }
}

function renderAssociationOptions() {
  const container = $("associationOptions");
  if (!state.runtime) return;
  const saved = new Set(savedOptionalAssociations());
  container.innerHTML = state.runtime.optionalAssociations
    .map((ext) => `
      <label>
        <input class="association-extension" type="checkbox"
               value="${escapeHtml(ext)}"${saved.has(ext) ? " checked" : ""}>
        <span>${escapeHtml(ext.slice(1).toUpperCase())}</span>
      </label>
    `)
    .join("");
}

async function initializeRuntimeInfo() {
  if (!state.runtime) {
    state.runtime = await window.clipView.getRuntimeInfo();
    applyUpdateState(state.runtime.updateState);
    renderAssociationOptions();
  }
  const supported = state.runtime.associationSupported;
  $("associationControls").classList.toggle("hidden", !supported);
  if (state.runtime.isPortable) {
    $("associationHelp").textContent =
      "포터블 버전에서는 확장자 파일 연결을 지원하지 않습니다. Windows 설치형을 사용하세요.";
  } else if (state.runtime.platform !== "win32") {
    $("associationHelp").textContent =
      "파일 연결 설정은 Windows 설치형에서만 지원합니다.";
  } else if (!supported) {
    $("associationHelp").textContent =
      "파일 연결 설정은 설치된 앱에서만 지원합니다.";
  } else {
    $("associationHelp").textContent =
      "선택한 형식을 Windows 기본 앱 후보로 등록합니다. 보안 정책상 실제 기본 앱 지정은 이어서 열리는 Windows 설정에서 직접 확인해야 합니다.";
    const enabled = localStorage.getItem("associationsEnabled") !== "false";
    const registrationToken = `${state.runtime.version}:capabilities-v1`;
    const syncedVersion = localStorage.getItem("associationRegistrationVersion");
    if (enabled && syncedVersion !== registrationToken) {
      try {
        await window.clipView.syncAssociations([
          ...state.runtime.basicAssociations,
          ...savedOptionalAssociations(),
        ]);
        localStorage.setItem("associationRegistrationVersion", registrationToken);
      } catch {
        // The settings UI still allows the user to retry registration manually.
      }
    }
  }
}

function applyUpdateState(update) {
  if (!update) return;
  state.update = update;
  const status = $("updateStatus");
  const progress = $("updateProgress");
  const checkButton = $("checkUpdateBtn");
  const restartButton = $("restartUpdateBtn");
  status.textContent = update.message ||
    `현재 버전 ${state.runtime?.version || update.currentVersion || "-"}`;
  const downloading = update.status === "downloading";
  progress.classList.toggle("hidden", !downloading);
  progress.value = update.percent || 0;
  checkButton.disabled = ["checking", "available", "downloading"].includes(update.status);
  restartButton.classList.toggle("hidden", update.status !== "downloaded");
}

function openSettings() {
  $("backgroundSelect").value = state.settings.background;
  $("slideIntervalSelect").value = String(state.settings.slideInterval);
  $("loopCheckbox").checked = state.settings.loop;
  $("settingsDialog").showModal();
  void initializeRuntimeInfo();
}

function bindActions() {
  const chooseAndOpen = async (kind) => openPath(await window.clipView.openDialog(kind));
  $("openFileBtn").onclick = () => chooseAndOpen("file");
  $("emptyOpenFileBtn").onclick = () => chooseAndOpen("file");
  $("openFolderBtn").onclick = () => chooseAndOpen("folder");
  $("emptyOpenFolderBtn").onclick = () => chooseAndOpen("folder");
  $("prevBtn").onclick = $("prevOverlay").onclick = () => moveTo(-1);
  $("nextBtn").onclick = $("nextOverlay").onclick = () => moveTo(1);
  $("zoomOutBtn").onclick = () => zoomBy(1 / 1.18);
  $("zoomInBtn").onclick = () => zoomBy(1.18);
  $("fitBtn").onclick = fitImage;
  $("actualBtn").onclick = actualSize;
  $("rotateLeftBtn").onclick = () => rotate(-90);
  $("rotateRightBtn").onclick = () => rotate(90);
  $("flipBtn").onclick = () => {
    state.flipX *= -1;
    applyTransform();
  };
  $("slideshowBtn").onclick = () => state.slideshowTimer ? stopSlideshow() : startSlideshow();
  $("fullscreenBtn").onclick = toggleFullscreen;
  $("infoBtn").onclick = () => toggleInfo();
  $("closeInfoBtn").onclick = () => toggleInfo(false);
  $("thumbBtn").onclick = () => toggleThumbnails();
  $("settingsBtn").onclick = openSettings;
  $("revealBtn").onclick = () => currentItem() && window.clipView.showInFolder(currentItem());
  $("openOriginalBtn").onclick = () => currentItem() && window.clipView.openOriginal(currentItem());
  $("cropModeSelect").onchange = (event) => {
    state.cropMode = event.target.value;
    localStorage.setItem("cropMode", state.cropMode);
    imageCache.clear();
    event.target.blur();
    stage.focus({ preventScroll: true });
    void loadCurrent();
  };

  $("pinBtn").onclick = async () => {
    const active = !$("pinBtn").classList.contains("active");
    await window.clipView.setAlwaysOnTop(active);
    $("pinBtn").classList.toggle("active", active);
  };

  $("backgroundSelect").onchange = (event) => {
    state.settings.background = event.target.value;
    localStorage.setItem("background", state.settings.background);
    applyBackground(state.settings.background);
  };
  $("slideIntervalSelect").onchange = (event) => {
    state.settings.slideInterval = Number(event.target.value);
    localStorage.setItem("slideInterval", String(state.settings.slideInterval));
    if (state.slideshowTimer) startSlideshow();
  };
  $("loopCheckbox").onchange = (event) => {
    state.settings.loop = event.target.checked;
    localStorage.setItem("loop", String(state.settings.loop));
  };
  $("applyAssociationBtn").onclick = async () => {
    try {
      const optional = selectedOptionalAssociations();
      const extensions = [...state.runtime.basicAssociations, ...optional];
      localStorage.setItem("associationExtensions", JSON.stringify(optional));
      localStorage.setItem("associationsEnabled", "true");
      await window.clipView.registerAssociations(extensions, true);
      localStorage.setItem(
        "associationRegistrationVersion",
        `${state.runtime.version}:capabilities-v1`,
      );
      showToast("파일 연결 정보를 적용했습니다.");
    } catch (error) {
      showToast(error?.message || "파일 연결을 적용하지 못했습니다.", true);
    }
  };
  $("clearAssociationBtn").onclick = async () => {
    try {
      await window.clipView.registerAssociations([], false);
      localStorage.setItem("associationExtensions", "[]");
      localStorage.setItem("associationsEnabled", "false");
      localStorage.setItem(
        "associationRegistrationVersion",
        `${state.runtime.version}:capabilities-v1`,
      );
      showToast("파일 연결을 모두 제거했습니다.");
    } catch (error) {
      showToast(error?.message || "파일 연결을 제거하지 못했습니다.", true);
    }
  };
  $("selectAllAssociationBtn").onclick = () => {
    document.querySelectorAll(".association-extension")
      .forEach((input) => { input.checked = true; });
  };
  $("clearOptionalAssociationBtn").onclick = () => {
    document.querySelectorAll(".association-extension")
      .forEach((input) => { input.checked = false; });
  };
  $("checkUpdateBtn").onclick = async () => {
    applyUpdateState({
      status: "checking",
      message: "새 버전을 확인하는 중...",
      percent: 0,
    });
    applyUpdateState(await window.clipView.checkForUpdates());
  };
  $("restartUpdateBtn").onclick = () => window.clipView.restartAndUpdate();
}

stage.addEventListener("wheel", (event) => {
  if (!state.imageDataUrl) return;
  event.preventDefault();
  zoomBy(event.deltaY < 0 ? 1.12 : 1 / 1.12);
}, { passive: false });

imageLayer.addEventListener("mousedown", (event) => {
  if (event.button !== 0) return;
  state.dragging = true;
  state.dragStartX = event.clientX;
  state.dragStartY = event.clientY;
  state.panStartX = state.panX;
  state.panStartY = state.panY;
  imageLayer.classList.add("dragging");
});

window.addEventListener("mousemove", (event) => {
  if (!state.dragging) return;
  state.panX = state.panStartX + event.clientX - state.dragStartX;
  state.panY = state.panStartY + event.clientY - state.dragStartY;
  applyTransform();
});

window.addEventListener("mouseup", () => {
  state.dragging = false;
  imageLayer.classList.remove("dragging");
});

window.addEventListener("resize", () => {
  if (state.fitMode) fitImage();
});

window.addEventListener("keydown", async (event) => {
  if ($("settingsDialog").open) return;
  const key = event.key.toLowerCase();
  if (event.ctrlKey && key === "o") {
    event.preventDefault();
    const kind = event.shiftKey ? "folder" : "file";
    openPath(await window.clipView.openDialog(kind));
  } else if (event.ctrlKey && key === "c") {
    const dataUrl = transformedPngDataUrl();
    if (dataUrl) {
      await window.clipView.copyImage(dataUrl);
      showToast("이미지를 클립보드에 복사했습니다.");
    }
  } else if (event.ctrlKey && key === "s") {
    const dataUrl = transformedPngDataUrl();
    if (dataUrl) {
      const stem = currentItem().name.replace(/\.[^.]+$/, "");
      await window.clipView.saveImageCopy(dataUrl, `${stem}.png`);
    }
  } else if (["arrowleft", "pageup"].includes(key)) {
    event.preventDefault();
    moveTo(-1);
  } else if (["arrowright", "pagedown", " "].includes(key)) {
    event.preventDefault();
    moveTo(1);
  } else if (key === "+" || key === "=") {
    zoomBy(1.18);
  } else if (key === "-") {
    zoomBy(1 / 1.18);
  } else if (key === "0") {
    fitImage();
  } else if (key === "1") {
    actualSize();
  } else if (key === "r") {
    rotate(event.shiftKey ? -90 : 90);
  } else if (key === "f") {
    state.flipX *= -1;
    applyTransform();
  } else if (key === "i") {
    toggleInfo();
  } else if (key === "t") {
    toggleThumbnails();
  } else if (key === "s") {
    state.slideshowTimer ? stopSlideshow() : startSlideshow();
  } else if (key === "enter" || key === "f11") {
    event.preventDefault();
    toggleFullscreen();
  } else if (key === "escape" && state.fullscreen) {
    toggleFullscreen();
  }
});

stage.addEventListener("dblclick", (event) => {
  if (event.target.closest("button")) return;
  void toggleFullscreen();
});

for (const eventName of ["dragenter", "dragover"]) {
  document.addEventListener(eventName, (event) => {
    event.preventDefault();
    $("dropHint").classList.remove("hidden");
  });
}
document.addEventListener("dragleave", (event) => {
  if (!event.relatedTarget) $("dropHint").classList.add("hidden");
});
document.addEventListener("drop", (event) => {
  event.preventDefault();
  $("dropHint").classList.add("hidden");
  const file = event.dataTransfer.files[0];
  if (file) openPath(window.clipView.pathForFile(file));
});

window.clipView.onOpenExternalPath(openPath);
window.clipView.onUpdateState(applyUpdateState);
bindActions();
applyBackground(state.settings.background);
void initializeRuntimeInfo().catch(() => {});
