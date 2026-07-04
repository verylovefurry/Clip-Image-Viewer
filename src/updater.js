"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { Readable } = require("stream");
const { pipeline } = require("stream/promises");
const AdmZip = require("adm-zip");

const LATEST_RELEASE_API =
  "https://api.github.com/repos/verylovefurry/clip-image-viewer/releases/latest";
const PORTABLE_ASSET_PATTERN = /windows-x64-portable\.zip$/i;
const CHECKSUM_ASSET_PATTERN = /^SHA256SUMS\.txt$/i;
const MAX_UPDATE_BYTES = 1024 * 1024 * 1024;
const MAX_UPDATE_ENTRIES = 20000;
const MAX_UPDATE_EXTRACTED_BYTES = 3 * 1024 * 1024 * 1024;

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
    signal: AbortSignal.timeout(15000),
  });
  if (!response.ok) {
    throw new Error(`GitHub 릴리스 확인 실패 (${response.status})`);
  }
  const release = await response.json();
  const asset = release.assets?.find((item) => PORTABLE_ASSET_PATTERN.test(item.name));
  const checksumAsset = release.assets?.find((item) => CHECKSUM_ASSET_PATTERN.test(item.name));
  return {
    version: String(release.tag_name || release.name || "").replace(/^v/i, ""),
    assetName: asset?.name || "",
    downloadUrl: asset?.browser_download_url || "",
    checksumUrl: checksumAsset?.browser_download_url || "",
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
  if (!release.checksumUrl) {
    throw new Error("릴리스에 SHA256SUMS.txt가 없어 업데이트를 검증할 수 없습니다.");
  }
  if (
    !/^[0-9A-Za-z][0-9A-Za-z._-]*$/.test(release.version) ||
    release.version.includes("..")
  ) {
    throw new Error("업데이트 버전 문자열이 올바르지 않습니다.");
  }
  if (!release.assetName || path.basename(release.assetName) !== release.assetName) {
    throw new Error("업데이트 파일 이름이 올바르지 않습니다.");
  }

  const updateRoot = path.join(destinationRoot, `clip-image-viewer-${release.version}`);
  const archivePath = path.join(updateRoot, release.assetName || "portable.zip");
  const stagingPath = path.join(updateRoot, "staging");
  fs.rmSync(updateRoot, { recursive: true, force: true });
  fs.mkdirSync(updateRoot, { recursive: true });

  const response = await fetchImpl(release.downloadUrl, {
    headers: { "User-Agent": "Clip-Image-Viewer" },
    redirect: "follow",
    signal: AbortSignal.timeout(15 * 60 * 1000),
  });
  if (!response.ok || !response.body) {
    throw new Error(`업데이트 다운로드 실패 (${response.status})`);
  }

  const total = Number(response.headers.get("content-length")) || 0;
  if (total > MAX_UPDATE_BYTES) {
    throw new Error("업데이트 파일이 안전 크기 제한을 초과합니다.");
  }
  let transferred = 0;
  const hash = crypto.createHash("sha256");
  const source = Readable.fromWeb(response.body);
  source.on("data", (chunk) => {
    transferred += chunk.length;
    if (transferred > MAX_UPDATE_BYTES) source.destroy(
      new Error("업데이트 파일이 안전 크기 제한을 초과합니다."),
    );
    hash.update(chunk);
    onProgress(total ? transferred / total * 100 : 0);
  });
  await pipeline(source, fs.createWriteStream(archivePath));

  const checksumResponse = await fetchImpl(release.checksumUrl, {
    headers: { "User-Agent": "Clip-Image-Viewer" },
    redirect: "follow",
    signal: AbortSignal.timeout(30000),
  });
  if (!checksumResponse.ok) {
    throw new Error(`업데이트 체크섬 다운로드 실패 (${checksumResponse.status})`);
  }
  const checksumText = await checksumResponse.text();
  const expected = checksumText
    .split(/\r?\n/)
    .map((line) => line.trim().match(/^([a-f0-9]{64})\s+\*?(.+)$/i))
    .find((match) => match && path.basename(match[2]) === release.assetName)?.[1];
  if (!expected || hash.digest("hex").toLowerCase() !== expected.toLowerCase()) {
    throw new Error("업데이트 파일의 SHA-256 검증에 실패했습니다.");
  }

  fs.mkdirSync(stagingPath, { recursive: true });
  const zip = new AdmZip(archivePath);
  const entries = zip.getEntries();
  const totalExtracted = entries.reduce(
    (sum, entry) => sum + Number(entry.header?.size || 0),
    0,
  );
  if (entries.length > MAX_UPDATE_ENTRIES || totalExtracted > MAX_UPDATE_EXTRACTED_BYTES) {
    throw new Error("업데이트 압축 파일 구성이 안전 제한을 초과합니다.");
  }
  const stagingRoot = `${path.resolve(stagingPath)}${path.sep}`;
  for (const entry of entries) {
    const destination = path.resolve(stagingPath, entry.entryName);
    if (destination !== path.resolve(stagingPath) && !destination.startsWith(stagingRoot)) {
      throw new Error("업데이트 압축 파일에 안전하지 않은 경로가 포함되어 있습니다.");
    }
  }
  zip.extractAllTo(stagingPath, true);
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
