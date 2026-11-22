mod tile;
mod vector;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, BufReader, Cursor, Read, SeekFrom},
};

use clipview_core::{BlendMode, composite_rgba};
use image::{ImageReader, Rgba, RgbaImage};
use rusqlite::{Connection, OptionalExtension, Row, types::{FromSql, ValueRef}};
use serde_json::{Value, json};

use crate::{
    Result, asset::Assets,
    collection::{Crop, Item, integer, number, read_only_database},
    error, picture,
};
use tile::{Geometry, Level, Placement, ReadSeek};

const SQLITE: &[u8] = b"SQLite format 3\0";
const MAX_DATABASE: u64 = 256 << 20;
const MAX_PREVIEW: i64 = 96 << 20;
const MAX_LAYERS: usize = 8_192;
const MAX_COMPOSITE_BYTES: u64 = 1 << 30;
/// Composite pixel budget. Larger canvases are shown at 1/2, 1/4, ... of their size so every
/// output pixel maps onto one stored mipmap level instead of being resampled.
const MAX_DISPLAY_PIXELS: u64 = 12 * 1024 * 1024;

fn open_source(item: &Item) -> Result<Box<dyn ReadSeek>> {
    match item {
        Item::ArchiveEntry { .. } => Ok(Box::new(Cursor::new(item.read(128 << 20)?))),
        _ => {
            let file = File::open(item.disk_path()).map_err(error)?;
            if file.metadata().map_err(error)?.len() > 2 << 30 {
                return Err("CLIP 크기가 안전 제한을 초과합니다.".into());
            }
            Ok(Box::new(file))
        }
    }
}

fn sqlite_size(header: &[u8; 100], remaining: u64) -> Option<u64> {
    let page = u16::from_be_bytes([header[16], header[17]]);
    let page = if page == 1 { 65_536 } else { u64::from(page) };
    let count = u64::from(tile::int(header, 28)?);
    let bytes = page * count;
    (header.starts_with(SQLITE) && (512..=65_536).contains(&page) && page.is_power_of_two()
        && bytes != 0 && bytes <= remaining && bytes <= MAX_DATABASE).then_some(bytes)
}

fn indexed_database(source: &mut dyn ReadSeek, len: u64) -> io::Result<Option<(u64, u64)>> {
    // Only trust the pointer when the container, chunk, SQLite size and footer agree.
    let mut head = [0; 56];
    if len < head.len() as u64 { return Ok(None); }
    source.seek(SeekFrom::Start(0))?;
    source.read_exact(&mut head)?;
    if &head[..8] != b"CSFCHUNK" || head[8..16] != len.to_be_bytes()
        || head[16..24] != 24u64.to_be_bytes() || &head[24..32] != b"CHNKHead"
        || head[32..40] != 40u64.to_be_bytes()
    { return Ok(None); }
    let offset = u64::from_be_bytes(head[48..56].try_into().unwrap());
    if offset < 80 || len.saturating_sub(offset) < 132 { return Ok(None); }
    source.seek(SeekFrom::Start(offset))?;
    let mut chunk = [0; 16];
    let mut header = [0; 100];
    source.read_exact(&mut chunk)?;
    source.read_exact(&mut header)?;
    let start = offset + 16;
    let Some(bytes) = sqlite_size(&header, len - start) else { return Ok(None) };
    if &chunk[..8] != b"CHNKSQLi" || chunk[8..] != bytes.to_be_bytes() || len - start - bytes != 16 {
        return Ok(None);
    }
    source.seek(SeekFrom::Start(start + bytes))?;
    let mut footer = [0; 16];
    source.read_exact(&mut footer)?;
    Ok((&footer == b"CHNKFoot\0\0\0\0\0\0\0\0").then_some((start, bytes)))
}

fn scan_database(source: &mut dyn ReadSeek, len: u64) -> Result<(u64, u64)> {
    let mut end = len;
    let block_size = len.min(1 << 20) as usize;
    let mut block = vec![0; block_size + SQLITE.len()];
    let mut overlap = 0;
    let db_start = loop {
        if end == 0 { return Err("CLIP 파일에서 SQLite 미리보기 영역을 찾지 못했습니다.".into()); }
        let start = end.saturating_sub(block_size as u64);
        let count = (end - start) as usize;
        if overlap != 0 { block.copy_within(block_size..block_size + overlap, count); }
        source.seek(SeekFrom::Start(start)).map_err(error)?;
        source.read_exact(&mut block[..count]).map_err(error)?;
        if let Some(found) = memchr::memmem::rfind(&block[..count + overlap], SQLITE) { break start + found as u64; }
        overlap = (SQLITE.len() - 1).min(count);
        block.copy_within(0..overlap, block_size);
        end = start;
    };
    let mut header = [0; 100];
    source.seek(SeekFrom::Start(db_start)).map_err(error)?;
    source.read_exact(&mut header).map_err(error)?;
    let bytes = sqlite_size(&header, len - db_start)
        .ok_or("CLIP SQLite 영역 크기가 올바르지 않거나 안전 제한을 초과합니다.")?;
    Ok((db_start, bytes))
}

fn database(source: &mut dyn ReadSeek) -> Result<(Connection, tempfile::NamedTempFile)> {
    let len = source.seek(SeekFrom::End(0)).map_err(error)?;
    let (db_start, bytes) = match indexed_database(source, len).map_err(error)? {
        Some(location) => location,
        None => scan_database(source, len)?,
    };
    source.seek(SeekFrom::Start(db_start)).map_err(error)?;
    let mut tmp = tempfile::NamedTempFile::new().map_err(error)?;
    let mut input = BufReader::with_capacity(bytes.min(128 << 10) as usize, source.take(bytes));
    if io::copy(&mut input, &mut tmp).map_err(error)? != bytes {
        return Err("CLIP SQLite 영역이 잘렸습니다.".into());
    }
    Ok((read_only_database(tmp.path())?, tmp))
}

fn preview(db: &Connection) -> Result<Vec<u8>> {
    db.query_row(
        "SELECT ImageData FROM CanvasPreview WHERE length(ImageData) BETWEEN 1 AND ?1 LIMIT 1",
        [MAX_PREVIEW], |r| r.get(0),
    ).map_err(|e| format!("CLIP 파일의 저장된 미리보기를 읽을 수 없습니다: {e}"))
}

pub fn read_preview(item: &Item) -> Result<Vec<u8>> {
    let mut source = open_source(item)?;
    let (_db, _file) = database(&mut *source)?;
    preview(&_db)
}

fn table_exists(db: &Connection, table: &str) -> Result<bool> {
    db.query_row("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1", [table], |_| Ok(()))
        .optional().map(|x| x.is_some()).map_err(error)
}

fn crop(r: &Row<'_>) -> Option<Crop> {
    let [cw, ch, tw, th] = ["CanvasWidth", "CanvasHeight", "CropFrameWidth", "CropFrameHeight"].map(|k| number(r, k));
    if integer(r, "CropFrameShow") == 0 || [cw, ch, tw, th].iter().any(|v| *v <= 0.) { return None; }
    let offset = |new, old| r.get::<_, Option<f64>>(new).ok().flatten().unwrap_or(number(r, old));
    Some(Crop {
        canvas_width: cw, canvas_height: ch, trim_width: tw, trim_height: th,
        bleed: number(r, "CropFrameDitch").max(0.),
        offset_x: offset("CropFrameCropOffsetX", "CropFrameOffsetX"),
        offset_y: offset("CropFrameCropOffsetY", "CropFrameOffsetY"),
        page_count: if cw > ch && tw < th { 2. } else { 1. },
    })
}

fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let dims = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(error)?.into_dimensions().map_err(error)?;
    picture::check_dimensions(dims.0, dims.1)?;
    Ok(dims)
}

#[derive(Default)]
struct Canvas {
    root: i64,
    width: f64,
    height: f64,
    dpi: f64,
    unit: i64,
    crop: Option<Crop>,
}

fn canvas(db: &Connection) -> Result<Canvas> {
    if !table_exists(db, "Canvas")? { return Ok(Canvas::default()); }
    Ok(db.query_row("SELECT * FROM Canvas LIMIT 1", [], |r| Ok(Canvas {
        root: integer(r, "CanvasRootFolder"), width: number(r, "CanvasWidth"), height: number(r, "CanvasHeight"),
        dpi: number(r, "CanvasResolution"), unit: integer(r, "CanvasUnit"), crop: crop(r),
    })).optional().map_err(error)?.unwrap_or_default())
}

#[derive(Clone, Copy)]
struct Edge { width: f64, color: [u8; 3] }

#[derive(Clone, Copy, Debug, PartialEq)]
/// Tone (halftone) layer effect. `EffectTone` stores `[enabled u32][resolution f64][dot kind u32]
/// [density source u32][density % u32][frequency f64][u32][u32][angle u32]` (byte offsets 0, 4, 12,
/// 16, 20, 24, 32, 36, 40).
struct Tone { frequency: f64, angle: u32, resolution: f64, enabled: bool, kind: u32, source: u32, density: f64 }

#[derive(Clone, Copy, Debug, PartialEq)]
struct Stop { color: [u8; 3], opacity: u8, position: f64 }

#[derive(Clone, Debug, PartialEq)]
enum Fill {
    Flat([u8; 3]),
    Gradient { stops: Vec<Stop>, repeat: u32, shape: u32, start: (f64, f64), end: (f64, f64) },
}

/// Big-endian reader for `GradationFillInfo`, mirroring the reference `parseClipGradient`.
struct FillReader<'a> { data: &'a [u8], at: usize }

impl FillReader<'_> {
    fn int(&mut self) -> Option<u32> {
        let value = tile::int(self.data, self.at)?;
        self.at += 4;
        Some(value)
    }

    fn double(&mut self) -> Option<f64> {
        let value = tile::double(self.data, self.at)?;
        self.at += 8;
        Some(value)
    }

    fn channel(&mut self) -> Option<u8> { Some((self.int()? >> 24) as u8) }

    fn name(&mut self) -> Option<Option<String>> {
        if self.at + 4 > self.data.len() { return Some(None); }
        let count = self.int()? as usize;
        let bytes = self.data.get(self.at..self.at.checked_add(count.checked_mul(2)?)?)?;
        self.at += bytes.len();
        let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
        Some(Some(String::from_utf16_lossy(&units)))
    }
}

fn parse_fill(data: &[u8]) -> Option<Fill> {
    if data.is_empty() { return None; }
    let mut r = FillReader { data, at: 0 };
    r.int()?;
    r.int()?;
    let (mut stops, mut geometry, mut flat) = (Vec::new(), None, None);
    while r.at < data.len() {
        let Some(name) = r.name()? else { break };
        match name.as_str() {
            "GradationData" => {
                let size = r.int()? as usize;
                let end = r.at + size;
                r.int()?;
                r.int()?;
                let count = r.int()? as usize;
                r.int()?;
                if count > 256 || end > data.len() { break; }
                if end.checked_sub(r.at) == Some(8 + count * 28) { r.int()?; r.int()?; }
                stops.clear();
                for _ in 0..count {
                    let color = [r.channel()?, r.channel()?, r.channel()?];
                    let opacity = r.channel()?;
                    r.int()?;
                    let position = (f64::from(r.int()?) / 32_768.).clamp(0., 1.);
                    let curves = r.int()? as usize;
                    if curves > 4096 || r.at + curves * 16 > end { break; }
                    r.at += curves * 16;
                    stops.push(Stop { color, opacity, position });
                }
                r.at = r.at.max(end);
            }
            "GradationSettingAdd0001" => {
                let size = r.int()? as usize;
                let end = r.at + size;
                if r.int()? != 0 { flat = Some([r.channel()?, r.channel()?, r.channel()?]); }
                r.at = r.at.max(end);
            }
            "GradationSetting" => {
                let (repeat, shape) = (r.int()?, r.int()?);
                r.int()?;
                for _ in 0..3 { r.double()?; }
                let start = (r.double()?, r.double()?);
                let end = (r.double()?, r.double()?);
                geometry = Some((repeat, shape, start, end));
            }
            _ => {
                if r.at + 4 > data.len() { break; }
                let size = r.int()? as usize;
                if size > data.len() - r.at { break; }
                r.at += size;
            }
        }
    }
    if let Some(color) = flat { return Some(Fill::Flat(color)); }
    let (repeat, shape, start, end) = geometry?;
    (!stops.is_empty()).then_some(Fill::Gradient { stops, repeat, shape, start, end })
}

/// Paints a fill layer at the output size. Gradient coordinates are stored in native canvas pixels.
fn render_fill(fill: &Fill, size: (u32, u32), native: (u32, u32)) -> RgbaImage {
    let Fill::Gradient { stops, repeat, shape, start, end } = fill else {
        let Fill::Flat([r, g, b]) = fill else { unreachable!() };
        return RgbaImage::from_pixel(size.0, size.1, Rgba([*r, *g, *b, 255]));
    };
    let scale = (size.0 as f64 / native.0.max(1) as f64, size.1 as f64 / native.1.max(1) as f64);
    let (sx, sy) = (start.0 * scale.0, start.1 * scale.1);
    let (dx, dy) = (end.0 * scale.0 - sx, end.1 * scale.1 - sy);
    let length = dx * dx + dy * dy;
    let radius = length.sqrt().max(1.);
    // SVG offsets never decrease; a stop before its predecessor takes the predecessor's offset.
    let mut stops = stops.clone();
    for i in 1..stops.len() { stops[i].position = stops[i].position.max(stops[i - 1].position); }
    let sample = |t: f64| -> [u8; 4] {
        let first = stops[0];
        let last = stops[stops.len() - 1];
        let pick = |s: Stop| [s.color[0], s.color[1], s.color[2], s.opacity];
        if t <= first.position { return pick(first); }
        if t >= last.position { return pick(last); }
        let upper = stops.iter().position(|s| s.position >= t).unwrap_or(stops.len() - 1);
        let (a, b) = (stops[upper - 1], stops[upper]);
        let span = b.position - a.position;
        let k = if span > 0. { (t - a.position) / span } else { 1. };
        let mix = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * k).round() as u8;
        [mix(a.color[0], b.color[0]), mix(a.color[1], b.color[1]), mix(a.color[2], b.color[2]), mix(a.opacity, b.opacity)]
    };
    RgbaImage::from_fn(size.0, size.1, |x, y| {
        let (px, py) = (x as f64 + 0.5 - sx, y as f64 + 0.5 - sy);
        let raw = if *shape == 0 {
            // A zero-length linear gradient paints its last stop, as SVG specifies.
            if length == 0. { 1. } else { (px * dx + py * dy) / length }
        } else { (px * px + py * py).sqrt() / radius };
        let t = match repeat {
            1 => raw - raw.floor(),
            2 => { let m = raw.rem_euclid(2.); if m > 1. { 2. - m } else { m } }
            _ => raw.clamp(0., 1.),
        };
        Rgba(sample(t))
    })
}

/// Halftone conversion matching the reference `applyClipToneEffect`.
fn apply_tone(image: &mut RgbaImage, tone: Tone, size: (u32, u32), native: (u32, u32), dpi: f64) {
    let scale = (size.0 as f64 / native.0.max(1) as f64).min(size.1 as f64 / native.1.max(1) as f64);
    let resolution = if tone.resolution != 0. { tone.resolution } else if dpi > 0. { dpi } else { 72. };
    let cell = (resolution * scale / tone.frequency).max(1.);
    let (sine, cosine) = f64::from(tone.angle).to_radians().sin_cos();
    let transition = (0.5 / cell).clamp(0.08, 0.5);
    let tau = std::f64::consts::TAU;
    // Rank of each spot value within a cell, so dot area grows linearly with density like CSP's
    // tones (the raw cosine spot would darken shadows and lighten highlights).
    const STEPS: usize = 1024;
    static RANK: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    let rank = RANK.get_or_init(|| {
        let mut histogram = vec![0u32; STEPS + 1];
        let n = 256;
        for j in 0..n {
            for i in 0..n {
                let (a, b) = ((i as f64 + 0.5) / n as f64 * tau, (j as f64 + 0.5) / n as f64 * tau);
                histogram[(((2. - a.cos() - b.cos()) / 4.) * STEPS as f64).round() as usize] += 1;
            }
        }
        let mut total = 0u32;
        histogram.iter().map(|&count| { let below = total; total += count; (f64::from(below) + f64::from(count) / 2.) / f64::from(n * n) }).collect()
    });
    let width = image.width() as usize;
    // Phases of the four sub-samples advance by a fixed step per pixel, so the cosines are
    // updated by rotation instead of evaluated; they are recomputed exactly every 256 pixels.
    let phase = |px: f64, py: f64| (((px * cosine + py * sine) / cell) * tau, ((-px * sine + py * cosine) / cell) * tau);
    let (step_a, step_b) = (cosine / cell * tau, -sine / cell * tau);
    let (rotate_a, rotate_b) = ((step_a.cos(), step_a.sin()), (step_b.cos(), step_b.sin()));
    const OFFSETS: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    let shade = |y: u32, line: &mut [u8]| {
        let mut rotors = [[(0f64, 0f64); 2]; 4];
        for (x, pixel) in line.chunks_exact_mut(4).enumerate() {
            if x % 256 == 0 {
                for (rotor, (ox, oy)) in rotors.iter_mut().zip(OFFSETS) {
                    let (a, b) = phase(x as f64 + ox, f64::from(y) + oy);
                    *rotor = [(a.cos(), a.sin()), (b.cos(), b.sin())];
                }
            } else {
                for rotor in &mut rotors {
                    for (value, (c, s)) in rotor.iter_mut().zip([rotate_a, rotate_b]) {
                        *value = (value.0 * c - value.1 * s, value.1 * c + value.0 * s);
                    }
                }
            }
            let alpha = f64::from(pixel[3]) / 255.;
            if alpha == 0. { continue; }
            let luminance = (f64::from(pixel[0]) * 0.2126 + f64::from(pixel[1]) * 0.7152 + f64::from(pixel[2]) * 0.0722) / 255.;
            // Source 2 uses the set density instead of the image brightness.
            let density = (if tone.source == 2 { tone.density } else { 1. - luminance } * alpha).clamp(0., 1.);
            let mut dot = 0.;
            for rotor in &rotors {
                let spot = rank[(((2. - rotor[0].0 - rotor[1].0) / 4.).clamp(0., 1.) * STEPS as f64).round() as usize];
                dot += ((density - spot) / transition + 0.5).clamp(0., 1.);
            }
            pixel.copy_from_slice(&[0, 0, 0, (255. * dot / 4. * alpha).round() as u8]);
        }
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
    let rows = (image.height() as usize).div_ceil(threads).max(1);
    let raw: &mut [u8] = image;
    std::thread::scope(|scope| {
        for (band, lines) in raw.chunks_mut(rows * width * 4).enumerate() {
            scope.spawn(move || for (index, line) in lines.chunks_exact_mut(width * 4).enumerate() {
                shade((band * rows + index) as u32, line);
            });
        }
    });
}

/// Layer property "expression colour" applied when displaying a colour layer
/// (`UsePreviewColorType`): grey, or monochrome with a threshold and black/white switches.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PreviewColor { monochrome: bool, black: bool, white: bool, threshold: f64, alpha_threshold: f64 }

fn apply_preview_color(image: &mut RgbaImage, mode: PreviewColor) {
    let threshold = (mode.threshold * 255.).round() as u32;
    let alpha_threshold = (mode.alpha_threshold * 255.).round() as u8;
    for p in image.pixels_mut() {
        if p[3] == 0 { continue; }
        let luma = (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2]) + 500) / 1000;
        if mode.monochrome {
            let dark = luma < threshold.max(1);
            if alpha_threshold > 0 { p[3] = if p[3] >= alpha_threshold { 255 } else { 0 }; }
            let keep = if dark { mode.black } else { mode.white };
            let v = if dark { 0 } else { 255 };
            *p = Rgba([v, v, v, if keep { p[3] } else { 0 }]);
        } else {
            let v = luma as u8;
            *p = Rgba([v, v, v, p[3]]);
        }
    }
}

/// Correction layer settings from `FilterLayerInfo`: `[kind u32][size u32][payload]`.
#[derive(Clone, Debug, PartialEq)]
enum Correction {
    /// Brightness and contrast, -100..=100.
    BrightnessContrast(f64, f64),
    /// RGB master, red, green and blue curves; points in 0..=1.
    ToneCurve(Vec<Vec<(f64, f64)>>),
    /// Hue shift in degrees, saturation and luminosity in -100..=100.
    HueSaturation(f64, f64, f64),
    /// Levels for the master, red, green and blue channels:
    /// `[input black, input midtone, input white, output black, output white]` in 0..=1.
    Levels(Vec<[f64; 5]>),
    /// Gradient map stops: position 0..=1 and RGBA colour, sorted by position.
    GradientMap(Vec<(f64, [f64; 4])>),
}

fn parse_correction(data: &[u8]) -> Option<Correction> {
    let (kind, len) = (tile::int(data, 0)?, tile::int(data, 4)? as usize);
    let body = data.get(8..8usize.checked_add(len)?)?;
    let int = |i: usize| tile::int(body, i * 4).map(|v| f64::from(v as i32));
    match kind {
        1 => Some(Correction::BrightnessContrast(int(0)?.clamp(-100., 100.), int(1)?.clamp(-100., 100.))),
        2 => {
            // Ten-byte records of five u16 values; master first, then red, green and blue.
            let records = (0..4).map(|k| {
                let at = k * 10;
                let word = |i: usize| body.get(at + i * 2..at + i * 2 + 2).map(|b| f64::from(u16::from_be_bytes([b[0], b[1]])) / 65535.);
                Some([word(0)?, word(1)?, word(2)?, word(3)?, word(4)?])
            }).collect::<Option<Vec<_>>>()?;
            Some(Correction::Levels(records))
        }
        3 => {
            // Four 1040-byte blocks (master, R, G, B): u16 point count, then u16 (x, y) pairs.
            let curves = (0..4).map(|k| {
                let block = body.get(k * 1040..k * 1040 + 1040)?;
                let count = usize::from(u16::from_be_bytes([block[0], block[1]]));
                if !(2..=256).contains(&count) { return None; }
                let word = |at: usize| f64::from(u16::from_be_bytes([block[at], block[at + 1]])) / 65535.;
                Some((0..count).map(|j| (word(2 + 4 * j), word(4 + 4 * j))).collect::<Vec<_>>())
            }).collect::<Option<Vec<_>>>()?;
            Some(Correction::ToneCurve(curves))
        }
        4 => Some(Correction::HueSaturation(int(0)?.clamp(-180., 180.), int(1)?.clamp(-100., 100.), int(2)?.clamp(-100., 100.))),
        9 => {
            // [size][header size][record size][count]...; records are
            // [kind][R][G][B][A][flags][position 0..=32768], 16-bit colours stored twice per word.
            let (header, record, count) = (tile::int(body, 4)? as usize, tile::int(body, 8)? as usize, tile::int(body, 12)? as usize);
            if record < 28 || !(1..=256).contains(&count) { return None; }
            let mut stops = (0..count).map(|k| {
                let at = header.checked_add(k.checked_mul(record)?)?;
                let word = |i: usize| tile::int(body, at + i * 4);
                let colour = [1, 2, 3, 4].map(|i| word(i).map(|v| f64::from(v >> 16) / 65535.));
                Some((f64::from(word(6)?) / 32768., [colour[0]?, colour[1]?, colour[2]?, colour[3]?]))
            }).collect::<Option<Vec<_>>>()?;
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            Some(Correction::GradientMap(stops))
        }
        _ => None,
    }
}

/// 8-bit lookup table of a natural cubic spline through the curve points.
fn curve_lut(points: &[(f64, f64)]) -> [u8; 256] {
    let mut pts: Vec<(f64, f64)> = points.to_vec();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
    let n = pts.len();
    let mut lut = [0u8; 256];
    if n < 2 { for (i, v) in lut.iter_mut().enumerate() { *v = i as u8; } return lut; }
    // Second derivatives (natural boundary) by the tridiagonal algorithm.
    let mut second = vec![0f64; n];
    let mut u = vec![0f64; n];
    for i in 1..n - 1 {
        let sig = (pts[i].0 - pts[i - 1].0) / (pts[i + 1].0 - pts[i - 1].0);
        let p = sig * second[i - 1] + 2.;
        second[i] = (sig - 1.) / p;
        let d = (pts[i + 1].1 - pts[i].1) / (pts[i + 1].0 - pts[i].0) - (pts[i].1 - pts[i - 1].1) / (pts[i].0 - pts[i - 1].0);
        u[i] = (6. * d / (pts[i + 1].0 - pts[i - 1].0) - sig * u[i - 1]) / p;
    }
    for i in (0..n - 1).rev() { second[i] = second[i] * second[i + 1] + u[i]; }
    second[n - 1] = 0.;
    for (i, out) in lut.iter_mut().enumerate() {
        let x = i as f64 / 255.;
        let y = if x <= pts[0].0 { pts[0].1 } else if x >= pts[n - 1].0 { pts[n - 1].1 } else {
            let k = pts.windows(2).position(|w| x <= w[1].0).unwrap_or(n - 2);
            let (a, b) = (pts[k], pts[k + 1]);
            let h = b.0 - a.0;
            let (ta, tb) = ((b.0 - x) / h, (x - a.0) / h);
            ta * a.1 + tb * b.1 + ((ta.powi(3) - ta) * second[k] + (tb.powi(3) - tb) * second[k + 1]) * h * h / 6.
        };
        *out = (y.clamp(0., 1.) * 255.).round() as u8;
    }
    lut
}

fn hue_rgb(p: f64, q: f64, t: f64) -> f64 {
    let t = t.rem_euclid(1.);
    if t < 1. / 6. { p + (q - p) * 6. * t } else if t < 0.5 { q } else if t < 2. / 3. { p + (q - p) * (2. / 3. - t) * 6. } else { p }
}

/// The colours of `image` with a correction applied at full strength (alpha unchanged, except a
/// gradient map's stop opacity). The caller blends it over the original with the layer's mode.
fn corrected(image: &RgbaImage, correction: &Correction) -> RgbaImage {
    let lut = |f: &dyn Fn(f64) -> f64| -> [u8; 256] { std::array::from_fn(|i| (f(i as f64 / 255.).clamp(0., 1.) * 255.).round() as u8) };
    let mut output = image.clone();
    match correction {
        Correction::BrightnessContrast(brightness, contrast) => {
            let (b, c) = (brightness / 100., contrast / 100.);
            let slant = ((c + 1.) * std::f64::consts::FRAC_PI_4).tan();
            let table = lut(&|v| (if b < 0. { v * (1. + b) } else { v + (1. - v) * b } - 0.5) * slant + 0.5);
            for p in output.pixels_mut() { for ch in 0..3 { p[ch] = table[usize::from(p[ch])]; } }
        }
        Correction::ToneCurve(curves) => {
            let master = curve_lut(&curves[0]);
            let tables: [[u8; 256]; 3] = std::array::from_fn(|ch| {
                let channel = curve_lut(&curves[ch + 1]);
                std::array::from_fn(|i| master[usize::from(channel[i])])
            });
            for p in output.pixels_mut() { for ch in 0..3 { p[ch] = tables[ch][usize::from(p[ch])]; } }
        }
        Correction::Levels(records) => {
            let level = |r: &[f64; 5]| -> [u8; 256] {
                let [black, mid, white, out_black, out_white] = *r;
                let range = (white - black).max(1e-6);
                let middle = ((mid - black) / range).clamp(0.01, 0.99);
                let exponent = 0.5f64.ln() / middle.ln();
                lut(&|v| out_black + (out_white - out_black) * ((v - black) / range).clamp(0., 1.).powf(exponent))
            };
            let master = level(&records[0]);
            let tables: [[u8; 256]; 3] = std::array::from_fn(|ch| {
                let channel = level(&records[ch + 1]);
                std::array::from_fn(|i| master[usize::from(channel[i])])
            });
            for p in output.pixels_mut() { for ch in 0..3 { p[ch] = tables[ch][usize::from(p[ch])]; } }
        }
        Correction::GradientMap(stops) => {
            let table: [[u8; 4]; 256] = std::array::from_fn(|i| {
                let x = i as f64 / 255.;
                let k = stops.iter().position(|s| s.0 >= x).unwrap_or(stops.len());
                let colour = if k == 0 { stops[0].1 } else if k == stops.len() { stops[k - 1].1 } else {
                    let (a, b) = (stops[k - 1], stops[k]);
                    let t = if b.0 > a.0 { (x - a.0) / (b.0 - a.0) } else { 1. };
                    std::array::from_fn(|ch| a.1[ch] + (b.1[ch] - a.1[ch]) * t)
                };
                colour.map(|v| (v.clamp(0., 1.) * 255.).round() as u8)
            });
            for p in output.pixels_mut() {
                let luma = (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2]) + 500) / 1000;
                let [r, g, b, a] = table[luma as usize];
                *p = Rgba([r, g, b, ((u16::from(p[3]) * u16::from(a) + 127) / 255) as u8]);
            }
        }
        Correction::HueSaturation(hue, saturation, luminosity) => {
            let mut cache: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
            for p in output.pixels_mut() {
                if p[3] == 0 { continue; }
                let key = [p[0], p[1], p[2]];
                let value = *cache.entry(key).or_insert_with(|| {
                    let [r, g, b] = key.map(|v| f64::from(v) / 255.);
                    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
                    let mut l = (max + min) / 2.;
                    let mut s = if max == min { 0. } else if l > 0.5 { (max - min) / (2. - max - min) } else { (max - min) / (max + min) };
                    let mut h = if max == min { 0. } else if max == r { ((g - b) / (max - min)).rem_euclid(6.) / 6. }
                        else if max == g { ((b - r) / (max - min) + 2.) / 6. } else { ((r - g) / (max - min) + 4.) / 6. };
                    h += hue / 360.;
                    s = if *saturation < 0. { s * (1. + saturation / 100.) } else { s + (1. - s) * saturation / 100. };
                    l = if *luminosity < 0. { l * (1. + luminosity / 100.) } else { l + (1. - l) * luminosity / 100. };
                    let (s, l) = (s.clamp(0., 1.), l.clamp(0., 1.));
                    let rgb = if s == 0. { [l; 3] } else {
                        let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
                        let p = 2. * l - q;
                        [hue_rgb(p, q, h + 1. / 3.), hue_rgb(p, q, h), hue_rgb(p, q, h - 1. / 3.)]
                    };
                    rgb.map(|v| (v.clamp(0., 1.) * 255.).round() as u8)
                });
                p[0] = value[0]; p[1] = value[1]; p[2] = value[2];
            }
        }
    }
    output
}

/// Recolours `target` like a CSP correction layer: the corrected copy is masked and blended back
/// with the layer's blend mode and opacity, keeping the target's coverage.
fn apply_correction(target: &mut RgbaImage, correction: &Correction, blend_mode: u8, strength: f64, mask: Option<&RgbaImage>) -> Result<()> {
    let mut fixed = corrected(target, correction);
    if let Some(mask) = mask {
        for (pixel, m) in fixed.pixels_mut().zip(mask.pixels()) { pixel[3] = ((u16::from(pixel[3]) * u16::from(m[3]) + 127) / 255) as u8; }
    }
    blend(target, &fixed, blend_mode, strength, true)
}

/// Low-resolution stored layer thumbnail stretched over the canvas, for layers (such as 3D)
/// without any independent pixel data. The thumbnail is drawn at the canvas aspect ratio.
fn thumbnail_layer(source: &mut dyn ReadSeek, level: &Level, frame: Frame<'_>) -> Result<RgbaImage> {
    let canvas = (level.geometry.width, level.geometry.height);
    let image = tile::read(source, level, canvas, Placement { canvas, offset: (0., 0.), color: [0; 3] }, false)?;
    let ratio = f64::from(frame.native.0) / f64::from(frame.native.1.max(1));
    let (w, h) = (f64::from(image.width()), f64::from(image.height()));
    let (cw, ch) = if w / h > ratio { ((h * ratio).round().max(1.), h) } else { (w, (w / ratio).round().max(1.)) };
    let (left, top) = (((w - cw) / 2.).floor() as u32, ((h - ch) / 2.).floor() as u32);
    let cropped = image::imageops::crop_imm(&image, left, top, cw as u32, ch as u32).to_image();
    Ok(image::imageops::resize(&cropped, frame.size.0, frame.size.1, image::imageops::FilterType::Triangle))
}

#[derive(Clone)]
/// A text object's stored render; `color` tints alpha-only caches (the object's own text colour).
/// `x`/`y` are canvas pixels, or offsets from the centre of balloon shape `anchor` when the text
/// bounds are stored relative to their balloon (the n-th such text belongs to the n-th shape).
struct TextSprite { level: Level, x: i32, y: i32, color: Option<[u8; 3]>, anchor: Option<usize> }

struct Node {
    id: i64,
    first: i64,
    next: i64,
    child: Vec<Node>,
    name: String,
    kind: &'static str,
    visible: bool,
    mask_visible: bool,
    opacity: f64,
    blend: u8,
    clipping: bool,
    offset: (f64, f64),
    render_offset: (f64, f64),
    mask_offset: (f64, f64),
    color: [u8; 3],
    raster: Vec<Level>,
    object: Vec<Level>,
    mask: Vec<Level>,
    thumb: Option<Level>,
    sprites: Vec<TextSprite>,
    texts: Vec<Value>,
    edge: Option<Edge>,
    /// `VectorObjectList` chunks owned by the layer (strokes, balloons, frame lines).
    vectors: Vec<(u64, Vec<u8>)>,
    /// Frame folder whose closed vector paths mask its contents to panels.
    panel_mask: bool,
    /// Translation applied to this frame folder's vector paths.
    vector_shift: (f64, f64),
    tone: Option<Tone>,
    fill: Option<Fill>,
    /// Image material: destination corners (top-left, top-right, bottom-left, bottom-right)
    /// of the original image (`object` levels) in canvas pixels.
    resizable: Option<[(f64, f64); 4]>,
    correction: Option<Correction>,
    preview_color: Option<PreviewColor>,
    /// "Layer colour" display: dark pixels take this colour, white stays white.
    layer_color: Option<[u8; 3]>,
    detail: Value,
    can_render: bool,
    folder: bool,
}

#[derive(Default)]
struct Storage {
    levels: HashMap<i64, Level>,
    mipmap: HashMap<i64, i64>,
    mipinfo: HashMap<i64, (i64, i64)>,
    thumb_ids: HashMap<i64, i64>,
    hints: HashMap<i64, (u8, u64)>,
    vectors: HashMap<i64, Vec<(u64, Vec<u8>)>>,
    brushes: HashMap<i64, vector::Brush>,
    tip_images: HashMap<i64, i64>,
}

fn blob(value: rusqlite::Result<ValueRef<'_>>) -> &[u8] {
    match value {
        Ok(ValueRef::Blob(bytes) | ValueRef::Text(bytes)) => bytes,
        _ => &[],
    }
}

struct LayerRow<'a, 'stmt> {
    row: &'a Row<'stmt>,
    columns: &'a RefCell<HashMap<&'static str, Option<usize>>>,
}

impl LayerRow<'_, '_> {
    fn index(&self, key: &'static str) -> Option<usize> {
        // Cache misses too: older schemas may not have all optional columns.
        *self.columns.borrow_mut().entry(key).or_insert_with(|| self.row.as_ref().column_index(key).ok())
    }

    fn get<T: FromSql>(&self, key: &'static str) -> Option<T> {
        self.row.get(self.index(key)?).ok()
    }

    fn number(&self, key: &'static str) -> f64 {
        self.get::<Option<f64>>(key).flatten().filter(|n| n.is_finite()).unwrap_or_default()
    }

    fn blob(&self, key: &'static str) -> &[u8] {
        self.index(key).map_or(&[], |index| blob(self.row.get_ref(index)))
    }
}

fn load_storage(db: &Connection) -> Result<Storage> {
    let mut result = Storage::default();
    if table_exists(db, "Offscreen")? && table_exists(db, "ExternalChunk")? {
        // Match Electron's byte keys even when SQLite stores one ID as TEXT and the other as BLOB.
        // Materialize the normalized keys so SQLite can index them for the join.
        let mut statement = db.prepare("WITH chunks AS MATERIALIZED (
                SELECT CAST(ExternalID AS BLOB) AS IdBytes, Offset FROM ExternalChunk
                WHERE length(CAST(ExternalID AS BLOB)) BETWEEN 1 AND 1024
            )
            SELECT Offscreen.*, chunks.Offset AS ChunkOffset
            FROM Offscreen JOIN chunks ON CAST(Offscreen.BlockData AS BLOB) = chunks.IdBytes
            WHERE length(Offscreen.BlockData) <= 1024 LIMIT 200001").map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let mut count = 0;
        while let Some(r) = rows.next().map_err(error)? {
            count += 1;
            if count > 200_000 { return Err("CLIP 외부 청크가 안전 개수 제한을 초과합니다.".into()); }
            if let Some(geometry) = Geometry::parse(blob(r.get_ref("Attribute")))
                && let Ok(offset) = u64::try_from(integer(r, "ChunkOffset"))
            {
                result.levels.insert(integer(r, "MainId"), Level {
                    geometry, offset, external: blob(r.get_ref("BlockData")).to_vec(), owner: integer(r, "LayerId"),
                });
            }
        }
    }
    if table_exists(db, "VectorObjectList")? && table_exists(db, "ExternalChunk")?
        && let Ok(mut statement) = db.prepare("WITH chunks AS MATERIALIZED (
                SELECT CAST(ExternalID AS BLOB) AS IdBytes, Offset FROM ExternalChunk
                WHERE length(CAST(ExternalID AS BLOB)) BETWEEN 1 AND 1024
            )
            SELECT VectorObjectList.LayerId, chunks.IdBytes, chunks.Offset
            FROM VectorObjectList JOIN chunks ON CAST(VectorObjectList.VectorData AS BLOB) = chunks.IdBytes
            ORDER BY VectorObjectList.MainId LIMIT 200001")
    {
        let mut rows = statement.query([]).map_err(error)?;
        let mut count = 0;
        while let Some(r) = rows.next().map_err(error)? {
            count += 1;
            if count > 200_000 { return Err("CLIP 벡터 청크가 안전 개수 제한을 초과합니다.".into()); }
            let (layer, id, offset) = (integer(r, "LayerId"), blob(r.get_ref(1)).to_vec(), integer(r, "Offset"));
            if layer != 0 && let Ok(offset) = u64::try_from(offset) { result.vectors.entry(layer).or_default().push((offset, id)); }
        }
    }
    if table_exists(db, "Mipmap")? {
        let mut statement = db.prepare("SELECT MainId, BaseMipmapInfo FROM Mipmap LIMIT 200000").map_err(error)?;
        for entry in statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(error)? {
            let (id, first) = entry.map_err(error)?;
            result.mipmap.insert(id, first);
        }
    }
    if table_exists(db, "MipmapInfo")? {
        let mut statement = db.prepare("SELECT MainId, Offscreen, NextIndex FROM MipmapInfo LIMIT 200000").map_err(error)?;
        for entry in statement.query_map([], |r| Ok((integer(r, "MainId"), (integer(r, "Offscreen"), integer(r, "NextIndex"))))).map_err(error)? {
            let (id, level) = entry.map_err(error)?;
            result.mipinfo.insert(id, level);
        }
    }
    if table_exists(db, "LayerThumbnail")? {
        let mut statement = db.prepare("SELECT MainId, ThumbnailOffscreen FROM LayerThumbnail LIMIT 200000").map_err(error)?;
        for entry in statement.query_map([], |r| Ok((integer(r, "MainId"), integer(r, "ThumbnailOffscreen")))).map_err(error)? {
            let (id, level) = entry.map_err(error)?;
            result.thumb_ids.insert(id, level);
        }
    }
    let mut tables = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' LIMIT 1024").map_err(error)?;
    for table in tables.query_map([], |r| r.get::<_, String>(0)).map_err(error)? {
        let table = table.map_err(error)?;
        if !table.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') { continue; }
        let lower = table.to_ascii_lowercase();
        let rank = if lower.starts_with("ruler") || lower.starts_with("specialruler") { 0 }
            else if lower.contains("3d") { 6 }
            else if table == "VectorObjectList" { 5 }
            else if lower.contains("tonelayer") { 3 }
            else if lower.contains("comicframe") { 2 }
            else if lower.starts_with("filterlayer") || lower.starts_with("effectlayer") { 1 }
            else { continue };
        let Ok(mut query) = db.prepare(&format!(r#"SELECT LayerId FROM "{table}" LIMIT 200000"#)) else { continue };
        for owner in query.query_map([], |r| r.get::<_, i64>(0)).map_err(error)?.flatten() {
            let info = result.hints.entry(owner).or_default();
            info.0 = info.0.max(rank);
            if rank == 5 { info.1 += 1; }
        }
    }
    load_brushes(db, &mut result)?;
    Ok(result)
}

/// Brush styles, effector graphs and tip images used by vector strokes (`extractClipBrushStyles`).
fn load_brushes(db: &Connection, result: &mut Storage) -> Result<()> {
    let mut graphs = HashMap::new();
    if table_exists(db, "BrushEffectorGraphData")?
        && let Ok(mut statement) = db.prepare("SELECT MainId, ControlNumber, ControlDataSize, ControlPoints FROM BrushEffectorGraphData LIMIT 100000")
    {
        let mut rows = statement.query([]).map_err(error)?;
        while let Some(r) = rows.next().map_err(error)? {
            let (count, data) = (integer(r, "ControlNumber").max(0) as usize, blob(r.get_ref("ControlPoints")));
            if integer(r, "ControlDataSize") != 16 || count > 4096 || data.len() < count * 16 { continue; }
            let mut points: Vec<(f64, f64)> = (0..count).filter_map(|i| Some((
                f64::from_be_bytes(data[i * 16..i * 16 + 8].try_into().ok()?),
                f64::from_be_bytes(data[i * 16 + 8..i * 16 + 16].try_into().ok()?),
            ))).filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
            if points.is_empty() { continue; }
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            graphs.insert(integer(r, "MainId"), points);
        }
    }
    let mut patterns = HashMap::new();
    if table_exists(db, "BrushPatternStyle")?
        && let Ok(mut statement) = db.prepare("SELECT MainId, ImageIndex FROM BrushPatternStyle LIMIT 100000")
    {
        let mut rows = statement.query([]).map_err(error)?;
        while let Some(r) = rows.next().map_err(error)? {
            let ids: Vec<i64> = blob(r.get_ref("ImageIndex")).chunks_exact(4).take(256)
                .map(|c| i64::from(u32::from_be_bytes(c.try_into().unwrap()))).collect();
            patterns.insert(integer(r, "MainId"), ids);
        }
    }
    if table_exists(db, "BrushPatternImage")?
        && let Ok(mut statement) = db.prepare("SELECT MainId, Mipmap FROM BrushPatternImage LIMIT 100000")
    {
        let mut rows = statement.query([]).map_err(error)?;
        while let Some(r) = rows.next().map_err(error)? { result.tip_images.insert(integer(r, "MainId"), integer(r, "Mipmap")); }
    }
    if !table_exists(db, "BrushStyle")? { return Ok(()); }
    let Ok(mut statement) = db.prepare("SELECT * FROM BrushStyle LIMIT 100000") else { return Ok(()) };
    let columns = RefCell::new(HashMap::new());
    let mut rows = statement.query([]).map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        let r = LayerRow { row, columns: &columns };
        let optional = |key| r.get::<Option<f64>>(key).flatten();
        let id = r.number("MainId") as i64;
        let thickness = optional("ThicknessBase").map_or(1., |t| if t.is_finite() { t.clamp(0.01, 1.) } else { 1. });
        result.brushes.insert(id, vector::Brush {
            id,
            size: vector::size_effector(r.blob("SizeEffector"), &graphs),
            thickness,
            rotation: r.number("RotationBase"),
            pattern: patterns.get(&(r.number("PatternStyle") as i64)).cloned(),
            interval: optional("IntervalBase").map_or(0.1, |v| if v.is_finite() && v != 0. { v.max(0.001) } else { 0.1 }),
            auto_interval: r.number("AutoIntervalType") != 0.,
            rotation_effector: r.number("RotationEffector") as i64,
            spray: r.number("SprayFlag") != 0.,
            spray_size: r.number("SpraySizeBase").max(0.),
            spray_effector: vector::size_effector(r.blob("SpraySizeEffector"), &graphs),
            spray_bias: r.number("SprayBias").clamp(-1., 1.),
            spray_rotation_effector: r.number("RotationEffectorInSpray") as i64,
            spray_rotation_random: r.number("RotationRandomInSpray"),
            dual: r.number("DualStyleIndex") as i64,
        });
    }
    Ok(())
}

impl Storage {
    fn chain(&self, id: i64) -> Vec<Level> {
        let mut next = *self.mipmap.get(&id).unwrap_or(&0);
        let mut visited = HashSet::new();
        let mut levels = Vec::new();
        while next != 0 && visited.len() < 32 && visited.insert(next) {
            let Some(&(level, link)) = self.mipinfo.get(&next) else { break };
            if let Some(level) = self.levels.get(&level) { levels.push(level.clone()); }
            next = link;
        }
        levels
    }
}

fn le(bytes: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?))
}

fn prefixed(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut entries = Vec::new();
    while !bytes.is_empty() && entries.len() < 1000 {
        let Some(len) = le(bytes, 0) else { return vec![] };
        let Some(entry) = bytes.get(4..4 + len as usize) else { return vec![] };
        entries.push(entry);
        bytes = &bytes[4 + len as usize..];
    }
    entries
}

fn color_channel(value: f64) -> u8 { (value * 255. / u32::MAX as f64).round().clamp(0., 255.) as u8 }

fn attrs(mut bytes: &[u8]) -> Value {
    if bytes.is_empty() { return Value::Null; }
    let mut result = json!({});
    while let (Some(tag), Some(len)) = (le(bytes, 0), le(bytes, 4)) {
        let Some(value) = bytes.get(8..8 + len as usize) else { break };
        let n = le(value, 0);
        let components = |start: usize| [0, 4, 8].map(|delta| color_channel(le(value, start + delta).unwrap_or_default() as f64));
        if let Some(n) = n {
            match tag {
                32 => result["fontSize"] = json!(n),
                33 => { result["style"] = json!(n); result["vertical"] = json!(n & 16 != 0); }
                34 if len >= 12 => result["color"] = json!(components(0)),
                35 => result["align"] = json!(n as i32),
                38 => result["horizontalInVertical"] = json!(n as i32),
                42 if len >= 16 => result["bounds"] = json!([0, 4, 8, 12].map(|i| le(value, i).unwrap() as i32)),
                48 => result["antiAliasing"] = json!(n as i32),
                50 => result["renderCacheId"] = json!(n),
                53 => result["wrapFrame"] = json!(n != 0),
                56 if len >= 24 => result["edge"] = json!({
                    "enabled": n != 0, "width": le(value, 4).unwrap() as i32 as f64 / 1000., "color": components(12),
                }),
                58 => result["rotation"] = json!(n as i32 as f64 / 10. - 180.),
                59 => result["skewX"] = json!(n as i32 as f64 / 10. - 90.),
                60 => result["skewY"] = json!(n as i32 as f64 / 10. - 90.),
                63 if len >= 8 => result["boxSize"] = json!([n as i32, le(value, 4).unwrap() as i32]),
                64 if len >= 32 => result["quad"] = json!(std::array::from_fn::<_, 8, _>(|i| le(value, i * 4).unwrap() as i32 as f64 / 100.)),
                _ => {}
            }
        }
        if tag == 31 { result["font"] = json!(String::from_utf8_lossy(value)); }
        bytes = &bytes[8 + len as usize..];
    }
    result
}

fn text_objects(row: &LayerRow<'_, '_>, store: &Storage, id: i64, balloon: bool) -> (Vec<Value>, Vec<TextSprite>, usize) {
    let strings = prefixed(row.blob("TextLayerStringArray"));
    let attributes = prefixed(row.blob("TextLayerAttributesArray"));
    let extended = prefixed(row.blob("TextLayerAddAttributesV01"));
    let mut texts = Vec::new();
    let mut sprites = Vec::new();
    let mut missing = 0;
    let mut entry = |text: &[u8], primary: &[u8], secondary: &[u8]| {
        let text = String::from_utf8_lossy(&text[..text.len().min(64 << 10)]).replace('\0', "");
        let attributes = attrs(primary);
        let extended = attrs(secondary);
        let cache = attributes["renderCacheId"].as_i64().or_else(|| extended["renderCacheId"].as_i64());
        let bounds = attributes["bounds"].as_array().or_else(|| extended["bounds"].as_array());
        if let Some(level) = cache.and_then(|id| store.levels.get(&id)).filter(|level| level.owner == id)
            && let Some(bounds) = bounds
        {
            let color = attributes["color"].as_array().or_else(|| extended["color"].as_array())
                .and_then(|c| (c.len() == 3).then(|| [0, 1, 2].map(|i| c[i].as_u64().unwrap_or(0).min(255) as u8)));
            let edge = |i: usize| bounds.get(i).and_then(Value::as_i64).unwrap_or_default() as i32;
            // Balloon texts may keep bounds around their balloon's centre instead of the canvas.
            let relative = balloon && edge(0) < 0 && edge(2) > 0 && edge(1) < 0 && edge(3) > 0;
            let anchor = relative.then(|| sprites.iter().filter(|s: &&TextSprite| s.anchor.is_some()).count());
            sprites.push(TextSprite { color, level: level.clone(), x: edge(0), y: edge(1), anchor });
        } else if !text.is_empty() { missing += 1; }
        texts.push(json!({"index": texts.len(), "text": text, "attributes": attributes, "extendedAttributes": extended}));
    };
    let lookup = |col| row.blob(col);
    if row.get::<i64>("TextLayerType").is_some() || !lookup("TextLayerString").is_empty() || !lookup("TextLayerAttributes").is_empty() {
        entry(lookup("TextLayerString"), lookup("TextLayerAttributes"), extended.first().copied().unwrap_or_default());
    }
    for i in 0..strings.len().max(attributes.len()) {
        entry(strings.get(i).copied().unwrap_or_default(), attributes.get(i).copied().unwrap_or_default(), extended.get(i + 1).copied().unwrap_or_default());
    }
    (texts, sprites, missing)
}

fn effect<'a>(data: &'a [u8], name: &str) -> Option<(usize, &'a [u8])> {
    let token = tile::utf16_marker(name);
    let offset = memchr::memmem::find(data, &token)?;
    let remaining = data.get(offset + token.len()..)?;
    (tile::int(remaining, 0)? != 0).then_some((offset, remaining))
}

fn decode_layer(row: &LayerRow<'_, '_>, store: &Storage) -> Node {
    let i = |column| row.number(column) as i64;
    let n = |column| row.number(column);
    let id = i("MainId");
    let hint = store.hints.get(&id).copied().unwrap_or_default();
    let code = i("LayerType");
    let folder = i("LayerFolder") != 0 || i("LayerFirstChildIndex") != 0;
    let vector = row.get::<i64>("VectorNormalType").is_some() || hint.1 > 0
        || ["VectorNormalStrokeIndex", "VectorNormalFillIndex", "VectorNormalBalloonIndex"].into_iter().any(|key| i(key) != 0);
    let text = row.get::<i64>("TextLayerType").is_some() || i("MaterialContentType") == 110
        || ["TextLayerString", "TextLayerStringArray", "TextLayerAttributes", "TextLayerAttributesArray"].into_iter().any(|key| !row.blob(key).is_empty());
    let frame = i("ComicFrameLineMipmap") != 0 || i("VectorNormalType") == 3 || hint.0 == 2;
    let gradient = !row.blob("GradationFillInfo").is_empty();
    let fill = if gradient { parse_fill(row.blob("GradationFillInfo")) } else { None };
    let filter = !row.blob("FilterLayerInfo").is_empty() || !row.blob("FilterLayerV132").is_empty()
        || i("SpecialRenderType") == 13 || code == 4098 || hint.0 == 1;
    let balloon = text && (i("TextLayerType") == 1 || i("VectorNormalBalloonIndex") != 0);
    let (kind, subtype, label) = if folder {
        if frame { ("group", "frame-folder", "컷 테두리 폴더") } else { ("group", "group", "폴더") }
    } else if balloon { ("text", "balloon", "말풍선 · 텍스트")
    } else if text { match i("ComicStoryInfoType") {
        1 => ("text", "story-information", "텍스트 (작품 정보)"),
        2 => ("text", "page-number", "텍스트 (페이지 번호)"),
        _ => ("text", "text", "텍스트"),
    }
    } else if code & !2 == 1 { ("raster", "raster", "래스터 (일반)")
    } else if code == 1584 { ("paper", "paper", "용지")
    } else if hint.0 == 6 { ("3d", "3d", "3D")
    } else if gradient {
        if matches!(fill, Some(Fill::Flat(_))) { ("gradient", "fill", "단색 채우기") } else { ("gradient", "gradient", "그레이디언트") }
    } else if frame { ("border", "border", "컷 테두리")
    } else if vector { ("vector", "vector", "벡터")
    } else if filter { ("effect", "correction", "색조 보정")
    } else if hint.0 == 3 { ("tone", "tone", "톤")
    } else if i("ResizableOriginalMipmap") != 0 || !row.blob("ResizableImageInfo").is_empty() || matches!(code, 2 | 3) {
        ("object", "image-material", "이미지 소재")
    } else { ("layer", "unknown", "종류 미확인") };
    let flags = i("LayerVisibility");
    let lock = i("LayerLock");
    let color = ["DrawColorMainRed", "DrawColorMainGreen", "DrawColorMainBlue"].map(|key| color_channel(n(key)));
    let opacity = (n("LayerOpacity") / 256.).clamp(0., 1.);
    let blend = i("LayerComposite").clamp(0, 255) as u8;
    let name = row.get::<String>("LayerName").unwrap_or_default().trim().to_owned();
    let offset = (n("LayerOffsetX"), n("LayerOffsetY"));
    let render_offset = (n("LayerRenderOffscrOffsetX"), n("LayerRenderOffscrOffsetY"));
    let mask_offset = (offset.0 + n("LayerMaskOffsetX") + n("LayerMaskOffscrOffsetX"), offset.1 + n("LayerMaskOffsetY") + n("LayerMaskOffscrOffsetY"));
    // Panel paths of a moved frame folder keep their original coordinates; the move is stored
    // as the mask offset (its offscreen offset only compensates the raster mask buffer).
    let vector_shift = (offset.0 + n("LayerMaskOffsetX"), offset.1 + n("LayerMaskOffsetY"));
    let raster = store.chain(i("LayerRenderMipmap"));
    let object = store.chain(i("ResizableOriginalMipmap"));
    let resizable = resizable_corners(row.blob("ResizableImageInfo")).filter(|_| !object.is_empty());
    let mask = store.chain(i("LayerLayerMaskMipmap"));
    let thumb = store.thumb_ids.get(&i("LayerRenderThumbnail")).and_then(|id| store.levels.get(id)).cloned();
    let (texts, sprites, missing) = if text { text_objects(row, store, id, balloon) } else { (vec![], vec![], 0) };
    let info = row.blob("LayerEffectInfo");
    let edge = effect(info, "EffectEdge").and_then(|(_, bytes)| {
        let width = tile::double(bytes, 4)?;
        let color = [12, 16, 20].map(|i| color_channel(tile::int(bytes, i).unwrap_or_default() as f64));
        // The first word is the on/off switch of the layer property.
        let enabled = tile::int(bytes, 0)? != 0;
        (enabled && width > 0. && width <= 10_000. && bytes.len() >= 24).then_some(Edge { width, color })
    });
    let tone = effect(info, "EffectTone").and_then(|(_, bytes)| {
        let frequency = tile::double(bytes, 24)?;
        (frequency > 0. && frequency <= 1000.).then_some(Tone {
            frequency, angle: tile::int(bytes, 40)?, resolution: tile::double(bytes, 4)?,
            enabled: tile::int(bytes, 0)? != 0,
            kind: tile::int(bytes, 12)?, source: tile::int(bytes, 16)?,
            density: f64::from(tile::int(bytes, 20)?.min(100)) / 100.,
        })
    });
    let mut effects = Vec::new();
    if edge.is_some() { effects.push("테두리"); }
    if tone.is_some() { effects.push("톤"); }
    if filter { effects.push("필터 효과"); }
    if effects.is_empty() && i("LayerEffectAttached") != 0 { effects.push("레이어 효과"); }
    let preview_color = (i("UsePreviewColorType") == 1 && matches!(i("PreviewColorTypeIndex"), 1 | 2)).then(|| PreviewColor {
        monochrome: i("PreviewColorTypeIndex") == 2,
        black: i("PreviewColorTypeBlackChecked") != 0, white: i("PreviewColorTypeWhiteChecked") != 0,
        threshold: n("PreviewColorTypeThresholdImage") / f64::from(u32::MAX),
        alpha_threshold: n("PreviewColorTypeThresholdAlpha") / f64::from(u32::MAX),
    });
    let matching_mask = flags & 2 == 0 || i("LayerLayerMaskMipmap") == 0 || !mask.is_empty();
    let has_mask = i("LayerLayerMaskMipmap") != 0 || i("LayerLayerMaskThumbnail") != 0;
    let mut vectors = store.vectors.get(&id).cloned().unwrap_or_default();
    // A frame folder can keep superseded panel layouts; CSP draws only the newest one.
    if folder && frame && vectors.len() > 1 { vectors.drain(..vectors.len() - 1); }
    let panel_mask = folder && has_mask && flags & 2 != 0 && !vectors.is_empty();
    // Text without a stored layer raster needs every object's render cache (no font fallback yet).
    let text_ready = !text || !raster.is_empty() || missing == 0;
    // Tone is reproduced on non-raster sources and groups, as in the reference renderer.
    let correction = if filter { parse_correction(row.blob("FilterLayerInfo")).or_else(|| parse_correction(row.blob("FilterLayerV132"))) } else { None };
    let renderable = (kind == "paper" || !raster.is_empty() || fill.is_some() || !vectors.is_empty() || resizable.is_some()
        || correction.is_some() || (!filter && thumb.is_some())
        || (text && !sprites.is_empty()))
        && text_ready && (!filter || correction.is_some()) && (!frame || !raster.is_empty() || !vectors.is_empty()) && matching_mask
        && !(kind == "object" && raster.is_empty() && resizable.is_none());
    let storage_info = |levels: &[Level]| levels.iter().max_by_key(|level| level.geometry.pixels()).map(|level|
        json!({"width": level.geometry.width, "height": level.geometry.height, "levels": levels.len()}));
    // CSP leaves story information and page numbers out of its canvas preview, so they start
    // hidden here; they can still be switched on.
    let preview_hidden = matches!(subtype, "story-information" | "page-number");
    let visible = flags & 1 != 0 && !preview_hidden;
    let details = json!({
        "id": id.to_string(), "type": kind, "kind": subtype, "typeLabel": label,
        "classificationSource": "CLIP 레이어 종류 및 저장 데이터",
        "layerTypeCode": code, "folderKind": i("LayerFolder"), "visible": visible, "previewHidden": preview_hidden && flags & 1 != 0,
        "maskVisible": flags & 2 != 0, "rulerVisible": flags & 4 != 0, "locked": lock & 1 != 0,
        "alphaLocked": lock & 16 != 0, "lockFlags": lock, "draft": i("DraftLayer") != 0,
        "opacity": (opacity * 100.).round(), "opacityExact": opacity * 100.,
        "clipping": i("LayerClip") != 0, "blendMode": if blend == 0 { "normal".into() } else { format!("clip-{blend}") },
        "blendModeLabel": blend_label(blend), "mask": i("LayerLayerMaskMipmap") != 0 || i("LayerLayerMaskThumbnail") != 0,
        "effects": effects, "edgeEffect": edge.map(|edge| json!({"width":edge.width, "color":edge.color})),
        "toneEffect": tone.map(|tone| json!({"frequency": tone.frequency, "angle": tone.angle, "resolution": tone.resolution})),
        "colorMode": row.get::<i64>("LayerColorTypeIndex").map(|mode| json!({
            "index": mode, "label": match mode { 0 => "컬러", 1 => "그레이", 2 => "모노크롬", _ => "표현색 미확인" },
            "channelsKnown": row.get::<i64>("LayerColorTypeBlackChecked").is_some() && row.get::<i64>("LayerColorTypeWhiteChecked").is_some(),
            "black": i("LayerColorTypeBlackChecked") != 0, "white": i("LayerColorTypeWhiteChecked") != 0,
        })),
        "paletteColor": (i("LayerUsePaletteColor") != 0).then(|| ["LayerPaletteRed", "LayerPaletteGreen", "LayerPaletteBlue"].map(|key| color_channel(n(key)))),
        "vectorInfo": (vector && kind != "raster").then(|| json!({
            "kindCode": row.get::<i64>("VectorNormalType"), "strokeSourceId": i("VectorNormalStrokeIndex"),
            "fillSourceId": i("VectorNormalFillIndex"), "objectSourceId": i("VectorNormalBalloonIndex"), "dataCount": hint.1,
        })),
        "textInfo": text.then(|| json!({"kindCode":row.get::<i64>("TextLayerType"), "storyType":i("ComicStoryInfoType")})),
        "textObjectCount": texts.len(), "textRasterCount": sprites.len(), "textRasterAvailable": text && !sprites.is_empty() && missing == 0,
        "textPreview": texts.first().and_then(|obj| obj["text"].as_str()), "textAttributes": texts.first().map(|obj| &obj["attributes"]),
        "hasRuler": i("RulerRange") != 0,
        "rasterAvailable": !raster.is_empty(), "thumbnailAvailable": !folder && (!raster.is_empty() || thumb.is_some() || kind == "paper" || fill.is_some() || !vectors.is_empty() || !sprites.is_empty() || !object.is_empty()),
        "storedRaster": storage_info(&raster), "storedMask": storage_info(&mask), "storedObject": storage_info(&object),
        "offsetX": offset.0, "offsetY": offset.1, "renderOffsetX": render_offset.0, "renderOffsetY": render_offset.1,
        "drawColor": color,
        "previewAccuracy": if folder && !vectors.is_empty() { "폴더 자체 벡터 + 하위 레이어 합성" } else if folder { "하위 레이어 합성" } else if !raster.is_empty() { "CSP 저장 레이어 래스터" }
            else if !vectors.is_empty() { if balloon { "말풍선 도형 · 텍스트 개체 합성(근사)" } else { "벡터 경로 재구성(근사)" } }
            else if !sprites.is_empty() { "CSP 저장 텍스트 개체 래스터" }
            else if matches!(fill, Some(Fill::Flat(_))) { "단색 채우기" } else if fill.is_some() { "그레이디언트 재구성(근사)" }
            else if thumb.is_some() { "저장 레이어 썸네일(저해상도)" }
            else if kind == "paper" { "용지 색상" } else { "레이어 메타데이터" },
    });
    Node {
        id, first: i("LayerFirstChildIndex"), next: i("LayerNextIndex"), child: vec![],
        name: if name.is_empty() { if folder { "그룹".into() } else { "이름 없는 레이어".into() } } else { name },
        kind, visible, mask_visible: flags & 2 != 0, opacity, blend,
        clipping: i("LayerClip") != 0, offset, render_offset, mask_offset,
        color, raster, object, mask, thumb, sprites, texts, edge, vectors, panel_mask, vector_shift: if folder && frame { vector_shift } else { offset }, tone, fill, resizable, correction, preview_color,
        layer_color: (i("DrawColorEnable") != 0 && !folder && kind != "paper").then_some(color), detail: details,
        // A frame folder without panel paths still composes its children and raster mask.
        // Frame folders often keep their panel mask only as vector paths (no raster mask level).
        can_render: if folder { !filter && (matching_mask || panel_mask) } else { renderable },
        folder,
    }
}

fn blend_label(blend: u8) -> String {
    [
        "표준", "비교(어둡게)", "곱하기", "색상 번", "선형 번", "빼기", "어두운 색상", "비교(밝게)",
        "스크린", "색상 닷지", "발광 닷지", "더하기", "더하기(발광)", "밝은 색상", "오버레이",
        "소프트 라이트", "하드 라이트", "선명한 라이트", "선형 라이트", "핀 라이트", "하드 혼합",
        "차이", "제외", "색조", "채도", "색상", "명도", "", "", "", "통과", "", "", "", "", "", "나누기",
    ].get(blend as usize).filter(|v| !v.is_empty()).map_or_else(|| format!("CLIP 합성 모드 {blend}"), |s| (*s).into())
}

fn build_tree(nodes: &mut HashMap<i64, Node>, mut id: i64, depth: u32) -> Result<Vec<Node>> {
    if depth > 32 { return Err("CLIP 레이어 폴더 깊이가 안전 제한을 초과합니다.".into()); }
    let mut tree = Vec::new();
    while id != 0 {
        let Some(mut node) = nodes.remove(&id) else { break };
        node.child = build_tree(nodes, node.first, depth + 1)?;
        id = node.next;
        tree.push(node);
    }
    tree.reverse();
    Ok(tree)
}

fn depth(nodes: &[Node]) -> u64 {
    nodes.iter().map(|n| if n.folder { 1 + depth(&n.child) } else { 0 }).max().unwrap_or(0)
}

pub struct Document {
    pub preview: Vec<u8>,
    preview_size: (u32, u32),
    pub crop: Option<Crop>,
    size: (u32, u32),
    native: (u32, u32),
    dpi: f64,
    nodes: Vec<Node>,
    brushes: std::rc::Rc<Brushes>,
    source: Box<dyn ReadSeek>,
}

/// Brush styles and lazily decoded tip images shared by one document's renders.
#[derive(Default)]
struct Brushes {
    styles: HashMap<i64, vector::Brush>,
    tips: HashMap<i64, Vec<Level>>,
    cache: RefCell<HashMap<String, Option<std::rc::Rc<vector::Pattern>>>>,
}

impl Document {
    pub fn read(item: &Item) -> Result<Self> {
        let mut source = open_source(item)?;
        let (db, _tmp) = database(&mut *source)?;
        let preview = preview(&db)?;
        let size = dimensions(&preview)?;
        let canvas = canvas(&db)?;
        let dpi = if canvas.dpi > 0. { canvas.dpi } else { 72. };
        let store = load_storage(&db)?;
        let mut nodes = HashMap::new();
        let mut order = Vec::new();
        if table_exists(&db, "Layer")? {
            let mut query = db.prepare("SELECT * FROM Layer ORDER BY _PW_ID LIMIT 8193").map_err(error)?;
            let columns = RefCell::new(HashMap::new());
            let mut rows = query.query([]).map_err(error)?;
            while let Some(row) = rows.next().map_err(error)? {
                if order.len() >= MAX_LAYERS { return Err("CLIP 레이어 수가 안전 제한을 초과합니다.".into()); }
                let layer = decode_layer(&LayerRow { row, columns: &columns }, &store);
                order.push(layer.id);
                nodes.insert(layer.id, layer);
            }
        }
        // The canvas size is stored in its document unit: 0 px, 1 cm, 2 mm, 3 inch, 4 pt.
        let per_unit = match canvas.unit { 0 => Some(1.), 1 => Some(dpi / 2.54), 2 => Some(dpi / 25.4), 3 => Some(dpi), 4 => Some(dpi / 72.), _ => None };
        let physical = per_unit.map(|k| ((canvas.width * k).round() as u32, (canvas.height * k).round() as u32));
        let preview_size = size;
        let ratio = size.0 as f64 / size.1 as f64;
        let trusted = physical.filter(|p| p.0 != 0 && p.1 != 0
            && (p.0 as f64 / p.1 as f64 - ratio).abs() <= 0.02_f64.max(ratio * 0.03));
        let native = trusted.unwrap_or_else(|| {
            nodes.values().flat_map(|n| &n.raster).map(|level| level.geometry)
                .max_by_key(|geometry| geometry.pixels()).map_or(size, |g| (g.width, g.height))
        });
        let root_start = nodes.remove(&canvas.root).map_or(0, |root| root.first);
        let mut tree = build_tree(&mut nodes, root_start, 0)?;
        if tree.is_empty() { tree = order.into_iter().rev().filter_map(|id| nodes.remove(&id)).collect(); }
        // Compose at the real canvas resolution (halved while over budget), not the small preview.
        let size = match trusted {
            Some(native) => {
                let depth = depth(&tree);
                let mut display = native;
                while u64::from(display.0) * u64::from(display.1) > MAX_DISPLAY_PIXELS
                    || u64::from(display.0) * u64::from(display.1) * 4 * (7 + depth * 2) > MAX_COMPOSITE_BYTES
                { display = (display.0.div_ceil(2), display.1.div_ceil(2)); }
                display
            }
            None => preview_size,
        };
        let tips = store.tip_images.iter().map(|(&id, &mipmap)| (id, store.chain(mipmap))).filter(|(_, levels)| !levels.is_empty()).collect();
        let brushes = std::rc::Rc::new(Brushes { styles: store.brushes, tips, cache: RefCell::default() });
        Ok(Self { preview, preview_size, crop: canvas.crop, size, native, dpi, nodes: tree, brushes, source })
    }

    /// Size of the stored CSP preview image.
    pub fn dimensions(&self) -> Result<(u32, u32)> { Ok(self.preview_size) }

    fn compositing_supported(&self) -> bool {
        u64::from(self.size.0) * u64::from(self.size.1) * 4 * (7 + depth(&self.nodes) * 2) <= MAX_COMPOSITE_BYTES
    }

    pub fn description(&self) -> Value { self.describe(None) }

    fn layered(&self) -> bool {
        self.compositing_supported() && visit(&self.nodes).any(Node::toggleable)
    }

    fn describe(&self, failure: Option<&str>) -> Value {
        if self.nodes.is_empty() { return Value::Null; }
        fn node_json(node: &Node, available: bool) -> Value {
            let mut json = node.detail.clone();
            json["name"] = json!(node.name);
            json["textObjects"] = json!(node.texts);
            json["toggleAvailable"] = json!(available && node.toggleable());
            json["children"] = json!(node.child.iter().map(|n| node_json(n, available)).collect::<Vec<_>>());
            json
        }
        fn unrendered(nodes: &[Node]) -> usize {
            nodes.iter().filter(|n| n.visible).map(|n| if n.folder { unrendered(&n.child) } else { usize::from(!n.has_source()) }).sum()
        }
        let supported = failure.is_none() && self.layered();
        let missing = if supported { unrendered(&self.nodes) } else { 0 };
        let note = match failure {
            Some(reason) => format!("레이어 합성에 실패해 CLIP 저장 캔버스 미리보기를 표시합니다: {reason}"),
            None if supported && missing > 0 => format!(
                "레이어를 처음부터 분리해 독립 합성합니다. 독립 데이터가 없는 표시 레이어 {missing}개(보정·3D 등)는 그리지 못하며 결과는 CSP와 다를 수 있습니다."),
            None if supported => "레이어를 처음부터 분리해 독립 합성합니다. CSP 저장 래스터와 텍스트 개체 이미지를 사용하며 톤·그레이디언트·효과는 CSP와 다를 수 있습니다.".into(),
            None => "CLIP 저장 캔버스 미리보기와 레이어 정보를 표시합니다. 이 파일의 레이어 합성은 지원되지 않습니다.".into(),
        };
        json!({
            "format": "CLIP", "width": self.size.0, "height": self.size.1,
            "nativeWidth": self.native.0, "nativeHeight": self.native.1, "resolution": self.dpi,
            "toggleSupported": supported, "pickSupported": supported, "approximateRendering": supported,
            "independentRendering": supported, "unrenderedLayers": missing,
            "thumbnailSupported": visit(&self.nodes).any(|n| n.detail["thumbnailAvailable"] == true),
            "note": note,
            "layers": self.nodes.iter().map(|n| node_json(n, supported)).collect::<Vec<_>>(),
        })
    }

    /// Composes every layer from its own stored data instead of patching CSP's merged preview.
    /// Only documents without independently renderable layers keep the stored preview.
    pub fn display(&mut self) -> (Display, Value) {
        if !self.layered() { return (Display::Preview, self.description()); }
        let brushes = std::rc::Rc::clone(&self.brushes);
        let frame = self.frame(&brushes);
        match compose(&mut *self.source, &self.nodes, frame, &Value::Null) {
            Ok(image) => (Display::Layers(image), self.description()),
            Err(reason) => (Display::Preview, self.describe(Some(&reason))),
        }
    }

    fn frame<'b>(&self, brushes: &'b Brushes) -> Frame<'b> { Frame { size: self.size, native: self.native, dpi: self.dpi, brushes } }
}

pub enum Display { Layers(RgbaImage), Preview }

/// Output size, native canvas size and resolution shared by every layer of one composition.
#[derive(Clone, Copy)]
struct Frame<'a> { size: (u32, u32), native: (u32, u32), dpi: f64, brushes: &'a Brushes }

/// Supplies brush-tip images to the vector renderer, decoding each at most once per document.
struct TipSource<'a> { source: &'a mut dyn ReadSeek, brushes: &'a Brushes }

impl vector::Tips for TipSource<'_> {
    fn tip(&mut self, brush: &vector::Brush, stroke: &vector::Stroke) -> Result<Option<std::rc::Rc<vector::Pattern>>> {
        let Some(ids) = &brush.pattern else { return Ok(None) };
        let Some(levels) = ids.iter().find_map(|id| self.brushes.tips.get(id)) else { return Ok(None) };
        let key = format!("{}:{:?}:{:?}", brush.id, stroke.color, stroke.fill);
        if let Some(cached) = self.brushes.cache.borrow().get(&key) { return Ok(cached.clone()); }
        // Like `chooseThumbnailLevel(levels, 1024)`: smallest level at least 2048 px, else the largest.
        let mut sorted: Vec<&Level> = levels.iter().collect();
        sorted.sort_by_key(|level| level.geometry.pixels());
        let level = sorted.iter().find(|l| l.geometry.width.max(l.geometry.height) >= 2048).or(sorted.last()).copied();
        let pattern = match level {
            Some(level) => {
                let canvas = (level.geometry.width, level.geometry.height);
                let image = tile::read(self.source, level, fit(canvas, 1024), Placement { canvas, offset: (0., 0.), color: [0; 3] }, false)?;
                Some(std::rc::Rc::new(vector::Pattern::prepare(image, stroke.color, stroke.fill.unwrap_or([255; 3]))))
            }
            None => None,
        };
        let mut cache = self.brushes.cache.borrow_mut();
        if cache.len() >= 32 { cache.clear(); }
        cache.insert(key, pattern.clone());
        Ok(pattern)
    }
}

/// Decodes and renders a layer's vector strokes at the output size.
fn vector_image(source: &mut dyn ReadSeek, node: &Node, frame: Frame<'_>) -> Result<Option<RgbaImage>> {
    let strokes = strokes(source, node, frame)?;
    if strokes.is_empty() { return Ok(None); }
    let mut tips = TipSource { source, brushes: frame.brushes };
    vector::render(&strokes, &frame.brushes.styles, &mut tips, frame.size, frame.native, node.folder).map(Some)
}

fn strokes(source: &mut dyn ReadSeek, node: &Node, frame: Frame<'_>) -> Result<Vec<vector::Stroke>> {
    let mut budget = vector::MAX_POINTS;
    let mut strokes = Vec::new();
    for (offset, id) in &node.vectors {
        strokes.extend(vector::decode(&tile::chunk(source, *offset, id)?, frame.native, node.vector_shift, &frame.brushes.styles, &mut budget));
        if budget == 0 { break; }
    }
    let (dx, dy) = node.vector_shift;
    if dx != 0. || dy != 0. {
        for point in strokes.iter_mut().flat_map(|stroke| stroke.points.iter_mut()) {
            point.x += dx;
            point.y += dy;
            for handle in &mut point.handles { handle.0 += dx; handle.1 += dy; }
        }
    }
    Ok(strokes)
}

impl Node {
    /// Folders can be toggled when anything inside them is drawn; children without independent
    /// data are omitted from the composite either way, so hiding the folder stays consistent.
    fn toggleable(&self) -> bool {
        self.can_render && (!self.folder || self.has_source() || self.child.iter().any(Node::toggleable))
    }

    fn shown(&self, overrides: &Value) -> bool {
        overrides.get(self.id.to_string()).and_then(Value::as_bool).unwrap_or(self.visible)
    }

    fn has_source(&self) -> bool {
        self.kind == "paper" || !self.raster.is_empty() || self.fill.is_some() || !self.vectors.is_empty() || !self.sprites.is_empty()
            || self.resizable.is_some() || self.correction.is_some()
            || (!self.folder && self.thumb.is_some() && self.detail["kind"] != "correction")
    }
}

fn visit(nodes: &[Node]) -> Box<dyn Iterator<Item = &Node> + '_> {
    Box::new(nodes.iter().flat_map(|node| std::iter::once(node).chain(visit(&node.child))))
}

fn nearest(levels: &[Level], size: (u32, u32)) -> Option<&Level> {
    levels.iter().min_by_key(|level| {
        let g = level.geometry;
        u64::from(g.width.abs_diff(size.0)) + u64::from(g.height.abs_diff(size.1))
    })
}

fn blend(dst: &mut RgbaImage, src: &RgbaImage, mode: u8, strength: f64, clipped: bool) -> Result<()> {
    let mode = BlendMode::new(mode).map_err(error)?;
    if dst.dimensions() != src.dimensions() { return Err("CLIP 레이어 합성 크기가 일치하지 않습니다.".into()); }
    // Pixels are independent, so splitting into row bands gives identical output.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
    let row = dst.width() as usize * 4;
    let band = (dst.height() as usize).div_ceil(threads).max(64) * row;
    let (dst, src) = (dst.as_mut(), src.as_raw());
    if threads == 1 || dst.len() <= band {
        return composite_rgba(dst, src, mode, strength, clipped).map_err(error);
    }
    std::thread::scope(|scope| {
        let workers: Vec<_> = dst.chunks_mut(band).zip(src.chunks(band))
            .map(|(d, s)| scope.spawn(move || composite_rgba(d, s, mode, strength, clipped)))
            .collect();
        workers.into_iter().try_for_each(|worker| worker.join().map_err(|_| "CLIP 레이어 합성 작업이 중단되었습니다.".to_owned())?.map_err(error))
    })
}

fn leaf(source: &mut dyn ReadSeek, node: &Node, frame: Frame<'_>) -> Result<Option<RgbaImage>> {
    let Frame { size, native, .. } = frame;
    if node.kind == "paper" {
        let [r, g, b] = node.color;
        return Ok(Some(RgbaImage::from_pixel(size.0, size.1, Rgba([r, g, b, 255]))));
    }
    // Fill and gradient layers are defined by their settings; a stored raster can be stale.
    if node.vectors.is_empty() && let Some(fill) = &node.fill { return Ok(Some(render_fill(fill, size, native))); }
    if let Some(level) = nearest(&node.raster, size) {
        return tile::read(source, level, size, Placement {
            canvas: native,
            color: node.color,
            offset: (node.offset.0 + node.render_offset.0, node.offset.1 + node.render_offset.1),
        }, false).map(Some);
    }
    if let Some(corners) = node.resizable { return image_material(source, node, corners, frame).map(Some); }
    // Vector shapes come before fills; a balloon also owns text objects drawn on top of them.
    let shapes = if node.vectors.is_empty() { None } else { vector_image(source, node, frame)? };
    if shapes.is_none() && let Some(fill) = &node.fill { return Ok(Some(render_fill(fill, size, native))); }
    if node.sprites.is_empty() {
        if shapes.is_none() && !node.folder && node.correction.is_none() && let Some(thumb) = &node.thumb {
            return thumbnail_layer(source, thumb, frame).map(Some);
        }
        return Ok(shapes);
    }
    let mut output = shapes.unwrap_or_else(|| RgbaImage::new(size.0, size.1));
    let centres: Vec<(f64, f64)> = if node.sprites.iter().any(|s| s.anchor.is_some()) {
        strokes(source, node, frame)?.iter().filter_map(|stroke| {
            let (mut l, mut t, mut r, mut b) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &stroke.points { l = l.min(p.x); t = t.min(p.y); r = r.max(p.x); b = b.max(p.y); }
            (l <= r).then_some(((l + r) / 2., (t + b) / 2.))
        }).collect()
    } else { Vec::new() };
    for sprite in &node.sprites {
        let origin = match sprite.anchor {
            // Shapes pair with relative texts in order; extra texts use the middle of all shapes.
            Some(index) if index < centres.len() => centres[index],
            Some(_) if !centres.is_empty() => {
                let n = centres.len() as f64;
                (centres.iter().map(|c| c.0).sum::<f64>() / n, centres.iter().map(|c| c.1).sum::<f64>() / n)
            }
            _ => node.offset,
        };
        let glyphs = tile::read_sprite(source, &sprite.level, size, Placement {
            canvas: native, color: sprite.color.unwrap_or(node.color),
            offset: (origin.0 + f64::from(sprite.x), origin.1 + f64::from(sprite.y)),
        })?;
        if let Some((glyphs, origin)) = glyphs { blend_at(&mut output, &glyphs, origin)?; }
    }
    Ok(Some(output))
}

/// Normal source-over of a smaller image placed at `origin`; identical to a full-canvas
/// blend because pixels outside the sprite are transparent.
fn blend_at(dst: &mut RgbaImage, src: &RgbaImage, origin: (u32, u32)) -> Result<()> {
    let (stride, width) = (dst.width() as usize * 4, src.width() as usize * 4);
    let mode = BlendMode::new(0).map_err(error)?;
    for (row, line) in src.as_raw().chunks_exact(width).enumerate() {
        let start = (origin.1 as usize + row) * stride + origin.0 as usize * 4;
        composite_rgba(&mut dst.as_mut()[start..start + width], line, mode, 1., false).map_err(error)?;
    }
    Ok(())
}

/// Bounding box `[left, top, right, bottom]` (inclusive) of pixels with any alpha.
fn coverage(image: &RgbaImage) -> Option<[usize; 4]> {
    let width = image.width() as usize;
    let mut bounds: Option<[usize; 4]> = None;
    for (y, row) in image.as_raw().chunks_exact(width * 4).enumerate() {
        let Some(first) = row.chunks_exact(4).position(|p| p[3] != 0) else { continue };
        let last = row.chunks_exact(4).rposition(|p| p[3] != 0).unwrap_or(first);
        bounds = Some(match bounds {
            Some([l, t, r, _]) => [l.min(first), t, r.max(last), y],
            None => [first, y, last, y],
        });
    }
    bounds
}

/// One-dimensional running maximum over `radius` pixels on either side (sliding-window deque).
fn dilate(input: &[u8], width: usize, height: usize, radius: usize, vertical: bool) -> Vec<u8> {
    let (cross, length, step, line) = if vertical { (width, height, width, 1) } else { (height, width, 1, width) };
    let mut output = vec![0; input.len()];
    let mut window: std::collections::VecDeque<(usize, u8)> = std::collections::VecDeque::with_capacity(radius * 2 + 2);
    for n in 0..cross {
        window.clear();
        let mut next = 0;
        for k in 0..length {
            while next < length && next <= k + radius {
                let value = input[n * line + next * step];
                while window.back().is_some_and(|entry| entry.1 <= value) { window.pop_back(); }
                window.push_back((next, value));
                next += 1;
            }
            while window.front().is_some_and(|entry| entry.0 + radius < k) { window.pop_front(); }
            output[n * line + k * step] = window.front().map_or(0, |entry| entry.1);
        }
    }
    output
}

/// `ResizableImageInfo`: destination corners of an image material (reference `parseClipResizableImage`).
fn resizable_corners(data: &[u8]) -> Option<[(f64, f64); 4]> {
    if data.len() < 184 { return None; }
    let (w, h) = (tile::int(data, 32)?, tile::int(data, 36)?);
    if w == 0 || h == 0 || w > 100_000 || h > 100_000 { return None; }
    let corner = |at: usize| Some((tile::double(data, at)?, tile::double(data, at + 8)?));
    Some([corner(120)?, corner(136)?, corner(152)?, corner(168)?])
}

/// Draws an image material's original through its affine placement with bilinear sampling.
fn image_material(source: &mut dyn ReadSeek, node: &Node, corners: [(f64, f64); 4], frame: Frame<'_>) -> Result<RgbaImage> {
    let Frame { size, native, .. } = frame;
    let scale = (size.0 as f64 / native.0.max(1) as f64, size.1 as f64 / native.1.max(1) as f64);
    let points = corners.map(|(x, y)| (x * scale.0, y * scale.1));
    let (min_x, max_x) = points.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (min_y, max_y) = points.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    // Decode near the size the material covers on screen.
    let target = ((max_x - min_x).max(max_y - min_y) * 1.5).ceil().clamp(64., 4096.) as u32;
    let level = {
        let mut levels: Vec<&Level> = node.object.iter().collect();
        levels.sort_by_key(|l| l.geometry.pixels());
        levels.iter().find(|l| l.geometry.width.max(l.geometry.height) >= target * 2).or(levels.last()).copied()
            .ok_or("이미지 소재 원본이 없습니다.")?
    };
    let canvas = (level.geometry.width, level.geometry.height);
    let decoded = tile::read(source, level, fit(canvas, target), Placement { canvas, offset: (0., 0.), color: node.color }, false)?;
    let (dw, dh) = (f64::from(decoded.width()), f64::from(decoded.height()));
    let [top_left, top_right, bottom_left, _] = points;
    // Columns of the source-to-canvas matrix, per decoded pixel.
    let (a, b) = ((top_right.0 - top_left.0) / dw, (top_right.1 - top_left.1) / dw);
    let (c, d) = ((bottom_left.0 - top_left.0) / dh, (bottom_left.1 - top_left.1) / dh);
    let det = a * d - b * c;
    let mut output = RgbaImage::new(size.0, size.1);
    if det.abs() < 1e-12 { return Ok(output); }
    let x0 = min_x.floor().max(0.) as u32;
    let y0 = min_y.floor().max(0.) as u32;
    let x1 = (max_x.ceil().max(0.) as u32).min(size.0);
    let y1 = (max_y.ceil().max(0.) as u32).min(size.1);
    let raw = decoded.as_raw();
    let (iw, ih) = (decoded.width() as i64, decoded.height() as i64);
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f64 + 0.5 - top_left.0, y as f64 + 0.5 - top_left.1);
            let u = (d * px - c * py) / det - 0.5;
            let v = (-b * px + a * py) / det - 0.5;
            if u < -0.5 || v < -0.5 || u > dw - 0.5 || v > dh - 0.5 { continue; }
            let (ix, iy) = (u.floor() as i64, v.floor() as i64);
            let (fx, fy) = (u - ix as f64, v - iy as f64);
            let mut sum = [0f64; 4];
            for (dy, wy) in [(0, 1. - fy), (1, fy)] {
                for (dx, wx) in [(0, 1. - fx), (1, fx)] {
                    let (sx, sy) = ((ix + dx).clamp(0, iw - 1), (iy + dy).clamp(0, ih - 1));
                    let p = &raw[((sy * iw + sx) * 4) as usize..][..4];
                    let weight = wx * wy;
                    let alpha = f64::from(p[3]) * weight;
                    for ch in 0..3 { sum[ch] += f64::from(p[ch]) * alpha; }
                    sum[3] += alpha;
                }
            }
            if sum[3] <= 0. { continue; }
            let px = [sum[0] / sum[3], sum[1] / sum[3], sum[2] / sum[3], sum[3]].map(|v| v.round().clamp(0., 255.) as u8);
            output.put_pixel(x, y, Rgba(px));
        }
    }
    Ok(output)
}

fn apply_properties(source: &mut dyn ReadSeek, layer: &Node, image: &mut RgbaImage, frame: Frame<'_>) -> Result<()> {
    let Frame { size, native, dpi, .. } = frame;
    if let Some(edge) = layer.edge {
        let ratio = (size.0 as f64 / native.0 as f64).min(size.1 as f64 / native.1 as f64);
        let radius = (edge.width * ratio).round() as usize;
        // The outline can only appear within `radius` of covered pixels, so work on that box.
        if radius > 0 && let Some([x0, y0, x1, y1]) = coverage(image) {
            let (w, h) = (size.0 as usize, size.1 as usize);
            let (left, top) = (x0.saturating_sub(radius), y0.saturating_sub(radius));
            let (right, bottom) = ((x1 + radius + 1).min(w), (y1 + radius + 1).min(h));
            let (rw, rh) = (right - left, bottom - top);
            let raw: &mut [u8] = image;
            let alpha: Vec<u8> = (top..bottom).flat_map(|y| (left..right).map(move |x| (y, x)))
                .map(|(y, x)| raw[(y * w + x) * 4 + 3]).collect();
            let expanded = dilate(&dilate(&alpha, rw, rh, radius, false), rw, rh, radius, true);
            let [r, g, b] = edge.color;
            let mode = BlendMode::new(0).map_err(error)?;
            let mut below = vec![0; rw * 4];
            for y in 0..rh {
                for (x, pixel) in below.chunks_exact_mut(4).enumerate() { pixel.copy_from_slice(&[r, g, b, expanded[y * rw + x]]); }
                let start = ((top + y) * w + left) * 4;
                let line = &mut raw[start..start + rw * 4];
                composite_rgba(&mut below, line, mode, 1., false).map_err(error)?;
                line.copy_from_slice(&below);
            }
        }
    }
    if let Some(mode) = layer.preview_color { apply_preview_color(image, mode); }
    if let Some(tint) = layer.layer_color {
        for p in image.pixels_mut().filter(|p| p[3] != 0) {
            let light = (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2]) + 500) / 1000;
            for ch in 0..3 { p[ch] = ((u32::from(tint[ch]) * (255 - light) + 255 * light + 127) / 255) as u8; }
        }
    }
    // A stored layer raster already reflects CSP's own tone processing; groups and
    // reconstructed sources (fills, paper, text objects) do not.
    if let Some(tone) = layer.tone && tone.enabled && (layer.folder || layer.raster.is_empty()) {
        apply_tone(image, tone, size, native, dpi);
    }
    if layer.mask_visible && let Some(level) = nearest(&layer.mask, size) {
        let mask = tile::read(source, level, size, Placement { canvas: native, color: [0; 3], offset: layer.mask_offset }, true)?;
        for (pixel, mask) in image.pixels_mut().zip(mask.pixels()) {
            pixel[3] = ((u16::from(pixel[3]) * u16::from(mask[3]) + 127) / 255) as u8;
        }
    }
    Ok(())
}

/// Keeps a frame folder's contents inside its panel outlines (SVG `dest-in` with a white fill).
fn apply_frame_mask(source: &mut dyn ReadSeek, layer: &Node, image: &mut RgbaImage, frame: Frame<'_>) -> Result<()> {
    if !layer.panel_mask { return Ok(()); }
    let segments: Vec<Vec<(f64, f64)>> = strokes(source, layer, frame)?.into_iter()
        .map(|stroke| stroke.points.iter().map(|p| (p.x, p.y)).collect()).collect();
    if let Some(mask) = vector::coverage(&segments, frame.size, frame.native) {
        for (pixel, &cover) in image.pixels_mut().zip(&mask) {
            pixel[3] = ((u16::from(pixel[3]) * u16::from(cover) + 127) / 255) as u8;
        }
    }
    Ok(())
}

fn compose(source: &mut dyn ReadSeek, layers: &[Node], frame: Frame<'_>, overrides: &Value) -> Result<RgbaImage> {
    let mut result = RgbaImage::new(frame.size.0, frame.size.1);
    compose_into(source, layers, frame, overrides, &mut result)?;
    Ok(result)
}

fn compose_into(source: &mut dyn ReadSeek, layers: &[Node], frame: Frame<'_>, overrides: &Value, result: &mut RgbaImage) -> Result<()> {
    let mut base: Option<(RgbaImage, u8, f64)> = None;
    let flush = |base: &mut Option<(RgbaImage, u8, f64)>, result: &mut RgbaImage| -> Result<()> {
        if let Some((img, op, strength)) = base.take() { blend(result, &img, op, strength, false)?; }
        Ok(())
    };
    for (index, layer) in layers.iter().enumerate().rev() {
        if !layer.clipping { flush(&mut base, result)?; }
        // Layers clipped to this one need it as an isolated base image.
        let clip_base = index > 0 && layers[index - 1].clipping && !layer.clipping;
        if !layer.shown(overrides) { continue; }
        // A correction layer recolours what is beneath it in its scope (or its clipping base).
        if let (Some(correction), false) = (&layer.correction, layer.folder) {
            let mask = match nearest(&layer.mask, frame.size).filter(|_| layer.mask_visible) {
                Some(level) => Some(tile::read(source, level, frame.size, Placement { canvas: frame.native, color: [0; 3], offset: layer.mask_offset }, true)?),
                None => None,
            };
            if layer.clipping {
                if let Some((base_image, _, _)) = &mut base { apply_correction(base_image, correction, layer.blend, layer.opacity, mask.as_ref())?; }
            } else {
                apply_correction(result, correction, layer.blend, layer.opacity, mask.as_ref())?;
            }
            continue;
        }
        if layer.folder && layer.blend == 30 && !layer.clipping && !clip_base && layer.opacity == 1.
            && layer.mask.is_empty() && layer.edge.is_none() && layer.tone.is_none()
            && layer.raster.is_empty() && layer.fill.is_none() && layer.vectors.is_empty()
        {
            compose_into(source, &layer.child, frame, overrides, result)?;
            continue;
        }
        let image = if layer.folder {
            let mut group = compose(source, &layer.child, frame, overrides)?;
            apply_frame_mask(source, layer, &mut group, frame)?;
            if let Some(own) = leaf(source, layer, frame)? { blend(&mut group, &own, 0, 1., false)?; }
            Some(group)
        } else { leaf(source, layer, frame)? };
        if let Some(mut image) = image {
            apply_properties(source, layer, &mut image, frame)?;
            if layer.clipping {
                if let Some((base_image, _, strength)) = &mut base {
                    if *strength != 1. {
                        for p in base_image.pixels_mut() { p[3] = (f64::from(p[3]) * *strength).round() as u8; }
                        *strength = 1.;
                    }
                    blend(base_image, &image, layer.blend, layer.opacity, true)?;
                }
            } else { base = Some((image, layer.blend, layer.opacity)); }
        }
    }
    flush(&mut base, result)
}

fn only_clip(item: &Item) -> Result<()> {
    if matches!(item.ext().as_str(), "clip" | "csp") { Ok(()) }
    else { Err("이 Rust 빌드는 해당 형식의 레이어 합성을 지원하지 않습니다.".into()) }
}

fn fit(size: (u32, u32), max: u32) -> (u32, u32) {
    let scale = (f64::from(max) / size.0.max(size.1).max(1) as f64).min(1.);
    ((size.0 as f64 * scale).round().max(1.) as u32, (size.1 as f64 * scale).round().max(1.) as u32)
}

pub fn thumbnail(assets: &Assets, item: &Item, id: &str) -> Result<Value> {
    only_clip(item)?;
    let mut doc = Document::read(item)?;
    let id: i64 = id.parse().map_err(error)?;
    let node = visit(&doc.nodes).find(|n| n.id == id).ok_or("CLIP 레이어를 찾지 못했습니다.")?;
    if node.folder { return Ok(Value::Null); }
    let brushes = std::rc::Rc::clone(&doc.brushes);
    let frame = Frame { size: fit(doc.size, 58), ..doc.frame(&brushes) };
    let thumbnail = |source: &mut dyn ReadSeek, level: &Level| {
        let native = (level.geometry.width, level.geometry.height);
        tile::read(source, level, fit(native, 58), Placement { canvas: native, offset: (0., 0.), color: node.color }, false)
    };
    let image = if let Some(stored) = &node.thumb {
        thumbnail(&mut *doc.source, stored)?
    } else if let Some(image) = leaf(&mut *doc.source, node, frame)? {
        image
    } else if let Some(level) = nearest(&node.object, frame.size) {
        thumbnail(&mut *doc.source, level)?
    } else { return Err("저장된 레이어 미리보기가 없습니다.".into()) };
    Ok(json!(assets.bytes(picture::encode(&image)?, "image/png")?))
}

/// Renders the requested visibility from separated layers. The stored CSP preview is never
/// used as a base, so hidden layers leave no residue from the merged image.
pub fn render(assets: &Assets, item: &Item, overrides: &Value, mode: &str) -> Result<Value> {
    only_clip(item)?;
    let mut doc = Document::read(item)?;
    if !doc.compositing_supported() { return Err("CLIP 레이어 합성 크기가 안전 제한을 초과합니다.".into()); }
    if visit(&doc.nodes).any(|node| node.shown(overrides) != node.visible && !node.toggleable()) {
        return Err("독립 합성을 지원하지 않는 CLIP 레이어가 포함되어 있습니다.".into());
    }
    let brushes = std::rc::Rc::clone(&doc.brushes);
    let frame = doc.frame(&brushes);
    let image = compose(&mut *doc.source, &doc.nodes, frame, overrides)?;
    // Keep the trim/bleed view chosen by the viewer after a toggle.
    let area = doc.crop.clone().or_else(|| item.crop());
    let image = match area.as_ref().and_then(|c| picture::crop_rect(c, mode, image.width(), image.height())) {
        Some([l, t, r, b]) => image::imageops::crop_imm(&image, l, t, r - l, b - t).to_image(),
        None => image,
    };
    picture::rendered_area(assets, item, image, doc.description(), "CLIP 레이어 합성", area.is_some())
}

pub fn prepare(item: &Item) -> Result<Value> {
    only_clip(item)?;
    let doc = Document::read(item)?;
    if !doc.layered() {
        return Err("이 CLIP 파일에서 독립 합성 가능한 레이어를 찾지 못했습니다.".into());
    }
    Ok(json!(true))
}

pub fn pick(item: &Item, x: f64, y: f64, overrides: &Value, mode: &str) -> Result<Value> {
    only_clip(item)?;
    let mut doc = Document::read(item)?;
    // Points arrive in the displayed (possibly trimmed) image; move them to canvas pixels.
    let area = doc.crop.clone().or_else(|| item.crop());
    let (x, y) = match area.as_ref().and_then(|c| picture::crop_rect(c, mode, doc.size.0, doc.size.1)) {
        Some([l, t, ..]) => (x + f64::from(l), y + f64::from(t)),
        None => (x, y),
    };
    if !x.is_finite() || !y.is_finite() || x < 0. || y < 0. || x >= doc.size.0 as f64 || y >= doc.size.1 as f64 {
        return Ok(Value::Null);
    }
    if !doc.compositing_supported() { return Err("CLIP 레이어 합성 크기가 안전 제한을 초과합니다.".into()); }
    fn find(source: &mut dyn ReadSeek, nodes: &[Node], frame: Frame<'_>, at: (u32, u32), overrides: &Value) -> Result<Option<i64>> {
        for node in nodes.iter().filter(|node| node.shown(overrides) && node.opacity > 0.) {
            if node.folder {
                if let Some(id) = find(source, &node.child, frame, at, overrides)? { return Ok(Some(id)); }
            } else if node.can_render && let Some(mut image) = leaf(source, node, frame)? {
                apply_properties(source, node, &mut image, frame)?;
                if image.get_pixel(at.0, at.1)[3] > 8 { return Ok(Some(node.id)); }
            }
        }
        Ok(None)
    }
    let brushes = std::rc::Rc::clone(&doc.brushes);
    let frame = doc.frame(&brushes);
    Ok(find(&mut *doc.source, &doc.nodes, frame, (x as u32, y as u32), overrides)?.map_or(Value::Null, |id| json!(id.to_string())))
}

#[cfg(test)]
#[path = "clip/sample_tests.rs"]
mod sample_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_sqlite_requires_consistent_container_bounds() {
        const OFFSET: usize = 80;
        const START: usize = OFFSET + 16;
        const SIZE: usize = 512;
        const FOOT: usize = START + SIZE;
        const LEN: usize = FOOT + 16;
        let mut file = [0; LEN];
        file[..8].copy_from_slice(b"CSFCHUNK");
        file[8..16].copy_from_slice(&(LEN as u64).to_be_bytes());
        file[16..24].copy_from_slice(&24u64.to_be_bytes());
        file[24..32].copy_from_slice(b"CHNKHead");
        file[32..40].copy_from_slice(&40u64.to_be_bytes());
        file[48..56].copy_from_slice(&(OFFSET as u64).to_be_bytes());
        file[OFFSET..START].copy_from_slice(b"CHNKSQLi\0\0\0\0\0\0\x02\0");
        file[START..START + 16].copy_from_slice(SQLITE);
        file[START + 16..START + 18].copy_from_slice(&(SIZE as u16).to_be_bytes());
        file[START + 28..START + 32].copy_from_slice(&1u32.to_be_bytes());
        file[FOOT..FOOT + 8].copy_from_slice(b"CHNKFoot");
        let location = (START as u64, SIZE as u64);
        assert_eq!(indexed_database(&mut Cursor::new(file), LEN as u64).unwrap(), Some(location));
        for byte in [15, 31, 55, OFFSET, START - 2, FOOT] {
            let mut broken = file;
            broken[byte] ^= 1;
            assert_eq!(indexed_database(&mut Cursor::new(broken), LEN as u64).unwrap(), None);
            assert_eq!(scan_database(&mut Cursor::new(broken), LEN as u64).unwrap(), location);
        }
        file[START + 31] = 2;
        assert_eq!(indexed_database(&mut Cursor::new(file), LEN as u64).unwrap(), None);
        assert!(scan_database(&mut Cursor::new(file), LEN as u64).is_err());
    }

    #[test]
    fn legacy_scan_recovers_signature_across_blocks() {
        let mut file = vec![0; (1 << 20) + 20];
        let start = 12;
        file[start..start + SQLITE.len()].copy_from_slice(SQLITE);
        file[start + 16..start + 18].copy_from_slice(&512u16.to_be_bytes());
        file[start + 28..start + 32].copy_from_slice(&1u32.to_be_bytes());
        let len = file.len() as u64;
        assert_eq!(scan_database(&mut Cursor::new(file), len).unwrap(), (start as u64, 512));
    }

    fn section(out: &mut Vec<u8>, name: &str) {
        out.extend((name.len() as u32).to_be_bytes());
        out.extend(tile::utf16_marker(name));
    }

    fn gradient_info(flat: Option<[u8; 3]>) -> Vec<u8> {
        let mut data = vec![0; 8];
        section(&mut data, "GradationData");
        let mut body = Vec::new();
        for value in [0u32, 0, 2, 0] { body.extend(value.to_be_bytes()); }
        for (rgb, opacity, position) in [([255u32, 0, 0], 255u32, 0u32), ([0, 0, 255], 128, 32_768)] {
            for value in rgb.into_iter().chain([opacity, 0, position, 0]).enumerate().map(|(i, v)| if i < 4 { v << 24 } else { v }) {
                body.extend(value.to_be_bytes());
            }
        }
        data.extend((body.len() as u32).to_be_bytes());
        data.extend(body);
        section(&mut data, "GradationSetting");
        for value in [0u32, 0, 1] { data.extend(value.to_be_bytes()); }
        for value in [0., 0., 0., 0., 0., 100., 0.] { data.extend(f64::to_be_bytes(value)); }
        if let Some(color) = flat {
            section(&mut data, "GradationSettingAdd0001");
            let body: Vec<u8> = [1u32].into_iter().chain(color.map(|c| u32::from(c) << 24)).flat_map(u32::to_be_bytes).collect();
            data.extend((body.len() as u32).to_be_bytes());
            data.extend(body);
        }
        data
    }

    #[test]
    fn parses_and_paints_linear_gradient_in_native_coordinates() {
        let fill = parse_fill(&gradient_info(None)).unwrap();
        let Fill::Gradient { stops, start, end, .. } = &fill else { panic!("expected gradient") };
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[1], Stop { color: [0, 0, 255], opacity: 128, position: 1. });
        assert_eq!((*start, *end), ((0., 0.), (100., 0.)));
        // Output is half the native size, so the gradient spans the full 50 px width.
        let image = render_fill(&fill, (50, 2), (100, 4));
        assert_eq!(image.get_pixel(0, 0).0, [252, 0, 3, 254]);
        assert_eq!(image.get_pixel(49, 1).0, [3, 0, 252, 129]);
        let middle = image.get_pixel(25, 0).0;
        assert!(middle[0].abs_diff(middle[2]) <= 6 && middle[3].abs_diff(191) <= 2, "{middle:?}");
    }

    #[test]
    fn flat_fill_takes_precedence_and_rejects_truncated_data() {
        assert_eq!(parse_fill(&gradient_info(Some([10, 20, 30]))), Some(Fill::Flat([10, 20, 30])));
        let image = render_fill(&Fill::Flat([10, 20, 30]), (3, 2), (6, 4));
        assert!(image.pixels().all(|p| p.0 == [10, 20, 30, 255]));
        let data = gradient_info(None);
        assert_eq!(parse_fill(&data[..data.len() - 3]), None);
        assert_eq!(parse_fill(&[]), None);
    }

    #[test]
    fn tone_turns_coverage_into_black_halftone_dots() {
        let tone = Tone { frequency: 10., angle: 45, resolution: 100., enabled: true, kind: 1, source: 0, density: 0.2 };
        let mut solid = RgbaImage::from_pixel(40, 40, Rgba([0, 0, 0, 255]));
        apply_tone(&mut solid, tone, (40, 40), (40, 40), 72.);
        let coverage = solid.pixels().map(|p| f64::from(p[3]) / 255.).sum::<f64>() / 1600.;
        assert!(solid.pixels().all(|p| p.0[..3] == [0, 0, 0]) && coverage > 0.97, "{coverage}");
        let mut grey = RgbaImage::from_pixel(40, 40, Rgba([128, 128, 128, 255]));
        apply_tone(&mut grey, tone, (40, 40), (40, 40), 72.);
        let coverage = grey.pixels().map(|p| f64::from(p[3]) / 255.).sum::<f64>() / 1600.;
        assert!((0.35..0.65).contains(&coverage), "{coverage}");
        assert!(grey.pixels().any(|p| p[3] == 0) && grey.pixels().any(|p| p[3] == 255));
        let mut clear = RgbaImage::new(4, 4);
        apply_tone(&mut clear, tone, (4, 4), (4, 4), 72.);
        assert!(clear.pixels().all(|p| p.0 == [0; 4]));
    }

    #[test]
    fn tone_area_follows_density_linearly_and_set_density_ignores_brightness() {
        let tone = Tone { frequency: 10., angle: 45, resolution: 400., enabled: true, kind: 1, source: 0, density: 0.2 };
        let coverage = |image: &RgbaImage| image.pixels().map(|p| f64::from(p[3]) / 255.).sum::<f64>() / f64::from(image.width() * image.height());
        for (grey, expected) in [(191u8, 0.25), (64, 0.75)] {
            let mut image = RgbaImage::from_pixel(200, 200, Rgba([grey, grey, grey, 255]));
            apply_tone(&mut image, tone, (200, 200), (200, 200), 72.);
            assert!((coverage(&image) - expected).abs() < 0.04, "{grey}: {}", coverage(&image));
        }
        let mut light = RgbaImage::from_pixel(200, 200, Rgba([230, 230, 230, 255]));
        apply_tone(&mut light, Tone { source: 2, ..tone }, (200, 200), (200, 200), 72.);
        assert!((coverage(&light) - 0.2).abs() < 0.04, "{}", coverage(&light));
    }

    #[test]
    fn parses_levels_and_gradient_map_corrections() {
        let mut levels = vec![0, 0, 0, 2, 0, 0, 0, 40];
        for record in [[0x1134u16, 0x8899, 0xffff, 0, 0xffff], [0, 0x7fff, 0xffff, 0, 0xffff], [0, 0x7fff, 0xffff, 0, 0xffff], [0, 0x7fff, 0xffff, 0, 0xffff]] {
            for v in record { levels.extend(v.to_be_bytes()); }
        }
        let Some(Correction::Levels(records)) = parse_correction(&levels) else { panic!("levels") };
        assert!((records[0][0] - 0x1134 as f64 / 65535.).abs() < 1e-9 && records[1][1] > 0.49);
        let image = RgbaImage::from_pixel(1, 1, Rgba([0x11, 0x11, 0x11, 255]));
        assert!(corrected(&image, &Correction::Levels(records)).get_pixel(0, 0)[0] <= 1);

        let word = |v: u32| v.to_be_bytes();
        let mut body = Vec::new();
        for v in [0u32, 24, 28, 2, 16, 0] { body.extend(word(v)); }
        for (kind, colour, position) in [(3u32, 0x0000_0000u32, 0u32), (0, 0xffff_ffff, 0x8000)] {
            body.extend(word(kind));
            for _ in 0..3 { body.extend(word(colour)); }
            body.extend(word(0xffff_ffff));
            body.extend(word(0));
            body.extend(word(position));
        }
        let mut data = word(9).to_vec();
        data.extend(word(body.len() as u32));
        data.extend(body);
        let Some(Correction::GradientMap(stops)) = parse_correction(&data) else { panic!("gradient map") };
        assert_eq!(stops.len(), 2);
        // Black-to-white map keeps luminance; the alpha of the target is preserved.
        let grey = RgbaImage::from_pixel(1, 1, Rgba([10, 200, 30, 128]));
        let out = corrected(&grey, &Correction::GradientMap(stops));
        let luma = (299 * 10 + 587 * 200 + 114 * 30 + 500) / 1000;
        assert_eq!(out.get_pixel(0, 0).0, [luma as u8, luma as u8, luma as u8, 128]);
    }

    #[test]
    fn preview_colour_converts_to_grey_or_thresholded_monochrome() {
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([200, 40, 40, 255]));
        image.put_pixel(1, 0, Rgba([250, 250, 250, 255]));
        let mut grey = image.clone();
        apply_preview_color(&mut grey, PreviewColor { monochrome: false, black: true, white: true, threshold: 0.5, alpha_threshold: 0.5 });
        assert!(grey.pixels().all(|p| p[0] == p[1] && p[1] == p[2]));
        apply_preview_color(&mut image, PreviewColor { monochrome: true, black: true, white: false, threshold: 0.5, alpha_threshold: 0.5 });
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(1, 0)[3], 0);
    }

    #[test]
    fn reads_legacy_clip_fixture_without_editing_it() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/sample.clip");
        let item = Item::File { path, crop: None };
        let doc = Document::read(&item).unwrap();
        assert!(!doc.preview.is_empty());
        assert!(doc.size.0 > 0 && doc.size.1 > 0);
    }
}
