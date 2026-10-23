"use strict";

const MAX_PATTERN_PIXEL_VISITS = 100 * 1024 * 1024;

function createPatternStroke(points, width, height, scale, offsetX, offsetY, spraySize = 0) {
  let left = width; let top = height; let right = 0; let bottom = 0;
  const padding = points.reduce((size, point) => Math.max(size, point.width), spraySize) * scale;
  for (const point of points) for (const position of [point, ...(point.handles || [])]) {
    const x = position.x * scale + offsetX; const y = position.y * scale + offsetY;
    left = Math.min(left, x - padding); top = Math.min(top, y - padding);
    right = Math.max(right, x + padding); bottom = Math.max(bottom, y + padding);
  }
  left = Math.max(0, Math.floor(left)); top = Math.max(0, Math.floor(top));
  right = Math.min(width, Math.ceil(right)); bottom = Math.min(height, Math.ceil(bottom));
  const w = Math.max(0, right - left); const h = Math.max(0, bottom - top);
  return { left, top, width: w, height: h, data: Buffer.alloc(w * h * 4) };
}

function stampPattern(surface, pattern, x, y, width, height, rotation, budget) {
  if (!surface.width || !surface.height || width <= 0 || height <= 0) return;
  x -= surface.left; y -= surface.top;
  const angle = rotation * Math.PI / 180;
  const cos = Math.cos(angle); const sin = Math.sin(angle);
  const halfX = (Math.abs(cos) * width + Math.abs(sin) * height) / 2;
  const halfY = (Math.abs(sin) * width + Math.abs(cos) * height) / 2;
  const left = Math.max(0, Math.floor(x - halfX));
  const top = Math.max(0, Math.floor(y - halfY));
  const right = Math.min(surface.width, Math.ceil(x + halfX));
  const bottom = Math.min(surface.height, Math.ceil(y + halfY));
  budget.visits += Math.max(0, right - left) * Math.max(0, bottom - top);
  if (budget.visits > MAX_PATTERN_PIXEL_VISITS) throw new Error("CLIP 브러시 합성의 안전 처리 한도를 초과했습니다.");
  const source = pattern.pixels;
  for (let py = top; py < bottom; py += 1) for (let px = left; px < right; px += 1) {
    const dx = px + 0.5 - x; const dy = py + 0.5 - y;
    const sx = ((dx * cos + dy * sin) / width + 0.5) * pattern.width - 0.5;
    const sy = ((dy * cos - dx * sin) / height + 0.5) * pattern.height - 0.5;
    const ix = Math.floor(sx); const iy = Math.floor(sy);
    const fx = sx - ix; const fy = sy - iy;
    let red = 0; let green = 0; let blue = 0; let alpha = 0;
    for (let row = 0; row <= 1; row += 1) for (let column = 0; column <= 1; column += 1) {
      const tx = ix + column; const ty = iy + row;
      if (tx < 0 || ty < 0 || tx >= pattern.width || ty >= pattern.height) continue;
      const index = (ty * pattern.width + tx) * 4;
      const weight = (column ? fx : 1 - fx) * (row ? fy : 1 - fy);
      red += source[index] * weight; green += source[index + 1] * weight;
      blue += source[index + 2] * weight; alpha += source[index + 3] * weight;
    }
    if (!alpha) continue;
    const index = (py * surface.width + px) * 4;
    const a = alpha / 255;
    const retained = surface.data[index + 3] / 255 * (1 - a);
    const outputAlpha = a + retained;
    surface.data[index] = Math.round((red + surface.data[index] * retained) / outputAlpha);
    surface.data[index + 1] = Math.round((green + surface.data[index + 1] * retained) / outputAlpha);
    surface.data[index + 2] = Math.round((blue + surface.data[index + 2] * retained) / outputAlpha);
    surface.data[index + 3] = Math.round(outputAlpha * 255);
  }
}

function mergePatternStroke(target, surface, width, opacity) {
  for (let y = 0; y < surface.height; y += 1) for (let x = 0; x < surface.width; x += 1) {
    const sourceIndex = (y * surface.width + x) * 4;
    const alpha = surface.data[sourceIndex + 3] / 255 * opacity;
    if (!alpha) continue;
    const index = ((y + surface.top) * width + x + surface.left) * 4;
    const retained = target[index + 3] / 255 * (1 - alpha);
    const outputAlpha = alpha + retained;
    for (let channel = 0; channel < 3; channel += 1) {
      target[index + channel] = Math.round((surface.data[sourceIndex + channel] * alpha + target[index + channel] * retained) / outputAlpha);
    }
    target[index + 3] = Math.round(outputAlpha * 255);
  }
}

module.exports = { createPatternStroke, stampPattern, mergePatternStroke };
