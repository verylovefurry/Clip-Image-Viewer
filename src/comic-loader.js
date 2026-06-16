"use strict";

const fs = require("fs");
const path = require("path");

let sqlPromise;

async function getSql() {
  if (!sqlPromise) {
    const initSqlJs = require("sql.js");
    sqlPromise = initSqlJs({
      locateFile: (file) => require.resolve(`sql.js/dist/${file}`),
    });
  }
  return sqlPromise;
}

function firstRow(db, query) {
  const statement = db.prepare(query);
  try {
    return statement.step() ? statement.getAsObject() : null;
  } finally {
    statement.free();
  }
}

function allRows(db, query) {
  const statement = db.prepare(query);
  const rows = [];
  try {
    while (statement.step()) rows.push(statement.getAsObject());
    return rows;
  } finally {
    statement.free();
  }
}

function numberOrZero(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : 0;
}

function projectCrop(project) {
  const canvasWidth = numberOrZero(project.DefaultPageWidth);
  const canvasHeight = numberOrZero(project.DefaultPageHeight);
  const trimWidth = numberOrZero(project.DefaultPageCropWidth);
  const trimHeight = numberOrZero(project.DefaultPageCropHeight);
  if (
    !numberOrZero(project.DefaultPageUseCropFrame) ||
    !canvasWidth ||
    !canvasHeight ||
    !trimWidth ||
    !trimHeight
  ) {
    return null;
  }
  return {
    canvasWidth,
    canvasHeight,
    trimWidth,
    trimHeight,
    bleed: Math.max(0, numberOrZero(project.DefaultPageCropDitch)),
    offsetX: numberOrZero(project.DefaultPageCropOffsetX),
    offsetY: numberOrZero(project.DefaultPageCropOffsetY),
  };
}

function resolveLinkedPage(cmcPath, linkPath) {
  let relative = String(linkPath || "").trim();
  if (!relative) return null;
  relative = relative.replace(/^\.[\\/]/, "");
  if (relative.startsWith(".:")) relative = relative.slice(2);
  relative = relative.replace(/:/g, path.sep).replace(/[\\/]/g, path.sep);
  const resolved = path.resolve(path.dirname(cmcPath), relative);
  return fs.existsSync(resolved) ? resolved : null;
}

function orderedPageNodes(project, nodes) {
  const byMainId = new Map(nodes.map((node) => [numberOrZero(node.MainId), node]));
  const byRowId = new Map(nodes.map((node) => [numberOrZero(node._PW_ID), node]));
  const rootId = numberOrZero(project.ProjectRootCanvasNode);
  const root = byMainId.get(rootId) || byRowId.get(rootId);
  const ordered = [];
  const visited = new Set();
  let nextId = numberOrZero(root?.FirstChildIndex);

  while (nextId && !visited.has(nextId)) {
    visited.add(nextId);
    const node = byMainId.get(nextId) || byRowId.get(nextId);
    if (!node) break;
    if (numberOrZero(node.Type) === 2 && node.LinkPath) ordered.push(node);
    nextId = numberOrZero(node.NextIndex);
  }

  if (ordered.length) return ordered;
  return nodes
    .filter((node) => numberOrZero(node.Type) === 2 && node.LinkPath)
    .sort((a, b) => (
      numberOrZero(a.CanvasIndex) - numberOrZero(b.CanvasIndex) ||
      numberOrZero(a.MainId) - numberOrZero(b.MainId)
    ));
}

async function loadComicProject(cmcPath) {
  const SQL = await getSql();
  const db = new SQL.Database(fs.readFileSync(cmcPath));
  try {
    const project = firstRow(db, "SELECT * FROM Project LIMIT 1");
    if (!project) throw new Error("CMC 프로젝트 정보를 찾지 못했습니다.");
    const nodes = allRows(db, "SELECT * FROM CanvasNode");
    const crop = projectCrop(project);
    const items = orderedPageNodes(project, nodes)
      .map((node) => resolveLinkedPage(cmcPath, node.LinkPath))
      .filter(Boolean)
      .map((pagePath) => ({
        kind: "file",
        path: pagePath,
        name: path.basename(pagePath),
        crop,
      }));

    if (!items.length) {
      throw new Error("CMC 프로젝트에서 열 수 있는 CLIP 페이지를 찾지 못했습니다.");
    }

    return {
      items,
      comic: {
        path: cmcPath,
        name: path.basename(cmcPath),
        cropAvailable: Boolean(crop),
      },
    };
  } finally {
    db.close();
  }
}

module.exports = {
  loadComicProject,
  orderedPageNodes,
  projectCrop,
  resolveLinkedPage,
};
