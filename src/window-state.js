"use strict";

const DEFAULT_WIDTH = 720;
const DEFAULT_HEIGHT = 960;
const MIN_WIDTH = 320;
const MIN_HEIGHT = 320;

function createDefaultBounds(workArea) {
  const scale = Math.min(
    1,
    workArea.width / DEFAULT_WIDTH,
    workArea.height / DEFAULT_HEIGHT,
  );
  const width = Math.max(1, Math.round(DEFAULT_WIDTH * scale));
  const height = Math.max(1, Math.round(DEFAULT_HEIGHT * scale));
  return {
    x: workArea.x + Math.round((workArea.width - width) / 2),
    y: workArea.y + Math.round((workArea.height - height) / 2),
    width,
    height,
  };
}

function constrainBounds(bounds, workArea) {
  if (
    !bounds ||
    !Number.isFinite(bounds.width) ||
    !Number.isFinite(bounds.height)
  ) {
    return createDefaultBounds(workArea);
  }

  const minimumWidth = Math.min(MIN_WIDTH, workArea.width);
  const minimumHeight = Math.min(MIN_HEIGHT, workArea.height);
  const width = Math.min(
    workArea.width,
    Math.max(minimumWidth, Math.round(bounds.width)),
  );
  const height = Math.min(
    workArea.height,
    Math.max(minimumHeight, Math.round(bounds.height)),
  );
  const fallbackX = workArea.x + Math.round((workArea.width - width) / 2);
  const fallbackY = workArea.y + Math.round((workArea.height - height) / 2);
  const requestedX = Number.isFinite(bounds.x) ? Math.round(bounds.x) : fallbackX;
  const requestedY = Number.isFinite(bounds.y) ? Math.round(bounds.y) : fallbackY;

  return {
    x: Math.min(
      Math.max(requestedX, workArea.x),
      workArea.x + workArea.width - width,
    ),
    y: Math.min(
      Math.max(requestedY, workArea.y),
      workArea.y + workArea.height - height,
    ),
    width,
    height,
  };
}

module.exports = {
  DEFAULT_HEIGHT,
  DEFAULT_WIDTH,
  constrainBounds,
  createDefaultBounds,
};
