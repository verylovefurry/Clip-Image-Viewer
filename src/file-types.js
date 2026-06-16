"use strict";

const fs = require("fs");
const path = require("path");

const DIRECT_EXTENSIONS = new Set([
  ".jpg", ".jpeg", ".jfif", ".png", ".apng", ".gif", ".webp", ".bmp",
  ".svg", ".ico", ".avif",
]);
const CONVERT_EXTENSIONS = new Set([".tif", ".tiff", ".heic", ".heif", ".hif"]);
const MAGICK_EXTENSIONS = new Set([
  ".dds",
  ".j2k", ".j2c", ".jpc", ".jp2", ".jpf", ".jpx",
  ".tga",
  ".pcx",
  ".pgm", ".pnm", ".ppm", ".pbm", ".pam",
  ".jxl",
  ".exr",
  ".qoi",
]);
const JXR_EXTENSIONS = new Set([".jxr", ".hdp", ".wdp"]);
const BPG_EXTENSIONS = new Set([".bpg"]);
const RAW_EXTENSIONS = new Set([
  ".3fr", ".ari", ".arw", ".bay", ".cap", ".cr2", ".cr3", ".crw",
  ".dcr", ".dcs", ".dng", ".drf", ".eip", ".erf", ".fff", ".gpr",
  ".iiq", ".k25", ".kdc", ".mdc", ".mef", ".mos", ".mrw", ".nef",
  ".nrw", ".obm", ".orf", ".pef", ".ptx", ".pxn", ".r3d", ".raf",
  ".raw", ".rwl", ".rw2", ".rwz", ".sr2", ".srf", ".srw", ".sti",
  ".x3f",
]);
const PSD_EXTENSIONS = new Set([".psd", ".psb"]);
const CLIP_EXTENSIONS = new Set([".clip", ".csp"]);
const ARCHIVE_EXTENSIONS = new Set([".zip", ".cbz"]);
const PROJECT_EXTENSIONS = new Set([".cmc"]);
const BASIC_ASSOCIATION_EXTENSIONS = new Set([
  ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp",
]);
const IMAGE_EXTENSIONS = new Set([
  ...DIRECT_EXTENSIONS,
  ...CONVERT_EXTENSIONS,
  ...MAGICK_EXTENSIONS,
  ...JXR_EXTENSIONS,
  ...BPG_EXTENSIONS,
  ...RAW_EXTENSIONS,
  ...PSD_EXTENSIONS,
  ...CLIP_EXTENSIONS,
]);
const BROWSABLE_EXTENSIONS = new Set([...IMAGE_EXTENSIONS, ...ARCHIVE_EXTENSIONS]);
const SUPPORTED_EXTENSIONS = new Set([...BROWSABLE_EXTENSIONS, ...PROJECT_EXTENSIONS]);
const OPTIONAL_ASSOCIATION_EXTENSIONS = [...SUPPORTED_EXTENSIONS]
  .filter((ext) => !BASIC_ASSOCIATION_EXTENSIONS.has(ext))
  .sort();

function extensionOf(filePath) {
  return path.extname(filePath).toLowerCase();
}

function isSupported(filePath) {
  return SUPPORTED_EXTENSIONS.has(extensionOf(filePath));
}

function isImage(filePath) {
  return IMAGE_EXTENSIONS.has(extensionOf(filePath));
}

function naturalCompare(a, b) {
  return a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });
}

function listFolder(folderPath) {
  return fs.readdirSync(folderPath, { withFileTypes: true })
    .filter((entry) => entry.isFile() && BROWSABLE_EXTENSIONS.has(extensionOf(entry.name)))
    .map((entry) => ({
      kind: ARCHIVE_EXTENSIONS.has(extensionOf(entry.name)) ? "archive" : "file",
      path: path.join(folderPath, entry.name),
      name: entry.name,
    }))
    .sort((a, b) => naturalCompare(a.name, b.name));
}

function findComicProject(folderPath) {
  const projects = fs.readdirSync(folderPath, { withFileTypes: true })
    .filter((entry) => entry.isFile() && PROJECT_EXTENSIONS.has(extensionOf(entry.name)))
    .map((entry) => path.join(folderPath, entry.name))
    .sort((a, b) => naturalCompare(path.basename(a), path.basename(b)));
  return projects[0] || null;
}

module.exports = {
  ARCHIVE_EXTENSIONS,
  BASIC_ASSOCIATION_EXTENSIONS,
  BPG_EXTENSIONS,
  CLIP_EXTENSIONS,
  CONVERT_EXTENSIONS,
  DIRECT_EXTENSIONS,
  IMAGE_EXTENSIONS,
  JXR_EXTENSIONS,
  MAGICK_EXTENSIONS,
  OPTIONAL_ASSOCIATION_EXTENSIONS,
  PROJECT_EXTENSIONS,
  PSD_EXTENSIONS,
  RAW_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  extensionOf,
  findComicProject,
  isImage,
  isSupported,
  listFolder,
  naturalCompare,
};
