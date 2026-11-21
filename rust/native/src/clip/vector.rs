//! CLIP vector strokes from `VectorObjectList` chunks and their approximate rendering.
//! Decoding mirrors `vectorStrokeInfo`/`vectorRecord`/`decodeClipVector` in the reference
//! `clip-layer-reader.js`; rendering mirrors `clipVectorSvg` and `clip-brush-renderer.js`
//! (elliptical stamps, brush-tip patterns, spray, closed-path fills). It is not CSP's brush
//! engine: textures, ribbons and CSP's exact random sequence are not reproduced.

use std::collections::HashMap;

use image::RgbaImage;

use crate::Result;

const MIN_HEADER: usize = 88;
const MIN_POINT: usize = 88;
pub const MAX_POINTS: usize = 200_000;
const MAX_STAMPS: f64 = 300_000.;
const MAX_PATTERN_VISITS: u64 = 100 * 1024 * 1024;

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> { u32_at(data, at).map(|n| n as i32) }

fn f64_at(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_be_bytes(data.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn f32_at(data: &[u8], at: usize) -> Option<f64> { u32_at(data, at).map(|n| f64::from(f32::from_bits(n))) }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effector { pub enabled: bool, pub minimum: f64, pub graph: Option<Vec<(f64, f64)>> }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SizeEffector {
    pub pressure: Effector,
    pub velocity: Effector,
    pub smooth: Effector,
    pub random: Effector,
    pub tilt: Effector,
    pub velocity_maximum: f64,
}

/// Parses `SizeEffector`: optional modulators packed in flag order.
pub fn size_effector(data: &[u8], graphs: &HashMap<i64, Vec<(f64, f64)>>) -> SizeEffector {
    let mut result = SizeEffector { velocity_maximum: 1., ..Default::default() };
    let Some(flags) = u32_at(data, 0) else { return result };
    if flags & 1 == 0 { return result; }
    let mut offset = 4;
    for (flag, graph) in [(0x10, true), (0x20, true), (0x40, false), (0x80, false), (0x100, false)] {
        if flags & flag == 0 { continue; }
        let size = if graph { 8 } else { 4 };
        if offset + size > data.len() { break; }
        let minimum = f32_at(data, offset).unwrap_or(f64::NAN);
        let effector = Effector {
            enabled: minimum.is_finite(),
            minimum: if minimum.is_finite() { minimum.clamp(0., 1.) } else { 0. },
            graph: if graph { u32_at(data, offset + 4).and_then(|id| graphs.get(&i64::from(id)).cloned()) } else { None },
        };
        match flag {
            0x10 => result.pressure = effector,
            0x20 => result.velocity = effector,
            0x40 => result.smooth = effector,
            0x80 => result.random = effector,
            _ => result.tilt = effector,
        }
        offset += size;
    }
    let maximum = if result.velocity.enabled && offset + 4 <= data.len() { f32_at(data, offset).unwrap_or(1.) } else { 1. };
    result.velocity_maximum = if maximum.is_finite() { maximum.clamp(0., 1.) } else { 1. };
    result
}

/// Brush style fields used by the stroke renderer (`BrushStyle` row).
#[derive(Clone, Debug, PartialEq)]
pub struct Brush {
    pub id: i64,
    pub size: SizeEffector,
    pub thickness: f64,
    pub rotation: f64,
    /// Image ids of the brush-tip pattern, when the style has a pattern row.
    pub pattern: Option<Vec<i64>>,
    pub interval: f64,
    pub auto_interval: bool,
    pub rotation_effector: i64,
    pub spray: bool,
    pub spray_size: f64,
    pub spray_effector: SizeEffector,
    pub spray_bias: f64,
    pub spray_rotation_effector: i64,
    pub spray_rotation_random: f64,
    pub dual: i64,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            id: 0, size: SizeEffector { velocity_maximum: 1., ..Default::default() }, thickness: 1., rotation: 0.,
            pattern: None, interval: 0.1, auto_interval: false, rotation_effector: 0, spray: false, spray_size: 0.,
            spray_effector: SizeEffector { velocity_maximum: 1., ..Default::default() }, spray_bias: 0.,
            spray_rotation_effector: 0, spray_rotation_random: 0., dual: 0,
        }
    }
}

fn graph(points: &Option<Vec<(f64, f64)>>, input: f64) -> f64 {
    let value = input.clamp(0., 1.);
    let Some(points) = points.as_ref().filter(|p| !p.is_empty()) else { return value };
    if value <= points[0].0 { return points[0].1; }
    for pair in points.windows(2) {
        let ((lx, ly), (rx, ry)) = (pair[0], pair[1]);
        if value <= rx {
            let distance = rx - lx;
            let ratio = if distance > 0. { (value - lx) / distance } else { 0. };
            return ly + (ry - ly) * ratio;
        }
    }
    points[points.len() - 1].1
}

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub thickness: f64,
    pub rotation: f64,
    pub handles: Vec<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub points: Vec<Point>,
    pub closed: bool,
    pub color: [u8; 3],
    pub fill: Option<[u8; 3]>,
    pub opacity: f64,
    pub brush: Option<i64>,
    pub seed: u32,
}

struct Header { size: usize, count: usize, stride: usize, point_data: usize, base: f64, brush: i64, closed: bool, color: [u8; 3], fill: Option<[u8; 3]>, opacity: f64, seed: u32 }

fn channel(value: u32) -> u8 { if value <= 0xffff { (value >> 8) as u8 } else { (value >> 24) as u8 } }

fn header(data: &[u8], offset: usize) -> Option<Header> {
    if offset + MIN_HEADER > data.len() { return None; }
    let size = u32_at(data, offset)? as usize;
    let style = u32_at(data, offset + 4)? as usize;
    let stride = u32_at(data, offset + 8)? as usize;
    let point_data = u32_at(data, offset + 12)? as usize;
    let count = u32_at(data, offset + 16)? as usize;
    if !(MIN_HEADER..=4096).contains(&size) || style + 12 > size || !(MIN_POINT..=1024).contains(&stride)
        || !(MIN_POINT..=stride).contains(&point_data) || count > MAX_POINTS
        || offset + size + count * stride > data.len()
    { return None; }
    let radius_offset = size - 12;
    let base = f64_at(data, offset + radius_offset)? * 2.;
    if !(base.is_finite() && base > 0. && base <= 10_000.) { return None; }
    let descriptor = u32_at(data, offset + style)?;
    let extended = radius_offset.saturating_sub(style) >= 12 && radius_offset >= style;
    let brush = i64::from(u32_at(data, offset + style + if extended { 4 } else { 0 })?);
    let has_fill = extended && descriptor & 0x20 != 0 && u32_at(data, offset + style + 8)? != 0;
    let colors = |deltas: [usize; 3]| -> Option<[u8; 3]> {
        Some([channel(u32_at(data, offset + deltas[0])?), channel(u32_at(data, offset + deltas[1])?), channel(u32_at(data, offset + deltas[2])?)])
    };
    let opacity = f64_at(data, offset + 64)?;
    Some(Header {
        size, count, stride, point_data, base, brush,
        closed: u32_at(data, offset + 20)? & 8 != 0 || has_fill,
        color: colors([40, 44, 48])?,
        fill: if has_fill { Some(colors([52, 56, 60])?) } else { None },
        opacity: if opacity.is_finite() { opacity.clamp(0., 1.) } else { 1. },
        seed: u32_at(data, offset + size - 4)?,
    })
}

fn point(data: &[u8], offset: usize, max: (f64, f64), head: &Header, brush: Option<&Brush>) -> Option<Point> {
    if offset + MIN_POINT > data.len() { return None; }
    let (x, y) = (f64_at(data, offset)?, f64_at(data, offset + 8)?);
    let [left, top, right, bottom] = [16, 20, 24, 28].map(|delta| i32_at(data, offset + delta).map_or(0., f64::from));
    let flags = u32_at(data, offset + 32)?;
    let (pressure, velocity, smooth) = (f32_at(data, offset + 36)?, f32_at(data, offset + 40)?, f32_at(data, offset + 44)?);
    let (angle, tilt, stored) = (f32_at(data, offset + 48)?, f32_at(data, offset + 52)?, f32_at(data, offset + 56)?);
    let envelope = (right - left).max(bottom - top);
    let valid = left <= right && top <= bottom && envelope >= 1.
        && left >= -max.0 && top >= -max.1 && right <= max.0 && bottom <= max.1 && envelope <= max.0.max(max.1)
        && x.is_finite() && y.is_finite() && (0. ..=1.).contains(&pressure) && (0. ..=1.).contains(&velocity)
        && x >= left - 2. && x <= right + 2. && y >= top - 2. && y <= bottom + 2.;
    if !valid { return None; }
    let fixed = flags & 0x1000 != 0;
    let mut factor = 1.;
    if let Some(style) = brush {
        let size = &style.size;
        if size.pressure.enabled && !fixed {
            factor *= size.pressure.minimum + (1. - size.pressure.minimum) * graph(&size.pressure.graph, pressure);
        }
        if size.velocity.enabled {
            factor *= size.velocity.minimum + (size.velocity_maximum - size.velocity.minimum) * graph(&size.velocity.graph, velocity);
        }
        if size.smooth.enabled {
            factor *= 1. - (if smooth.is_finite() { smooth } else { 0. }).clamp(0., 1.) * (1. - size.smooth.minimum);
        }
        if size.tilt.enabled {
            factor *= size.tilt.minimum + (if tilt.is_finite() { tilt.abs() } else { 0. }).min(1.) * (1. - size.tilt.minimum);
        }
    }
    let size_factor = if stored.is_finite() && stored > 0. && stored <= 10_000. { stored } else { 1. };
    let width = (head.base * size_factor * if fixed { 1. } else { factor }).clamp(0.01, 10_000.);
    let thickness = brush.map_or(1., |b| b.thickness);
    let rotation = match brush {
        Some(b) if b.rotation_effector & 0x10 != 0 && angle.is_finite() => angle,
        Some(b) => b.rotation,
        None => 0.,
    };
    let mut handles = Vec::new();
    if matches!(head.stride - head.point_data, 16 | 32) {
        let mut index = head.point_data;
        while index + 16 <= head.stride {
            let (hx, hy) = (f64_at(data, offset + index)?, f64_at(data, offset + index + 8)?);
            if hx.is_finite() && hy.is_finite() && hx.abs() <= max.0 && hy.abs() <= max.1 { handles.push((hx, hy)); }
            index += 16;
        }
    }
    Some(Point { x, y, width, thickness, rotation, handles })
}

/// Decodes strokes from one chunk; `budget` is the remaining point allowance.
pub fn decode(data: &[u8], native: (u32, u32), brushes: &HashMap<i64, Brush>, budget: &mut usize) -> Vec<Stroke> {
    let max = ((f64::from(native.0) * 2.).ceil().max(4096.), (f64::from(native.1) * 2.).ceil().max(4096.));
    let mut result = Vec::new();
    let mut offset = 0;
    while offset + MIN_HEADER <= data.len() && *budget > 0 {
        let Some(head) = header(data, offset) else { offset += 4; continue };
        let brush = brushes.get(&head.brush);
        let available = head.count.min(*budget);
        let points: Vec<_> = (0..available)
            .filter_map(|index| point(data, offset + head.size + index * head.stride, max, &head, brush))
            .collect();
        *budget -= available;
        if !points.is_empty() {
            result.push(Stroke {
                points, closed: head.closed, color: head.color, fill: head.fill, opacity: head.opacity,
                brush: brush.map(|b| b.id), seed: head.seed,
            });
        }
        offset += head.size + head.count * head.stride;
    }
    result
}

/// Rasterizes closed polygons (output pixels) under the nonzero rule with 4× vertical
/// sampling. Returns 0..=1 coverage for the clipped bounding box `[left, top, right, bottom)`.
pub fn polygons(paths: &[Vec<(f64, f64)>], size: (u32, u32)) -> Option<(Vec<f32>, [usize; 4])> {
    const SUB: usize = 4;
    let mut edges = Vec::new();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for path in paths.iter().filter(|p| p.len() >= 3) {
        for (i, &a) in path.iter().enumerate() {
            let b = path[(i + 1) % path.len()];
            if !(a.0.is_finite() && a.1.is_finite()) { continue; }
            (x0, y0, x1, y1) = (x0.min(a.0), y0.min(a.1), x1.max(a.0), y1.max(a.1));
            if a.1 != b.1 && b.0.is_finite() && b.1.is_finite() { edges.push((a, b)); }
        }
    }
    if edges.is_empty() { return None; }
    let (w, h) = (size.0 as usize, size.1 as usize);
    let left = (x0.floor().max(0.) as usize).min(w);
    let top = (y0.floor().max(0.) as usize).min(h);
    let right = (x1.ceil().max(0.) as usize + 1).min(w);
    let bottom = (y1.ceil().max(0.) as usize + 1).min(h);
    if left >= right || top >= bottom { return None; }
    let width = right - left;
    let mut output = vec![0f32; width * (bottom - top)];
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for y in top..bottom {
        let row = &mut output[(y - top) * width..(y - top + 1) * width];
        for sub in 0..SUB {
            let sy = y as f64 + (sub as f64 + 0.5) / SUB as f64;
            crossings.clear();
            for &((ax, ay), (bx, by)) in &edges {
                let (lo, hi, dir) = if ay < by { (ay, by, 1) } else { (by, ay, -1) };
                if sy < lo || sy >= hi { continue; }
                crossings.push((ax + (sy - ay) / (by - ay) * (bx - ax), dir));
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                if winding == 0 { continue; }
                let start = (pair[0].0 - left as f64).clamp(0., width as f64);
                let end = (pair[1].0 - left as f64).clamp(0., width as f64);
                if end <= start { continue; }
                // `start < end <= width`, so `first < width`; `last` reaches `width` only at the edge.
                let (first, last) = (start.floor() as usize, end.floor() as usize);
                let weight = 1. / SUB as f32;
                if first == last {
                    row[first] += (end - start) as f32 * weight;
                } else {
                    row[first] += (first as f64 + 1. - start) as f32 * weight;
                    for cell in &mut row[first + 1..last.min(width)] { *cell += weight; }
                    if last < width { row[last] += (end - last as f64) as f32 * weight; }
                }
            }
        }
        for cell in row.iter_mut() { *cell = cell.clamp(0., 1.); }
    }
    Some((output, [left, top, right, bottom]))
}

/// Anti-aliased 0..=255 coverage of closed native-coordinate polygons over the whole output.
pub fn coverage(segments: &[Vec<(f64, f64)>], size: (u32, u32), native: (u32, u32)) -> Option<Vec<u8>> {
    let scale = (f64::from(size.0) / f64::from(native.0.max(1)), f64::from(size.1) / f64::from(native.1.max(1)));
    let scaled: Vec<Vec<(f64, f64)>> = segments.iter().map(|s| s.iter().map(|&(x, y)| (x * scale.0, y * scale.1)).collect()).collect();
    let (cover, [left, top, right, bottom]) = polygons(&scaled, size)?;
    let w = size.0 as usize;
    let mut output = vec![0u8; w * size.1 as usize];
    for (row, y) in (top..bottom).enumerate() {
        for (column, x) in (left..right).enumerate() {
            output[y * w + x] = (cover[row * (right - left) + column] * 255.).round() as u8;
        }
    }
    Some(output)
}

/// A brush-tip image resized to 128×128 with premultiplied colour.
pub struct Pattern { pub pixels: Vec<f32>, pub width: usize, pub height: usize, pub aspect: (f64, f64) }

impl Pattern {
    /// Mirrors the reference preparation: greyscale tips are tinted between the stroke's
    /// main colour (black) and secondary colour (white), then resized to 128×128.
    pub fn prepare(mut image: RgbaImage, main: [u8; 3], secondary: [u8; 3]) -> Self {
        let grayscale = image.pixels().all(|p| p[3] == 0 || (p[0] == p[1] && p[0] == p[2]));
        if grayscale {
            for p in image.pixels_mut() {
                let value = f64::from(p[0]) / 255.;
                for c in 0..3 { p[c] = (f64::from(main[c]) * (1. - value) + f64::from(secondary[c]) * value).round() as u8; }
            }
        }
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        let longest = w.max(h);
        let elongated = longest > w.min(h) * 8.;
        let aspect = if elongated { (1., 1.) } else { (w / longest, h / longest) };
        let resized = image::imageops::resize(&image, 128, 128, image::imageops::FilterType::Lanczos3);
        let pixels = resized.pixels().flat_map(|p| {
            let alpha = f32::from(p[3]) / 255.;
            [f32::from(p[0]) * alpha, f32::from(p[1]) * alpha, f32::from(p[2]) * alpha, f32::from(p[3])]
        }).collect();
        Self { pixels, width: 128, height: 128, aspect }
    }
}

/// Premultiplied-free RGBA float surface covering part of the output.
struct Surface { left: usize, top: usize, width: usize, height: usize, data: Vec<f32> }

impl Surface {
    fn around(points: &[Point], size: (u32, u32), scale: f64, extra: f64) -> Self {
        let padding = points.iter().map(|p| p.width).fold(extra, f64::max) * scale + 2.;
        let (mut l, mut t, mut r, mut b) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in points {
            for (x, y) in std::iter::once((p.x, p.y)).chain(p.handles.iter().copied()) {
                let (x, y) = (x * scale, y * scale);
                (l, t, r, b) = (l.min(x - padding), t.min(y - padding), r.max(x + padding), b.max(y + padding));
            }
        }
        let clamp = |v: f64, limit: u32| if v.is_finite() { v.clamp(0., f64::from(limit)) } else { 0. };
        let (left, top) = (clamp(l.floor(), size.0) as usize, clamp(t.floor(), size.1) as usize);
        let (right, bottom) = (clamp(r.ceil(), size.0) as usize, clamp(b.ceil(), size.1) as usize);
        let (width, height) = (right.saturating_sub(left), bottom.saturating_sub(top));
        Self { left, top, width, height, data: vec![0.; width * height * 4] }
    }
}

/// Coverage of an anti-aliased rotated ellipse at a pixel centre offset `(dx, dy)`.
fn ellipse(dx: f64, dy: f64, rx: f64, ry: f64, cos: f64, sin: f64) -> f64 {
    let (ex, ey) = (rx.max(0.5), ry.max(0.5));
    let ink = (rx * ry) / (ex * ey);
    let u = dx * cos + dy * sin;
    let v = -dx * sin + dy * cos;
    let s = ((u / ex).powi(2) + (v / ey).powi(2)).sqrt();
    if s == 0. { return ink; }
    let gradient = ((u / (ex * ex)).powi(2) + (v / (ey * ey)).powi(2)).sqrt() / s;
    let distance = (s - 1.) / gradient.max(1e-9);
    (0.5 - distance).clamp(0., 1.) * ink
}

fn curve(previous: &Point, point: &Point, ratio: f64) -> (f64, f64) {
    let inverse = 1. - ratio;
    let (left, right) = (&previous.handles, &point.handles);
    if left.len() == 2 && right.len() == 2 {
        let f = |a: f64, b: f64, c: f64, d: f64| inverse.powi(3) * a + 3. * inverse.powi(2) * ratio * b + 3. * inverse * ratio.powi(2) * c + ratio.powi(3) * d;
        return (f(previous.x, left[1].0, right[0].0, point.x), f(previous.y, left[1].1, right[0].1, point.y));
    }
    if left.len() == 1 {
        let f = |a: f64, b: f64, c: f64| inverse.powi(2) * a + 2. * inverse * ratio * b + ratio.powi(2) * c;
        return (f(previous.x, left[0].0, point.x), f(previous.y, left[0].1, point.y));
    }
    (previous.x + (point.x - previous.x) * ratio, previous.y + (point.y - previous.y) * ratio)
}

/// Closed fill outline with the same curve rules as the reference `clipClosedVectorPath`.
fn outline(points: &[Point], scale: f64) -> Vec<(f64, f64)> {
    if points.len() < 3 { return vec![]; }
    let mut path = vec![(points[0].x * scale, points[0].y * scale)];
    for (index, current) in points.iter().enumerate() {
        let next = &points[(index + 1) % points.len()];
        let steps = if current.handles.len() == 2 && next.handles.len() == 2 || current.handles.len() == 1 { 16 } else { 1 };
        for step in 1..=steps {
            let t = f64::from(step) / f64::from(steps);
            let inverse = 1. - t;
            let (x, y) = if current.handles.len() == 2 && next.handles.len() == 2 {
                let (a, b) = (current.handles[1], next.handles[0]);
                let f = |p: f64, q: f64, r: f64, s: f64| inverse.powi(3) * p + 3. * inverse.powi(2) * t * q + 3. * inverse * t * t * r + t.powi(3) * s;
                (f(current.x, a.0, b.0, next.x), f(current.y, a.1, b.1, next.y))
            } else if current.handles.len() == 1 {
                let a = current.handles[0];
                let f = |p: f64, q: f64, r: f64| inverse * inverse * p + 2. * inverse * t * q + t * t * r;
                (f(current.x, a.0, next.x), f(current.y, a.1, next.y))
            } else { (next.x, next.y) };
            path.push((x * scale, y * scale));
        }
    }
    path
}

fn over(target: &mut [f32], color: [f32; 3], alpha: f32) {
    if alpha <= 0. { return; }
    let retained = target[3] * (1. - alpha);
    let out = alpha + retained;
    for c in 0..3 { target[c] = (color[c] * alpha + target[c] * retained) / out; }
    target[3] = out;
}

fn stamp_pattern(surface: &mut Surface, pattern: &Pattern, x: f64, y: f64, width: f64, height: f64, rotation: f64, visits: &mut u64) -> Result<()> {
    if surface.width == 0 || surface.height == 0 || width <= 0. || height <= 0. { return Ok(()); }
    let (x, y) = (x - surface.left as f64, y - surface.top as f64);
    let (sin, cos) = rotation.to_radians().sin_cos();
    let half_x = (cos.abs() * width + sin.abs() * height) / 2.;
    let half_y = (sin.abs() * width + cos.abs() * height) / 2.;
    let left = (x - half_x).floor().max(0.) as usize;
    let top = (y - half_y).floor().max(0.) as usize;
    let right = ((x + half_x).ceil().max(0.) as usize).min(surface.width);
    let bottom = ((y + half_y).ceil().max(0.) as usize).min(surface.height);
    *visits += (right.saturating_sub(left) * bottom.saturating_sub(top)) as u64;
    if *visits > MAX_PATTERN_VISITS { return Err("CLIP 브러시 합성의 안전 처리 한도를 초과했습니다.".into()); }
    let (pw, ph) = (pattern.width as f64, pattern.height as f64);
    for py in top..bottom {
        for px in left..right {
            let (dx, dy) = (px as f64 + 0.5 - x, py as f64 + 0.5 - y);
            let sx = ((dx * cos + dy * sin) / width + 0.5) * pw - 0.5;
            let sy = ((dy * cos - dx * sin) / height + 0.5) * ph - 0.5;
            let (ix, iy) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - ix, sy - iy);
            let mut sum = [0f64; 4];
            for row in 0..=1 {
                for column in 0..=1 {
                    let (tx, ty) = (ix as i64 + column, iy as i64 + row);
                    if tx < 0 || ty < 0 || tx >= pattern.width as i64 || ty >= pattern.height as i64 { continue; }
                    let index = (ty as usize * pattern.width + tx as usize) * 4;
                    let weight = (if column == 1 { fx } else { 1. - fx }) * (if row == 1 { fy } else { 1. - fy });
                    for c in 0..4 { sum[c] += f64::from(pattern.pixels[index + c]) * weight; }
                }
            }
            if sum[3] == 0. { continue; }
            let index = (py * surface.width + px) * 4;
            let a = sum[3] / 255.;
            let data = &mut surface.data[index..index + 4];
            let retained = f64::from(data[3]) * (1. - a);
            let out = a + retained;
            // `sum[0..3]` is premultiplied by the tip alpha (0..255 colour × 0..1 alpha).
            for c in 0..3 { data[c] = ((sum[c] + f64::from(data[c]) * 255. * retained) / out / 255.) as f32; }
            data[3] = out as f32;
        }
    }
    Ok(())
}

/// Source-over of a straight-alpha float surface (colour 0..1) onto an RGBA float canvas.
fn merge(target: &mut [f32], width: usize, surface: &Surface, opacity: f64) {
    for y in 0..surface.height {
        for x in 0..surface.width {
            let source = &surface.data[(y * surface.width + x) * 4..][..4];
            let alpha = source[3] * opacity as f32;
            if alpha <= 0. { continue; }
            let index = ((y + surface.top) * width + x + surface.left) * 4;
            over(&mut target[index..index + 4], [source[0], source[1], source[2]], alpha);
        }
    }
}

/// Loads a brush-tip pattern for a stroke, keyed by the caller.
pub trait Tips {
    fn tip(&mut self, brush: &Brush, stroke: &Stroke) -> Result<Option<std::rc::Rc<Pattern>>>;
}

/// Renders strokes at the output size. Coordinates are native canvas pixels scaled by
/// `min(width/nativeWidth, height/nativeHeight)`, as in the reference renderer.
pub fn render(strokes: &[Stroke], brushes: &HashMap<i64, Brush>, tips: &mut dyn Tips, size: (u32, u32), native: (u32, u32), frame: bool) -> Result<RgbaImage> {
    let scale = (f64::from(size.0) / f64::from(native.0.max(1))).min(f64::from(size.1) / f64::from(native.1.max(1)));
    let (w, h) = (size.0 as usize, size.1 as usize);
    // Dual brushes draw the same path a second time with the secondary style.
    let expanded: Vec<(Stroke, Option<&Brush>)> = strokes.iter().flat_map(|stroke| {
        let brush = stroke.brush.and_then(|id| brushes.get(&id));
        let dual = brush.and_then(|b| brushes.get(&b.dual)).filter(|_| brush.is_some_and(|b| b.dual != 0));
        std::iter::once((stroke.clone(), brush)).chain(dual.map(|d| (Stroke { fill: None, brush: Some(d.id), ..stroke.clone() }, Some(d))))
    }).collect();
    let paths: Vec<Vec<Point>> = expanded.iter().map(|(stroke, _)| {
        let mut points = stroke.points.clone();
        if (frame || stroke.closed) && points.len() >= 3 { points.push(points[0].clone()); }
        points
    }).collect();
    let steps = |previous: &Point, point: &Point, brush: Option<&Brush>| -> f64 {
        let distance = (point.x - previous.x).hypot(point.y - previous.y) * scale;
        let thinnest = (previous.width * previous.thickness).min(point.width * point.thickness) * scale;
        let spacing = match brush {
            Some(b) if b.pattern.is_some() => (thinnest * if b.auto_interval { 0.15 } else { b.interval }).max(0.5),
            _ => (thinnest * 0.4).clamp(0.25, 1.),
        };
        (distance / spacing).ceil().max(1.)
    };
    let desired: f64 = paths.len() as f64 + paths.iter().zip(&expanded)
        .map(|(path, (_, brush))| path.windows(2).map(|p| steps(&p[0], &p[1], *brush)).sum::<f64>()).sum::<f64>();
    let reduction = (desired / MAX_STAMPS).max(1.);
    let mut before: Option<Vec<f32>> = None;
    let mut after: Option<Vec<f32>> = None;
    let mut body = vec![0f32; w * h * 4];
    // Union of every touched rectangle; later passes skip untouched canvas.
    let mut dirty = [w, h, 0usize, 0usize];
    let mut touch = |l: usize, t: usize, r: usize, b: usize| {
        if l < r && t < b { dirty = [dirty[0].min(l), dirty[1].min(t), dirty[2].max(r), dirty[3].max(b)]; }
    };
    let mut deferred: Vec<(Surface, [u8; 3], f64)> = Vec::new();
    let mut visits = 0u64;
    for ((stroke, brush), path) in expanded.iter().zip(&paths) {
        if path.is_empty() { continue; }
        let mut random_state = (if stroke.seed == 0 { 1 } else { stroke.seed }) ^ brush.map_or(0, |b| b.id as u32);
        let mut random = || { random_state = random_state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); f64::from(random_state) / 4_294_967_296. };
        let tip = match brush { Some(b) => tips.tip(b, stroke)?, None => None };
        let mut surface = Surface::around(path, size, scale, if tip.is_some() { brush.map_or(0., |b| b.spray_size) } else { 0. });
        let mut stamp = |previous: &Point, point: &Point, ratio: f64, surface: &mut Surface| -> Result<()> {
            let (cx, cy) = curve(previous, point, ratio);
            let (mut x, mut y) = (cx * scale, cy * scale);
            let mut brush_width = (previous.width + (point.width - previous.width) * ratio).max(0.01) * scale;
            let thickness = (previous.thickness + (point.thickness - previous.thickness) * ratio).clamp(0.01, 1.);
            let mut delta = (point.rotation - previous.rotation) % 360.;
            if delta > 180. { delta -= 360.; }
            if delta < -180. { delta += 360.; }
            let mut rotation = previous.rotation + delta * ratio;
            if let (Some(b), Some(_)) = (brush.filter(|b| b.spray), tip.as_ref()) {
                let angle = random() * std::f64::consts::TAU;
                let radius = brush_width / 2. * random().powf((1. + b.spray_bias) / 2.);
                x += angle.cos() * radius;
                y += angle.sin() * radius;
                let effector = &b.spray_effector.random;
                let factor = if effector.enabled { effector.minimum + random() * (1. - effector.minimum) } else { 1. };
                brush_width = (b.spray_size * scale * factor).max(0.01);
                if b.spray_rotation_effector & 0x80 != 0 { rotation += random() * 360. * b.spray_rotation_random; }
            } else if tip.is_some() && brush.is_none_or(|b| b.rotation_effector & 0x10 == 0) {
                rotation += (point.y - previous.y).atan2(point.x - previous.x).to_degrees() + 90.;
            }
            if let Some(tip) = &tip {
                return stamp_pattern(surface, tip, x, y, brush_width * tip.aspect.0, brush_width * thickness * tip.aspect.1, rotation, &mut visits);
            }
            let rx = (brush_width / 2.).max(0.005);
            let ry = (rx * thickness).max(0.005);
            let (sin, cos) = rotation.to_radians().sin_cos();
            let reach = rx.max(ry).max(0.5) + 1.;
            let (ox, oy) = (x - surface.left as f64, y - surface.top as f64);
            let x0 = (ox - reach).floor().max(0.) as usize;
            let y0 = (oy - reach).floor().max(0.) as usize;
            let x1 = ((ox + reach).ceil().max(0.) as usize).min(surface.width);
            let y1 = ((oy + reach).ceil().max(0.) as usize).min(surface.height);
            for py in y0..y1 {
                for px in x0..x1 {
                    let cover = ellipse(px as f64 + 0.5 - ox, py as f64 + 0.5 - oy, rx, ry, cos, sin) as f32;
                    if cover > 0. {
                        let alpha = &mut surface.data[(py * surface.width + px) * 4 + 3];
                        *alpha += cover * (1. - *alpha);
                    }
                }
            }
            Ok(())
        };
        stamp(&path[0], &path[0], 0., &mut surface)?;
        for pair in path.windows(2) {
            let count = (steps(&pair[0], &pair[1], *brush) / reduction).ceil().max(1.) as u32;
            for step in 1..=count { stamp(&pair[0], &pair[1], f64::from(step) / f64::from(count), &mut surface)?; }
        }
        let spray = brush.is_some_and(|b| b.spray);
        if tip.is_some() {
            let target = if spray { &mut after } else { &mut before }.get_or_insert_with(|| vec![0.; w * h * 4]);
            merge(target, w, &surface, stroke.opacity);
            touch(surface.left, surface.top, surface.left + surface.width, surface.top + surface.height);
        } else if spray {
            deferred.push((surface, stroke.color, stroke.opacity));
        } else {
            tint(&mut surface, stroke.color);
            merge(&mut body, w, &surface, stroke.opacity);
            touch(surface.left, surface.top, surface.left + surface.width, surface.top + surface.height);
        }
    }
    for (stroke, _) in &expanded {
        let Some(color) = stroke.fill else { continue };
        let path = outline(&stroke.points, scale);
        let Some((cover, [left, top, right, bottom])) = polygons(&[path], size) else { continue };
        let fill = [f32::from(color[0]) / 255., f32::from(color[1]) / 255., f32::from(color[2]) / 255.];
        touch(left, top, right, bottom);
        for (row, y) in (top..bottom).enumerate() {
            for (column, x) in (left..right).enumerate() {
                let alpha = cover[row * (right - left) + column] * stroke.opacity as f32;
                over(&mut body[(y * w + x) * 4..][..4], fill, alpha);
            }
        }
    }
    for (mut surface, color, opacity) in deferred {
        tint(&mut surface, color);
        merge(&mut body, w, &surface, opacity);
        touch(surface.left, surface.top, surface.left + surface.width, surface.top + surface.height);
    }
    let [left, top, right, bottom] = dirty;
    let mut image = RgbaImage::new(size.0, size.1);
    if left >= right || top >= bottom { return Ok(image); }
    let mut canvas = before.unwrap_or_else(|| vec![0.; w * h * 4]);
    let byte = |v: f32| (v * 255.).round().clamp(0., 255.) as u8;
    let raw: &mut [u8] = &mut image;
    for y in top..bottom {
        let span = (y * w + left) * 4..(y * w + right) * 4;
        composite(&mut canvas[span.clone()], &body[span.clone()]);
        if let Some(after) = &after { composite(&mut canvas[span.clone()], &after[span.clone()]); }
        for (out, value) in raw[span.clone()].iter_mut().zip(&canvas[span]) { *out = byte(*value); }
    }
    Ok(image)
}

fn tint(surface: &mut Surface, color: [u8; 3]) {
    let color = color.map(|c| f32::from(c) / 255.);
    for pixel in surface.data.chunks_exact_mut(4) { pixel[..3].copy_from_slice(&color); }
}

fn composite(target: &mut [f32], source: &[f32]) {
    for (t, s) in target.chunks_exact_mut(4).zip(source.chunks_exact(4)) { over(t, [s[0], s[1], s[2]], s[3]); }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoTips;
    impl Tips for NoTips {
        fn tip(&mut self, _: &Brush, _: &Stroke) -> Result<Option<std::rc::Rc<Pattern>>> { Ok(None) }
    }

    #[test]
    fn square_coverage_is_exact_inside_and_partial_on_edges() {
        let square = vec![vec![(1., 1.), (3., 1.), (3., 3.), (1., 3.)]];
        let mask = coverage(&square, (4, 4), (4, 4)).unwrap();
        assert_eq!(&mask[4..8], &[0, 255, 255, 0]);
        assert_eq!(&mask[..4], &[0; 4]);
        let half = coverage(&[vec![(0.5, 0.), (2., 0.), (2., 2.), (0.5, 2.)]], (2, 2), (2, 2)).unwrap();
        assert_eq!(half, vec![128, 255, 128, 255]);
        // Two overlapping subpaths with the same winding stay filled under the nonzero rule.
        let overlap = vec![vec![(0., 0.), (2., 0.), (2., 2.), (0., 2.)], vec![(1., 0.), (3., 0.), (3., 2.), (1., 2.)]];
        assert_eq!(coverage(&overlap, (3, 1), (3, 2)).unwrap(), vec![255, 255, 255]);
    }

    fn record(x: f64, y: f64, pressure: f32) -> Vec<u8> {
        let mut point = vec![0; MIN_POINT];
        point[..8].copy_from_slice(&x.to_be_bytes());
        point[8..16].copy_from_slice(&y.to_be_bytes());
        for (at, value) in [(16, x as i32 - 2), (20, y as i32 - 2), (24, x as i32 + 2), (28, y as i32 + 2)] {
            point[at..at + 4].copy_from_slice(&value.to_be_bytes());
        }
        point[36..40].copy_from_slice(&pressure.to_be_bytes());
        point
    }

    fn chunk(points: &[(f64, f64, f32)], fill: bool) -> Vec<u8> {
        let mut data = vec![0xff; 8];
        let mut head = vec![0; MIN_HEADER + 12];
        let size = head.len() as u32;
        // Extended style block at 24: descriptor, brush id 7, fill flag.
        for (at, value) in [(0, size), (4, 24), (8, MIN_POINT as u32), (12, MIN_POINT as u32), (16, points.len() as u32),
            (24, if fill { 0x20 } else { 0 }), (28, 7), (32, u32::from(fill)), (40, 0xff00_0000), (56, 0xff00_0000)] {
            head[at as usize..at as usize + 4].copy_from_slice(&value.to_be_bytes());
        }
        head[64..72].copy_from_slice(&1.0f64.to_be_bytes());
        let radius = size as usize - 12;
        head[radius..radius + 8].copy_from_slice(&2.0f64.to_be_bytes());
        data.extend(head);
        for &(x, y, p) in points { data.extend(record(x, y, p)); }
        data
    }

    #[test]
    fn decodes_stroke_with_pressure_curve_and_skips_garbage() {
        let data = chunk(&[(10., 10., 1.), (90., 10., 0.5), (50., 80., 0.)], true);
        let mut brushes = HashMap::new();
        let pressure = Effector { enabled: true, minimum: 0.5, graph: None };
        brushes.insert(7, Brush { id: 7, size: SizeEffector { pressure, velocity_maximum: 1., ..Default::default() }, ..Default::default() });
        let mut budget = MAX_POINTS;
        let strokes = decode(&data, (100, 100), &brushes, &mut budget);
        assert_eq!(strokes.len(), 1);
        let stroke = &strokes[0];
        assert_eq!((stroke.color, stroke.fill, stroke.closed, stroke.brush), ([255, 0, 0], Some([0, 255, 0]), true, Some(7)));
        // Base diameter 4; factor = 0.5 + 0.5 × pressure.
        assert_eq!(stroke.points.iter().map(|p| p.width).collect::<Vec<_>>(), vec![4., 3., 2.]);
        assert_eq!(budget, MAX_POINTS - 3);
        let mut empty = 0;
        assert!(decode(&data, (100, 100), &brushes, &mut empty).is_empty());
    }

    #[test]
    fn renders_round_stroke_and_closed_fill_under_body() {
        let data = chunk(&[(10., 10., 1.), (90., 10., 1.), (90., 90., 1.), (10., 90., 1.)], true);
        let mut budget = MAX_POINTS;
        let strokes = decode(&data, (100, 100), &HashMap::new(), &mut budget);
        let image = render(&strokes, &HashMap::new(), &mut NoTips, (100, 100), (100, 100), false).unwrap();
        // Stroke body (red) on the outline, green fill inside, transparent outside.
        // The fill is painted after stroke bodies, so check the stroke's outer half.
        for (x, y) in [(50, 8), (8, 50)] {
            let pixel = image.get_pixel(x, y).0;
            assert!(pixel[..3] == [255, 0, 0] && pixel[3] >= 250, "{pixel:?}");
        }
        assert_eq!(image.get_pixel(50, 50).0, [0, 255, 0, 255]);
        assert_eq!(image.get_pixel(50, 5)[3], 0);
        assert_eq!(image.get_pixel(5, 5)[3], 0);
    }

    #[test]
    fn thin_ellipse_keeps_its_ink() {
        let total: f64 = (-3..=3).flat_map(|y| (-3..=3).map(move |x| (x, y)))
            .map(|(x, y)| ellipse(f64::from(x), f64::from(y), 0.2, 0.2, 1., 0.)).sum();
        assert!((total - 0.16).abs() < 0.01, "{total}");
    }
}
