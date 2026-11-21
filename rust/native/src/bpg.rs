//! BPG decoding with the pure-Rust `rust_h265` HEVC decoder.
//!
//! BPG stores HEVC data without VPS/SPS NAL units; the SPS fields are coded in a compact
//! header (BPG specification 0.9.5, section 3). This module rebuilds standard VPS/SPS NAL
//! units from that header and the documented assumptions, then converts the decoded planes
//! with the same fixed-point chroma interpolation and colour matrices as libbpg 0.9.8, so
//! outputs can be compared with libbpg byte for byte.
//!
//! Not supported, reported as errors: alpha and grayscale BPG (support ended; both need
//! monochrome HEVC, which the decoder does not implement), 4:2:2 pictures and CMYK/W planes.

use image::RgbaImage;
use rust_h265::{Decoder, NalUnitType, PixelData, parse_annex_b};

use crate::{Result, error};

const MAGIC: [u8; 4] = [0x42, 0x50, 0x47, 0xfb];
const MAX_INPUT: usize = 64 << 20;
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_FRAMES: usize = 1000;
const MAX_OUTPUT: u64 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    /// 0 gray, 1/4 4:2:0, 2/5 4:2:2, 3 4:4:4 (4 and 5 use MPEG-2 chroma positions).
    pub format: u8,
    pub bit_depth: u8,
    pub color_space: u8,
    pub limited_range: bool,
    pub alpha: bool,
    pub premultiplied: bool,
    pub w_plane: bool,
    pub animation: Option<Animation>,
    /// Byte offset of `hevc_header_and_data()`.
    data: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animation { pub loops: u16, pub num: u16, pub den: u16 }

/// One decoded picture; `delay` is in seconds as `num/den`.
pub struct Frame { pub image: RgbaImage, pub delay: (u32, u32) }

fn ue7(data: &[u8], at: &mut usize) -> Result<u32> {
    let invalid = || "BPG 정수 필드가 손상되었습니다.".to_owned();
    let first = *data.get(*at).ok_or_else(invalid)?;
    *at += 1;
    if first == 0x80 { return Err(invalid()); }
    let mut value = u64::from(first & 0x7f);
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = *data.get(*at).ok_or_else(invalid)?;
        *at += 1;
        value = (value << 7) | u64::from(byte & 0x7f);
        if value > u64::from(u32::MAX) { return Err(invalid()); }
    }
    Ok(value as u32)
}

pub fn header(data: &[u8]) -> Result<Header> {
    if data.len() < 6 || data[..4] != MAGIC { return Err("BPG 파일 형식이 아닙니다.".into()); }
    let (flags1, flags2) = (data[4], data[5]);
    let format = flags1 >> 5;
    let alpha1 = flags1 & 0x10 != 0;
    let bit_depth = (flags1 & 0x0f) + 8;
    let color_space = flags2 >> 4;
    let extension = flags2 & 0x08 != 0;
    let alpha2 = flags2 & 0x04 != 0;
    if format > 5 || bit_depth > 14 || color_space > 4 || (format == 0 && color_space != 0) {
        return Err("BPG 헤더 값이 올바르지 않습니다.".into());
    }
    let mut at = 6;
    let width = ue7(data, &mut at)?;
    let height = ue7(data, &mut at)?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS || width > 65_535 || height > 65_535 {
        return Err("BPG 이미지 크기가 안전 제한을 초과합니다.".into());
    }
    let picture_len = ue7(data, &mut at)?;
    let mut animation = None;
    if extension {
        let len = ue7(data, &mut at)? as usize;
        let end = at.checked_add(len).filter(|&end| end <= data.len()).ok_or("BPG 확장 데이터가 잘렸습니다.")?;
        while at < end {
            let tag = ue7(&data[..end], &mut at)?;
            let size = ue7(&data[..end], &mut at)? as usize;
            let next = at.checked_add(size).filter(|&n| n <= end).ok_or("BPG 확장 데이터가 잘렸습니다.")?;
            if tag == 5 && flags2 & 1 != 0 {
                let mut cursor = at;
                let fields = [ue7(&data[..next], &mut cursor)?, ue7(&data[..next], &mut cursor)?, ue7(&data[..next], &mut cursor)?];
                if fields.iter().any(|&v| v > u32::from(u16::MAX)) || fields[1] == 0 || fields[2] == 0 {
                    return Err("BPG 애니메이션 정보가 올바르지 않습니다.".into());
                }
                animation = Some(Animation { loops: fields[0] as u16, num: fields[1] as u16, den: fields[2] as u16 });
            }
            at = next;
        }
    }
    if flags2 & 1 != 0 && animation.is_none() { return Err("BPG 애니메이션 제어 정보가 없습니다.".into()); }
    if picture_len != 0 && at + picture_len as usize > data.len() { return Err("BPG 이미지 데이터가 잘렸습니다.".into()); }
    Ok(Header {
        width, height, format, bit_depth, color_space, limited_range: flags2 & 0x02 != 0,
        alpha: alpha1 || alpha2, premultiplied: alpha1 && alpha2, w_plane: !alpha1 && alpha2,
        animation, data: at,
    })
}

/// MSB-first bit reader for the compact `hevc_header()`.
struct Bits<'a> { data: &'a [u8], bit: usize }

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32> {
        let byte = *self.data.get(self.bit / 8).ok_or("BPG HEVC 헤더가 잘렸습니다.")?;
        let value = (byte >> (7 - self.bit % 8)) & 1;
        self.bit += 1;
        Ok(u32::from(value))
    }

    fn bits(&mut self, n: u32) -> Result<u32> {
        (0..n).try_fold(0, |value, _| Ok((value << 1) | self.bit()?))
    }

    fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 { return Err("BPG HEVC 헤더 값이 너무 큽니다.".into()); }
        }
        Ok((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
}

/// RBSP writer that produces a NAL unit with emulation prevention bytes.
#[derive(Default)]
struct Writer { bytes: Vec<u8>, current: u8, used: u8 }

impl Writer {
    fn bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.current = (self.current << 1) | ((value >> i) & 1) as u8;
            self.used += 1;
            if self.used == 8 { self.bytes.push(self.current); self.current = 0; self.used = 0; }
        }
    }

    fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let length = 64 - code.leading_zeros();
        self.bits(0, length - 1);
        for i in (0..length).rev() { self.bits(((code >> i) & 1) as u32, 1); }
    }

    fn nal(mut self, header: [u8; 2]) -> Vec<u8> {
        self.bits(1, 1);
        while self.used != 0 { self.bits(0, 1); }
        let mut out = vec![0, 0, 1, header[0], header[1]];
        let mut zeros = 0;
        for byte in self.bytes {
            if zeros >= 2 && byte <= 3 { out.push(3); zeros = 0; }
            out.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        out
    }
}

fn profile_tier_level(w: &mut Writer, chroma: u32, bit_depth: u32) {
    let profile = if chroma == 1 && bit_depth == 8 { 1 } else if chroma == 1 && bit_depth == 10 { 2 } else { 4 };
    w.bits(0, 2); // general_profile_space
    w.bits(0, 1); // general_tier_flag
    w.bits(profile, 5);
    w.bits(1 << (31 - profile), 32); // compatibility flag for the chosen profile
    w.bits(0b1001, 4); // progressive, not interlaced, not non-packed, frame only
    w.bits(0, 32);
    w.bits(0, 12); // 43 constraint/reserved bits + general_inbld_flag
    w.bits(186, 8); // general_level_idc 6.2
}

/// Parses one compact header and returns the matching VPS+SPS Annex-B bytes.
fn parameter_sets(data: &[u8], at: &mut usize, h: &Header, chroma: u32) -> Result<Vec<u8>> {
    let len = ue7(data, at)? as usize;
    let body = data.get(*at..*at + len).ok_or("BPG HEVC 헤더가 잘렸습니다.")?;
    *at += len;
    let mut r = Bits { data: body, bit: 0 };
    let bit_depth = u32::from(h.bit_depth);
    let log2_min_cb = r.ue()?;
    let log2_diff_cb = r.ue()?;
    let log2_min_tb = r.ue()?;
    let log2_diff_tb = r.ue()?;
    let depth_intra = r.ue()?;
    if log2_min_cb > 3 || log2_diff_cb > 3 || log2_min_tb > 3 || log2_diff_tb > 3 || depth_intra > 4 {
        return Err("BPG HEVC 헤더 값이 올바르지 않습니다.".into());
    }
    let sao = r.bit()?;
    let pcm = r.bit()?;
    let pcm_fields = if pcm == 1 { Some([r.bits(4)?, r.bits(4)?, r.ue()?, r.ue()?, r.bit()?]) } else { None };
    let strong_intra = r.bit()?;
    let extension = r.bit()?;
    let (range, ext_bits) = if extension == 1 { (r.bit()?, r.bits(7)?) } else { (0, 0) };
    let range_flags = if range == 1 { Some(r.bits(9)?) } else { None };
    let cb = 1u32 << (log2_min_cb + 3);
    let padded = |n: u32| n.div_ceil(cb) * cb;

    let mut vps = Writer::default();
    vps.bits(0, 4); // vps_video_parameter_set_id
    vps.bits(1, 1); // base layer internal
    vps.bits(1, 1); // base layer available
    vps.bits(0, 6); // max layers - 1
    vps.bits(0, 3); // max sub layers - 1
    vps.bits(1, 1); // temporal id nesting
    vps.bits(0xffff, 16);
    profile_tier_level(&mut vps, chroma, bit_depth);
    vps.bits(1, 1); // sub layer ordering info present
    vps.ue(1); vps.ue(0); vps.ue(0); // DPB of two pictures: P frames reference the previous one
    vps.bits(0, 6); // vps_max_layer_id
    vps.ue(0); // num layer sets - 1
    vps.bits(0, 1); // timing info
    vps.bits(0, 1); // extension

    let mut sps = Writer::default();
    sps.bits(0, 4); // sps_video_parameter_set_id
    sps.bits(0, 3); // max sub layers - 1
    sps.bits(1, 1); // temporal id nesting
    profile_tier_level(&mut sps, chroma, bit_depth);
    sps.ue(0); // sps_seq_parameter_set_id
    sps.ue(chroma);
    if chroma == 3 { sps.bits(0, 1); } // separate_colour_plane_flag
    sps.ue(padded(h.width));
    sps.ue(padded(h.height));
    sps.bits(0, 1); // conformance window: cropped after decoding, as libbpg does
    sps.ue(bit_depth - 8);
    sps.ue(bit_depth - 8);
    sps.ue(4); // log2_max_pic_order_cnt_lsb_minus4
    sps.bits(1, 1);
    sps.ue(1); sps.ue(0); sps.ue(0);
    sps.ue(log2_min_cb);
    sps.ue(log2_diff_cb);
    sps.ue(log2_min_tb);
    sps.ue(log2_diff_tb);
    sps.ue(depth_intra); // inter depth equals intra depth
    sps.ue(depth_intra);
    sps.bits(0, 1); // scaling_list_enabled_flag
    sps.bits(1, 1); // amp_enabled_flag
    sps.bits(sao, 1);
    sps.bits(pcm, 1);
    if let Some([luma, chroma_depth, log2_min_pcm, log2_diff_pcm, loop_filter]) = pcm_fields {
        sps.bits(luma, 4);
        sps.bits(chroma_depth, 4);
        sps.ue(log2_min_pcm);
        sps.ue(log2_diff_pcm);
        sps.bits(loop_filter, 1);
    }
    sps.ue(0); // num_short_term_ref_pic_sets
    sps.bits(0, 1); // long_term_ref_pics_present_flag
    sps.bits(1, 1); // sps_temporal_mvp_enabled_flag
    sps.bits(strong_intra, 1);
    sps.bits(0, 1); // vui_parameters_present_flag
    sps.bits(extension, 1);
    if extension == 1 {
        sps.bits(range, 1);
        sps.bits(ext_bits, 7);
        if let Some(flags) = range_flags { sps.bits(flags, 9); }
    }
    let mut out = vps.nal([0x40, 0x01]);
    out.extend(sps.nal([0x42, 0x01]));
    Ok(out)
}

/// `frame_duration_sei` (payloadType 257) from a prefix SEI RBSP.
fn frame_duration(rbsp: &[u8]) -> Option<u32> {
    let mut at = 0;
    let mut field = || -> Option<u32> {
        let mut value = 0;
        loop {
            let byte = *rbsp.get(at)?;
            at += 1;
            value += u32::from(byte);
            if byte != 0xff { return Some(value); }
        }
    };
    let (kind, size) = (field()?, field()?);
    if kind != 257 || size < 2 { return None; }
    let duration = u32::from(u16::from_be_bytes([*rbsp.get(at)?, *rbsp.get(at + 1)?]));
    (duration != 0).then_some(duration)
}

fn samples(data: &PixelData) -> Vec<i32> {
    match data {
        PixelData::U8(v) => v.iter().map(|&s| i32::from(s)).collect(),
        PixelData::U16(v) => v.iter().map(|&s| i32::from(s)).collect(),
    }
}

const IP0: [i32; 4] = [40, -11, 4, -1];
const IP1: [i32; 7] = [-1, 4, -10, 57, 18, -6, 2];

/// libbpg `interp2_vh`: vertical then horizontal half-sample chroma upsampling of one row.
fn upsample_row(plane: &[i32], cw: usize, ch: usize, y: usize, width: usize, depth: i32, phase: u8, out: &mut [i32]) {
    let y2 = (y >> 1) as isize;
    let line = |k: isize| &plane[(y2 + k).clamp(0, ch as isize - 1) as usize * cw..][..cw];
    let rows: [&[i32]; 7] = std::array::from_fn(|i| line(i as isize - 3));
    let (shift, rnd) = (depth - 8, (1 << (depth - 8)) >> 1);
    let n2 = width.div_ceil(2);
    let tmp: Vec<i32> = (0..n2).map(|i| {
        let taps: [i32; 7] = std::array::from_fn(|k| rows[k][i]);
        let sum: i32 = if y & 1 == 0 {
            (0..7).map(|k| taps[k] * IP1[6 - k]).sum()
        } else {
            (0..7).map(|k| taps[k] * IP1[k]).sum()
        };
        ((sum + rnd) >> shift) as i16 as i32
    }).collect();
    let at = |i: isize| tmp[i.clamp(0, n2 as isize - 1) as usize];
    let max = (1 << depth) - 1;
    let shift1 = 20 - depth;
    let offset1 = 1 << (shift1 - 1);
    for (x, value) in out.iter_mut().enumerate().take(width) {
        let i = (x / 2) as isize;
        *value = if phase == 0 {
            if x % 2 == 0 {
                let shift0 = 14 - depth;
                (at(i) + ((1 << shift0) >> 1)) >> shift0
            } else {
                ((at(i - 3) + at(i + 4)) * IP0[3] + (at(i - 2) + at(i + 3)) * IP0[2]
                    + (at(i - 1) + at(i + 2)) * IP0[1] + (at(i) + at(i + 1)) * IP0[0] + offset1) >> shift1
            }
        } else {
            let taps: [i32; 7] = std::array::from_fn(|k| at(i + k as isize - 3));
            let sum: i32 = if x % 2 == 0 {
                (0..7).map(|k| taps[k] * IP1[6 - k]).sum()
            } else {
                (0..7).map(|k| taps[k] * IP1[k]).sum()
            };
            (sum + offset1) >> shift1
        }.clamp(0, max);
    }
}

/// libbpg `convert_init` for 8-bit output.
struct Convert { c_r_cr: i64, c_g_cb: i64, c_g_cr: i64, c_b_cb: i64, y_one: i64, y_offset: i64, center: i64 }

impl Convert {
    fn new(depth: u8, space: u8, limited: bool) -> Self {
        let shift = 22;
        let in_max = f64::from((1u32 << depth) - 1);
        let mult = 255. * f64::from(1u32 << shift) / in_max;
        let (mult_y, mult_c) = if limited {
            (255. * f64::from(1u32 << shift) / f64::from(219u32 << (depth - 8)), 255. * f64::from(1u32 << shift) / f64::from(224u32 << (depth - 8)))
        } else { (mult, mult) };
        let (k_r, k_b) = match space { 3 => (0.2126, 0.0722), 4 => (0.2627, 0.0593), _ => (0.299, 0.114) };
        let r = |v: f64| v.round_ties_even() as i64;
        let c_rnd = 1i64 << (shift - 1);
        let c_one = r(mult);
        let y_one = if limited { r(mult_y) } else { c_one };
        Self {
            c_r_cr: r(2. * (1. - k_r) * mult_c),
            c_g_cb: r(2. * k_b * (1. - k_b) / (1. - k_b - k_r) * mult_c),
            c_g_cr: r(2. * k_r * (1. - k_r) / (1. - k_b - k_r) * mult_c),
            c_b_cb: r(2. * (1. - k_b) * mult_c),
            y_one,
            y_offset: if limited { -(16i64 << (depth - 8)) * y_one + c_rnd } else { c_rnd },
            center: 1i64 << (depth - 1),
        }
    }

    fn pixel(&self, space: u8, depth: u8, limited: bool, y: i32, cb: i32, cr: i32) -> [u8; 3] {
        let clamp = |v: i64| (v >> 22).clamp(0, 255) as u8;
        let (y, cb, cr) = (i64::from(y), i64::from(cb), i64::from(cr));
        match space {
            1 if depth == 8 && !limited => [cr as u8, y as u8, cb as u8],
            1 => [clamp(cr * self.y_one + self.y_offset), clamp(y * self.y_one + self.y_offset), clamp(cb * self.y_one + self.y_offset)],
            2 => {
                let (cb, cr) = (cb - self.center, cr - self.center);
                [clamp((y - cb + cr) * self.y_one + self.y_offset), clamp((y + cb) * self.y_one + self.y_offset), clamp((y - cb - cr) * self.y_one + self.y_offset)]
            }
            _ => {
                let value = y * self.y_one;
                let (cb, cr) = (cb - self.center, cr - self.center);
                [
                    clamp(value + self.c_r_cr * cr + self.y_offset),
                    clamp(value - self.c_g_cb * cb - self.c_g_cr * cr + self.y_offset),
                    clamp(value + self.c_b_cb * cb + self.y_offset),
                ]
            }
        }
    }
}

fn to_rgba(frame: &rust_h265::Frame, h: &Header) -> Result<RgbaImage> {
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    let (w, hgt) = (h.width as usize, h.height as usize);
    if fw < w || fh < hgt { return Err("BPG 디코딩 결과 크기가 헤더보다 작습니다.".into()); }
    let (y, u, v) = (samples(&frame.y), samples(&frame.u), samples(&frame.v));
    let convert = Convert::new(h.bit_depth, h.color_space, h.limited_range);
    let depth = i32::from(h.bit_depth);
    let subsampled = frame.chroma_format_idc == 1;
    let (cw, ch) = if subsampled { (fw.div_ceil(2), fh.div_ceil(2)) } else { (fw, fh) };
    if y.len() < fw * fh || u.len() < cw * ch || v.len() < cw * ch { return Err("BPG 디코딩 결과 평면이 손상되었습니다.".into()); }
    // libbpg interpolates only the visible ceil(n/2) chroma samples.
    let (vis_w, vis_h) = (w.div_ceil(2), hgt.div_ceil(2));
    let phase = if h.format == 4 { 0 } else { 1 };
    let mut image = RgbaImage::new(h.width, h.height);
    let (mut cb_row, mut cr_row) = (vec![0; w], vec![0; w]);
    let crop = |plane: &[i32]| -> Vec<i32> {
        (0..vis_h).flat_map(|row| plane[row * cw..row * cw + vis_w].iter().copied()).collect()
    };
    let (u_vis, v_vis) = if subsampled { (crop(&u), crop(&v)) } else { (vec![], vec![]) };
    for row in 0..hgt {
        if subsampled {
            upsample_row(&u_vis, vis_w, vis_h, row, w, depth, phase, &mut cb_row);
            upsample_row(&v_vis, vis_w, vis_h, row, w, depth, phase, &mut cr_row);
        } else {
            cb_row.copy_from_slice(&u[row * cw..row * cw + w]);
            cr_row.copy_from_slice(&v[row * cw..row * cw + w]);
        }
        for x in 0..w {
            let rgb = convert.pixel(h.color_space, h.bit_depth, h.limited_range, y[row * fw + x], cb_row[x], cr_row[x]);
            image.put_pixel(x as u32, row as u32, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        }
    }
    Ok(image)
}

/// Decodes every frame. Fails explicitly for formats this decoder cannot reproduce.
pub fn decode(data: &[u8]) -> Result<(Header, Vec<Frame>)> { decode_with(data, false) }

/// `skip_alpha` exists for tests that compare the colour planes of alpha fixtures with libbpg.
fn decode_with(data: &[u8], skip_alpha: bool) -> Result<(Header, Vec<Frame>)> {
    if data.len() > MAX_INPUT { return Err("BPG 파일 크기가 안전 제한을 초과합니다.".into()); }
    let h = header(data)?;
    match h.format {
        0 => return Err("흑백 BPG는 지원하지 않습니다.".into()),
        2 | 5 => return Err("4:2:2 BPG는 지원하지 않습니다.".into()),
        _ => {}
    }
    if h.w_plane { return Err("CMYK(W 평면) BPG는 지원하지 않습니다.".into()); }
    if h.alpha && !skip_alpha { return Err("알파 채널(투명도)이 있는 BPG는 지원하지 않습니다.".into()); }
    let chroma = match h.format { 4 => 1, f => u32::from(f) };
    let mut at = h.data;
    if h.alpha {
        // The alpha stream's parameter sets are parsed only to reach the colour header.
        parameter_sets(data, &mut at, &h, 0)?;
    }
    let mut stream = parameter_sets(data, &mut at, &h, chroma)?;
    stream.extend([0, 0, 1]);
    stream.extend_from_slice(&data[at..]);
    let mut decoder = Decoder::new().with_display_order();
    let mut pictures = Vec::new();
    let mut durations = Vec::new();
    let mut pending = 1;
    let pixels = u64::from(h.width) * u64::from(h.height) * 4;
    let keep = |frame: rust_h265::Frame, pictures: &mut Vec<rust_h265::Frame>| -> Result<()> {
        if pictures.len() >= MAX_FRAMES || (pictures.len() as u64 + 1) * pixels > MAX_OUTPUT {
            return Err("BPG 프레임 수 또는 출력 크기가 안전 제한을 초과합니다.".into());
        }
        pictures.push(frame);
        Ok(())
    };
    for nal in parse_annex_b(&stream) {
        if nal.nuh_layer_id != 0 { continue; }
        match nal.nal_unit_type {
            NalUnitType::PrefixSei => { if let Some(d) = frame_duration(&nal.rbsp) { pending = d; } continue; }
            NalUnitType::SuffixSei => continue,
            _ => {}
        }
        let vcl = !matches!(nal.nal_unit_type, NalUnitType::Vps | NalUnitType::Sps | NalUnitType::Pps);
        if vcl && nal.rbsp.first().is_some_and(|b| b & 0x80 != 0) {
            durations.push(pending);
            pending = 1;
        }
        if let Some(frame) = decoder.decode_nal(&nal).map_err(|e| format!("BPG HEVC 디코딩 실패: {e}"))? { keep(frame, &mut pictures)?; }
        while let Some(frame) = decoder.next_output() { keep(frame, &mut pictures)?; }
        if h.animation.is_none() && !pictures.is_empty() { break; }
    }
    while let Some(frame) = decoder.flush() { keep(frame, &mut pictures)?; }
    if pictures.is_empty() { return Err("BPG 이미지 프레임을 찾지 못했습니다.".into()); }
    if h.animation.is_none() { pictures.truncate(1); }
    let frames = pictures.iter().enumerate().map(|(index, picture)| {
        let repeat = durations.get(index).copied().unwrap_or(1);
        let delay = h.animation.map_or((0, 1), |a| (u32::from(a.num) * repeat, u32::from(a.den)));
        Ok(Frame { image: to_rgba(picture, &h)?, delay })
    }).collect::<Result<Vec<_>>>()?;
    Ok((h, frames))
}

/// Encodes frames as APNG (animation) or PNG (single frame) for the web view.
pub fn encode_animation(frames: &[Frame], loops: u16) -> Result<Vec<u8>> {
    let first = frames.first().ok_or("프레임이 없습니다.")?;
    let (width, height) = first.image.dimensions();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    if frames.len() > 1 { encoder.set_animated(frames.len() as u32, u32::from(loops)).map_err(error)?; }
    let mut writer = encoder.write_header().map_err(error)?;
    for frame in frames {
        if frames.len() > 1 {
            // Browsers treat delays under 10 ms as 100 ms; match libbpg's minimum of 10 ms.
            let (num, den) = frame.delay;
            let ms = (u64::from(num) * 1000 / u64::from(den.max(1))).max(10);
            writer.set_frame_delay(u16::try_from(ms.min(65_535)).map_err(error)?, 1000).map_err(error)?;
        }
        writer.write_image_data(frame.image.as_raw()).map_err(error)?;
    }
    writer.finish().map_err(error)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rejects_bad_headers() {
        assert!(header(&[0x42, 0x50, 0x47, 0xfb, 0, 0]).is_err());
        // Oversized dimensions are rejected before any HEVC data is read.
        assert!(header(&[0x42, 0x50, 0x47, 0xfb, 0x20, 0, 0x84, 0x80, 0, 1, 0]).is_err());
        assert!(header(b"not bpg").is_err());
        let mut at = 0;
        assert!(ue7(&[0x80, 1], &mut at).is_err());
        at = 0;
        assert_eq!(ue7(&[0x84, 0x1e], &mut at).unwrap(), 542);
        at = 0;
        assert_eq!(ue7(&[0xac, 0xbe, 0x17], &mut at).unwrap(), 728_855);
    }

    #[test]
    fn writes_exp_golomb_with_emulation_prevention() {
        let mut w = Writer::default();
        for value in [0, 1, 2, 7] { w.ue(value); }
        // 1 010 011 0001000 + stop bit -> 1010 0110 0010 0010 (0xA6 0x22)
        assert_eq!(w.nal([0x40, 1]), vec![0, 0, 1, 0x40, 1, 0xa6, 0x22]);
        let mut zeros = Writer::default();
        zeros.bits(0, 24);
        assert_eq!(zeros.nal([0x42, 1]), vec![0, 0, 1, 0x42, 1, 0, 0, 3, 0, 0x80]);
    }

    fn fixture(name: &str) -> Option<Vec<u8>> {
        let dir = std::env::var_os("CLIPVIEW_BPG_FIXTURES").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../outputs/bpg-eval"));
        std::fs::read(dir.join(name)).ok()
    }

    /// Reference RGBA from libbpg 0.9.8 (`bpg-eval/*.rgba`, every frame concatenated).
    #[test]
    #[ignore = "requires local libbpg reference fixtures"]
    fn matches_libbpg_reference_pixels() {
        for (name, frames) in [("3-320p", 1), ("042-10b", 1), ("clock", 40)] {
            let data = fixture(&format!("{name}.bpg")).expect("fixture");
            let reference = fixture(&format!("{name}.rgba")).expect("reference");
            if name != "042-10b" { assert!(decode(&data).is_err(), "{name}: alpha BPG must be rejected"); }
            let (h, decoded) = decode_with(&data, true).unwrap();
            assert_eq!(decoded.len(), frames, "{name}");
            let ours: Vec<u8> = decoded.iter().flat_map(|f| f.image.as_raw().iter().copied()).collect();
            assert_eq!(ours.len(), reference.len(), "{name} {h:?}");
            // The alpha plane is monochrome HEVC, which the Rust decoder cannot read, so alpha
            // files are compared where libbpg's output is opaque (premultiplication is a no-op).
            let mut compared = 0;
            let differing = ours.chunks_exact(4).zip(reference.chunks_exact(4))
                .filter(|(_, r)| !h.alpha || r[3] == 255)
                .inspect(|_| compared += 1)
                .filter(|(o, r)| o[..3] != r[..3] || (!h.alpha && o[3] != r[3])).count();
            assert!(compared > ours.len() / 4 / 20, "{name}: too few opaque pixels ({compared})");
            assert_eq!(differing, 0, "{name}: {differing} of {compared} pixels differ");
            if frames > 1 {
                assert!(decoded.iter().all(|f| f.delay == (1, 20)), "{name}");
                let apng = encode_animation(&decoded, h.animation.unwrap().loops).unwrap();
                assert!(apng.windows(4).any(|w| w == b"acTL"));
            }
        }
    }
}

