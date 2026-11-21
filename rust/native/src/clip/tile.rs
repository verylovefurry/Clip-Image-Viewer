use std::io::{Read, Seek, SeekFrom};

use flate2::read::ZlibDecoder;
use image::{Rgba, RgbaImage};

use crate::{Result, collection::read_limited, error};

const TILE: u32 = 256;
const PLANE: usize = 256 * 256;
const LIMIT: u64 = 96 << 20;

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

pub fn int(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

pub fn double(data: &[u8], at: usize) -> Option<f64> {
    let n = f64::from_be_bytes(data.get(at..at.checked_add(8)?)?.try_into().ok()?);
    n.is_finite().then_some(n)
}

pub fn utf16_marker(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn wide(data: &[u8], offset: usize, expected: &str) -> bool {
    data.get(offset..offset + expected.len() * 2).is_some_and(|bytes| {
        bytes.chunks_exact(2).zip(expected.bytes()).all(|(word, letter)| word == [0, letter])
    })
}

#[derive(Clone, Copy)]
enum Packing { Rgba, Gray, BitGray, Alpha, BitAlpha }

#[derive(Clone, Copy)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    cols: u32,
    rows: u32,
    fill: bool,
    packing: Packing,
}

impl Geometry {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if int(data, 0)? != 16 || int(data, 4)? != 102 || !matches!(int(data, 8)?, 42 | 58)
            || int(data, 16)? != 9 || !wide(data, 20, "Parameter")
            || int(data, 118)? != 9 || !wide(data, 122, "InitColor") { return None; }
        let (width, height, cols, rows) = (int(data, 38)?, int(data, 42)?, int(data, 46)?, int(data, 50)?);
        if width == 0 || height == 0 || cols == 0 || rows == 0 || cols.saturating_mul(rows) > 100_000
            || u64::from(width) * u64::from(height) > 512 << 20 { return None; }
        let (alpha, color, bits) = (int(data, 58)?, int(data, 62)?, int(data, 86)?);
        let packing = match (alpha, color, bits) {
            (1, 4, n) if n != 32 => Packing::Rgba,
            (1, 1, 32) => Packing::BitGray,
            (1, 1, _) => Packing::Gray,
            (0, 1, 32) | (1, 0, 32) => Packing::BitAlpha,
            (0, 1, _) | (1, 0, _) => Packing::Alpha,
            _ => return None,
        };
        Some(Self { width, height, cols, rows, packing, fill: int(data, 144)? != 0 })
    }

    pub fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

#[derive(Clone)]
pub struct Level {
    pub geometry: Geometry,
    pub offset: u64,
    pub external: Vec<u8>,
    pub owner: i64,
}

fn external(file: &mut dyn ReadSeek, level: &Level) -> Result<Vec<u8>> {
    chunk(file, level.offset, &level.external)
}

/// Reads one `CHNKExta` payload after checking that its identifier matches `expected`.
pub fn chunk(file: &mut dyn ReadSeek, offset: u64, expected: &[u8]) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).map_err(error)?;
    let mut header = [0u8; 16];
    file.read_exact(&mut header).map_err(error)?;
    if &header[..8] != b"CHNKExta" { return Err("CLIP 외부 청크 헤더가 올바르지 않습니다.".into()); }
    let total = u64::from_be_bytes(header[8..16].try_into().unwrap());
    if total == 0 || total > LIMIT { return Err("CLIP 외부 청크 크기가 안전 제한을 초과합니다.".into()); }
    let mut size = [0; 8];
    file.read_exact(&mut size).map_err(error)?;
    let id_len = u64::from_be_bytes(size);
    if id_len != expected.len() as u64 || id_len > 1024 || id_len + 16 > total {
        return Err("CLIP 외부 청크 식별자 길이가 올바르지 않습니다.".into());
    }
    let mut id = vec![0; id_len as usize];
    file.read_exact(&mut id).map_err(error)?;
    if id != expected { return Err("CLIP 외부 청크 식별자가 일치하지 않습니다.".into()); }
    file.read_exact(&mut size).map_err(error)?;
    let count = u64::from_be_bytes(size);
    if count != total - 16 - id_len { return Err("CLIP 외부 청크 길이가 일치하지 않습니다.".into()); }
    let mut bytes = vec![0; count as usize];
    file.read_exact(&mut bytes).map_err(error)?;
    Ok(bytes)
}

fn blocks(mut data: &[u8], max: usize) -> Result<Vec<Option<&[u8]>>> {
    let mut result = Vec::with_capacity(max);
    let invalid = || "CLIP 레이어 타일 데이터가 손상되었습니다.".to_owned();
    while !data.is_empty() {
        if wide(data, 8, "BlockDataBeginChunk") {
            let len = int(data, 0).ok_or_else(invalid)? as usize;
            let block = data.get(..len).ok_or_else(invalid)?;
            let inner_end = len.checked_sub(38).ok_or_else(invalid)?;
            if int(block, inner_end) != Some(17) || !wide(block, inner_end + 4, "BlockDataEndChunk") {
                return Err(invalid());
            }
            let inner = block.get(46..inner_end).ok_or_else(invalid)?;
            match int(inner, 16) {
                Some(0) => result.push(None),
                Some(1) if int(inner, 20).is_some_and(|size| size as usize + 24 == inner.len()) => {
                    result.push(Some(inner.get(28..).ok_or_else(invalid)?));
                }
                _ => return Err(invalid()),
            }
            if result.len() > max { return Err(invalid()); }
            data = &data[len..];
        } else if int(data, 0) == Some(11) && wide(data, 4, "BlockStatus") {
            let size = int(data, 30).ok_or_else(invalid)? as usize;
            data = data.get(38 + size * 4..).ok_or_else(invalid)?;
        } else if int(data, 0) == Some(13) && wide(data, 4, "BlockCheckSum") {
            data = data.get(42 + result.len() * 4..).ok_or_else(invalid)?;
        } else {
            return Err(invalid());
        }
    }
    if result.len() != max { return Err(invalid()); }
    Ok(result)
}

#[derive(Clone, Copy)]
pub struct Placement {
    pub canvas: (u32, u32),
    pub offset: (f64, f64),
    pub color: [u8; 3],
}

pub fn read(file: &mut dyn ReadSeek, level: &Level, size: (u32, u32), place: Placement, mask: bool) -> Result<RgbaImage> {
    read_window(file, level, size, place, mask, [0, 0, size.0, size.1])
}

/// Reads only the output rectangle covered by the level, for small sprites on a large canvas.
/// Returns the pixels and their top-left position, or `None` when nothing lands on the canvas.
pub fn read_sprite(file: &mut dyn ReadSeek, level: &Level, size: (u32, u32), place: Placement) -> Result<Option<(RgbaImage, (u32, u32))>> {
    let g = level.geometry;
    let (sx, sy) = (size.0 as f64 / place.canvas.0.max(1) as f64, size.1 as f64 / place.canvas.1.max(1) as f64);
    let edge = |origin: f64, extent: u32, scale: f64, limit: u32| [
        (origin * scale).round().clamp(0., limit as f64) as u32,
        ((origin + extent as f64) * scale).round().clamp(0., limit as f64) as u32,
    ];
    let [l, r] = edge(place.offset.0, g.width, sx, size.0);
    let [t, b] = edge(place.offset.1, g.height, sy, size.1);
    if l >= r || t >= b { return Ok(None); }
    Ok(Some((read_window(file, level, size, place, false, [l, t, r, b])?, (l, t))))
}

fn read_window(file: &mut dyn ReadSeek, level: &Level, size: (u32, u32), place: Placement, mask: bool, window: [u32; 4]) -> Result<RgbaImage> {
    let g = level.geometry;
    let chunk = external(file, level)?;
    let tiles = blocks(&chunk, g.cols as usize * g.rows as usize)?;
    let expected = match g.packing {
        Packing::Rgba => PLANE * 5, Packing::Gray => PLANE * 2, Packing::BitGray => PLANE / 4,
        Packing::Alpha => PLANE, Packing::BitAlpha => PLANE / 8,
    };
    if mask && !matches!(g.packing, Packing::Alpha | Packing::BitAlpha) {
        return Err("CLIP 마스크 픽셀 형식을 지원하지 않습니다.".into());
    }
    let (w, h) = size;
    let (sx, sy) = (w as f64 / place.canvas.0.max(1) as f64, h as f64 / place.canvas.1.max(1) as f64);
    let (ox, oy) = place.offset;
    let [wl, wt, wr, wb] = window;
    let mut output = RgbaImage::from_pixel(wr - wl, wb - wt, Rgba([0, 0, 0, if mask && g.fill { 255 } else { 0 }]));
    let rect = |x0: u32, y0: u32, x1: u32, y1: u32| [
        ((ox + x0 as f64) * sx).round().clamp(0., w as f64).clamp(wl as f64, wr as f64) as u32,
        ((oy + y0 as f64) * sy).round().clamp(0., h as f64).clamp(wt as f64, wb as f64) as u32,
        ((ox + x1 as f64) * sx).round().clamp(0., w as f64).clamp(wl as f64, wr as f64) as u32,
        ((oy + y1 as f64) * sy).round().clamp(0., h as f64).clamp(wt as f64, wb as f64) as u32,
    ];
    if g.fill && !mask {
        let [l, t, r, b] = rect(0, 0, g.width, g.height);
        let [r0, g0, b0] = match g.packing { Packing::Alpha | Packing::BitAlpha => place.color, _ => [255; 3] };
        for y in t..b { for x in l..r { output.put_pixel(x - wl, y - wt, Rgba([r0, g0, b0, 255])); } }
    }
    struct Job<'a> { tile: &'a [u8], origin: (u32, u32), end: (u32, u32), rect: [u32; 4] }
    let jobs: Vec<Job<'_>> = tiles.into_iter().enumerate().filter_map(|(index, tile)| {
        let tile = tile?;
        let origin = ((index as u32 % g.cols) * TILE, (index as u32 / g.cols) * TILE);
        let end = ((origin.0 + TILE).min(g.width), (origin.1 + TILE).min(g.height));
        if origin.0 >= end.0 || origin.1 >= end.1 { return None; }
        let rect = rect(origin.0, origin.1, end.0, end.1);
        (rect[0] < rect[2] && rect[1] < rect[3]).then_some(Job { tile, origin, end, rect })
    }).collect();
    let inflate = |job: &Job<'_>| -> Result<Vec<u8>> {
        let pixels = read_limited(ZlibDecoder::new(job.tile), expected as u64)?;
        if pixels.len() != expected { return Err("CLIP 레이어 타일의 픽셀 길이가 일치하지 않습니다.".into()); }
        Ok(pixels)
    };
    // Inflation dominates layer decoding and tiles are independent. Batches bound the
    // decompressed bytes held at once; placement stays sequential and deterministic.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
    let stride = (wr - wl) as usize * 4;
    let raw: &mut [u8] = &mut output;
    for batch in jobs.chunks(threads * 4) {
        let decoded: Vec<Result<Vec<u8>>> = if threads == 1 || batch.len() < 2 {
            batch.iter().map(inflate).collect()
        } else {
            std::thread::scope(|scope| {
                let workers: Vec<_> = batch.chunks(batch.len().div_ceil(threads))
                    .map(|part| scope.spawn(move || part.iter().map(inflate).collect::<Vec<_>>()))
                    .collect();
                workers.into_iter().flat_map(|worker| worker.join().unwrap_or_else(|_| vec![Err("CLIP 타일 해제 작업이 중단되었습니다.".into())])).collect()
            })
        };
        for (job, pixels) in batch.iter().zip(decoded) {
            let pixels = pixels?;
            let [l, t, r, b] = job.rect;
            let (tx, ty) = job.origin;
            let (right, bottom) = job.end;
            let columns: Vec<usize> = (l..r).map(|x| {
                ((((x as f64 + 0.5) / sx - ox).floor() as i64).clamp(tx as i64, right as i64 - 1) as u32 - tx) as usize
            }).collect();
            let bit = |plane: usize, n: usize| ((pixels[plane + (n >> 3)] >> (7 - (n & 7))) & 1) * 255;
            for y in t..b {
                let source_y = (((y as f64 + 0.5) / sy - oy).floor() as i64).clamp(ty as i64, bottom as i64 - 1) as u32 - ty;
                let row = source_y as usize * TILE as usize;
                let begin = (y - wt) as usize * stride + (l - wl) as usize * 4;
                let line = &mut raw[begin..begin + columns.len() * 4];
                for (out, &column) in line.chunks_exact_mut(4).zip(&columns) {
                    let p = row + column;
                    let rgba = match g.packing {
                        Packing::Rgba => {
                            let rgb = PLANE + p * 4;
                            [pixels[rgb + 2], pixels[rgb + 1], pixels[rgb], pixels[p]]
                        }
                        Packing::Gray => { let gray = pixels[PLANE + p]; [gray, gray, gray, pixels[p]] }
                        Packing::BitGray => { let gray = bit(PLANE / 8, p); [gray, gray, gray, bit(0, p)] }
                        Packing::Alpha => [place.color[0], place.color[1], place.color[2], pixels[p]],
                        Packing::BitAlpha => [place.color[0], place.color[1], place.color[2], bit(0, p)],
                    };
                    out.copy_from_slice(&rgba);
                }
            }
        }
    }
    Ok(output)
}
