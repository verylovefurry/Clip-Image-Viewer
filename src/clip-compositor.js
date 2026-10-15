"use strict";

// W3C source-over/nonseparable blending, with CLIP's numeric mode mapping and
// premultiplied-source glow modes. No unsupported mode is silently made normal.
const SUPPORTED_MODES = new Set([...Array(27).keys(), 30, 36]);
const clamp = (value) => Math.max(0, Math.min(1, value));

function byte(value) {
  const bounded = Math.max(0, Math.min(255, value));
  const integer = Math.floor(bounded);
  const fraction = bounded - integer;
  return fraction === 0.5 ? integer + (integer & 1) : Math.round(bounded);
}

function luminance(color) { return color[0] * 0.3 + color[1] * 0.59 + color[2] * 0.11; }
function saturation(color) { return Math.max(...color) - Math.min(...color); }

function setLuminance(color, target) {
  const difference = target - luminance(color);
  let result = color.map((channel) => channel + difference);
  const low = Math.min(...result);
  const high = Math.max(...result);
  if (low < 0) result = result.map((channel) => target + (channel - target) * target / (target - low));
  if (high > 1) result = result.map((channel) => target + (channel - target) * (1 - target) / (high - target));
  return result;
}

function setSaturation(color, target) {
  const low = Math.min(...color);
  const range = Math.max(...color) - low;
  return range ? color.map((channel) => (channel - low) * target / range) : [0, 0, 0];
}

function burn(backdrop, source) { return source <= 0 ? 0 : 1 - Math.min(1, (1 - backdrop) / source); }
function dodge(backdrop, source) { return source >= 1 ? 1 : Math.min(1, backdrop / (1 - source)); }
function hardLight(backdrop, source) { return source <= 0.5 ? 2 * backdrop * source : 1 - 2 * (1 - backdrop) * (1 - source); }

function blendChannel(mode, backdrop, source) {
  switch (mode) {
    case 0: case 30: return source;
    case 1: return Math.min(backdrop, source);
    case 2: return backdrop * source;
    case 3: return burn(backdrop, source);
    case 4: return clamp(backdrop + source - 1);
    case 5: return Math.max(0, backdrop - source);
    case 7: return Math.max(backdrop, source);
    case 8: return backdrop + source - backdrop * source;
    case 9: case 10: return dodge(backdrop, source);
    case 11: case 12: return Math.min(1, backdrop + source);
    case 14: return hardLight(source, backdrop);
    case 15: {
      const curve = backdrop <= 0.25 ? ((16 * backdrop - 12) * backdrop + 4) * backdrop : Math.sqrt(backdrop);
      return source <= 0.5 ? backdrop - (1 - 2 * source) * backdrop * (1 - backdrop) : backdrop + (2 * source - 1) * (curve - backdrop);
    }
    case 16: return hardLight(backdrop, source);
    case 17: return source <= 0.5 ? burn(backdrop, source * 2) : dodge(backdrop, (source - 0.5) * 2);
    case 18: return clamp(backdrop + 2 * source - 1);
    case 19: return source <= 0.5 ? Math.min(backdrop, source * 2) : Math.max(backdrop, (source - 0.5) * 2);
    case 20: return backdrop + source >= 1 ? 1 : 0;
    case 21: return Math.abs(backdrop - source);
    case 22: return backdrop + source - 2 * backdrop * source;
    case 36: return Math.min(1, backdrop / Math.max(source, 1e-9));
    default: throw new Error(`Unsupported scalar CLIP blend mode: ${mode}`);
  }
}

function blendColor(mode, backdrop, source) {
  if (mode === 6) return luminance(source) < luminance(backdrop) ? source : backdrop;
  if (mode === 13) return luminance(source) > luminance(backdrop) ? source : backdrop;
  if (mode === 23) return setLuminance(setSaturation(source, saturation(backdrop)), luminance(backdrop));
  if (mode === 24) return setLuminance(setSaturation(backdrop, saturation(source)), luminance(backdrop));
  if (mode === 25) return setLuminance(source, luminance(backdrop));
  if (mode === 26) return setLuminance(backdrop, luminance(source));
  return backdrop.map((channel, index) => blendChannel(mode, channel, source[index]));
}

function modeCode(value) {
  const code = typeof value === "number" ? value : value === "normal" ? 0 : Number(String(value).replace(/^clip-/, ""));
  if (!SUPPORTED_MODES.has(code)) throw new Error(`지원하지 않는 CLIP 합성 모드입니다: ${value}`);
  return code;
}

function compositeRgba(destination, source, mode = 0, opacity = 1, clipped = false) {
  if (destination.length !== source.length || destination.length % 4) throw new Error("CLIP 합성 픽셀 크기가 일치하지 않습니다.");
  const code = modeCode(mode);
  const strength = clamp(opacity);
  if (!strength) return destination;
  const glow = code === 10 || code === 12;
  const nonseparable = [6, 13, 23, 24, 25, 26].includes(code);
  for (let offset = 0; offset < source.length; offset += 4) {
    const sourceAlpha = source[offset + 3] / 255 * strength;
    if (!sourceAlpha) continue;
    const backdropAlpha = destination[offset + 3] / 255;
    if (clipped && !backdropAlpha) continue;
    if (!clipped && (code === 0 || code === 30) && sourceAlpha === 1) {
      destination[offset] = source[offset]; destination[offset + 1] = source[offset + 1];
      destination[offset + 2] = source[offset + 2]; destination[offset + 3] = 255;
      continue;
    }
    const outputAlpha = clipped ? backdropAlpha : sourceAlpha + backdropAlpha * (1 - sourceAlpha);
    let mixed;
    if (nonseparable) mixed = blendColor(code,
      [destination[offset], destination[offset + 1], destination[offset + 2]].map((value) => value / 255),
      [source[offset], source[offset + 1], source[offset + 2]].map((value) => value / 255));
    for (let channel = 0; channel < 3; channel += 1) {
      const backdrop = destination[offset + channel] / 255;
      const foreground = source[offset + channel] / 255;
      const blend = nonseparable ? mixed[channel] : blendChannel(code, backdrop, glow ? foreground * sourceAlpha : foreground);
      const result = clipped
        ? glow ? blend : backdrop * (1 - sourceAlpha) + blend * sourceAlpha
        : glow
          ? ((1 - backdropAlpha) * sourceAlpha * foreground + backdropAlpha * blend) / outputAlpha
          : ((1 - backdropAlpha) * sourceAlpha * foreground + backdropAlpha * sourceAlpha * blend +
              (1 - sourceAlpha) * backdropAlpha * backdrop) / outputAlpha;
      destination[offset + channel] = byte(result * 255);
    }
    destination[offset + 3] = byte(outputAlpha * 255);
  }
  return destination;
}

module.exports = { blendColor, compositeRgba, modeCode };
