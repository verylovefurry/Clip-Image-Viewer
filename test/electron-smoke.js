"use strict";

const fs = require("fs");
const { nativeImage } = require("electron");

function ignoreBrokenPipe(stream) {
  if (!stream || typeof stream.on !== "function") return;
  stream.on("error", (error) => {
    if (error?.code === "EPIPE") return;
    process.exitCode = 1;
  });
}

ignoreBrokenPipe(process.stdout);
ignoreBrokenPipe(process.stderr);

function wait(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function attachElectronSmoke({
  app,
  mainWindow,
  expectedPath,
  productName,
  screen,
  smokeListTest,
  smokeNavigationTest,
  smokeSettingsTest,
  smokeUseWindowState,
  optionalAssociationCount,
  fileAssociationSupported,
}) {
  mainWindow.webContents.once("did-finish-load", async () => {
    try {
      await wait(500);
      let title = await mainWindow.webContents.executeJavaScript("document.title");
      let mediaState = null;
      if (expectedPath) {
        for (let attempt = 0; attempt < 100; attempt += 1) {
          mediaState = await mainWindow.webContents.executeJavaScript(`(() => {
            const layer = document.getElementById("imageLayer");
            const loading = document.getElementById("loading");
            const image = document.getElementById("viewerImage");
            const video = document.getElementById("viewerVideo");
            return {
              imageRendered: image.complete && image.naturalWidth > 0 &&
                !image.classList.contains("hidden"),
              videoRendered: video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA &&
                !video.classList.contains("hidden"),
              videoPlaying: !video.paused && !video.ended,
              visible: !layer.classList.contains("hidden"),
              loadingHidden: loading.classList.contains("hidden"),
            };
          })()`);
          title = await mainWindow.webContents.executeJavaScript("document.title");
          if (
            title !== productName &&
            mediaState.visible &&
            mediaState.loadingHidden &&
            (mediaState.imageRendered || mediaState.videoRendered) &&
            (!process.env.CLIPVIEW_SMOKE_EXPECT_VIDEO || mediaState.videoPlaying)
          ) break;
          await wait(100);
        }
        if (title === productName || !mediaState?.visible || !mediaState?.loadingHidden) {
          throw new Error(`Media did not finish loading: ${JSON.stringify(mediaState)}`);
        }
        if (process.env.CLIPVIEW_SMOKE_EXPECT_VIDEO && !mediaState.videoPlaying) {
          throw new Error(`Video did not autoplay: ${JSON.stringify(mediaState)}`);
        }
      }

      await mainWindow.webContents.executeJavaScript(
        "new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))",
      );
      await wait(250);

      if (process.env.CLIPVIEW_SMOKE_VIDEO_SHORTCUT === "1") {
        const beforeTitle = title;
        mainWindow.webContents.sendInputEvent({ type: "keyDown", keyCode: "SPACE" });
        mainWindow.webContents.sendInputEvent({ type: "keyUp", keyCode: "SPACE" });
        await wait(150);
        const paused = await mainWindow.webContents.executeJavaScript(`(() => ({
          paused: document.getElementById("viewerVideo").paused,
          title: document.title,
        }))()`);
        if (!paused.paused || paused.title !== beforeTitle) {
          throw new Error(`Space did not pause the current video: ${JSON.stringify(paused)}`);
        }
        mainWindow.webContents.sendInputEvent({ type: "keyDown", keyCode: "SPACE" });
        mainWindow.webContents.sendInputEvent({ type: "keyUp", keyCode: "SPACE" });
        await wait(150);
        const resumed = await mainWindow.webContents.executeJavaScript(`(() => ({
          paused: document.getElementById("viewerVideo").paused,
          title: document.title,
        }))()`);
        if (resumed.paused || resumed.title !== beforeTitle) {
          throw new Error(`Space did not resume the current video: ${JSON.stringify(resumed)}`);
        }
      }

      if (process.env.CLIPVIEW_SMOKE_DECODER_RACE === "1") {
        const decoderTest = await mainWindow.webContents.executeJavaScript(`(async () => {
          const originalDecode = HTMLImageElement.prototype.decode;
          HTMLImageElement.prototype.decode = function () {
            if (this.isConnected && this.id === "viewerImage") {
              return Promise.reject(new DOMException("The source image cannot be decoded.", "EncodingError"));
            }
            return originalDecode.call(this);
          };
          try {
            const next = (state.index + 1) % state.items.length;
            await selectItem(next, { immediate: true });
            const visible = document.getElementById("viewerImage");
            if (imageLayer.classList.contains("hidden") || !visible.naturalWidth ||
                visible.alt !== currentItem().name) {
              throw new Error("Transient display decoding failure left the viewer empty");
            }
            await selectItem((next + 1) % state.items.length, { immediate: true });
            await selectItem(next, { immediate: true });
            if (!image.naturalWidth || image.alt !== currentItem().name) {
              throw new Error("Cached image failed after a return navigation");
            }
            return { index: state.index, name: currentItem().name };
          } finally {
            HTMLImageElement.prototype.decode = originalDecode;
          }
        })()`);
        console.log("Display decoder regression passed: " + JSON.stringify(decoderTest));
      }

      if (process.env.CLIPVIEW_SMOKE_RAPID_NAVIGATION === "1") {
        const navigation = await mainWindow.webContents.executeJavaScript(`(async () => {
          if (state.items.length < 3) throw new Error("Rapid navigation needs at least three items");
          const errors = [];
          const handleRejection = (event) => errors.push(String(event.reason));
          window.addEventListener("unhandledrejection", handleRejection);
          const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
          try {
            toggleThumbnails(true);
            for (let index = 0; index < 72; index += 1) {
              moveTo(index < 48 ? 1 : -1);
              await delay(index % 3 === 0 ? 5 : 55);
              if (index % 12 === 0) {
                toggleThumbnails(false);
                toggleThumbnails(true);
              }
            }
            for (let index = 0; index < 48; index += 1) moveTo(index % 3 ? 1 : -1);
            const expectedIndex = state.index;
            let settled = false;
            for (let attempt = 0; attempt < 200; attempt += 1) {
              await delay(100);
              settled = loading.classList.contains("hidden") &&
                !imageLayer.classList.contains("hidden") &&
                (state.mediaType === "video"
                  ? video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA && !video.paused
                  : image.complete && image.naturalWidth > 0 && image.alt === currentItem().name);
              if (settled) break;
            }
            if (!settled || state.index !== expectedIndex) {
              throw new Error("Rapid navigation did not show the last selected item: " + currentItem().name);
            }
            if (state.mediaType === "image") {
              const expected = await window.clipView.loadImage(currentItem(), state.cropMode);
              if (image.src !== expected.dataUrl || state.imageDataUrl !== expected.dataUrl) {
                throw new Error("An obsolete image overwrote the last selection");
              }
              if (state.layerPreparationStarted) throw new Error("Hidden layer panel started a full render");
            }
            if (imageCache.size > 3) throw new Error("Navigation retained obsolete image cache entries");
            const bytes = [...imageCache.values()].reduce((sum, entry) => sum + entry.bytes, 0);
            if (bytes > MAX_IMAGE_CACHE_BYTES && imageCache.size > 1) {
              throw new Error("Decoded image cache exceeded its memory budget");
            }
            if (errors.length) throw new Error(errors.join("; "));
            return { moves: 120, index: state.index, name: currentItem().name, cacheEntries: imageCache.size };
          } finally {
            toggleThumbnails(false);
            window.removeEventListener("unhandledrejection", handleRejection);
          }
        })()`);
        title = await mainWindow.webContents.executeJavaScript("document.title");
        console.log(`Rapid navigation smoke passed: ${JSON.stringify(navigation)}`);
      }

      if (process.env.CLIPVIEW_SMOKE_THUMBNAIL_WINDOW === "1") {
        const thumbnails = await mainWindow.webContents.executeJavaScript(`(async () => {
          toggleThumbnails(true);
          const strip = document.getElementById("thumbnailStrip");
          const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
          const active = strip.querySelector(".thumb-item.active img");
          for (let attempt = 0; attempt < 100 && !active?.naturalWidth; attempt += 1) await delay(100);
          if (!active?.naturalWidth) throw new Error("Active thumbnail did not load");
          const initialCount = strip.querySelectorAll("img[src]").length;
          if (state.items.length > 30 && initialCount >= state.items.length) {
            throw new Error("Offscreen thumbnails loaded eagerly");
          }
          const last = strip.lastElementChild;
          last.scrollIntoView({ block: "nearest", inline: "end" });
          for (let attempt = 0; attempt < 100 && !last.querySelector("img").naturalWidth; attempt += 1) {
            await delay(100);
          }
          if (!last.querySelector("img").naturalWidth) throw new Error("Scrolled thumbnail did not load");
          toggleThumbnails(false);
          if (thumbnailObserver || thumbnailController) throw new Error("Hidden thumbnails are still active");
          return { initialCount, items: state.items.length };
        })()`);
        console.log(`Thumbnail window smoke passed: ${JSON.stringify(thumbnails)}`);
      }

      if (process.env.CLIPVIEW_SMOKE_CORRUPT_IMAGE) {
        const corruptPath = process.env.CLIPVIEW_SMOKE_CORRUPT_IMAGE;
        const originalPath = await mainWindow.webContents.executeJavaScript("currentItem().path");
        await mainWindow.webContents.executeJavaScript("openPath(" + JSON.stringify(corruptPath) + ")");
        const failed = await mainWindow.webContents.executeJavaScript(`(() => ({
          errorVisible: !errorState.classList.contains("hidden"),
          emptyHidden: emptyState.classList.contains("hidden"),
          retryEnabled: !document.getElementById("retryFileBtn").disabled,
          busy: stage.getAttribute("aria-busy"),
        }))()`);
        if (!failed.errorVisible || !failed.emptyHidden || !failed.retryEnabled || failed.busy !== "false") {
          throw new Error("Unreadable file did not show a recoverable error: " + JSON.stringify(failed));
        }
        fs.copyFileSync(require("path").join(require("path").dirname(corruptPath), "valid.png"), corruptPath);
        await mainWindow.webContents.executeJavaScript(`(async () => {
          releaseImageEntry(imageKey(state.index));
          await loadCurrent();
          if (!errorState.classList.contains("hidden") || imageLayer.classList.contains("hidden")) {
            throw new Error("Retry did not recover the repaired image");
          }
          const controller = new AbortController();
          const pending = decodeImage(state.imageDataUrl, controller.signal);
          controller.abort();
          try { await pending; throw new Error("Cancelled image unexpectedly loaded"); }
          catch (error) { if (error.code !== "ERR_IMAGE_REQUEST_CANCELLED") throw error; }
        })()`);
        await mainWindow.webContents.executeJavaScript("openPath(" + JSON.stringify(originalPath) + ")");
        console.log("Invalid image recovery and decode cancellation passed");
      }

      if (process.env.CLIPVIEW_SMOKE_UI_QA_DIR) {
        const directory = process.env.CLIPVIEW_SMOKE_UI_QA_DIR;
        fs.mkdirSync(directory, { recursive: true });
        const originalBounds = mainWindow.getBounds();
        const capture = async (name) => {
          await wait(120);
          fs.writeFileSync(require("path").join(directory, name), (await mainWindow.webContents.capturePage()).toPNG());
        };
        for (const [width, height] of [[1200, 820], [720, 900], [480, 640], [320, 400]]) {
          mainWindow.setContentSize(width, height);
          await wait(120);
          const layout = await mainWindow.webContents.executeJavaScript(`(() => {
            const outside = [];
            for (const selector of [".topbar", ".bottombar"]) {
              const parent = document.querySelector(selector).getBoundingClientRect();
              for (const child of document.querySelector(selector).querySelectorAll("button, summary, select")) {
                if (child.closest(".tools-menu")) continue;
                const rect = child.getBoundingClientRect();
                if (!rect.width || !rect.height) continue;
                if (rect.height > 34) outside.push("wrapped:" + (child.id || child.tagName));
                if (rect.left < parent.left - 1 || rect.right > parent.right + 1 ||
                    rect.top < parent.top - 1 || rect.bottom > parent.bottom + 1) outside.push(child.id || child.tagName);
              }
            }
            return { width: window.innerWidth, height: window.innerHeight, outside };
          })()`);
          if (layout.outside.length) throw new Error("Toolbar overflow: " + JSON.stringify(layout));
          if (process.env.CLIPVIEW_SMOKE_EXPECT_VIDEO) {
            const videoLayout = await mainWindow.webContents.executeJavaScript(`(() => {
              const originalMode = state.videoPlaybackMode;
              showVideoControls("full", 0, { forceFull: true });
              const parent = videoControls.getBoundingClientRect();
              const outside = [];
              for (const mode of Object.keys(VIDEO_PLAYBACK_MODES)) {
                setVideoPlaybackMode(mode);
                for (const control of videoControls.querySelectorAll("button, select, input")) {
                  const rect = control.getBoundingClientRect();
                  if (rect.width > 0 && (rect.left < parent.left - 1 || rect.right > parent.right + 1 ||
                      rect.top < parent.top - 1 || rect.bottom > parent.bottom + 1)) outside.push(mode + ":" + control.id);
                }
              }
              setVideoPlaybackMode(originalMode);
              showVideoControls("full", 0, { forceFull: true });
              return outside;
            })()`);
            if (videoLayout.length) throw new Error("Video toolbar overflow: " + videoLayout.join(", "));
          }
          await capture(`viewer-${width}.png`);
          await mainWindow.webContents.executeJavaScript("openSettings()");
          for (const page of ["view", "files", "update"]) {
            await mainWindow.webContents.executeJavaScript("selectSettingsPage(" + JSON.stringify(page) + ")");
            const dialog = await mainWindow.webContents.executeJavaScript(`(() => {
              const dialog = document.getElementById("settingsDialog").getBoundingClientRect();
              const footer = document.querySelector(".dialog-footer").getBoundingClientRect();
              return dialog.left >= 0 && dialog.right <= window.innerWidth && dialog.top >= 0 &&
                dialog.bottom <= window.innerHeight && footer.bottom <= dialog.bottom;
            })()`);
            if (!dialog) throw new Error("Settings dialog exceeded the window");
            await capture(`settings-${page}-${width}.png`);
          }
          await mainWindow.webContents.executeJavaScript("document.getElementById('settingsDialog').close()");
        }
        mainWindow.setContentSize(1200, 820);
        await mainWindow.webContents.executeJavaScript(`(async () => {
          const layered = state.items.findIndex((item) => item.name.endsWith(".psd") || item.name.endsWith(".clip"));
          if (layered >= 0) await selectItem(layered, { immediate: true });
          if (state.layerDocument) toggleLayers(true);
        })()`);
        await wait(700);
        await capture("layers.png");
        await mainWindow.webContents.executeJavaScript("toggleLayers(false); toggleInfo(true)");
        await capture("information.png");
        await mainWindow.webContents.executeJavaScript("toggleInfo(false)");
        mainWindow.setBounds(originalBounds);
        await wait(150);
        title = await mainWindow.webContents.executeJavaScript("document.title");
        console.log("UI layout passed at 320, 480, 720 and 1200 pixels");
      }

      if (process.env.CLIPVIEW_SMOKE_SHELL_THUMBNAIL) {
        const shellThumbnail = await nativeImage.createThumbnailFromPath(
          process.env.CLIPVIEW_SMOKE_SHELL_THUMBNAIL,
          { width: 256, height: 256 },
        );
        if (shellThumbnail.isEmpty()) {
          throw new Error("Windows shell did not create a WebM thumbnail");
        }
      }

      if (process.env.CLIPVIEW_SMOKE_VIDEO_THUMBNAIL === "1") {
        await mainWindow.webContents.executeJavaScript(
          "document.getElementById('thumbBtn').click()",
        );
        let thumbnail = null;
        for (let attempt = 0; attempt < 100; attempt += 1) {
          thumbnail = await mainWindow.webContents.executeJavaScript(`(() => {
            const image = document.querySelector(".thumb-item.active img");
            return image ? {
              complete: image.complete,
              width: image.naturalWidth,
              source: image.src.slice(0, 32),
            } : null;
          })()`);
          if (
            thumbnail?.complete &&
            thumbnail.width > 0 &&
            thumbnail.source.startsWith("data:image/jpeg")
          ) break;
          await wait(100);
        }
        if (!thumbnail?.source.startsWith("data:image/jpeg")) {
          throw new Error(`Video thumbnail was not extracted: ${JSON.stringify(thumbnail)}`);
        }
      }

      if (smokeListTest) {
        const selection = await mainWindow.webContents.executeJavaScript(`(() => {
          document.getElementById("thumbBtn").click();
          const items = [...document.querySelectorAll(".thumb-item")];
          const current = items.findIndex((item) => item.classList.contains("active"));
          const target = current === 0 ? 1 : 0;
          if (items[target]) items[target].click();
          return { count: items.length, current, target };
        })()`);
        if (selection.count < 2) throw new Error("Thumbnail smoke test needs at least two items");

        const previousTitle = title;
        for (let attempt = 0; attempt < 40; attempt += 1) {
          await wait(100);
          title = await mainWindow.webContents.executeJavaScript("document.title");
          const visible = await mainWindow.webContents.executeJavaScript(
            "!document.getElementById('imageLayer').classList.contains('hidden')",
          );
          if (title !== previousTitle && visible) break;
        }
        if (title === previousTitle) {
          throw new Error(`Thumbnail selection failed: ${JSON.stringify(selection)}`);
        }
      }

      if (smokeNavigationTest) {
        await mainWindow.webContents.executeJavaScript(`(() => {
          const select = document.getElementById("cropModeSelect");
          select.value = "trim";
          select.dispatchEvent(new Event("change", { bubbles: true }));
        })()`);
        await wait(800);
        const before = await mainWindow.webContents.executeJavaScript(`(() => {
          const select = document.getElementById("cropModeSelect");
          select.focus();
          return { mode: select.value, title: document.title };
        })()`);
        mainWindow.webContents.sendInputEvent({ type: "keyDown", keyCode: "LEFT" });
        mainWindow.webContents.sendInputEvent({ type: "keyUp", keyCode: "LEFT" });
        await wait(80);
        const immediate = await mainWindow.webContents.executeJavaScript(`(() => ({
          loadingHidden: document.getElementById("loading").classList.contains("hidden"),
          mode: document.getElementById("cropModeSelect").value,
          title: document.title,
        }))()`);
        if (immediate.mode !== before.mode) {
          throw new Error(`Arrow key changed crop mode: ${JSON.stringify({ before, immediate })}`);
        }
        if (immediate.title === before.title) throw new Error("Arrow key did not navigate");
        title = immediate.title;
      }

      if (smokeSettingsTest) {
        await mainWindow.webContents.executeJavaScript(
          "document.getElementById('settingsBtn').click()",
        );
        let settings;
        for (let attempt = 0; attempt < 40; attempt += 1) {
          await wait(50);
          settings = await mainWindow.webContents.executeJavaScript(`(() => ({
            open: document.getElementById("settingsDialog").open,
            count: document.querySelectorAll(".association-extension").length,
            controlsHidden: document.getElementById("associationControls")
              .classList.contains("hidden"),
          }))()`);
          if (settings.count) break;
        }
        if (!settings?.open || settings.count !== optionalAssociationCount) {
          throw new Error(`Association settings failed: ${JSON.stringify(settings)}`);
        }
        if (settings.controlsHidden !== !fileAssociationSupported()) {
          throw new Error(`Portable association UI failed: ${JSON.stringify(settings)}`);
        }
        await mainWindow.webContents.executeJavaScript(
          "document.getElementById('settingsDialog').close()",
        );
      }

      if (process.env.CLIPVIEW_SMOKE_OPEN_LAYERS === "1") {
        const layers = await mainWindow.webContents.executeJavaScript(`(() => {
          const button = document.getElementById("layerBtn");
          if (!button.classList.contains("hidden")) button.click();
          document.querySelector(".layer-name")?.click();
          return {
            panelVisible: !document.getElementById("layerPanel").classList.contains("hidden"),
            count: document.querySelectorAll(".layer-row").length,
            detailsVisible: !document.getElementById("layerDetails").classList.contains("hidden"),
            thumbnailCount: document.querySelectorAll(".layer-thumbnail").length,
            folderCount: document.querySelectorAll(".layer-folder-toggle").length,
            folderThumbnailCount: document.querySelectorAll(
              ".layer-group-row .layer-thumbnail",
            ).length,
          };
        })()`);
        if (
          !layers.panelVisible ||
          !layers.count ||
          !layers.detailsVisible ||
          layers.folderThumbnailCount
        ) {
          throw new Error(`Layer panel failed: ${JSON.stringify(layers)}`);
        }
        if (layers.folderCount) {
          const folderToggle = await mainWindow.webContents.executeJavaScript(`(() => {
            const button = document.querySelector(".layer-folder-toggle");
            const before = document.querySelectorAll(".layer-row").length;
            button.click();
            const collapsed = document.querySelector(".layer-folder-toggle")
              ?.getAttribute("aria-expanded") === "false";
            const after = document.querySelectorAll(".layer-row").length;
            document.querySelector(".layer-folder-toggle")?.click();
            const restored = document.querySelectorAll(".layer-row").length;
            return { before, after, restored, collapsed };
          })()`);
          if (
            !folderToggle.collapsed ||
            folderToggle.after >= folderToggle.before ||
            folderToggle.restored !== folderToggle.before
          ) {
            throw new Error(`Layer folder toggle failed: ${JSON.stringify(folderToggle)}`);
          }
        }
        const layerMetadata = await mainWindow.webContents.executeJavaScript(`(() => {
          if (state.layerDocument?.format !== "CLIP") return null;
          const rows = [...document.querySelectorAll(".layer-row")];
          const all = [...state.layerIndex.values()];
          if (!rows.every((row) => row.querySelector(".layer-kind")?.textContent === layerById(row.dataset.layerId)?.typeLabel)) {
            throw new Error("Layer kinds are missing or inconsistent in the list");
          }
          const results = [];
          for (const type of ["raster", "vector", "text"]) {
            const layer = all.find((entry) => entry.type === type);
            if (!layer) continue;
            const before = performance.now();
            selectLayer(layer.id);
            const details = document.getElementById("layerDetailList").textContent;
            if (!details.includes(layer.typeLabel)) throw new Error("Selected layer kind is missing");
            if (type === "text") {
              if (document.querySelectorAll(".layer-text-object").length !== layer.textObjects.length) throw new Error("Text objects are missing");
              if (!document.getElementById("layerTextObjects").textContent.includes("쓰기 방향")) throw new Error("Text properties are missing");
            }
            const panel = document.getElementById("layerPanel").getBoundingClientRect();
            const detail = document.getElementById("layerDetails").getBoundingClientRect();
            if (detail.bottom > panel.bottom + 1 || detail.height < 1) throw new Error("Layer details are not visible in the panel");
            results.push({ type, selectionMs: Math.round(performance.now() - before) });
          }
          if (!rows.every((row) => row.isConnected)) throw new Error("Layer selection rebuilt the tree");
          return results;
        })()`);
        if (layerMetadata) console.log(`Layer metadata smoke: ${JSON.stringify(layerMetadata)}`);
        if (layers.thumbnailCount) {
          let thumbnailsReady = false;
          for (let attempt = 0; attempt < 100; attempt += 1) {
            await wait(100);
            thumbnailsReady = await mainWindow.webContents.executeJavaScript(`(() => (
              [...document.querySelectorAll(".layer-thumbnail")]
                .some((thumbnail) => thumbnail.naturalWidth > 0)
            ))()`);
            if (thumbnailsReady) break;
          }
          if (!thumbnailsReady) throw new Error("Layer thumbnails did not finish loading");
        }
        if (process.env.CLIPVIEW_SMOKE_TOGGLE_LAYER === "1") {
          const before = await mainWindow.webContents.executeJavaScript(
            "document.getElementById('viewerImage').src",
          );
          const targetLayerType = process.env.CLIPVIEW_SMOKE_LAYER_TYPE;
          const selector = targetLayerType
            ? `.layer-row[data-layer-type="${targetLayerType}"] .layer-visibility:not(:disabled)`
            : process.env.CLIPVIEW_SMOKE_TOGGLE_FOLDER === "1"
              ? ".layer-group-row .layer-visibility:not(:disabled)"
              : ".layer-visibility:not(:disabled)";
          const toggleStartedAt = Date.now();
          const toggled = await mainWindow.webContents.executeJavaScript(`(() => {
            const button = document.querySelector(${JSON.stringify(selector)});
            if (!button) return false;
            button.click();
            return true;
          })()`);
          if (!toggled) throw new Error("No toggleable layer was found");
          let changed = false;
          for (let attempt = 0; attempt < 200; attempt += 1) {
            await wait(100);
            changed = await mainWindow.webContents.executeJavaScript(`(() => (
              document.getElementById("loading").classList.contains("hidden") &&
              document.getElementById("viewerImage").src !== ${JSON.stringify(before)}
            ))()`);
            if (changed) break;
          }
          if (!changed) throw new Error("Layer visibility did not update the image");
          console.log(`Layer toggle smoke duration: ${Date.now() - toggleStartedAt}ms`);
        }
        if (process.env.CLIPVIEW_SMOKE_LAYER_DETAILS === "1") {
          const performance = await mainWindow.webContents.executeJavaScript(`(async () => {
            const all = [...state.layerIndex.values()];
            const rows = [...document.querySelectorAll(".layer-row")];
            const objects = [...document.querySelectorAll(".layer-text-object")];
            const results = [];
            for (const type of ["raster", "group"]) {
              const layer = all.find((entry) => entry.type === type && entry.toggleAvailable && layerIsVisible(entry.id));
              if (!layer) continue;
              const button = state.layerRows.get(layer.id)?.querySelector(".layer-visibility");
              if (!button) continue;
              button.focus();
              const childVisibility = Object.fromEntries(flattenLayers(layer.children).map((child) => [child.id, state.layerVisibility[child.id]]));
              const durations = [];
              const uiDurations = [];
              for (let index = 0; index < 4; index += 1) {
                const expected = !state.layerVisibility[layer.id];
                const start = performance.now();
                const pending = toggleLayerVisibility(layer.id);
                uiDurations.push(performance.now() - start);
                await pending;
                durations.push(Math.round(performance.now() - start));
                if (state.renderedLayerVisibility[layer.id] !== expected) throw new Error("Layer render failed");
                if (document.activeElement !== button || !button.isConnected) throw new Error("Layer toggle lost its button/focus");
                if (!Object.entries(childVisibility).every(([id, visible]) => state.layerVisibility[id] === visible)) throw new Error("Folder toggle changed child switches");
                if (type === "group" && !expected) {
                  for (const child of flattenLayers(layer.children)) {
                    if (state.layerRows.get(child.id)?.dataset.effectiveVisible !== "false") throw new Error("Inherited visibility is not shown");
                  }
                }
              }
              results.push({ type, renderMs: durations, maxUiMs: Math.round(Math.max(...uiDurations)) });
            }
            if (!rows.every((row) => row.isConnected) || !objects.every((object) => object.isConnected)) throw new Error("Visibility changes rebuilt layer information");
            return results;
          })()`);
          console.log(`Layer visibility performance: ${JSON.stringify(performance)}`);
        }
        if (process.env.CLIPVIEW_SMOKE_LAYER_QA_DIR) {
          const directory = process.env.CLIPVIEW_SMOKE_LAYER_QA_DIR;
          fs.mkdirSync(directory, { recursive: true });
          const originalBounds = mainWindow.getBounds();
          for (const [width, height] of [[1200, 820], [720, 900], [480, 640], [320, 400]]) {
            mainWindow.setContentSize(width, height);
            await wait(150);
            const layerLayout = await mainWindow.webContents.executeJavaScript(`(() => {
              const text = [...state.layerIndex.values()].find((entry) => entry.kind === "text" && entry.textObjects.length > 1);
              if (text) selectLayer(text.id);
              const panel = document.getElementById("layerPanel").getBoundingClientRect();
              const tree = document.getElementById("layerTree");
              const detail = document.getElementById("layerDetails").getBoundingClientRect();
              return { outside: panel.left < 0 || panel.right > window.innerWidth || detail.bottom > panel.bottom + 1,
                treeOverflow: tree.scrollWidth > tree.clientWidth + 1, treeHeight: tree.clientHeight, detailsHeight: detail.height };
            })()`);
            if (layerLayout.outside || layerLayout.treeOverflow || layerLayout.treeHeight < 40 || layerLayout.detailsHeight < 30) {
              throw new Error("Layer panel layout failed: " + JSON.stringify(layerLayout));
            }
            await wait(100);
            fs.writeFileSync(require("path").join(directory, `layer-details-${width}.png`), (await mainWindow.webContents.capturePage()).toPNG());
          }
          mainWindow.setBounds(originalBounds);
          await wait(150);
        }
      }

      await mainWindow.webContents.executeJavaScript(
        "new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))",
      );
      const geometry = await mainWindow.webContents.executeJavaScript(`(() => {
        const stage = document.getElementById("stage").getBoundingClientRect();
        const image = document.getElementById("imageLayer").classList.contains("video-active")
          ? document.getElementById("viewerVideo") : document.getElementById("viewerImage");
        const topbar = document.querySelector(".topbar").getBoundingClientRect();
        const bottombar = document.querySelector(".bottombar").getBoundingClientRect();
        if (!(image.naturalWidth || image.videoWidth) || image.classList.contains("hidden")) return null;
        const imageRect = image.getBoundingClientRect();
        return {
          deltaX: imageRect.left + imageRect.width / 2 - (stage.left + stage.width / 2),
          deltaY: imageRect.top + imageRect.height / 2 - (stage.top + stage.height / 2),
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

      const navigationGeometry = await mainWindow.webContents.executeJavaScript(`(() => (
        ["prevOverlay", "nextOverlay", "prevBtn", "nextBtn"].flatMap((id) => {
          const button = document.getElementById(id);
          if (button.classList.contains("hidden")) return [];
          const icon = button.querySelector(".nav-icon");
          const buttonRect = button.getBoundingClientRect();
          const iconRect = icon.getBoundingClientRect();
          const pathBox = icon.querySelector("path").getBBox();
          return [{
            id,
            deltaX: iconRect.left + iconRect.width / 2 -
              (buttonRect.left + buttonRect.width / 2),
            deltaY: iconRect.top + iconRect.height / 2 -
              (buttonRect.top + buttonRect.height / 2),
            pathDeltaX: pathBox.x + pathBox.width / 2 - 10,
            pathDeltaY: pathBox.y + pathBox.height / 2 - 10,
          }];
        })
      ))()`);
      const misaligned = navigationGeometry.find((item) => (
        Math.abs(item.deltaX) > 0.1 ||
        Math.abs(item.deltaY) > 0.1 ||
        Math.abs(item.pathDeltaX) > 0.1 ||
        Math.abs(item.pathDeltaY) > 0.1
      ));
      if (misaligned) throw new Error(`Navigation icon is not centered: ${JSON.stringify(misaligned)}`);

      const fullscreenBefore = mainWindow.isFullScreen();
      await mainWindow.webContents.executeJavaScript(`(() => {
        document.getElementById("prevOverlay").dispatchEvent(new MouseEvent("dblclick", {
          bubbles: true,
          cancelable: true,
        }));
      })()`);
      await wait(100);
      if (mainWindow.isFullScreen() !== fullscreenBefore) {
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
      console.log(`${productName} smoke test passed: ${title} (${bounds.width}x${bounds.height})`);
      app.exit(0);
    } catch (error) {
      console.error(`${productName} smoke test failed: ${error.stack || error.message}`);
      app.exit(1);
    }
  });
}

module.exports = { attachElectronSmoke };
