use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::Mutex,
};

use tauri::http::{Request, Response, StatusCode};

use crate::{Result, error};

const TARGET_MEMORY: usize = 48 << 20;
const MAX_BYTES: usize = 96 << 20;
const MAX_ASSETS: usize = 600;
const MAX_RANGE: u64 = 2 << 20;
const BASE: &str = if cfg!(windows) { "http://clipmedia.localhost/" } else { "clipmedia://localhost/" };

enum Content {
    File(PathBuf, u64),
    Encoded(Vec<u8>),
}

struct Asset {
    content: Content,
    mime: &'static str,
    range_required: bool,
}

impl Asset {
    fn memory(&self) -> usize {
        match &self.content {
            Content::File(_, _) => 0,
            Content::Encoded(bytes) => bytes.len(),
        }
    }

    fn len(&self) -> u64 {
        match &self.content {
            Content::File(_, len) => *len,
            Content::Encoded(bytes) => bytes.len() as u64,
        }
    }

    fn slice(&self, start: u64, end: u64) -> Result<Vec<u8>> {
        match &self.content {
            Content::File(path, _) => {
                let mut file = File::open(path).map_err(error)?;
                file.seek(SeekFrom::Start(start)).map_err(error)?;
                let mut buffer = vec![0; (end - start) as usize];
                file.read_exact(&mut buffer).map_err(error)?;
                Ok(buffer)
            }
            Content::Encoded(bytes) => Ok(bytes[start as usize..end as usize].to_vec()),
        }
    }
}

#[derive(Default)]
struct Map {
    assets: HashMap<String, std::sync::Arc<Asset>>,
    order: VecDeque<String>,
    memory: usize,
}

#[derive(Default)]
pub struct Assets(Mutex<Map>);

impl Assets {
    pub fn contains_url(&self, url: &str) -> bool {
        url.strip_prefix(BASE).is_some_and(|token| self.lookup(token).is_some())
    }

    fn insert(&self, asset: Asset) -> Result<String> {
        let size = asset.memory();
        if size > MAX_BYTES { return Err("이미지 출력 크기가 안전 제한을 초과합니다.".into()); }
        let mut map = self.0.lock().map_err(error)?;
        while !map.order.is_empty()
            && (map.memory + size > TARGET_MEMORY.max(size) || map.order.len() >= MAX_ASSETS)
        {
            let stale = map.order.pop_front().expect("cache does not start empty");
            if let Some(asset) = map.assets.remove(&stale) { map.memory -= asset.memory(); }
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        map.memory += size;
        map.order.push_back(token.clone());
        map.assets.insert(token.clone(), std::sync::Arc::new(asset));
        Ok(format!("{BASE}{token}"))
    }

    pub fn file(&self, path: PathBuf, mime: &'static str, media: bool) -> Result<String> {
        let metadata = path.metadata().map_err(error)?;
        if !metadata.is_file() || (!media && metadata.len() > MAX_BYTES as u64) {
            return Err("파일 크기가 안전 제한을 초과합니다.".into());
        }
        self.insert(Asset { content: Content::File(path, metadata.len()), mime, range_required: media })
    }

    pub fn bytes(&self, data: Vec<u8>, mime: &'static str) -> Result<String> {
        self.insert(Asset { content: Content::Encoded(data), mime, range_required: false })
    }

    pub fn encoded_image(&self, url: &str) -> Result<Vec<u8>> {
        let token = url.strip_prefix(BASE).ok_or("올바르지 않은 미리보기 주소입니다.")?;
        let asset = self.lookup(token).ok_or("미리보기 데이터가 만료되었습니다.")?;
        if !asset.mime.starts_with("image/") { return Err("이미지 주소가 아닙니다.".into()); }
        asset.slice(0, asset.len())
    }

    fn lookup(&self, token: &str) -> Option<std::sync::Arc<Asset>> {
        let mut map = self.0.lock().ok()?;
        let asset = map.assets.get(token)?.clone();
        map.order.retain(|other| other != token);
        map.order.push_back(token.into());
        Some(asset)
    }

    pub fn serve(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let response = Response::builder().header("Access-Control-Allow-Origin", "*")
            .header("X-Content-Type-Options", "nosniff")
            .header("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'")
            .header("Cache-Control", "no-store");
        let head = request.method() == "HEAD";
        if request.method() != "GET" && !head {
            return response.status(StatusCode::METHOD_NOT_ALLOWED).body(Vec::new()).unwrap();
        }
        let token = request.uri().path().trim_start_matches('/');
        let Some(asset) = self.lookup(token) else {
            return response.status(StatusCode::NOT_FOUND).body(Vec::new()).unwrap();
        };
        let len = asset.len();
        let mut response = response.header("Content-Type", asset.mime).header("Accept-Ranges", "bytes");
        let range = request.headers().get("Range");
        let (start, end) = if let Some(value) = range {
            let Some((start, end)) = value.to_str().ok().and_then(|text| parse_range(text, len)) else {
                return response.status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header("Content-Range", format!("bytes */{len}")).body(Vec::new()).unwrap();
            };
            (start, end.min(start + MAX_RANGE))
        } else if asset.range_required && !head && len > MAX_RANGE {
            (0, MAX_RANGE)
        } else { (0, len) };
        if range.is_some() || end < len {
            response = response.status(StatusCode::PARTIAL_CONTENT)
                .header("Content-Range", format!("bytes {start}-{}/{len}", end - 1));
        }
        match if head { Ok(Vec::new()) } else { asset.slice(start, end) } {
            Ok(bytes) => response.header("Content-Length", end - start).body(bytes).unwrap(),
            Err(_) => response.status(StatusCode::GONE).body(Vec::new()).unwrap(),
        }
    }
}

fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if len == 0 || end.contains(',') { return None; }
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        return (suffix > 0).then_some((len.saturating_sub(suffix), len));
    }
    let start: u64 = start.parse().ok()?;
    let end = if end.is_empty() { len } else { end.parse::<u64>().ok()?.saturating_add(1).min(len) };
    (start < end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_partial_content_from_registered_source_only() {
        let assets = Assets::default();
        let url = assets.bytes(vec![1, 2, 3, 4, 5], "video/mp4").unwrap();
        let request = Request::builder().uri(url).header("Range", "bytes=1-3").body(vec![]).unwrap();
        let response = assets.serve(request);
        assert_eq!(response.status(), 206);
        assert_eq!(response.headers()["Content-Range"], "bytes 1-3/5");
        assert_eq!(*response.body(), [2, 3, 4]);
        let request = Request::builder().uri("/../../secret").body(vec![]).unwrap();
        assert_eq!(assets.serve(request).status(), 404);
    }

    #[test]
    fn range_syntax_and_bounds() {
        assert_eq!(parse_range("bytes=-2", 10), Some((8, 10)));
        assert_eq!(parse_range("bytes=8-", 10), Some((8, 10)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=1-0", 10), None);
        assert_eq!(parse_range("bytes=0-1,3-4", 10), None);
    }

    #[test]
    fn cached_asset_urls_expire_when_evicted() {
        let assets = Assets::default();
        let first = assets.bytes(vec![42], "image/png").unwrap();
        assert!(assets.contains_url(&first));
        for _ in 0..MAX_ASSETS {
            assets.bytes(vec![1], "image/png").unwrap();
        }
        assert!(!assets.contains_url(&first));
        assert!(!assets.contains_url("http://example.com/image.png"));
    }
}
