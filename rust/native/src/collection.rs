use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, Row};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Result, error};

pub const IMAGE_EXTENSIONS: &str = "jpg jpeg jfif png apng gif webp bmp svg ico avif tif tiff heic heif hif dds j2k j2c jpc jp2 jpf jpx tga pcx pgm pnm ppm pbm pam jxl exr qoi jxr hdp wdp bpg 3fr ari arw bay cap cr2 cr3 dcr dcs dng drf eip erf fff gpr iiq k25 kdc mdc mef mos mrw nef nrw obm orf pef ptx pxn r3d raf raw rwl rw2 rwz sr2 srf srw sti psd psb clip csp";
pub const VIDEO_EXTENSIONS: &str = "mp4 m4v mov qt webm mkv avi wmv asf flv f4v ogv ogm 3gp 3g2 mpg mpeg mpe m1v m2v ts mts m2ts vob divx xvid mxf dv amv rm rmvb";
pub const AUDIO_EXTENSIONS: &str = "mp3 wav wave flac ogg oga opus m4a m4b aac weba";
pub const BASIC: [&str; 6] = [".bmp", ".gif", ".jpeg", ".jpg", ".png", ".webp"];
const MAX_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_ARCHIVE_TOTAL: u64 = 4 << 30;
const MAX_ENTRY: u64 = 1 << 30;

pub fn extension(path: &Path) -> String {
    path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase()
}

pub fn name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

pub fn media_kind(ext: &str) -> &'static str {
    if IMAGE_EXTENSIONS.split_whitespace().any(|value| value == ext) {
        "image"
    } else if VIDEO_EXTENSIONS.split_whitespace().any(|value| value == ext) {
        "video"
    } else if AUDIO_EXTENSIONS.split_whitespace().any(|value| value == ext) {
        "audio"
    } else {
        "other"
    }
}

pub fn extensions() -> Vec<String> {
    [IMAGE_EXTENSIONS, VIDEO_EXTENSIONS, AUDIO_EXTENSIONS, "zip cbz cmc"]
        .into_iter().flat_map(str::split_whitespace).map(str::to_owned).collect()
}

pub fn mime(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "png" | "apng" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "mp3" => "audio/mpeg",
        "aac" => "audio/aac",
        "wav" | "wave" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "m4a" | "m4b" => "audio/mp4",
        "weba" => "audio/webm",
        "mp4" | "m4v" | "f4v" => "video/mp4",
        "mov" | "qt" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" | "divx" | "xvid" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "asf" => "video/x-ms-asf",
        "flv" => "video/x-flv",
        "ogv" | "ogm" => "video/ogg",
        "3gp" => "video/3gpp",
        "3g2" => "video/3gpp2",
        "mpg" | "mpeg" | "mpe" | "m1v" | "m2v" | "vob" => "video/mpeg",
        "ts" | "mts" | "m2ts" => "video/mp2t",
        "mxf" => "application/mxf",
        "dv" => "video/dv",
        "amv" => "video/x-amv",
        "rm" => "application/vnd.rn-realmedia",
        "rmvb" => "application/vnd.rn-realmedia-vbr",
        _ => return None,
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crop {
    pub canvas_width: f64,
    pub canvas_height: f64,
    pub trim_width: f64,
    pub trim_height: f64,
    #[serde(default)]
    pub bleed: f64,
    #[serde(default)]
    pub offset_x: f64,
    #[serde(default)]
    pub offset_y: f64,
    #[serde(default = "one")]
    pub page_count: f64,
}

fn one() -> f64 { 1. }

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Item {
    File { path: PathBuf, #[serde(default)] crop: Option<Crop> },
    Archive { path: PathBuf },
    #[serde(rename_all = "camelCase")]
    ArchiveEntry { archive_path: PathBuf, entry_name: String },
}

impl Item {
    pub fn disk_path(&self) -> &Path {
        match self {
            Self::File { path, .. } | Self::Archive { path } => path,
            Self::ArchiveEntry { archive_path, .. } => archive_path,
        }
    }

    pub fn ext(&self) -> String {
        match self {
            Self::ArchiveEntry { entry_name, .. } => extension(Path::new(entry_name)),
            _ => extension(self.disk_path()),
        }
    }

    pub fn crop(&self) -> Option<Crop> {
        match self {
            Self::File { crop, .. } => crop.clone(),
            _ => None,
        }
    }

    pub fn read(&self, max: u64) -> Result<Vec<u8>> {
        match self {
            Self::ArchiveEntry { archive_path, entry_name } => {
                let mut zip = archive(archive_path)?;
                let entry = zip.by_name(entry_name).map_err(error)?;
                if !entry.is_file() || entry.size() > max.min(MAX_ENTRY) {
                    return Err("압축 이미지 크기가 안전 제한을 초과합니다.".into());
                }
                read_limited(entry, max.min(MAX_ENTRY))
            }
            _ => {
                let f = File::open(self.disk_path()).map_err(error)?;
                if f.metadata().map_err(error)?.len() > max {
                    return Err("이미지 파일 크기가 안전 제한을 초과합니다.".into());
                }
                read_limited(f, max)
            }
        }
    }
}

pub fn read_limited(input: impl Read, max: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    input.take(max + 1).read_to_end(&mut data).map_err(error)?;
    if data.len() as u64 > max { return Err("파일 크기가 안전 제한을 초과합니다.".into()); }
    Ok(data)
}

fn archive(path: &Path) -> Result<zip::ZipArchive<File>> {
    let file = File::open(path).map_err(error)?;
    if file.metadata().map_err(error)?.len() > 2 << 30 {
        return Err("압축 파일 크기가 안전 제한을 초과합니다.".into());
    }
    let archive = zip::ZipArchive::new(file).map_err(error)?;
    if archive.len() > MAX_ARCHIVE_ENTRIES
        || archive.decompressed_size().is_none_or(|size| size > MAX_ARCHIVE_TOTAL as u128)
    {
        return Err("압축 파일의 항목 수 또는 해제 크기가 안전 제한을 초과합니다.".into());
    }
    Ok(archive)
}

// Compare digit runs without parsing into a fixed-width integer.
pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let l = left.to_lowercase();
    let r = right.to_lowercase();
    let (mut a, mut b) = (l.as_bytes(), r.as_bytes());
    while let (Some(&x), Some(&y)) = (a.first(), b.first()) {
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let al = a.iter().take_while(|c| c.is_ascii_digit()).count();
            let bl = b.iter().take_while(|c| c.is_ascii_digit()).count();
            let ar = a[..al].trim_ascii_start_matches_zero();
            let br = b[..bl].trim_ascii_start_matches_zero();
            let order = ar.len().cmp(&br.len()).then_with(|| ar.cmp(br));
            if order != Ordering::Equal { return order; }
            a = &a[al..];
            b = &b[bl..];
        } else {
            if x != y { return x.cmp(&y); }
            a = &a[1..];
            b = &b[1..];
        }
    }
    a.len().cmp(&b.len()).then_with(|| left.cmp(right))
}

trait TrimDigitRun {
    fn trim_ascii_start_matches_zero(&self) -> &[u8];
}
impl TrimDigitRun for [u8] {
    fn trim_ascii_start_matches_zero(&self) -> &[u8] {
        &self[self.iter().take_while(|&&b| b == b'0').count()..]
    }
}

fn archive_ext(ext: &str) -> bool { matches!(ext, "zip" | "cbz") }

fn archive_items(path: &Path) -> Result<Vec<Value>> {
    let mut archive = archive(path)?;
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(error)?;
        if entry.is_file() && media_kind(&extension(Path::new(entry.name()))) == "image" {
            names.push(entry.name().to_owned());
        }
    }
    names.sort_by(|a, b| natural_cmp(a, b));
    Ok(names.into_iter().map(|entry| json!({
        "kind": "archive-entry", "archivePath": path, "entryName": entry,
        "name": entry.rsplit(['/', '\\']).next().unwrap_or(""),
        "displayPath": format!("{} / {entry}", name(path)),
    })).collect())
}

pub fn display_absolute(path: &Path) -> Result<PathBuf> {
    // Avoid Win32 extended prefixes in the user-facing paths and shell calls.
    std::path::absolute(path).map_err(error)
}

fn ordinary(path: &Path) -> Value {
    let ext = extension(path);
    json!({
        "kind": if archive_ext(&ext) { "archive" } else { "file" },
        "path": path, "name": name(path), "mediaType": media_kind(&ext),
    })
}

pub fn open(path: &Path) -> Result<Value> {
    let target = display_absolute(path)?;
    let stat = fs::metadata(&target).map_err(error)?;
    let ext = extension(&target);
    if !stat.is_dir() {
        if ext == "cmc" { return comic(&target, None); }
        if archive_ext(&ext) {
            let items = archive_items(&target)?;
            return Ok(json!({"index": if items.is_empty() { -1 } else { 0 }, "items": items, "basePath": target}));
        }
        if media_kind(&ext) == "other" || !stat.is_file() {
            return Err("지원하지 않는 파일 형식입니다.".into());
        }
    }
    let folder = if stat.is_dir() { target.as_path() } else {
        target.parent().ok_or("파일의 폴더를 찾지 못했습니다.")?
    };
    let mut paths: Vec<PathBuf> = fs::read_dir(folder).map_err(error)?
        .filter_map(io::Result::ok).filter(|e| e.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|e| e.path()).collect();
    paths.sort_by(|a, b| natural_cmp(&name(a), &name(b)));
    for project in paths.iter().filter(|p| extension(p) == "cmc") {
        if let Ok(collection) = comic(project, (!stat.is_dir()).then_some(target.as_path()))
            && collection["index"].as_i64().is_some_and(|i| i >= 0)
        {
            return Ok(collection);
        }
    }
    paths.retain(|p| {
        let ext = extension(p);
        media_kind(&ext) != "other" || (stat.is_dir() && archive_ext(&ext))
    });
    let index = if stat.is_dir() {
        if paths.is_empty() { -1 } else { 0 }
    } else if let Some(index) = paths.iter().position(|p| same_path(p, &target)) {
        index as i64
    } else {
        paths.push(target.clone());
        paths.len() as i64 - 1
    };
    Ok(json!({"items": paths.iter().map(|p| ordinary(p)).collect::<Vec<_>>(), "index": index, "basePath": folder}))
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || (cfg!(windows) && a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()))
}

pub fn read_only_database(path: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(error)?;
    db.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON; PRAGMA cache_size=-2048;").map_err(error)?;
    Ok(db)
}

pub fn number(row: &Row<'_>, column: &str) -> f64 {
    row.get::<_, Option<f64>>(column).ok().flatten().filter(|n| n.is_finite()).unwrap_or_default()
}

pub fn integer(row: &Row<'_>, column: &str) -> i64 { number(row, column) as i64 }

struct Node { id: i64, row_id: i64, next: i64, first: i64, order: i64, kind: i64, link: String }

fn linked_page(folder: &Path, link: &str) -> Option<PathBuf> {
    let link = link.trim();
    let link = link.strip_prefix(".:").or_else(|| link.strip_prefix(".\\"))
        .or_else(|| link.strip_prefix("./")).unwrap_or(link);
    let normalized = link.replace([':', '\\'], "/");
    let relative = Path::new(&normalized);
    if relative.components().any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        || !matches!(extension(relative).as_str(), "clip" | "csp")
    { return None; }
    let page = folder.join(relative);
    let canonical = page.canonicalize().ok()?;
    (canonical.starts_with(folder.canonicalize().ok()?) && canonical.is_file()).then_some(page)
}

fn comic(path: &Path, target: Option<&Path>) -> Result<Value> {
    let db = read_only_database(path)?;
    let (root, crop) = db.query_row("SELECT * FROM Project LIMIT 1", [], |r| {
        let (cw, ch, tw, th) = (
            number(r, "DefaultPageWidth"), number(r, "DefaultPageHeight"),
            number(r, "DefaultPageCropWidth"), number(r, "DefaultPageCropHeight"),
        );
        let crop = if integer(r, "DefaultPageUseCropFrame") != 0 && [cw, ch, tw, th].iter().all(|&v| v > 0.) {
            json!({"canvasWidth": cw, "canvasHeight": ch, "trimWidth": tw, "trimHeight": th,
                "bleed": number(r, "DefaultPageCropDitch").max(0.),
                "offsetX": number(r, "DefaultPageCropOffsetX"), "offsetY": number(r, "DefaultPageCropOffsetY")})
        } else { Value::Null };
        Ok((integer(r, "ProjectRootCanvasNode"), crop))
    }).map_err(error)?;
    let mut query = db.prepare("SELECT * FROM CanvasNode LIMIT 10001").map_err(error)?;
    let nodes = query.query_map([], |r| Ok(Node {
        id: integer(r, "MainId"), row_id: integer(r, "_PW_ID"), next: integer(r, "NextIndex"),
        first: integer(r, "FirstChildIndex"), order: integer(r, "CanvasIndex"),
        kind: integer(r, "Type"), link: r.get("LinkPath").unwrap_or_default(),
    })).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
    if nodes.len() > 10_000 { return Err("CMC 페이지 수가 안전 제한을 초과합니다.".into()); }
    let by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let by_row_id: HashMap<_, _> = nodes.iter().map(|n| (n.row_id, n)).collect();
    let lookup = |id| by_id.get(&id).or_else(|| by_row_id.get(&id)).copied();
    let mut next = lookup(root).map_or(0, |n| n.first);
    let mut visited = HashSet::new();
    let mut ordered = Vec::new();
    while next != 0 && visited.insert(next) {
        let Some(node) = lookup(next) else { break };
        if node.kind == 2 && !node.link.is_empty() { ordered.push(node); }
        next = node.next;
    }
    if ordered.is_empty() {
        ordered.extend(nodes.iter().filter(|node| node.kind == 2 && !node.link.is_empty()));
        ordered.sort_by_key(|n| (n.order, n.id));
    }
    let folder = path.parent().ok_or("CMC 프로젝트 폴더를 찾지 못했습니다.")?;
    let pages: Vec<_> = ordered.iter().filter_map(|node| linked_page(folder, &node.link)).collect();
    if pages.is_empty() { return Err("CMC 프로젝트에서 열 수 있는 CLIP 페이지를 찾지 못했습니다.".into()); }
    let index = target.map_or(0, |target| pages.iter().position(|p| same_path(p, target)).map_or(-1, |i| i as i64));
    let items: Vec<_> = pages.iter().map(|page| json!({
        "kind": "file", "path": page, "name": name(page), "mediaType": "image", "crop": crop,
    })).collect();
    Ok(json!({"items": items, "index": index, "basePath": folder,
        "comic": {"path": path, "name": name(path), "cropAvailable": !crop.is_null()}}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_page_order() {
        let mut names = ["Page10.clip", "page2.clip", "page1.clip", "page999999999999999999999.clip"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["page1.clip", "page2.clip", "Page10.clip", "page999999999999999999999.clip"]);
    }

    #[test]
    fn comic_links_stay_inside_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("pages")).unwrap();
        let page = dir.path().join("pages").join("한글 1.clip");
        File::create(&page).unwrap();
        assert_eq!(linked_page(dir.path(), ".:pages:한글 1.clip"), Some(page));
        assert_eq!(linked_page(dir.path(), "../Pages/unsafe.clip"), None);
        assert_eq!(linked_page(dir.path(), "C:\\Windows\\system.clip"), None);
    }

    #[test]
    fn opening_page_checks_all_comic_projects() {
        let dir = tempfile::tempdir().unwrap();
        for (index, page) in [(1, "first.clip"), (2, "second.clip")] {
            File::create(dir.path().join(page)).unwrap();
            let db = Connection::open(dir.path().join(format!("project{index}.cmc"))).unwrap();
            db.execute_batch("CREATE TABLE Project (ProjectRootCanvasNode INTEGER);\
                INSERT INTO Project VALUES (1);\
                CREATE TABLE CanvasNode (MainId INTEGER, _PW_ID INTEGER, NextIndex INTEGER,\
                    FirstChildIndex INTEGER, CanvasIndex INTEGER, Type INTEGER, LinkPath TEXT);\
                INSERT INTO CanvasNode VALUES (1, 1, 0, 2, 0, 0, '');").unwrap();
            db.execute("INSERT INTO CanvasNode VALUES (2, 2, 0, 0, 0, 2, ?1)", [page]).unwrap();
        }
        let opened = open(&dir.path().join("second.clip")).unwrap();
        assert_eq!(opened["comic"]["name"], "project2.cmc");
        assert_eq!(opened["items"][0]["name"], "second.clip");
    }
}
