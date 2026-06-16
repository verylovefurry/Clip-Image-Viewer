"use strict";

const fs = require("fs");
const path = require("path");
const { Readable } = require("stream");
const { pipeline } = require("stream/promises");
const AdmZip = require("adm-zip");

const LATEST_RELEASE_API =
  "https://api.github.com/repos/verylovefurry/clip-image-viewer/releases/latest";
const PORTABLE_ASSET_PATTERN = /windows-x64-portable\.zip$/i;

function versionParts(version) {
  return String(version)
    .replace(/^v/i, "")
    .split(/[.+-]/, 3)
    .map((part) => Number.parseInt(part, 10) || 0);
}

function isNewerVersion(candidate, current) {
  const next = versionParts(candidate);
  const installed = versionParts(current);
  for (let index = 0; index < 3; index += 1) {
    if (next[index] !== installed[index]) return next[index] > installed[index];
  }
  return false;
}

async function getLatestPortableRelease(fetchImpl = globalThis.fetch) {
  const response = await fetchImpl(LATEST_RELEASE_API, {
    headers: {
      Accept: "application/vnd.github+json",
      "User-Agent": "Clip-Image-Viewer",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub 릴리스 확인 실패 (${response.status})`);
  }
  const release = await response.json();
  const asset = release.assets?.find((item) => PORTABLE_ASSET_PATTERN.test(item.name));
  return {
    version: String(release.tag_name || release.name || "").replace(/^v/i, ""),
    assetName: asset?.name || "",
    downloadUrl: asset?.browser_download_url || "",
    releaseUrl: release.html_url || "",
  };
}

async function downloadPortableUpdate({
  release,
  destinationRoot,
  fetchImpl = globalThis.fetch,
  onProgress = () => {},
}) {
  if (!release.downloadUrl) {
    throw new Error("릴리스에 Windows 포터블 ZIP이 없습니다.");
  }

  const updateRoot = path.join(destinationRoot, `clip-image-viewer-${release.version}`);
  const archivePath = path.join(updateRoot, release.assetName || "portable.zip");
  const stagingPath = path.join(updateRoot, "staging");
  fs.rmSync(updateRoot, { recursive: true, force: true });
  fs.mkdirSync(updateRoot, { recursive: true });

  const response = await fetchImpl(release.downloadUrl, {
    headers: { "User-Agent": "Clip-Image-Viewer" },
    redirect: "follow",
  });
  if (!response.ok || !response.body) {
    throw new Error(`업데이트 다운로드 실패 (${response.status})`);
  }

  const total = Number(response.headers.get("content-length")) || 0;
  let transferred = 0;
  const source = Readable.fromWeb(response.body);
  source.on("data", (chunk) => {
    transferred += chunk.length;
    onProgress(total ? transferred / total * 100 : 0);
  });
  await pipeline(source, fs.createWriteStream(archivePath));

  fs.mkdirSync(stagingPath, { recursive: true });
  new AdmZip(archivePath).extractAllTo(stagingPath, true);
  if (!fs.existsSync(path.join(stagingPath, "ClipImageViewer.exe"))) {
    throw new Error("포터블 업데이트 파일 구성이 올바르지 않습니다.");
  }
  fs.rmSync(archivePath, { force: true });
  return stagingPath;
}

module.exports = {
  downloadPortableUpdate,
  getLatestPortableRelease,
  isNewerVersion,
};
