"use strict";

const state = {
  items: [],
  index: -1,
  mediaType: "",
  imageDataUrl: "",
  videoSrc: "",
  subtitleObjectUrls: [],
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
  layersVisible: false,
  layerDocument: null,
  layerVisibility: {},
  renderedLayerVisibility: {},
  selectedLayerId: "",
  layerRenderSequence: 0,
  layerThumbnails: {},
  layerThumbnailLoading: new Set(),
  layerThumbnailSequence: 0,
  collapsedLayerIds: new Set(),
  layerPreparationStarted: false,
  runtime: null,
  update: null,
  videoControlsTimer: null,
  videoControlsMode: "full",
  videoControlsLockedMode: "",
  videoPlaybackMode: localStorage.getItem("videoPlaybackMode") || "sequential",
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
const video = $("viewerVideo");
const videoControls = $("videoControls");
const videoSeek = $("videoSeek");
const videoPlayBtn = $("videoPlayBtn");
const videoTimeLabel = $("videoTimeLabel");
const videoTitleLabel = $("videoTitleLabel");
const videoModeBtn = $("videoModeBtn");
const videoSubtitleBtn = $("videoSubtitleBtn");
const videoMuteBtn = $("videoMuteBtn");
const videoVolume = $("videoVolume");
const videoSpeedSelect = $("videoSpeedSelect");
const videoFullscreenBtn = $("videoFullscreenBtn");
const videoContextMenu = $("videoContextMenu");
const emptyState = $("emptyState");
const loading = $("loading");
const toast = $("toast");
let toastTimer;
let loadSequence = 0;
let openSequence = 0;
let selectionSequence = 0;
let cancelPendingVideoLoad = null;
let videoClickTimer = null;
let videoControlInteracting = false;
let thumbnailRenderSequence = 0;
const imageCache = new Map();

function currentItem() {
  return state.items[state.index] || null;
}

const VIDEO_PLAYBACK_MODES = {
  sequential: "순서",
  repeatAll: "전체반복",
  repeatOne: "한 영상 반복",
  shuffle: "무작위",
};

function isVideoItem(item = currentItem()) {
  return item?.mediaType === "video";
}

function hasMedia() {
  return Boolean(state.imageDataUrl || state.videoSrc);
}

function clearSubtitleTracks() {
  state.subtitleObjectUrls.forEach((url) => URL.revokeObjectURL(url));
  state.subtitleObjectUrls = [];
  video.querySelectorAll("track").forEach((track) => track.remove());
}

function clearVideo() {
  clearTimeout(videoClickTimer);
  videoClickTimer = null;
  cancelPendingVideoLoad?.();
  cancelPendingVideoLoad = null;
  clearTimeout(state.videoControlsTimer);
  clearSubtitleTracks();
  video.pause();
  video.onloadedmetadata = null;
  video.onerror = null;
  video.removeAttribute("src");
  video.removeAttribute("width");
  video.removeAttribute("height");
  video.load();
  state.videoSrc = "";
  videoControls.classList.add("hidden");
  videoControls.classList.remove("controls-hidden", "peek-progress", "peek-volume");
  state.videoControlsLockedMode = "";
  document.body.classList.remove("video-cursor-hidden");
  imageLayer.classList.remove("video-active");
  hideVideoContextMenu();
  updateVideoUi();
}

function clearImage() {
  image.removeAttribute("src");
  image.classList.add("hidden");
  state.imageDataUrl = "";
  clearLayerDocument();
}

function flattenLayers(layers, output = []) {
  for (const layer of layers || []) {
    output.push(layer);
    flattenLayers(layer.children, output);
  }
  return output;
}

function clearLayerDocument() {
  state.layerThumbnailSequence += 1;
  state.layerDocument = null;
  state.layerVisibility = {};
  state.renderedLayerVisibility = {};
  state.selectedLayerId = "";
  state.layerThumbnails = {};
  state.layerThumbnailLoading.clear();
  state.collapsedLayerIds.clear();
  state.layerPreparationStarted = false;
  state.layersVisible = false;
  $("layerPanel")?.classList.add("hidden");
  $("layerBtn")?.classList.add("hidden");
  $("layerBtn")?.classList.remove("active");
}

function setLayerDocument(document) {
  state.layerThumbnailSequence += 1;
  state.layerDocument = document || null;
  state.layerVisibility = {};
  state.selectedLayerId = "";
  state.layerThumbnails = {};
  state.layerThumbnailLoading.clear();
  state.collapsedLayerIds.clear();
  state.layerPreparationStarted = false;
  if (!document?.layers?.length) {
    clearLayerDocument();
    return;
  }
  flattenLayers(document.layers).forEach((layer) => {
    state.layerVisibility[layer.id] = layer.visible;
  });
  state.renderedLayerVisibility = { ...state.layerVisibility };
  $("layerBtn").classList.remove("hidden");
  renderLayerPanel();
}

function layerById(id) {
  return flattenLayers(state.layerDocument?.layers).find((layer) => layer.id === id) || null;
}

function layerAncestorIds(layers, id, ancestors = []) {
  for (const layer of layers || []) {
    if (layer.id === id) return ancestors;
    const nested = layerAncestorIds(layer.children, id, [...ancestors, layer.id]);
    if (nested) return nested;
  }
  return null;
}

function renderLayerDetails() {
  const layer = layerById(state.selectedLayerId);
  const panel = $("layerDetails");
  const textObjectsPanel = $("layerTextObjects");
  panel.classList.toggle("hidden", !layer);
  if (!layer) {
    textObjectsPanel.classList.add("hidden");
    textObjectsPanel.innerHTML = "";
    return;
  }
  $("layerDetailName").textContent = layer.name;
  const values = [
    ["종류", layer.typeLabel || (layer.type === "group" ? "그룹" : layer.type)],
    ["합성", layer.blendModeLabel || layer.blendMode || "표준"],
    ["불투명도", `${layer.opacity}%`],
    ["클리핑", layer.clipping ? "있음" : "없음"],
    ["마스크", layer.mask ? "있음" : "없음"],
    ["효과", layer.effects?.length ? layer.effects.join(", ") : "없음"],
    ["미리보기", layer.previewAccuracy || "레이어 데이터"],
  ];
  if (layer.type === "text") {
    values.splice(1, 0, ["텍스트 객체", `${layer.textObjects?.length || 0}개`]);
  }
  if (layer.locked) values.push(["잠금", "잠김"]);
  if (layer.draft) values.push(["속성", "초안 레이어"]);
  $("layerDetailList").innerHTML = values
    .map(([key, value]) => `<dt>${key}</dt><dd>${escapeHtml(String(value))}</dd>`)
    .join("");
  const textObjects = layer.textObjects || [];
  textObjectsPanel.classList.toggle("hidden", !textObjects.length);
  textObjectsPanel.innerHTML = textObjects.map((object, index) => {
    const attributes = object.attributes || {};
    const fontSize = Number.isFinite(attributes.fontSize)
      ? `${attributes.fontSize / 100} pt`
      : "크기 정보 없음";
    const bounds = attributes.bounds?.length === 4
      ? `${attributes.bounds[0]}, ${attributes.bounds[1]} · ${
          attributes.bounds[2] - attributes.bounds[0]
        }×${attributes.bounds[3] - attributes.bounds[1]}`
      : "위치 정보 없음";
    return `
      <article class="layer-text-object">
        <div class="layer-text-object-heading">객체 ${index + 1}</div>
        <div class="layer-text-object-content">${escapeHtml(object.text || "(빈 텍스트)")}</div>
        <div class="layer-text-object-meta">${escapeHtml(
          `${attributes.font || "글꼴 정보 없음"} · ${fontSize} · ${bounds}`,
        )}</div>
      </article>`;
  }).join("");
}

function renderLayerPanel() {
  const layerDocument = state.layerDocument;
  if (!layerDocument) return;
  $("layerHelp").textContent = layerDocument.note || "";
  const tree = $("layerTree");
  tree.innerHTML = "";
  const append = (layers, depth = 0) => {
    for (const layer of layers || []) {
      const isGroup = layer.type === "group" || Boolean(layer.children?.length);
      const element = document.createElement("div");
      element.className = `layer-row ${isGroup ? "layer-group-row" : "layer-leaf-row"}${
        state.selectedLayerId === layer.id ? " selected" : ""
      }`;
      element.style.setProperty("--layer-depth", String(depth));
      element.dataset.layerId = layer.id;
      element.dataset.layerType = layer.type;

      const visibility = document.createElement("button");
      visibility.type = "button";
      visibility.className = `layer-visibility${
        state.layerVisibility[layer.id] ? " active" : ""
      }`;
      const toggleAvailable = layerDocument.toggleSupported && layer.toggleAvailable !== false;
      visibility.title = toggleAvailable
        ? (state.layerVisibility[layer.id] ? "레이어 숨기기" : "레이어 표시")
        : "CSP 전용 렌더러가 필요한 레이어입니다.";
      visibility.disabled = !toggleAvailable;
      visibility.addEventListener("click", (event) => {
        event.stopPropagation();
        void toggleLayerVisibility(layer.id);
      });

      const visual = document.createElement(isGroup ? "button" : "span");
      if (isGroup) {
        const collapsed = state.collapsedLayerIds.has(layer.id);
        visual.type = "button";
        visual.className = `layer-folder-toggle${collapsed ? " collapsed" : ""}`;
        visual.setAttribute("aria-expanded", String(!collapsed));
        visual.title = collapsed ? "폴더 펼치기" : "폴더 접기";
        visual.innerHTML = `
          <span class="layer-disclosure" aria-hidden="true"></span>
          <span class="layer-folder-icon" aria-hidden="true"></span>
        `;
        const toggleFolder = (event) => {
          event.stopPropagation();
          if (collapsed) state.collapsedLayerIds.delete(layer.id);
          else state.collapsedLayerIds.add(layer.id);
          renderLayerPanel();
        };
        visual.addEventListener("click", toggleFolder);
      } else if (
        layerDocument.thumbnailSupported &&
        layer.thumbnailAvailable !== false
      ) {
        visual.className = "layer-thumb-frame";
        const thumbnail = document.createElement("img");
        thumbnail.className = "layer-thumbnail";
        thumbnail.dataset.layerId = layer.id;
        thumbnail.alt = "";
        thumbnail.decoding = "async";
        if (state.layerThumbnails[layer.id]) {
          thumbnail.src = state.layerThumbnails[layer.id];
        }
        visual.appendChild(thumbnail);
      } else {
        visual.className = "layer-placeholder-icon";
        visual.setAttribute("aria-hidden", "true");
      }

      const name = document.createElement("button");
      name.type = "button";
      name.className = "layer-name";
      name.innerHTML = `
        <span class="layer-title">${escapeHtml(layer.name)}</span>
        <span class="layer-meta">${layer.opacity}% · ${
          escapeHtml(layer.blendModeLabel || layer.blendMode || "표준")
        }</span>
      `;
      name.addEventListener("click", () => selectLayer(layer.id));

      element.append(visibility, visual, name);
      tree.appendChild(element);
      if (!isGroup || !state.collapsedLayerIds.has(layer.id)) {
        append(layer.children, depth + 1);
      }
    }
  };
  append(layerDocument.layers);
  renderLayerDetails();
  if (state.layersVisible) void ensureLayerThumbnails();
}

async function ensureLayerThumbnails() {
  const layerDocument = state.layerDocument;
  if (!layerDocument?.thumbnailSupported) return;
  const sequence = state.layerThumbnailSequence;
  const item = currentItem();
  if (!item) return;
  const visibleLayers = [];
  const appendVisible = (layers) => {
    for (const layer of layers || []) {
      visibleLayers.push(layer);
      if (!state.collapsedLayerIds.has(layer.id)) appendVisible(layer.children);
    }
  };
  appendVisible(layerDocument.layers);
  const queue = visibleLayers.filter((layer) => (
    layer.type !== "group" &&
    !layer.children?.length &&
    layer.thumbnailAvailable !== false &&
    !Object.prototype.hasOwnProperty.call(state.layerThumbnails, layer.id) &&
    !state.layerThumbnailLoading.has(layer.id)
  ));
  if (!queue.length) return;
  queue.forEach((layer) => state.layerThumbnailLoading.add(layer.id));

  const loadNext = async () => {
    while (queue.length && sequence === state.layerThumbnailSequence) {
      const layer = queue.shift();
      try {
        const dataUrl = await window.clipView.loadLayerThumbnail(item, layer.id);
        if (sequence !== state.layerThumbnailSequence) continue;
        state.layerThumbnails[layer.id] = dataUrl || "";
        if (!dataUrl) continue;
        const thumbnail = document.querySelector(
          `.layer-thumbnail[data-layer-id="${CSS.escape(layer.id)}"]`,
        );
        if (thumbnail) thumbnail.src = dataUrl;
      } catch {
        // A missing or unsupported layer preview does not block the layer panel.
        if (sequence === state.layerThumbnailSequence) {
          state.layerThumbnails[layer.id] = "";
        }
      } finally {
        if (sequence === state.layerThumbnailSequence) {
          state.layerThumbnailLoading.delete(layer.id);
        }
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(2, queue.length) }, loadNext));
}

function selectLayer(id) {
  state.selectedLayerId = id;
  layerAncestorIds(state.layerDocument?.layers, id)?.forEach((ancestorId) => {
    state.collapsedLayerIds.delete(ancestorId);
  });
  renderLayerPanel();
  document.querySelector(`.layer-row[data-layer-id="${CSS.escape(id)}"]`)
    ?.scrollIntoView({ block: "nearest" });
}

async function toggleLayerVisibility(id) {
  const layer = layerById(id);
  if (!state.layerDocument?.toggleSupported || layer?.toggleAvailable === false) return;
  state.layerVisibility[id] = !state.layerVisibility[id];
  renderLayerPanel();
  const sequence = ++state.layerRenderSequence;
  const loadingTimer = setTimeout(() => {
    if (sequence === state.layerRenderSequence) loading.classList.remove("hidden");
  }, 150);
  try {
    const result = await window.clipView.renderLayeredImage(
      currentItem(),
      state.layerVisibility,
    );
    if (sequence !== state.layerRenderSequence) return;
    await decodeImage(result.dataUrl);
    state.imageDataUrl = result.dataUrl;
    state.metadata = result.metadata;
    image.src = result.dataUrl;
    await image.decode();
    if (sequence !== state.layerRenderSequence) return;
    state.naturalWidth = image.naturalWidth;
    state.naturalHeight = image.naturalHeight;
    state.renderedLayerVisibility = { ...state.layerVisibility };
    if (state.fitMode) fitImage();
    else applyTransform();
    updateUi();
  } catch (error) {
    if (sequence === state.layerRenderSequence) {
      state.layerVisibility = { ...state.renderedLayerVisibility };
      renderLayerPanel();
      showToast(error?.message || "레이어 표시를 변경하지 못했습니다.", true);
    }
  } finally {
    clearTimeout(loadingTimer);
    if (sequence === state.layerRenderSequence) loading.classList.add("hidden");
  }
}

function toggleLayers(force) {
  if (!state.layerDocument) return;
  state.layersVisible = force ?? !state.layersVisible;
  $("layerPanel").classList.toggle("hidden", !state.layersVisible);
  $("layerBtn").classList.toggle("active", state.layersVisible);
  if (state.layersVisible && state.infoVisible) toggleInfo(false);
  if (state.layersVisible) {
    void ensureLayerThumbnails();
    if (!state.layerPreparationStarted) {
      state.layerPreparationStarted = true;
      const item = currentItem();
      setTimeout(() => {
        if (item) void window.clipView.prepareLayeredImage(item).catch(() => {});
      }, 150);
    }
  }
  setTimeout(() => state.fitMode && fitImage(), 0);
}

function imagePointFromEvent(event) {
  if (!state.naturalWidth || !state.naturalHeight || !state.zoom) return null;
  const rect = stage.getBoundingClientRect();
  let x = event.clientX - (rect.left + rect.width / 2 + state.panX);
  let y = event.clientY - (rect.top + rect.height / 2 + state.panY);
  const angle = -state.rotation * Math.PI / 180;
  const rotatedX = x * Math.cos(angle) - y * Math.sin(angle);
  const rotatedY = x * Math.sin(angle) + y * Math.cos(angle);
  x = rotatedX / (state.zoom * state.flipX) + state.naturalWidth / 2;
  y = rotatedY / state.zoom + state.naturalHeight / 2;
  if (x < 0 || y < 0 || x >= state.naturalWidth || y >= state.naturalHeight) return null;
  return { x, y };
}

async function pickLayerAt(event) {
  if (!state.layerDocument?.pickSupported) return;
  const point = imagePointFromEvent(event);
  if (!point) return;
  try {
    const id = await window.clipView.pickLayer(
      currentItem(),
      point.x,
      point.y,
      state.layerVisibility,
    );
    if (!id) {
      showToast("해당 위치에서 레이어를 찾지 못했습니다.");
      return;
    }
    if (!state.layersVisible) toggleLayers(true);
    selectLayer(id);
  } catch (error) {
    showToast(error?.message || "레이어를 선택하지 못했습니다.", true);
  }
}

function hideVideoContextMenu() {
  videoContextMenu.classList.add("hidden");
}

function setVideoControlsMode(mode) {
  state.videoControlsMode = mode;
  videoControls.classList.toggle("peek-progress", mode === "progress");
  videoControls.classList.toggle("peek-volume", mode === "volume");
}

function hideVideoControls() {
  if (state.mediaType !== "video") return;
  if (videoControlInteracting) {
    clearTimeout(state.videoControlsTimer);
    state.videoControlsTimer = setTimeout(hideVideoControls, 500);
    return;
  }
  videoControls.classList.add("controls-hidden");
  state.videoControlsLockedMode = "";
  document.body.classList.toggle("video-cursor-hidden", !video.paused);
  hideVideoContextMenu();
}

function videoControlsVisible() {
  return (
    !videoControls.classList.contains("hidden") &&
    !videoControls.classList.contains("controls-hidden")
  );
}

function showVideoControls(mode = "full", timeout = 2000, options = {}) {
  if (state.mediaType !== "video") return;
  clearTimeout(state.videoControlsTimer);
  const locked = state.videoControlsLockedMode;
  const effectiveMode = (
    mode === "full" &&
    locked &&
    videoControlsVisible() &&
    !options.forceFull
  ) ? locked : mode;
  setVideoControlsMode(effectiveMode);
  state.videoControlsLockedMode = (
    options.lockMode ||
    (locked && effectiveMode === locked && mode === "full" && videoControlsVisible())
  ) ? effectiveMode : "";
  videoControls.classList.remove("hidden", "controls-hidden");
  document.body.classList.remove("video-cursor-hidden");
  if (timeout > 0) {
    state.videoControlsTimer = setTimeout(hideVideoControls, timeout);
  }
}

function showVideoInputFeedback(mode, timeout = 1400) {
  if (state.videoControlsLockedMode) {
    showVideoControls(state.videoControlsLockedMode, timeout, { lockMode: true });
  } else if (videoControlsVisible() && state.videoControlsMode === "full") {
    showVideoControls("full", 2000);
  } else {
    showVideoControls(mode, timeout, { lockMode: true });
  }
}

function showVideoContextMenu(event) {
  if (state.mediaType !== "video") return;
  event.preventDefault();
  showVideoControls("full", 4000, { forceFull: true });
  videoContextMenu.classList.remove("hidden");
  const rect = stage.getBoundingClientRect();
  const menuRect = videoContextMenu.getBoundingClientRect();
  const left = Math.min(
    Math.max(event.clientX - rect.left, 8),
    Math.max(8, rect.width - menuRect.width - 8),
  );
  const top = Math.min(
    Math.max(event.clientY - rect.top, 8),
    Math.max(8, rect.height - menuRect.height - 8),
  );
  videoContextMenu.style.left = `${left}px`;
  videoContextMenu.style.top = `${top}px`;
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
  if (isVideoItem(item)) return Promise.reject(new Error("이미지 파일이 아닙니다."));
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
    if (isVideoItem(state.items[index])) continue;
    void loadImageCached(index).catch(() => {});
  }
}

function formatDuration(seconds) {
  if (!Number.isFinite(seconds) || seconds <= 0) return "-";
  const total = Math.round(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  return hours
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(secs).padStart(2, "0")}`
    : `${minutes}:${String(secs).padStart(2, "0")}`;
}

function attachSubtitleTracks(subtitles) {
  clearSubtitleTracks();
  subtitles.forEach((subtitle, index) => {
    const url = URL.createObjectURL(new Blob([subtitle.vtt], { type: "text/vtt" }));
    state.subtitleObjectUrls.push(url);
    const track = document.createElement("track");
    track.kind = "subtitles";
    track.label = subtitle.label || subtitle.name || "자막";
    track.srclang = subtitle.srclang || "und";
    track.src = url;
    track.default = index === 0;
    video.appendChild(track);
    track.addEventListener("load", () => {
      track.track.mode = index === 0 ? "showing" : "disabled";
      updateVideoUi();
    });
  });
}

function formatVideoTime(seconds) {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0:00";
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  return hours
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(secs).padStart(2, "0")}`
    : `${minutes}:${String(secs).padStart(2, "0")}`;
}

function subtitleTrackShowing() {
  return [...video.textTracks].some((track) => track.mode === "showing");
}

function setSubtitleTrackMode(showing) {
  [...video.textTracks].forEach((track, index) => {
    track.mode = showing && index === 0 ? "showing" : "disabled";
  });
  updateVideoUi();
}

function updateRangeProgress(input, percent) {
  input.style.setProperty("--progress", `${Math.max(0, Math.min(100, percent))}%`);
}

function updateVideoUi() {
  const duration = Number.isFinite(video.duration) ? video.duration : 0;
  const current = Number.isFinite(video.currentTime) ? video.currentTime : 0;
  const progress = duration ? current / duration * 100 : 0;
  videoSeek.value = String(duration ? Math.round(current / duration * 1000) : 0);
  updateRangeProgress(videoSeek, progress);
  videoTimeLabel.textContent = `${formatVideoTime(current)} / ${formatVideoTime(duration)}`;
  videoTitleLabel.textContent = currentItem()?.name || "";
  videoPlayBtn.classList.toggle("playing", !video.paused);
  videoPlayBtn.title = video.paused
    ? "재생 (Ctrl+Space)"
    : "일시정지 (Ctrl+Space)";
  if (video.paused) document.body.classList.remove("video-cursor-hidden");
  videoVolume.value = String(video.muted ? 0 : video.volume);
  updateRangeProgress(videoVolume, (video.muted ? 0 : video.volume) * 100);
  videoMuteBtn.classList.toggle("active", video.muted || video.volume === 0);
  videoSpeedSelect.value = String(video.playbackRate);
  const hasSubtitles = video.textTracks.length > 0;
  videoSubtitleBtn.disabled = !hasSubtitles;
  videoSubtitleBtn.classList.toggle("active", hasSubtitles && subtitleTrackShowing());
  videoModeBtn.textContent = VIDEO_PLAYBACK_MODES[state.videoPlaybackMode] || "순서";
  videoContextMenu.querySelectorAll("[data-video-mode]").forEach((button) => {
    button.classList.toggle("active", button.dataset.videoMode === state.videoPlaybackMode);
  });
  videoContextMenu.querySelectorAll("[data-video-speed]").forEach((button) => {
    button.classList.toggle("active", Number(button.dataset.videoSpeed) === video.playbackRate);
  });
  videoContextMenu.querySelector("[data-video-action='playPause']").textContent = (
    video.paused ? "재생" : "일시정지"
  );
  videoContextMenu.querySelector("[data-video-action='mute']").classList.toggle(
    "active",
    video.muted || video.volume === 0,
  );
  videoContextMenu.querySelector("[data-video-action='subtitles']").classList.toggle(
    "active",
    hasSubtitles && subtitleTrackShowing(),
  );
  videoContextMenu.querySelector("[data-video-action='subtitles']").disabled = !hasSubtitles;
}

function setVideoPlaybackMode(mode) {
  if (!VIDEO_PLAYBACK_MODES[mode]) return;
  state.videoPlaybackMode = mode;
  localStorage.setItem("videoPlaybackMode", mode);
  updateVideoUi();
  showVideoControls("full", 2000);
}

function cycleVideoPlaybackMode() {
  const modes = Object.keys(VIDEO_PLAYBACK_MODES);
  const next = modes[(modes.indexOf(state.videoPlaybackMode) + 1) % modes.length];
  setVideoPlaybackMode(next);
}

function videoItemIndices() {
  return state.items
    .map((item, index) => (isVideoItem(item) ? index : -1))
    .filter((index) => index >= 0);
}

function nextVideoIndexForMode() {
  const indices = videoItemIndices();
  if (!indices.length) return -1;
  const position = indices.indexOf(state.index);
  if (state.videoPlaybackMode === "shuffle") {
    if (indices.length === 1) return indices[0];
    const choices = indices.filter((index) => index !== state.index);
    return choices[Math.floor(Math.random() * choices.length)];
  }
  if (state.videoPlaybackMode === "repeatAll") {
    return indices[(position + 1 + indices.length) % indices.length];
  }
  const next = indices[position + 1];
  return Number.isInteger(next) ? next : -1;
}

async function playVideoIndex(index) {
  if (index < 0) return;
  await selectItem(index);
  if (state.mediaType === "video") {
    void video.play().catch(() => {});
    showVideoControls("full", 2000);
  }
}

function handleVideoEnded() {
  if (state.mediaType !== "video") return;
  if (state.videoPlaybackMode === "repeatOne") {
    video.currentTime = 0;
    void video.play().catch(() => {});
    showVideoControls("progress", 1200);
    return;
  }
  const next = nextVideoIndexForMode();
  if (next >= 0) void playVideoIndex(next);
  else showVideoControls("full", 0);
}

function handleVideoContextAction(action) {
  switch (action) {
    case "playPause":
      toggleVideoPlayback();
      break;
    case "restart":
      video.currentTime = 0;
      void video.play().catch(() => {});
      break;
    case "seekBack":
      seekVideoBy(-5);
      break;
    case "seekForward":
      seekVideoBy(5);
      break;
    case "mute":
      video.muted = !video.muted;
      updateVideoUi();
      showVideoControls("volume", 1200, { lockMode: true });
      break;
    case "subtitles":
      if (video.textTracks.length) setSubtitleTrackMode(!subtitleTrackShowing());
      break;
    case "fit":
      fitImage();
      break;
    case "actual":
      actualSize();
      break;
    case "fullscreen":
      void toggleFullscreen();
      break;
    case "reveal":
      if (currentItem()) void window.clipView.showInFolder(currentItem());
      break;
    case "openOriginal":
      if (currentItem()) void window.clipView.openOriginal(currentItem());
      break;
    default:
      return;
  }
  updateVideoUi();
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
  const transform = [
    `translate(${state.panX}px, ${state.panY}px)`,
    `rotate(${state.rotation}deg)`,
    `scale(${state.zoom * state.flipX}, ${state.zoom})`,
    "translate(-50%, -50%)",
  ].join(" ");
  image.style.transform = state.mediaType === "image" ? transform : "";
  video.style.transform = state.mediaType === "video" ? transform : "";
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
  if (!hasMedia()) return;
  state.fitMode = false;
  state.zoom = Math.min(32, Math.max(0.02, state.zoom * factor));
  applyTransform();
}

function rotate(amount) {
  if (!hasMedia()) return;
  state.rotation = (state.rotation + amount + 360) % 360;
  if (state.fitMode) fitImage();
  else applyTransform();
}

function updateInfoPanel() {
  const item = currentItem();
  const meta = state.metadata || {};
  const values = state.mediaType === "video" ? [
    ["파일", item?.name || "-"],
    ["경로", item?.displayPath || item?.path || "-"],
    ["크기", `${meta.width || 0} × ${meta.height || 0}`],
    ["형식", meta.format || "-"],
    ["브라우저 호환", meta.browserSupport || "-"],
    ["파일 용량", formatBytes(meta.byteSize)],
    ["재생 시간", formatDuration(meta.duration)],
    ["자막", meta.subtitleCount ? `${meta.subtitleCount}개` : "없음"],
    ["불러오기", meta.source || "-"],
    ["수정 시각", meta.modifiedAt ? new Date(meta.modifiedAt).toLocaleString() : "-"],
  ] : [
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
  $("fileTitle").textContent = item?.name || "미디어를 열어주세요";
  document.title = item ? `${item.name} - Clip Image Viewer` : "Clip Image Viewer";
  $("counter").textContent = state.items.length ? `${state.index + 1} / ${state.items.length}` : "0 / 0";
  $("dimensionLabel").textContent = state.metadata?.width
    ? `${state.metadata.width} × ${state.metadata.height} · ${state.metadata.format}`
    : "-";
  $("prevOverlay").classList.toggle("hidden", state.items.length < 2);
  $("nextOverlay").classList.toggle("hidden", state.items.length < 2);
  const showCropModes = !isVideoItem(item) && Boolean(state.comic?.cropAvailable);
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
  if (isVideoItem(item)) {
    await loadCurrentVideo(item);
    return;
  }
  await loadCurrentImage(item);
}

async function loadCurrentImage(item) {
  const sequence = ++loadSequence;
  state.layerRenderSequence += 1;
  const cached = imageCache.has(imageKey(state.index));
  loading.classList.toggle("hidden", cached);
  emptyState.classList.add("hidden");
  clearVideo();
  clearLayerDocument();
  video.classList.add("hidden");
  state.mediaType = "image";
  if (!state.imageDataUrl) imageLayer.classList.add("hidden");

  try {
    const result = await loadImageCached(state.index);
    if (sequence !== loadSequence) return;
    state.imageDataUrl = result.dataUrl;
    state.metadata = result.metadata;
    setLayerDocument(result.layerDocument);
    state.rotation = 0;
    state.flipX = 1;
    state.panX = 0;
    state.panY = 0;
    state.fitMode = true;

    image.src = result.dataUrl;
    await image.decode();
    if (sequence !== loadSequence) return;
    state.naturalWidth = image.naturalWidth;
    state.naturalHeight = image.naturalHeight;
    image.alt = item.name;
    image.classList.remove("hidden");
    imageLayer.classList.remove("hidden");
    fitImage();
    updateUi();
    preloadAdjacentImages();
    if (state.layerDocument?.toggleSupported) {
      state.layerPreparationStarted = true;
      setTimeout(() => {
        if (sequence === loadSequence && currentItem() === item) {
          void window.clipView.prepareLayeredImage(item).catch(() => {});
        }
      }, 100);
    }
  } catch (error) {
    if (sequence !== loadSequence) return;
    state.imageDataUrl = "";
    state.mediaType = "";
    imageLayer.classList.add("hidden");
    emptyState.classList.remove("hidden");
    showToast(error?.message || "이미지를 열지 못했습니다.", true);
  } finally {
    if (sequence === loadSequence) loading.classList.add("hidden");
  }
}

async function loadCurrentVideo(item) {
  const sequence = ++loadSequence;
  loading.classList.remove("hidden");
  emptyState.classList.add("hidden");
  clearImage();
  clearVideo();
  state.mediaType = "video";
  imageLayer.classList.add("hidden");

  try {
    const [media, subtitles] = await Promise.all([
      window.clipView.mediaFileUrl(item),
      window.clipView.findSubtitles(item).catch(() => []),
    ]);
    if (sequence !== loadSequence) return;
    const browserSupport = media.mime ? video.canPlayType(media.mime) : "";

    state.rotation = 0;
    state.flipX = 1;
    state.panX = 0;
    state.panY = 0;
    state.fitMode = true;

    await new Promise((resolve, reject) => {
      const cleanup = () => {
        video.removeEventListener("loadedmetadata", handleLoaded);
        video.removeEventListener("error", handleError);
        cancelPendingVideoLoad = null;
      };
      const handleLoaded = () => {
        cleanup();
        resolve();
      };
      const handleError = () => {
        cleanup();
        reject(new Error(
          browserSupport
            ? "동영상 파일 또는 코덱을 재생할 수 없습니다."
            : "Electron이 이 동영상 컨테이너 또는 코덱을 지원하지 않습니다.",
        ));
      };
      cancelPendingVideoLoad = () => {
        cleanup();
        reject(new Error("동영상 불러오기가 취소되었습니다."));
      };
      video.addEventListener("loadedmetadata", handleLoaded, { once: true });
      video.addEventListener("error", handleError, { once: true });
      video.controls = false;
      video.src = media.url;
      video.load();
    });
    if (sequence !== loadSequence) return;

    state.videoSrc = media.url;
    state.naturalWidth = video.videoWidth || 16;
    state.naturalHeight = video.videoHeight || 9;
    video.setAttribute("width", String(state.naturalWidth));
    video.setAttribute("height", String(state.naturalHeight));
    state.metadata = {
      ...media.metadata,
      width: state.naturalWidth,
      height: state.naturalHeight,
      duration: video.duration,
      subtitleCount: subtitles.length,
      browserSupport: browserSupport || "확인되지 않음",
    };
    attachSubtitleTracks(subtitles);
    video.classList.remove("hidden");
    imageLayer.classList.remove("hidden");
    imageLayer.classList.add("video-active");
    fitImage();
    updateVideoUi();
    showVideoControls("full", 2000);
    updateUi();
    preloadAdjacentImages();
  } catch (error) {
    if (sequence !== loadSequence) return;
    state.videoSrc = "";
    state.mediaType = "";
    state.metadata = null;
    videoControls.classList.add("hidden");
    imageLayer.classList.remove("video-active");
    imageLayer.classList.add("hidden");
    emptyState.classList.remove("hidden");
    showToast(error?.message || "동영상을 열지 못했습니다.", true);
  } finally {
    if (sequence === loadSequence) loading.classList.add("hidden");
  }
}

async function openPath(targetPath) {
  if (!targetPath) return;
  const sequence = ++openSequence;
  stopSlideshow();
  try {
    const collection = await window.clipView.openPath(targetPath);
    if (sequence !== openSequence) return;
    if (!collection.items.length) {
      showToast("지원되는 파일이 없습니다.", true);
      return;
    }
    state.items = collection.items;
    state.index = collection.index;
    selectionSequence += 1;
    state.comic = collection.comic || null;
    imageCache.clear();
    thumbnailRenderSequence += 1;
    $("thumbnailStrip").innerHTML = "";
    updateUi();
    await loadCurrent();
    if (state.thumbsVisible) renderThumbnails();
  } catch (error) {
    if (sequence !== openSequence) return;
    showToast(error?.message || "경로를 열지 못했습니다.", true);
  }
}

async function selectItem(index, options = {}) {
  if (!Number.isInteger(index) || index < 0 || index >= state.items.length) return;
  if (options.stopSlideshow !== false) stopSlideshow();
  const sequence = ++selectionSequence;
  state.index = index;
  updateUi();
  await new Promise((resolve) => setTimeout(resolve, options.immediate ? 0 : 40));
  if (sequence !== selectionSequence) return;
  await loadCurrent();
}

function moveTo(delta, options = {}) {
  if (state.items.length < 2) return;
  let next = state.index + delta;
  if (state.settings.loop) {
    next = (next + state.items.length) % state.items.length;
  } else {
    next = Math.max(0, Math.min(state.items.length - 1, next));
  }
  if (next !== state.index) {
    void selectItem(next, options);
  }
}

function toggleInfo(force) {
  state.infoVisible = force ?? !state.infoVisible;
  $("infoPanel").classList.toggle("hidden", !state.infoVisible);
  $("infoBtn").classList.toggle("active", state.infoVisible);
  if (state.infoVisible && state.layersVisible) toggleLayers(false);
  setTimeout(() => state.fitMode && fitImage(), 0);
}

function toggleThumbnails(force) {
  state.thumbsVisible = force ?? !state.thumbsVisible;
  $("thumbnailPanel").classList.toggle("hidden", !state.thumbsVisible);
  $("thumbBtn").classList.toggle("active", state.thumbsVisible);
  if (state.thumbsVisible) renderThumbnails();
  setTimeout(() => state.fitMode && fitImage(), 0);
}

function videoPlaceholderDataUrl(item) {
  const label = escapeHtml((item.name || "VIDEO").replace(/^(.{22}).+$/, "$1..."));
  const svg = `
    <svg xmlns="http://www.w3.org/2000/svg" width="300" height="152" viewBox="0 0 300 152">
      <rect width="300" height="152" rx="12" fill="#151923"/>
      <path d="M130 49v54l48-27z" fill="#73a6ff"/>
      <text x="150" y="128" fill="#cbd2dd" font-family="Segoe UI, sans-serif"
            font-size="18" text-anchor="middle">VIDEO</text>
      <text x="150" y="145" fill="#7f8998" font-family="Segoe UI, sans-serif"
            font-size="11" text-anchor="middle">${label}</text>
    </svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

async function renderThumbnails() {
  const sequence = ++thumbnailRenderSequence;
  const items = state.items;
  const strip = $("thumbnailStrip");
  strip.innerHTML = "";
  items.forEach((item, index) => {
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
      const item = items[index];
      try {
        const source = isVideoItem(item)
          ? videoPlaceholderDataUrl(item)
          : await window.clipView.loadThumbnail(item);
        if (sequence !== thumbnailRenderSequence || !element.isConnected) return;
        element.querySelector("img").src = source;
      } catch {
        if (sequence !== thumbnailRenderSequence || !element.isConnected) return;
        element.querySelector("span").textContent = `미리보기 없음 · ${item.name}`;
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
  state.slideshowTimer = setInterval(
    () => moveTo(1, { stopSlideshow: false, immediate: true }),
    state.settings.slideInterval,
  );
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

function toggleVideoPlayback() {
  if (state.mediaType !== "video") return;
  if (video.paused) {
    void video.play().catch((error) => {
      showToast(error?.message || "동영상을 재생하지 못했습니다.", true);
    });
  } else {
    video.pause();
  }
  updateVideoUi();
}

function seekVideoBy(seconds) {
  if (state.mediaType !== "video" || !Number.isFinite(video.duration)) return;
  video.currentTime = Math.max(0, Math.min(video.duration, video.currentTime + seconds));
  updateVideoUi();
  showVideoControls("progress", 1200, { lockMode: true });
}

function changeVideoVolume(delta) {
  if (state.mediaType !== "video") return;
  video.muted = false;
  video.volume = Math.max(0, Math.min(1, video.volume + delta));
  updateVideoUi();
  showVideoControls("volume", 1200, { lockMode: true });
}

function handleVideoShortcut(event, key) {
  if (state.mediaType !== "video" || !event.ctrlKey || event.altKey || event.shiftKey) {
    return false;
  }
  if (key === " ") {
    event.preventDefault();
    toggleVideoPlayback();
    return true;
  }
  if (key === "arrowleft") {
    event.preventDefault();
    seekVideoBy(-5);
    return true;
  }
  if (key === "arrowright") {
    event.preventDefault();
    seekVideoBy(5);
    return true;
  }
  if (key === "arrowup") {
    event.preventDefault();
    changeVideoVolume(0.1);
    return true;
  }
  if (key === "arrowdown") {
    event.preventDefault();
    changeVideoVolume(-0.1);
    return true;
  }
  if (key === "m") {
    event.preventDefault();
    video.muted = !video.muted;
    updateVideoUi();
    showVideoControls("volume", 1200, { lockMode: true });
    return true;
  }
  return false;
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
  if (!state.runtime) return;
  const saved = new Set(savedOptionalAssociations());
  const renderList = (extensions) => extensions
    .map((ext) => `
      <label>
        <input class="association-extension" type="checkbox"
               value="${escapeHtml(ext)}"${saved.has(ext) ? " checked" : ""}>
        <span>${escapeHtml(ext.slice(1).toUpperCase())}</span>
      </label>
    `)
    .join("");
  $("imageAssociationOptions").innerHTML = renderList(
    state.runtime.optionalImageAssociations || state.runtime.optionalAssociations || [],
  );
  $("videoAssociationOptions").innerHTML = renderList(
    state.runtime.optionalVideoAssociations || [],
  );
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
    const savedEnabled = localStorage.getItem("associationsEnabled");
    const enabled = savedEnabled === null
      ? state.runtime.associationsEnabled !== false
      : savedEnabled !== "false";
    if (savedEnabled === null) {
      localStorage.setItem("associationsEnabled", String(enabled));
    }
    const registrationToken = `${state.runtime.version}:capabilities-v2`;
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
    if (!hasMedia()) return;
    state.flipX *= -1;
    applyTransform();
  };
  $("slideshowBtn").onclick = () => state.slideshowTimer ? stopSlideshow() : startSlideshow();
  $("fullscreenBtn").onclick = toggleFullscreen;
  videoPlayBtn.onclick = toggleVideoPlayback;
  videoMuteBtn.onclick = () => {
    video.muted = !video.muted;
    updateVideoUi();
    showVideoInputFeedback("volume");
  };
  videoVolume.oninput = (event) => {
    video.volume = Number(event.target.value);
    video.muted = video.volume === 0;
    updateVideoUi();
    showVideoInputFeedback("volume");
  };
  videoSeek.oninput = (event) => {
    if (state.mediaType !== "video" || !Number.isFinite(video.duration)) return;
    video.currentTime = Number(event.target.value) / 1000 * video.duration;
    updateVideoUi();
    showVideoInputFeedback("progress");
  };
  videoSpeedSelect.onchange = (event) => {
    video.playbackRate = Number(event.target.value) || 1;
    updateVideoUi();
    showVideoControls("full", 2000);
  };
  videoModeBtn.onclick = cycleVideoPlaybackMode;
  videoSubtitleBtn.onclick = () => {
    if (!video.textTracks.length) return;
    setSubtitleTrackMode(!subtitleTrackShowing());
    showVideoControls("full", 2000);
  };
  videoFullscreenBtn.onclick = toggleFullscreen;
  video.onclick = (event) => {
    if (event.detail !== 1) return;
    clearTimeout(videoClickTimer);
    videoClickTimer = setTimeout(() => {
      videoClickTimer = null;
      toggleVideoPlayback();
    }, 220);
  };
  $("infoBtn").onclick = () => toggleInfo();
  $("closeInfoBtn").onclick = () => toggleInfo(false);
  $("layerBtn").onclick = () => toggleLayers();
  $("closeLayerBtn").onclick = () => toggleLayers(false);
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
        `${state.runtime.version}:capabilities-v2`,
      );
      showToast("파일 연결 정보를 적용했습니다.");
    } catch (error) {
      showToast(error?.message || "파일 연결을 적용하지 못했습니다.", true);
    }
  };
  $("clearAssociationBtn").onclick = async () => {
    try {
      await window.clipView.registerAssociations([]);
      localStorage.setItem("associationExtensions", "[]");
      localStorage.setItem("associationsEnabled", "false");
      localStorage.setItem(
        "associationRegistrationVersion",
        `${state.runtime.version}:capabilities-v2`,
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
  videoControls.addEventListener("mousedown", (event) => event.stopPropagation());
  videoControls.addEventListener("dblclick", (event) => event.stopPropagation());
  videoControls.addEventListener("mousemove", () => showVideoControls("full", 2000));
  for (const input of [videoSeek, videoVolume]) {
    input.addEventListener("pointerdown", () => {
      videoControlInteracting = true;
      showVideoControls(state.videoControlsMode, 0, {
        lockMode: state.videoControlsMode !== "full",
      });
    });
    input.addEventListener("pointerup", () => {
      videoControlInteracting = false;
      showVideoControls(state.videoControlsMode, 2000, {
        lockMode: state.videoControlsMode !== "full",
      });
    });
    input.addEventListener("pointercancel", () => {
      videoControlInteracting = false;
    });
  }
  videoContextMenu.querySelectorAll("[data-video-mode]").forEach((button) => {
    button.addEventListener("click", () => {
      setVideoPlaybackMode(button.dataset.videoMode);
      hideVideoContextMenu();
    });
  });
  videoContextMenu.querySelectorAll("[data-video-speed]").forEach((button) => {
    button.addEventListener("click", () => {
      video.playbackRate = Number(button.dataset.videoSpeed) || 1;
      updateVideoUi();
      hideVideoContextMenu();
      showVideoControls("full", 2000, { forceFull: true });
    });
  });
  videoContextMenu.querySelectorAll("[data-video-action]").forEach((button) => {
    button.addEventListener("click", () => {
      if (button.disabled) return;
      handleVideoContextAction(button.dataset.videoAction);
      hideVideoContextMenu();
    });
  });
  ["play", "pause", "timeupdate", "durationchange", "volumechange", "ratechange", "loadedmetadata"]
    .forEach((eventName) => video.addEventListener(eventName, updateVideoUi));
  video.addEventListener("ended", handleVideoEnded);
}

stage.addEventListener("wheel", (event) => {
  if (!hasMedia()) return;
  if (event.target.closest(".video-controls")) return;
  event.preventDefault();
  zoomBy(event.deltaY < 0 ? 1.12 : 1 / 1.12);
}, { passive: false });

stage.addEventListener("mousemove", () => {
  if (state.mediaType === "video") showVideoControls("full", 2000);
});

stage.addEventListener("contextmenu", (event) => {
  if (state.mediaType === "video") {
    showVideoContextMenu(event);
  }
});

imageLayer.addEventListener("mousedown", (event) => {
  if (event.button !== 0) return;
  if (event.target.closest(".video-controls")) return;
  if (state.mediaType === "video" && event.target === video) return;
  if (event.ctrlKey && state.mediaType === "image" && state.layerDocument?.pickSupported) {
    event.preventDefault();
    void pickLayerAt(event);
    return;
  }
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

window.addEventListener("pointerup", () => {
  if (!videoControlInteracting) return;
  videoControlInteracting = false;
  showVideoControls(state.videoControlsMode, 2000, {
    lockMode: state.videoControlsMode !== "full",
  });
});

window.addEventListener("pointercancel", () => {
  videoControlInteracting = false;
});

window.addEventListener("click", (event) => {
  if (!event.target.closest(".video-context-menu")) hideVideoContextMenu();
});

window.addEventListener("resize", () => {
  if (state.fitMode) fitImage();
});

window.addEventListener("keydown", async (event) => {
  if ($("settingsDialog").open) return;
  const key = event.key.toLowerCase();
  if (handleVideoShortcut(event, key)) {
    return;
  }
  const formControl = event.target.closest?.("input, select, textarea");
  if (formControl && formControl.id !== "cropModeSelect") return;
  if (event.ctrlKey && key === "o") {
    event.preventDefault();
    const kind = event.shiftKey ? "folder" : "file";
    openPath(await window.clipView.openDialog(kind));
  } else if (event.ctrlKey && key === "c") {
    event.preventDefault();
    const dataUrl = transformedPngDataUrl();
    if (dataUrl) {
      await window.clipView.copyImage(dataUrl);
      showToast("이미지를 클립보드에 복사했습니다.");
    }
  } else if (event.ctrlKey && key === "s") {
    event.preventDefault();
    const dataUrl = transformedPngDataUrl();
    if (dataUrl) {
      const stem = currentItem().name.replace(/\.[^.]+$/, "");
      await window.clipView.saveImageCopy(dataUrl, `${stem}.png`);
    }
  } else if (event.ctrlKey || event.altKey || event.metaKey) {
    return;
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
    if (!hasMedia()) return;
    state.flipX *= -1;
    applyTransform();
  } else if (key === "i") {
    toggleInfo();
  } else if (key === "l") {
    toggleLayers();
  } else if (key === "t") {
    toggleThumbnails();
  } else if (key === "s") {
    state.slideshowTimer ? stopSlideshow() : startSlideshow();
  } else if (key === "enter" || key === "f11") {
    event.preventDefault();
    toggleFullscreen();
  } else if (key === "escape") {
    if (!videoContextMenu.classList.contains("hidden")) {
      hideVideoContextMenu();
    } else if (state.fullscreen) {
      toggleFullscreen();
    }
  }
});

stage.addEventListener("dblclick", (event) => {
  if (event.target.closest("button, input, select, .video-controls")) return;
  clearTimeout(videoClickTimer);
  videoClickTimer = null;
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
window.clipView.onFullscreenState((enabled) => {
  state.fullscreen = enabled;
  document.body.classList.toggle("fullscreen-ui", enabled);
  setTimeout(() => state.fitMode && fitImage(), 0);
});
bindActions();
applyBackground(state.settings.background);
void initializeRuntimeInfo().catch(() => {});
