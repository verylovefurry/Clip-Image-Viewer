//! Automatic updates for the Rust build from GitHub Releases, for both the portable ZIP and
//! the NSIS installer. Only Rust assets are considered, so the Electron channel is untouched:
//! - portable:  `Clip-Image-Viewer-<version>-rust-windows-x64-portable.zip`
//! - installer: `Clip-Image-Viewer-Setup-<version>-rust-x64.exe`
//! Every download must match its line in the release's `SHA256SUMS.txt`.
//!
//! Applying a portable update runs the *new* executable from the staging folder with
//! [`APPLY_FLAG`]: it waits for the old process to exit, copies the staged files over the
//! portable folder (keeping backups and rolling back on failure) and starts the app again.
//! The installer runs passively with `/P /R /UPDATE`, which reinstalls in place and restarts.

use std::{
    fs, io::{Read, Write}, path::{Path, PathBuf}, sync::Mutex, time::Duration,
};

use semver::Version;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Result, error};

const RELEASES_API: &str = "https://api.github.com/repos/verylovefurry/Clip-Image-Viewer/releases?per_page=30";
const DOWNLOAD_PREFIX: &str = "https://github.com/verylovefurry/Clip-Image-Viewer/releases/download/";
const USER_AGENT: &str = "Clip-Image-Viewer";
const MAX_DOWNLOAD: u64 = 1 << 30;
const MAX_ENTRIES: usize = 2_000;
const MAX_EXTRACTED: u64 = 2 << 30;
const STAGING_PREFIX: &str = "clip-image-viewer-update-";
const EXECUTABLE: &str = "ClipImageViewer.exe";
pub const APPLY_FLAG: &str = "--clipview-apply-portable-update";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flavor { Portable, Installer }

impl Flavor {
    pub fn current() -> Self { if crate::associations::portable() { Self::Portable } else { Self::Installer } }

    /// Version encoded in a release asset name of this flavour.
    fn version(self, name: &str) -> Option<Version> {
        let core = match self {
            Self::Portable => name.strip_prefix("Clip-Image-Viewer-")?.strip_suffix("-rust-windows-x64-portable.zip")?,
            Self::Installer => name.strip_prefix("Clip-Image-Viewer-Setup-")?.strip_suffix("-rust-x64.exe")?,
        };
        Version::parse(core).ok()
    }

    fn label(self) -> &'static str { match self { Self::Portable => "포터블", Self::Installer => "설치형" } }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release { pub version: Version, pub asset: String, pub url: String, pub checksums: String }

/// Newest release newer than `current` with an asset for `flavor` and a checksum file.
/// Prereleases are offered only to users already running a prerelease.
pub fn select(releases: &Value, flavor: Flavor, current: &Version) -> Option<Release> {
    releases.as_array()?.iter()
        .filter(|r| r["draft"] != true && (r["prerelease"] != true || !current.pre.is_empty()))
        .filter_map(|r| {
            let assets = r["assets"].as_array()?;
            let url = |a: &Value| a["browser_download_url"].as_str().map(str::to_owned);
            let checksums = assets.iter().find(|a| a["name"] == "SHA256SUMS.txt").and_then(url)?;
            assets.iter().find_map(|a| {
                let name = a["name"].as_str()?;
                let version = flavor.version(name)?;
                Some(Release { version, asset: name.to_owned(), url: url(a)?, checksums: checksums.clone() })
            })
        })
        .filter(|r| r.version > *current)
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// The SHA-256 listed for `asset` in a `sha256sum`-style file.
pub fn expected_hash(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && name.rsplit(['/', '\\']).next() == Some(asset))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15 * 60))).timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(USER_AGENT).build().into()
}

fn checked_url<'a>(url: &'a str, allow_any: bool) -> Result<&'a str> {
    // GitHub repository names are case-insensitive in URLs.
    if allow_any || url.get(..DOWNLOAD_PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(DOWNLOAD_PREFIX)) { Ok(url) } else { Err("업데이트 주소가 공식 릴리스가 아닙니다.".into()) }
}

/// Streams `url` to `path`, returning its SHA-256. `progress` receives 0..=100.
fn download(url: &str, path: &Path, progress: &mut dyn FnMut(f64)) -> Result<String> {
    let response = agent().get(url).call().map_err(|e| format!("업데이트 다운로드 실패: {e}"))?;
    let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    if total > MAX_DOWNLOAD { return Err("업데이트 파일이 안전 크기 제한을 초과합니다.".into()); }
    let mut reader = response.into_body().into_with_config().limit(MAX_DOWNLOAD).reader();
    let mut file = fs::File::create(path).map_err(error)?;
    let mut hash = Sha256::new();
    let (mut buffer, mut done, mut last) = (vec![0; 256 << 10], 0u64, -1.0f64);
    loop {
        let count = reader.read(&mut buffer).map_err(|e| format!("업데이트 다운로드 실패: {e}"))?;
        if count == 0 { break; }
        hash.update(&buffer[..count]);
        file.write_all(&buffer[..count]).map_err(error)?;
        done += count as u64;
        if total > 0 {
            let percent = (done as f64 / total as f64 * 100.).floor();
            if percent > last { last = percent; progress(percent); }
        }
    }
    file.flush().map_err(error)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Extracts a portable ZIP with path, count and size checks; the root must hold the app.
pub fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive).map_err(error)?).map_err(error)?;
    if zip.len() > MAX_ENTRIES { return Err("업데이트 압축 파일 항목 수가 안전 제한을 초과합니다.".into()); }
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(error)?;
        let relative = entry.enclosed_name().ok_or("업데이트 압축 파일에 안전하지 않은 경로가 있습니다.")?;
        let target = destination.join(relative);
        if entry.is_dir() { fs::create_dir_all(&target).map_err(error)?; continue; }
        total += entry.size();
        if total > MAX_EXTRACTED { return Err("업데이트 압축 해제 크기가 안전 제한을 초과합니다.".into()); }
        if let Some(parent) = target.parent() { fs::create_dir_all(parent).map_err(error)?; }
        let mut out = fs::File::create(&target).map_err(error)?;
        let size = entry.size();
        std::io::copy(&mut (&mut entry).take(size), &mut out).map_err(error)?;
    }
    if !destination.join(EXECUTABLE).is_file() { return Err("포터블 업데이트 파일 구성이 올바르지 않습니다.".into()); }
    Ok(())
}

enum Staged { Portable(PathBuf), Installer(PathBuf) }

pub struct Updater { state: Mutex<Value>, staged: Mutex<Option<(Version, Staged)>>, busy: Mutex<()> }

impl Updater {
    pub fn new(current: &str) -> Self {
        let message = if Self::supported() { format!("현재 버전 {current}") } else { "개발·진단 실행에서는 자동 업데이트를 확인하지 않습니다.".into() };
        Self {
            state: Mutex::new(json!({"status": if Self::supported() {"idle"} else {"disabled"}, "currentVersion": current, "version": "", "percent": 0, "message": message})),
            staged: Mutex::new(None), busy: Mutex::new(()),
        }
    }

    pub fn supported() -> bool {
        cfg!(windows) && !cfg!(debug_assertions) && std::env::var_os("CLIPVIEW_RUST_SMOKE").is_none()
            && std::env::var_os("CLIPVIEW_USER_DATA_DIR").is_none()
    }

    pub fn state(&self) -> Value { self.state.lock().map(|s| s.clone()).unwrap_or(Value::Null) }

    fn set(&self, emit: &dyn Fn(&Value), patch: Value) -> Value {
        let mut state = self.state.lock().expect("update state");
        for (key, value) in patch.as_object().into_iter().flatten() { state[key] = value.clone(); }
        emit(&state);
        state.clone()
    }

    /// Checks GitHub and, when newer, downloads and verifies the update.
    pub fn check(&self, current: &str, emit: &dyn Fn(&Value)) -> Value {
        if !Self::supported() { return self.state(); }
        let Ok(_guard) = self.busy.try_lock() else { return self.state() };
        if self.staged.lock().is_ok_and(|s| s.is_some()) { return self.state(); }
        match self.run(current, Flavor::current(), RELEASES_API, false, emit) {
            Ok(state) => state,
            Err(e) => self.set(emit, json!({"status": "error", "percent": 0, "message": format!("업데이트 실패: {e}")})),
        }
    }

    fn run(&self, current: &str, flavor: Flavor, api: &str, allow_any: bool, emit: &dyn Fn(&Value)) -> Result<Value> {
        let current_version = Version::parse(current).map_err(error)?;
        self.set(emit, json!({"status": "checking", "percent": 0, "message": "GitHub에서 새 버전을 확인하는 중..."}));
        let releases: Value = agent().get(api).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28")
            .call().map_err(|e| format!("GitHub 릴리스 확인 실패: {e}"))?
            .into_body().with_config().limit(8 << 20).read_json().map_err(error)?;
        let Some(release) = select(&releases, flavor, &current_version) else {
            return Ok(self.set(emit, json!({"status": "up-to-date", "version": "", "percent": 0, "message": format!("최신 버전 {current}을 사용 중입니다.")})));
        };
        let version = release.version.to_string();
        let label = flavor.label();
        self.set(emit, json!({"status": "downloading", "version": version, "percent": 0, "message": format!("{label} 버전 {version} 다운로드 중 0%")}));
        let root = std::env::temp_dir().join(format!("{STAGING_PREFIX}{version}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).map_err(error)?;
        let package = root.join(&release.asset);
        let mut progress = |percent: f64| {
            self.set(emit, json!({"percent": percent, "message": format!("{label} 버전 {version} 다운로드 중 {percent:.0}%")}));
        };
        let actual = download(checked_url(&release.url, allow_any)?, &package, &mut progress)?;
        let sums = agent().get(checked_url(&release.checksums, allow_any)?).call().map_err(|e| format!("체크섬 다운로드 실패: {e}"))?
            .into_body().with_config().limit(1 << 20).read_to_string().map_err(error)?;
        if expected_hash(&sums, &release.asset).as_deref() != Some(actual.as_str()) {
            let _ = fs::remove_dir_all(&root);
            return Err("업데이트 파일의 SHA-256 검증에 실패했습니다.".into());
        }
        let staged = match flavor {
            Flavor::Portable => {
                let staging = root.join("staging");
                extract(&package, &staging)?;
                let _ = fs::remove_file(&package);
                Staged::Portable(staging)
            }
            Flavor::Installer => Staged::Installer(package),
        };
        *self.staged.lock().map_err(error)? = Some((release.version.clone(), staged));
        Ok(self.set(emit, json!({"status": "downloaded", "percent": 100, "message": format!("버전 {version} 준비 완료. 재시작하면 업데이트됩니다.")})))
    }

    /// Starts the staged update; the caller exits the app afterwards.
    pub fn launch(&self) -> Result<()> {
        let staged = self.staged.lock().map_err(error)?;
        let (_, staged) = staged.as_ref().ok_or("준비된 업데이트가 없습니다.")?;
        let mut command = match staged {
            Staged::Portable(staging) => {
                let target = std::env::current_exe().map_err(error)?.parent().ok_or("실행 폴더를 찾지 못했습니다.")?.to_path_buf();
                let mut command = std::process::Command::new(staging.join(EXECUTABLE));
                command.arg(APPLY_FLAG).arg(std::process::id().to_string()).arg(staging).arg(target);
                command
            }
            Staged::Installer(installer) => {
                let mut command = std::process::Command::new(installer);
                command.args(["/P", "/R", "/UPDATE"]);
                command
            }
        };
        #[cfg(windows)]
        { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0008); } // CREATE_NO_WINDOW | DETACHED_PROCESS
        command.spawn().map_err(|e| format!("업데이트를 시작할 수 없습니다: {e}"))?;
        Ok(())
    }
}

/// Removes staging folders left by earlier updates. Folders in use are skipped.
pub fn clean_old_staging() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(STAGING_PREFIX) { let _ = fs::remove_dir_all(entry.path()); }
    }
}

fn files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() { pending.push(entry.path()); } else { found.push(entry.path()); }
        }
    }
    Ok(found)
}

fn retry<T>(mut action: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut attempt = 0;
    loop {
        match action() {
            Err(_) if attempt < 20 => { attempt += 1; std::thread::sleep(Duration::from_millis(250)); }
            other => return other,
        }
    }
}

/// Copies `staging` over `target`, backing up replaced files; restores everything on failure.
pub fn replace(staging: &Path, target: &Path) -> Result<()> {
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let outcome = (|| -> std::io::Result<()> {
        for source in files(staging)? {
            let relative = source.strip_prefix(staging).map_err(std::io::Error::other)?;
            let destination = target.join(relative);
            if let Some(parent) = destination.parent() { fs::create_dir_all(parent)?; }
            let backup = destination.exists().then(|| destination.with_extension(format!(
                "{}update-backup", destination.extension().map_or(String::new(), |e| format!("{}.", e.to_string_lossy())))));
            if let Some(backup) = &backup {
                let _ = fs::remove_file(backup);
                retry(|| fs::rename(&destination, backup))?;
            }
            done.push((destination.clone(), backup));
            retry(|| fs::copy(&source, &destination))?;
        }
        Ok(())
    })();
    match outcome {
        Ok(()) => {
            for (_, backup) in &done { if let Some(backup) = backup { let _ = fs::remove_file(backup); } }
            Ok(())
        }
        Err(e) => {
            for (destination, backup) in done.iter().rev() {
                let _ = fs::remove_file(destination);
                if let Some(backup) = backup { let _ = fs::rename(backup, destination); }
            }
            Err(format!("업데이트 파일 교체 실패(원래 파일 복원): {e}"))
        }
    }
}

/// Entry point of the portable update helper: `<flag> <pid> <staging> <target>`.
pub fn apply_main(args: &[std::ffi::OsString]) -> i32 {
    let [pid, staging, target] = args else { return 2 };
    let (staging, target) = (PathBuf::from(staging), PathBuf::from(target));
    if !staging.is_absolute() || !target.is_absolute() || !target.join(EXECUTABLE).is_file() || !staging.join(EXECUTABLE).is_file() {
        return 2;
    }
    if let Some(pid) = pid.to_str().and_then(|p| p.parse::<u32>().ok()) { wait_for_exit(pid); }
    let code = if replace(&staging, &target).is_ok() { 0 } else { 1 };
    // Start the updated app, or the restored old one after a failed copy.
    let _ = std::process::Command::new(target.join(EXECUTABLE)).current_dir(&target).spawn();
    code
}

#[cfg(windows)]
fn wait_for_exit(pid: u32) {
    use windows::Win32::{Foundation::CloseHandle, System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject}};
    unsafe {
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(process, 60_000);
            let _ = CloseHandle(process);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_exit(_: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn releases(prerelease: bool) -> Value {
        json!([
            {"draft": false, "prerelease": false, "assets": [
                {"name": "Clip-Image-Viewer-1.9.0-windows-x64-portable.zip", "browser_download_url": "https://x/electron.zip"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://x/sums1"}]},
            {"draft": false, "prerelease": prerelease, "assets": [
                {"name": "Clip-Image-Viewer-2.0.0-beta.1-rust-windows-x64-portable.zip", "browser_download_url": "https://x/p.zip"},
                {"name": "Clip-Image-Viewer-Setup-2.0.0-beta.1-rust-x64.exe", "browser_download_url": "https://x/s.exe"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://x/sums2"}]},
            {"draft": true, "prerelease": false, "assets": [
                {"name": "Clip-Image-Viewer-9.0.0-rust-windows-x64-portable.zip", "browser_download_url": "https://x/draft.zip"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://x/sums3"}]},
        ])
    }

    #[test]
    fn selects_only_rust_assets_and_respects_prerelease_channel() {
        let alpha = Version::parse("2.0.0-alpha.1").unwrap();
        let portable = select(&releases(true), Flavor::Portable, &alpha).unwrap();
        assert_eq!((portable.version.to_string(), portable.url.as_str()), ("2.0.0-beta.1".into(), "https://x/p.zip"));
        assert_eq!(select(&releases(true), Flavor::Installer, &alpha).unwrap().url, "https://x/s.exe");
        // A stable user is not offered a prerelease; Electron and draft assets never match.
        assert!(select(&releases(true), Flavor::Portable, &Version::parse("1.0.0").unwrap()).is_none());
        assert!(select(&releases(false), Flavor::Portable, &Version::parse("2.0.0-beta.1").unwrap()).is_none());
    }

    #[test]
    fn finds_hashes_and_rejects_foreign_urls() {
        let hash = "a".repeat(64);
        let sums = format!("{} *other.zip\n{hash}  dist/Clip-Image-Viewer-Setup-2.0.0-rust-x64.exe\n", "b".repeat(64));
        assert_eq!(expected_hash(&sums, "Clip-Image-Viewer-Setup-2.0.0-rust-x64.exe"), Some(hash));
        assert_eq!(expected_hash("xyz name", "name"), None);
        assert!(checked_url("https://evil.example/x.zip", false).is_err());
        assert!(checked_url(&format!("{DOWNLOAD_PREFIX}v2/a.zip"), false).is_ok());
    }

    #[test]
    fn replace_backs_up_and_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let (staging, target) = (dir.path().join("staging"), dir.path().join("target"));
        fs::create_dir_all(staging.join("licenses")).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(staging.join(EXECUTABLE), b"new").unwrap();
        fs::write(staging.join("licenses/a.txt"), b"license").unwrap();
        fs::write(target.join(EXECUTABLE), b"old").unwrap();
        fs::write(target.join("keep.txt"), b"user file").unwrap();
        replace(&staging, &target).unwrap();
        assert_eq!(fs::read(target.join(EXECUTABLE)).unwrap(), b"new");
        assert_eq!(fs::read(target.join("licenses/a.txt")).unwrap(), b"license");
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"user file");
        assert!(files(&target).unwrap().iter().all(|p| !p.to_string_lossy().contains("update-backup")));
        // A folder that cannot be created (a file is in the way) rolls back the copied exe.
        fs::write(staging.join(EXECUTABLE), b"newer").unwrap();
        fs::create_dir_all(staging.join("sub")).unwrap();
        fs::write(staging.join("sub/x.txt"), b"x").unwrap();
        fs::write(target.join("sub"), b"a file, not a folder").unwrap();
        assert!(replace(&staging, &target).is_err());
        assert_eq!(fs::read(target.join(EXECUTABLE)).unwrap(), b"new");
        assert!(!target.join(format!("{EXECUTABLE}.update-backup")).exists());
    }

    #[test]
    #[ignore = "contacts api.github.com"]
    fn live_github_release_check() {
        let updater = Updater::new("2.0.0-alpha.1");
        let state = updater.run("2.0.0-alpha.1", Flavor::Portable, RELEASES_API, false, &|_| {}).unwrap();
        println!("live: {state}");
        assert!(matches!(state["status"].as_str(), Some("up-to-date" | "downloaded")));
    }

    /// Serves a fake GitHub API, ZIP and checksum file from a local socket.
    #[test]
    fn downloads_verifies_and_stages_portable_update() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("update.zip");
        {
            let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
            zip.start_file(EXECUTABLE, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"new exe").unwrap();
            zip.finish().unwrap();
        }
        let package = fs::read(&zip_path).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let asset = "Clip-Image-Viewer-9.9.9-rust-windows-x64-portable.zip";
        let api = json!([{"draft": false, "prerelease": false, "assets": [
            {"name": asset, "browser_download_url": format!("{base}/zip")},
            {"name": "SHA256SUMS.txt", "browser_download_url": format!("{base}/sums")}]}]).to_string();
        let sums = format!("{:x}  {asset}\n", Sha256::digest(&package));
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let body: Vec<u8> = if line.contains("/zip") { package.clone() } else if line.contains("/sums") { sums.clone().into_bytes() } else { api.clone().into_bytes() };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let updater = Updater::new("2.0.0");
        let events = Mutex::new(Vec::new());
        let state = updater.run("2.0.0", Flavor::Portable, &format!("{base}/api"), true, &|s| events.lock().unwrap().push(s["status"].clone())).unwrap();
        server.join().unwrap();
        assert_eq!(state["status"], "downloaded");
        assert!(events.lock().unwrap().iter().any(|s| s == "downloading"));
        let staged = updater.staged.lock().unwrap();
        let Some((version, Staged::Portable(staging))) = staged.as_ref() else { panic!("portable staging") };
        assert_eq!(version.to_string(), "9.9.9");
        assert_eq!(fs::read(staging.join(EXECUTABLE)).unwrap(), b"new exe");
        let _ = fs::remove_dir_all(staging.parent().unwrap());
    }
}
