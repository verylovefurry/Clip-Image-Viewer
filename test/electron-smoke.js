"use strict";

const fs = require("fs");

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
      if (expectedPath) {
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
          if (title !== productName && rendered) break;
          await wait(100);
        }
        if (title === productName) throw new Error("Image did not finish loading");
      }

      await mainWindow.webContents.executeJavaScript(
        "new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))",
      );
      await wait(250);

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
        if (layers.thumbnailCount) {
          let thumbnailsReady = false;
          for (let attempt = 0; attempt < 100; attempt += 1) {
            await wait(100);
            thumbnailsReady = await mainWindow.webContents.executeJavaScript(`(() => (
              document.querySelector(".layer-thumbnail")?.naturalWidth > 0
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
