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
const VIDEO_EXTENSIONS = new Set([
  ".mp4", ".m4v", ".mov", ".qt", ".webm", ".mkv",
  ".avi", ".wmv", ".asf", ".flv", ".f4v",
  ".ogv", ".ogg", ".ogm",
  ".3gp", ".3g2",
  ".mpg", ".mpeg", ".mpe", ".m1v", ".m2v",
  ".ts", ".mts", ".m2ts", ".vob",
  ".divx", ".xvid", ".mxf", ".dv", ".amv",
  ".rm", ".rmvb",
]);
const VIDEO_MIME_TYPES = {
  ".mp4": "video/mp4",
  ".m4v": "video/mp4",
  ".mov": "video/quicktime",
  ".qt": "video/quicktime",
  ".webm": "video/webm",
  ".mkv": "video/x-matroska",
  ".avi": "video/x-msvideo",
  ".wmv": "video/x-ms-wmv",
  ".asf": "video/x-ms-asf",
  ".flv": "video/x-flv",
  ".f4v": "video/mp4",
  ".ogv": "video/ogg",
  ".ogg": "video/ogg",
  ".ogm": "video/ogg",
  ".3gp": "video/3gpp",
  ".3g2": "video/3gpp2",
  ".mpg": "video/mpeg",
  ".mpeg": "video/mpeg",
  ".mpe": "video/mpeg",
  ".m1v": "video/mpeg",
  ".m2v": "video/mpeg",
  ".ts": "video/mp2t",
  ".mts": "video/mp2t",
  ".m2ts": "video/mp2t",
  ".vob": "video/mpeg",
  ".divx": "video/x-msvideo",
  ".xvid": "video/x-msvideo",
  ".mxf": "application/mxf",
  ".dv": "video/dv",
  ".amv": "video/x-amv",
  ".rm": "application/vnd.rn-realmedia",
  ".rmvb": "application/vnd.rn-realmedia-vbr",
};
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
const BROWSABLE_EXTENSIONS = new Set([
  ...IMAGE_EXTENSIONS,
  ...VIDEO_EXTENSIONS,
  ...ARCHIVE_EXTENSIONS,
]);
const SUPPORTED_EXTENSIONS = new Set([...BROWSABLE_EXTENSIONS, ...PROJECT_EXTENSIONS]);
const OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS = [
  ...IMAGE_EXTENSIONS,
  ...ARCHIVE_EXTENSIONS,
  ...PROJECT_EXTENSIONS,
]
  .filter((ext) => !BASIC_ASSOCIATION_EXTENSIONS.has(ext))
  .sort();
const OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS = [...VIDEO_EXTENSIONS].sort();
const OPTIONAL_ASSOCIATION_EXTENSIONS = [
  ...OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS,
  ...OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS,
].sort();

function extensionOf(filePath) {
  return path.extname(filePath).toLowerCase();
}

function isSupported(filePath) {
  return SUPPORTED_EXTENSIONS.has(extensionOf(filePath));
}

function isImage(filePath) {
  return IMAGE_EXTENSIONS.has(extensionOf(filePath));
}

function isVideo(filePath) {
  return VIDEO_EXTENSIONS.has(extensionOf(filePath));
}

function mediaTypeForPath(filePath) {
  if (isVideo(filePath)) return "video";
  if (isImage(filePath)) return "image";
  return "other";
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
      mediaType: mediaTypeForPath(entry.name),
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
  OPTIONAL_IMAGE_ASSOCIATION_EXTENSIONS,
  OPTIONAL_VIDEO_ASSOCIATION_EXTENSIONS,
  PROJECT_EXTENSIONS,
  PSD_EXTENSIONS,
  RAW_EXTENSIONS,
  SUPPORTED_EXTENSIONS,
  VIDEO_EXTENSIONS,
  VIDEO_MIME_TYPES,
  extensionOf,
  findComicProject,
  isImage,
  isVideo,
  isSupported,
  listFolder,
  mediaTypeForPath,
  naturalCompare,
};
