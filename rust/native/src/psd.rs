use std::{
    fs::File,
    io::{BufRead, BufReader, Cursor, Seek},
    path::Path,
};

use flate2::{Decompress, FlushDecompress, Status};
use image::RgbaImage;
use lcms2::{Flags, Intent, PixelFormat, Profile, Transform};

use crate::{Result, error, picture::check_dimensions};

#[path = "psd_layers.rs"]
pub(crate) mod layers;

const TRUNCATED: &str = "PSD/PSB 병합 이미지 데이터가 잘렸습니다.";
const MAX_PLANAR_BYTES: u64 = 384 << 20;

struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let (bytes, rest) = self.0.split_at_checked(count).ok_or(TRUNCATED)?;
        self.0 = rest;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?.try_into().map_err(error)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn length(&mut self, large: bool) -> Result<usize> {
        let len = if large {
            u64::from_be_bytes(self.array()?)
        } else {
            u64::from(self.u32()?)
        };
        usize::try_from(len).map_err(error)
    }

    fn section(&mut self, large: bool) -> Result<Self> {
        let len = self.length(large)?;
        Ok(Self(self.take(len)?))
    }
}

#[derive(Default)]
struct Resources<'a> {
    icc: Option<&'a [u8]>,
    transparent_index: Option<u16>,
    alpha_ids: Option<&'a [u8]>,
    no_merged_preview: bool,
}

impl<'a> Resources<'a> {
    fn read(mut section: Bytes<'a>) -> Result<Self> {
        let mut result = Self::default();
        while !section.0.is_empty() {
            section.take(4)?; // Resource signature.
            let id = section.u16()?;
            let name_len = usize::from(section.take(1)?[0]);
            section.take(name_len + (name_len + 1) % 2)?;
            let mut data = section.section(false)?;
            section.take(data.0.len() % 2)?;
            match id {
                1039 if !data.0.is_empty() => result.icc = Some(data.0),
                1047 if data.0.len() >= 2 => result.transparent_index = Some(data.u16()?),
                1053 if data.0.len() >= 4 && data.0.len() % 4 == 0 => {
                    result.alpha_ids = Some(data.0)
                }
                1057 if data.0.get(4) == Some(&0) => result.no_merged_preview = true,
                _ => {}
            }
        }
        Ok(result)
    }
}

struct Sections<R> {
    source: R,
    position: u64,
    end: u64,
}

impl<R: BufRead + Seek> Sections<R> {
    fn new(source: R, len: u64) -> Self {
        Self { source, position: 0, end: len }
    }

    fn remaining(&self) -> u64 {
        self.end - self.position
    }

    fn read(&mut self, out: &mut [u8]) -> Result<()> {
        let count = out.len() as u64;
        if count > self.remaining() {
            return Err(TRUNCATED.into());
        }
        self.source.read_exact(out).map_err(error)?;
        self.position += count;
        Ok(())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut bytes = [0; N];
        self.read(&mut bytes)?;
        Ok(bytes)
    }

    fn rest(&mut self) -> Result<Vec<u8>> {
        let mut bytes = vec![0; usize::try_from(self.remaining()).map_err(error)?];
        self.read(&mut bytes)?;
        Ok(bytes)
    }

    fn skip(&mut self, count: u64) -> Result<()> {
        if count > self.remaining() {
            return Err(TRUNCATED.into());
        }
        if count != 0 {
            self.source.seek_relative(i64::try_from(count).map_err(error)?).map_err(error)?;
            self.position += count;
        }
        Ok(())
    }

    fn length(&mut self, large: bool) -> Result<u64> {
        Ok(if large {
            u64::from_be_bytes(self.array()?)
        } else {
            u64::from(u32::from_be_bytes(self.array()?))
        })
    }

    fn section<T>(&mut self, large: bool, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let count = self.length(large)?;
        if count > self.remaining() {
            return Err(TRUNCATED.into());
        }
        let end = self.end;
        self.end = self.position + count;
        let value = parse(self).and_then(|value| {
            self.skip(self.remaining())?;
            Ok(value)
        });
        self.end = end;
        value
    }

    fn skip_padding(&mut self) -> Result<()> {
        while self.remaining() != 0 {
            let remaining = self.remaining();
            let bytes = self.source.fill_buf().map_err(error)?;
            if bytes.is_empty() {
                return Err(TRUNCATED.into());
            }
            let bytes = &bytes[..bytes.len().min(remaining as usize)];
            let count = bytes.iter().take_while(|&&byte| byte == 0).count();
            let done = count < bytes.len();
            self.source.consume(count);
            self.position += count as u64;
            if done {
                break;
            }
        }
        Ok(())
    }
}

fn wide_key(large: bool, signature: &[u8; 4], key: &[u8; 4]) -> bool {
    signature == b"8B64" || large && matches!(key,
        b"LMsk" | b"Lr16" | b"Lr32" | b"Layr" | b"Mt16" | b"Mt32" | b"Mtrn" |
        b"Alph" | b"FMsk" | b"lnk2" | b"FEid" | b"FXid" | b"PxSD")
}

fn layer_flags(section: &mut Sections<impl BufRead + Seek>, large: bool) -> Result<(i16, bool)> {
    if section.remaining() == 0 {
        return Ok((0, false));
    }
    let mut count = section.section(large, |info| {
        if info.remaining() == 0 { Ok(0) } else { Ok(i16::from_be_bytes(info.array()?)) }
    })?;
    // Some CSP exports terminate the section with zero alignment padding
    // rather than a four-byte Global Layer Mask Information length.
    if section.remaining() < 4 {
        section.skip_padding()?;
        return if section.remaining() == 0 { Ok((count, count < 0)) } else { Err(TRUNCATED.into()) };
    }
    let mask_len = section.length(false)?;
    section.skip(mask_len)?;
    let mut alpha = count < 0;
    while section.remaining() >= 12 {
        let signature = section.array::<4>()?;
        if !matches!(&signature, b"8BIM" | b"8B64") {
            break;
        }
        let key = section.array::<4>()?;
        let long_length = wide_key(large, &signature, &key);
        section.section(long_length, |data| {
            if matches!(&key, b"Lr16" | b"Lr32" | b"Layr") && data.remaining() >= 2 {
                let nested = i16::from_be_bytes(data.array()?);
                if count == 0 {
                    count = nested;
                }
                alpha |= nested < 0;
            }
            Ok(())
        })?;
        alpha |= matches!(&key, b"Mtrn" | b"Mt16" | b"Mt32");
        // Photoshop writers pad these blocks differently. Skip bounded zero padding.
        section.skip_padding()?;
    }
    Ok((count, alpha))
}

struct Zip<'a> {
    input: &'a [u8],
    decoder: Decompress,
    ended: bool,
}

impl Zip<'_> {
    fn read(&mut self, mut output: &mut [u8]) -> Result<()> {
        while !output.is_empty() && !self.ended {
            let (old_in, old_out) = (self.decoder.total_in(), self.decoder.total_out());
            let status = self
                .decoder
                .decompress(self.input, output, FlushDecompress::None)
                .map_err(error)?;
            let used = (self.decoder.total_in() - old_in) as usize;
            let written = (self.decoder.total_out() - old_out) as usize;
            self.input = &self.input[used..];
            output = &mut output[written..];
            self.ended = status == Status::StreamEnd;
            if used == 0 && written == 0 && !self.ended {
                return Err(TRUNCATED.into());
            }
        }
        if output.is_empty() {
            Ok(())
        } else {
            Err(TRUNCATED.into())
        }
    }

    fn finish(mut self) -> Result<()> {
        if !self.ended {
            let written = self.decoder.total_out();
            let status = self
                .decoder
                .decompress(self.input, &mut [0], FlushDecompress::Finish)
                .map_err(error)?;
            if self.decoder.total_out() != written {
                return Err("PSD/PSB ZIP 픽셀 수가 헤더와 일치하지 않습니다.".into());
            }
            if status != Status::StreamEnd {
                return Err(TRUNCATED.into());
            }
        }
        Ok(())
    }
}

enum Rows<'a> {
    Raw(Bytes<'a>),
    Rle {
        sizes: Bytes<'a>,
        data: Bytes<'a>,
        large: bool,
    },
    Zip(Zip<'a>),
}

impl<'a> Rows<'a> {
    fn read<'row>(&mut self, scratch: &'row mut [u8]) -> Result<&'row mut [u8]> {
        match self {
            Self::Raw(data) => scratch.copy_from_slice(data.take(scratch.len())?),
            Self::Rle { sizes, data, large } => {
                let count = if *large {
                    sizes.u32()? as usize
                } else {
                    usize::from(sizes.u16()?)
                };
                unpack_bits(data.take(count)?, scratch)?;
            }
            Self::Zip(zip) => zip.read(scratch)?,
        }
        Ok(scratch)
    }

    fn finish(self) -> Result<()> {
        if let Self::Zip(zip) = self {
            zip.finish()?;
        }
        Ok(())
    }
}

fn unpack_bits(mut input: &[u8], mut output: &mut [u8]) -> Result<()> {
    let invalid = || "PSD/PSB RLE 행 길이가 올바르지 않습니다.".to_owned();
    while let Some((&op, rest)) = input.split_first() {
        input = rest;
        match op {
            0..=127 => {
                let count = usize::from(op) + 1;
                let (literal, rest) = input.split_at_checked(count).ok_or_else(invalid)?;
                let (write, left) = output.split_at_mut_checked(count).ok_or_else(invalid)?;
                write.copy_from_slice(literal);
                output = left;
                input = rest;
            }
            128 => {} // PackBits no-op.
            129..=255 => {
                let count = 257 - usize::from(op);
                let (&value, rest) = input.split_first().ok_or_else(invalid)?;
                let (write, left) = output.split_at_mut_checked(count).ok_or_else(invalid)?;
                write.fill(value);
                output = left;
                input = rest;
            }
        }
    }
    if output.is_empty() {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn undo_prediction(row: &mut [u8], depth: u16) {
    if depth == 16 {
        let mut last = 0u16;
        for bytes in row.chunks_exact_mut(2) {
            last = last.wrapping_add(u16::from_be_bytes([bytes[0], bytes[1]]));
            bytes.copy_from_slice(&last.to_be_bytes());
        }
    } else {
        let mut last = 0u8;
        for byte in row {
            last = last.wrapping_add(*byte);
            *byte = last;
        }
    }
}

fn remove_white_matte(pixel: &mut [u8; 4]) {
    let alpha = i32::from(pixel[3]);
    if (1..255).contains(&alpha) {
        for channel in &mut pixel[..3] {
            let color = (i32::from(*channel) + alpha - 255).max(0);
            *channel = ((color * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

const COLOR_BATCH: usize = 4096;

fn convert_colors(rgba: &mut [[u8; 4]], icc: Option<&[u8]>, mode: u16, alpha: &[u8]) -> Result<()> {
    let profile = icc.map(Profile::new_icc).transpose().map_err(error)?;
    let srgb = Profile::new_srgb();
    match (mode, profile) {
        (4, profile) => {
            let icc = profile
                .map(|profile| {
                    Transform::<[u8; 4], [u8; 3]>::new(
                        &profile,
                        PixelFormat::CMYK_8,
                        &srgb,
                        PixelFormat::RGB_8,
                        Intent::Perceptual,
                    )
                })
                .transpose()
                .map_err(error)?;
            let mut rgb = vec![[0; 3]; COLOR_BATCH];
            for (index, batch) in rgba.chunks_mut(COLOR_BATCH).enumerate() {
                let output = &mut rgb[..batch.len()];
                if let Some(icc) = &icc {
                    icc.transform_pixels(batch, output);
                } else {
                    for (value, out) in batch.iter().zip(output.iter_mut()) {
                        for i in 0..3 {
                            out[i] = (((255 - u16::from(value[i])) * (255 - u16::from(value[3]))
                                + 127)
                                / 255) as u8;
                        }
                    }
                }
                for (i, (pixel, &[r, g, b])) in batch.iter_mut().zip(output.iter()).enumerate() {
                    *pixel = [
                        r,
                        g,
                        b,
                        alpha.get(index * COLOR_BATCH + i).copied().unwrap_or(255),
                    ];
                }
            }
        }
        (1, Some(profile)) => {
            let icc = Transform::<u8, [u8; 3]>::new(
                &profile,
                PixelFormat::GRAY_8,
                &srgb,
                PixelFormat::RGB_8,
                Intent::Perceptual,
            )
            .map_err(error)?;
            let mut gray = vec![0; COLOR_BATCH];
            let mut rgb = vec![[0; 3]; COLOR_BATCH];
            for batch in rgba.chunks_mut(COLOR_BATCH) {
                for (pixel, value) in batch.iter().zip(&mut gray) {
                    *value = pixel[0];
                }
                icc.transform_pixels(&gray[..batch.len()], &mut rgb[..batch.len()]);
                for (pixel, rgb) in batch.iter_mut().zip(&rgb) {
                    pixel[..3].copy_from_slice(rgb);
                }
            }
        }
        (2 | 3, Some(profile)) => {
            let icc = Transform::<[u8; 4], [u8; 4]>::new_flags(
                &profile,
                PixelFormat::RGBA_8,
                &srgb,
                PixelFormat::RGBA_8,
                Intent::Perceptual,
                Flags::COPY_ALPHA,
            )
            .map_err(error)?;
            icc.transform_in_place(rgba);
        }
        _ => {}
    }
    Ok(())
}

struct Composite {
    large: bool,
    width: u32,
    height: u32,
    channels: usize,
    depth: u16,
    mode: u16,
    color_channels: usize,
    palette: Vec<u8>,
    resources: Vec<u8>,
    layer_count: i16,
    merged_alpha: bool,
    layer_offset: u64,
}

fn composite(data: &mut Sections<impl BufRead + Seek>) -> Result<Composite> {
    if &data.array::<4>()? != b"8BPS" {
        return Err("PSD/PSB 파일 헤더가 올바르지 않습니다.".into());
    }
    let large = match u16::from_be_bytes(data.array()?) {
        1 => false,
        2 => true,
        _ => return Err("지원하지 않는 PSD/PSB 버전입니다.".into()),
    };
    data.skip(6)?;
    let channels = usize::from(u16::from_be_bytes(data.array()?));
    let height = u32::from_be_bytes(data.array()?);
    let width = u32::from_be_bytes(data.array()?);
    check_dimensions(width, height)?;
    if width.max(height) > if large { 300_000 } else { 30_000 } || !(1..=56).contains(&channels) {
        return Err("PSD/PSB 헤더의 이미지 크기 또는 채널 수가 올바르지 않습니다.".into());
    }
    let depth = u16::from_be_bytes(data.array()?);
    let mode = u16::from_be_bytes(data.array()?);
    let color_channels = match (mode, depth) {
        (0, 1) | (1, 8 | 16) | (2, 8) => 1,
        (3, 8 | 16) => 3,
        (4, 8 | 16) => 4,
        _ => {
            return Err(format!(
                "지원하지 않는 PSD/PSB 병합 색상 모드입니다 (모드 {mode}, {depth}비트)."
            ));
        }
    };
    if channels < color_channels {
        return Err("PSD/PSB 색상 채널이 누락되었습니다.".into());
    }
    let palette = data.section(false, |data| {
        if mode != 2 {
            return Ok(Vec::new());
        }
        if data.remaining() != 768 {
            return Err("PSD/PSB 색상표가 올바르지 않습니다.".into());
        }
        data.rest()
    })?;
    let resources = data.section(false, |data| {
        if data.remaining() > 128 << 20 {
            return Err("PSD/PSB 이미지 리소스 크기가 안전 제한을 초과합니다.".into());
        }
        data.rest()
    })?;
    let layer_offset = data.position;
    let (layer_count, merged_alpha) = data.section(large, |data| layer_flags(data, large))?;
    Ok(Composite {
        large, width, height, channels, depth, mode, color_channels, palette, resources,
        layer_count, merged_alpha, layer_offset,
    })
}

pub fn merged_file(path: &Path, max_source: u64, max_composite: u64) -> Result<RgbaImage> {
    let file = File::open(path).map_err(error)?;
    let len = file.metadata().map_err(error)?.len();
    if len > max_source {
        return Err("이미지 파일 크기가 안전 제한을 초과합니다.".into());
    }
    let mut source = Sections::new(BufReader::new(file), len);
    let header = composite(&mut source)?;
    if source.remaining() > max_composite {
        return Err("PSD/PSB 합성 이미지 데이터가 안전 제한을 초과합니다.".into());
    }
    decode_composite(header, &source.rest()?)
}

pub fn merged(data: &[u8]) -> Result<RgbaImage> {
    let mut source = Sections::new(Cursor::new(data), data.len() as u64);
    let header = composite(&mut source)?;
    decode_composite(header, &data[source.position as usize..])
}

fn copy_channel<const CHANNEL: usize>(pixels: &mut [[u8; 4]], samples: &[u8]) {
    for (pixel, &sample) in pixels.iter_mut().zip(samples) {
        pixel[CHANNEL] = sample;
    }
}

/// The saved composite's planes are decoded one scanline at a time into the output image.
fn decode_composite(header: Composite, data: &[u8]) -> Result<RgbaImage> {
    let Composite {
        large, width, height, channels, depth, mode, color_channels, palette, resources,
        layer_count, merged_alpha, layer_offset: _,
    } = header;
    let resources = Resources::read(Bytes(&resources))?;
    if resources.no_merged_preview {
        return Err("PSD/PSB에 표시할 수 있는 병합 이미지가 저장되지 않았습니다.".into());
    }
    let mut data = Bytes(data);
    let alpha_channel = if channels <= color_channels || mode == 0 || mode == 2 {
        None
    } else if merged_alpha {
        Some(color_channels)
    } else if let Some(ids) = resources.alpha_ids {
        channels
            .checked_sub(ids.len() / 4)
            .and_then(|base| {
                ids.chunks_exact(4)
                    .position(|id| id == [0; 4])
                    .map(|index| base + index)
            })
            .filter(|&channel| channel >= color_channels)
    } else if layer_count == 0 && channels == color_channels + 1 {
        // Flat exporters can omit both the layer count and alpha identifiers.
        Some(color_channels)
    } else {
        None
    };

    let compression = data.u16()?;
    let width_usize = width as usize;
    let height_usize = height as usize;
    let row_bytes = (width_usize * usize::from(depth)).div_ceil(8);
    let row_count = height_usize * channels;
    if row_bytes as u64 * row_count as u64 > MAX_PLANAR_BYTES {
        return Err("PSD/PSB 압축 해제 크기가 안전 제한을 초과합니다.".into());
    }
    let mut rows = match compression {
        0 => {
            if data.0.len() < row_bytes * row_count {
                return Err(TRUNCATED.into());
            }
            Rows::Raw(data)
        }
        1 => Rows::Rle {
            sizes: Bytes(data.take(row_count * if large { 4 } else { 2 })?),
            data,
            large,
        },
        2 | 3 if compression == 2 || depth != 1 => Rows::Zip(Zip {
            input: data.0,
            decoder: Decompress::new(true),
            ended: false,
        }),
        _ => {
            return Err(format!(
                "지원하지 않는 PSD/PSB 압축 방식입니다 ({compression})."
            ));
        }
    };
    let pixel_count = width_usize * height_usize;
    let mut rgba = vec![255; pixel_count * 4];
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    let mut alpha = if mode == 4 && alpha_channel.is_some() {
        vec![0; pixel_count]
    } else {
        Vec::new()
    };
    let mut scratch = vec![0; row_bytes];
    for channel in 0..channels {
        for y in 0..height_usize {
            let line = rows.read(&mut scratch)?;
            if channel >= color_channels && alpha_channel != Some(channel) {
                continue;
            }
            if compression == 3 {
                undo_prediction(line, depth);
            }
            let start = y * width_usize;
            let row = &mut pixels[start..start + width_usize];
            if mode == 3 && depth == 8 {
                match channel {
                    0 => copy_channel::<0>(row, line),
                    1 => copy_channel::<1>(row, line),
                    2 => copy_channel::<2>(row, line),
                    _ => copy_channel::<3>(row, line),
                }
                continue;
            }
            for (x, pixel) in row.iter_mut().enumerate() {
                let sample = match depth {
                    1 => {
                        if line[x / 8] & (0x80 >> (x % 8)) == 0 {
                            255
                        } else {
                            0
                        }
                    }
                    8 => line[x],
                    16 => {
                        ((u32::from(u16::from_be_bytes([line[x * 2], line[x * 2 + 1]])) + 128)
                            / 257) as u8
                    }
                    _ => unreachable!(),
                };
                if alpha_channel == Some(channel) {
                    if mode == 4 {
                        alpha[start + x] = sample;
                    } else {
                        pixel[3] = sample;
                    }
                } else {
                    match mode {
                        0 | 1 => pixel[..3].fill(sample),
                        2 => {
                            let index = usize::from(sample);
                            *pixel = [
                                palette[index],
                                palette[256 + index],
                                palette[512 + index],
                                if resources.transparent_index == Some(u16::from(sample)) {
                                    0
                                } else {
                                    255
                                },
                            ];
                        }
                        3 => pixel[channel] = sample,
                        4 => pixel[channel] = 255 - sample,
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
    rows.finish()?;
    convert_colors(pixels, resources.icc, mode, &alpha)?;
    if merged_alpha && alpha_channel.is_some() && matches!(mode, 1 | 3 | 4) {
        for pixel in pixels {
            remove_white_matte(pixel);
        }
    }
    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "PSD/PSB 픽셀 수가 올바르지 않습니다.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    fn block(out: &mut Vec<u8>, bytes: &[u8], large: bool) {
        if large {
            out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        } else {
            out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        }
        out.extend_from_slice(bytes);
    }

    struct Fixture<'a> {
        large: bool,
        mode: u16,
        depth: u16,
        channels: u16,
        count: i16,
        compression: u16,
        resources: &'a [u8],
        planes: &'a [u8],
    }

    fn fixture(f: Fixture<'_>) -> Vec<u8> {
        let mut file = b"8BPS".to_vec();
        file.extend_from_slice(&(if f.large { 2u16 } else { 1u16 }).to_be_bytes());
        file.extend_from_slice(&[0; 6]);
        file.extend_from_slice(&f.channels.to_be_bytes());
        file.extend_from_slice(&2u32.to_be_bytes()); // Two rows exercise predictor reset.
        file.extend_from_slice(&2u32.to_be_bytes());
        file.extend_from_slice(&f.depth.to_be_bytes());
        file.extend_from_slice(&f.mode.to_be_bytes());
        block(&mut file, &[], false);
        block(&mut file, f.resources, false);
        let mut layer = Vec::new();
        block(&mut layer, &f.count.to_be_bytes(), f.large);
        block(&mut layer, &[], false);
        block(&mut file, &layer, f.large);
        file.extend_from_slice(&f.compression.to_be_bytes());
        file.extend_from_slice(f.planes);
        file
    }

    fn rgb(planes: &[u8], compression: u16, large: bool) -> Vec<u8> {
        fixture(Fixture {
            large,
            mode: 3,
            depth: 8,
            channels: 3,
            count: 0,
            compression,
            resources: &[],
            planes,
        })
    }

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        let mut writer = ZlibEncoder::new(Vec::new(), Compression::fast());
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap()
    }

    #[test]
    fn raw_and_zip_rgb_match_planar_composite() {
        let planes = [40, 1, 250, 93, 120, 2, 251, 94, 220, 3, 252, 95];
        let expected = [
            40, 120, 220, 255, 1, 2, 3, 255, 250, 251, 252, 255, 93, 94, 95, 255,
        ];
        for (compression, input) in [(0, planes.to_vec()), (2, zlib(&planes))] {
            assert_eq!(
                merged(&rgb(&input, compression, false)).unwrap().as_raw(),
                &expected
            );
        }
        // Deltas wrap, and each scanline starts with its absolute sample.
        let deltas = [40, 217, 250, 99, 120, 138, 251, 99, 220, 39, 252, 99];
        assert_eq!(
            merged(&rgb(&zlib(&deltas), 3, true)).unwrap().as_raw(),
            &expected
        );
    }

    #[test]
    fn psb_rle_uses_four_byte_scanline_counts() {
        let mut payload = Vec::new();
        for _ in 0..6 {
            payload.extend_from_slice(&2u32.to_be_bytes());
        }
        for value in [10, 20, 30, 40, 50, 60] {
            payload.extend_from_slice(&[255, value]);
        }
        let output = merged(&rgb(&payload, 1, true)).unwrap();
        assert_eq!(output.get_pixel(0, 0).0, [10, 30, 50, 255]);
        assert_eq!(output.get_pixel(1, 1).0, [20, 40, 60, 255]);
    }

    #[test]
    fn disk_composite_skips_layer_payload_but_reads_nested_alpha_flags() {
        let planes = [
            255, 127, 1, 255, 127, 127, 2, 255, 127, 255, 3, 255, 128, 128, 255, 0,
        ];
        let mut file = fixture(Fixture {
            large: true, mode: 3, depth: 8, channels: 4, count: 0, compression: 0,
            resources: &[], planes: &planes,
        });
        let mut layers = Vec::new();
        block(&mut layers, &[0; 16_384], true);
        block(&mut layers, &[], false);
        layers.extend_from_slice(b"8BIMLr16");
        block(&mut layers, &(-1i16).to_be_bytes(), true);
        let mut section = Vec::new();
        block(&mut section, &layers, true);
        // Header (26), color and resources (4 each), and original PSB layer block (22).
        file.splice(34..56, section);
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        temp.write_all(&file).unwrap();
        let decoded = merged_file(temp.path(), file.len() as u64, 128).unwrap();
        assert_eq!(decoded, merged(&file).unwrap());
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 128]);
        assert!(merged_file(temp.path(), file.len() as u64 - 1, 128).is_err());
        assert!(merged_file(temp.path(), file.len() as u64, 2).is_err());

        // A nested length may not consume bytes outside the declared layer section.
        let nested_length = 42 + layers.len() - 10;
        file[nested_length..nested_length + 8].copy_from_slice(&3u64.to_be_bytes());
        assert!(merged(&file).is_err());
    }

    #[test]
    fn csp_layer_info_without_global_mask_accepts_alignment_padding() {
        let flags = |bytes: &[u8]| layer_flags(&mut Sections::new(Cursor::new(bytes), bytes.len() as u64), false);
        assert_eq!(
            flags(&[0, 0, 0, 3, 0, 56, 0, 0, 0]).unwrap(),
            (56, false)
        );
        assert!(flags(&[0, 0, 0, 3, 0, 56, 0, 0, 1]).is_err());
    }

    #[test]
    fn gray_zip_prediction_accumulates_full_sixteen_bit_samples() {
        let deltas: Vec<_> = [0x29ffu16, 0x0102, 0xffff, 1]
            .into_iter()
            .flat_map(u16::to_be_bytes)
            .collect();
        let file = fixture(Fixture {
            large: false,
            mode: 1,
            depth: 16,
            channels: 1,
            count: 0,
            compression: 3,
            resources: &[],
            planes: &zlib(&deltas),
        });
        let output = merged(&file).unwrap();
        assert_eq!(
            output.pixels().map(|p| p[0]).collect::<Vec<_>>(),
            [42, 43, 255, 0]
        );
    }

    #[test]
    fn transparency_uses_layer_flag_not_spot_channel() {
        let planes = [
            255, 127, 1, 255, 127, 127, 2, 255, 127, 255, 3, 255, 128, 128, 255, 0,
        ];
        let make = |count| {
            fixture(Fixture {
                large: false,
                mode: 3,
                depth: 8,
                channels: 4,
                count,
                compression: 0,
                resources: &[],
                planes: &planes,
            })
        };
        let output = merged(&make(-1)).unwrap();
        assert_eq!(output.get_pixel(0, 0).0, [255, 0, 0, 128]);
        assert_eq!(output.get_pixel(1, 0).0, [0, 0, 255, 128]);
        assert_eq!(output.get_pixel(1, 1)[3], 0);
        assert_eq!(
            merged(&make(1)).unwrap().get_pixel(0, 0).0,
            [255, 127, 127, 255]
        );
    }

    #[test]
    fn cmyk_inverts_ink_values_but_not_alpha() {
        let planes = [
            0, 255, 255, 255, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 255, 255, 255,
            255, 0,
        ];
        let output = merged(&fixture(Fixture {
            large: false,
            mode: 4,
            depth: 8,
            channels: 5,
            count: -1,
            compression: 0,
            resources: &[],
            planes: &planes,
        }))
        .unwrap();
        assert_eq!(
            output.as_raw(),
            &[
                0, 255, 255, 255, 255, 0, 255, 255, 0, 0, 0, 255, 255, 255, 255, 0
            ]
        );
    }

    #[test]
    fn zip_rows_refuse_extra_output_and_truncated_checksum() {
        let oversized = zlib(&[0; 13]);
        assert!(merged(&rgb(&oversized, 2, false)).is_err());
        let mut incomplete = zlib(&[0; 12]);
        incomplete.pop();
        assert!(merged(&rgb(&incomplete, 2, false)).is_err());
    }

    #[test]
    fn invalid_packbits_cannot_write_past_row() {
        let mut row = [0; 2];
        assert!(unpack_bits(&[254, 1], &mut row).is_err());
        assert!(unpack_bits(&[1, 1], &mut row).is_err());
        assert!(unpack_bits(&[128, 255, 123, 128], &mut row).is_ok());
        assert_eq!(row, [123, 123]);
    }

    #[test]
    fn false_merged_preview_flag_is_not_treated_as_valid() {
        let resources = b"8BIM\x04\x21\0\0\0\0\0\x05\0\0\0\x01\0\0";
        let file = fixture(Fixture {
            large: false,
            mode: 3,
            depth: 8,
            channels: 3,
            count: 0,
            compression: 0,
            resources,
            planes: &[0; 12],
        });
        assert!(merged(&file).unwrap_err().contains("저장되지 않았습니다"));
    }
}
