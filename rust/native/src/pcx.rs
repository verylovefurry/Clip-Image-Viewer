//! ZSoft PCX decoding: 1/2/4/8-bit indexed (header or trailing 256-colour palette),
//! 1-bit EGA planar, and 8-bit planar RGB/RGBA, RLE or uncompressed.

use image::{Rgba, RgbaImage};

use crate::Result;

const MAX_PIXELS: u64 = 64 * 1024 * 1024;

fn invalid() -> String { "PCX 파일이 손상되었거나 지원하지 않는 형식입니다.".into() }

pub fn decode(data: &[u8]) -> Result<RgbaImage> {
    if data.len() < 128 || data[0] != 0x0a || data[2] > 1 { return Err(invalid()); }
    let word = |at: usize| u32::from(u16::from_le_bytes([data[at], data[at + 1]]));
    let (bits, planes) = (data[3], data[65]);
    let (x0, y0, x1, y1) = (word(4), word(6), word(8), word(10));
    if x1 < x0 || y1 < y0 { return Err(invalid()); }
    let (width, height) = (x1 - x0 + 1, y1 - y0 + 1);
    let stride = word(66) as usize;
    if u64::from(width) * u64::from(height) > MAX_PIXELS { return Err("PCX 이미지 크기가 안전 제한을 초과합니다.".into()); }
    let supported = matches!((bits, planes), (1 | 2 | 4 | 8, 1) | (1, 2..=4) | (8, 3 | 4));
    let line_bits = width as usize * usize::from(bits);
    if !supported || stride == 0 || stride * 8 < line_bits || stride > 65_535 { return Err(invalid()); }
    let line = stride * usize::from(planes);
    let total = line.checked_mul(height as usize).ok_or_else(invalid)?;
    let mut pixels = Vec::with_capacity(total);
    let mut at = 128;
    if data[2] == 1 {
        while pixels.len() < total {
            let byte = *data.get(at).ok_or_else(invalid)?;
            at += 1;
            if byte >= 0xc0 {
                let value = *data.get(at).ok_or_else(invalid)?;
                at += 1;
                let count = usize::from(byte & 0x3f).min(total - pixels.len());
                pixels.extend(std::iter::repeat_n(value, count));
            } else { pixels.push(byte); }
        }
    } else {
        pixels.extend_from_slice(data.get(at..at + total).ok_or_else(invalid)?);
    }
    let header_palette: Vec<[u8; 3]> = data[16..64].chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    let trailing = (bits == 8 && planes == 1 && data.len() >= 128 + 769 && data[data.len() - 769] == 0x0c)
        .then(|| data[data.len() - 768..].chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect::<Vec<_>>());
    let index_color = |index: usize| -> [u8; 3] {
        match (&trailing, bits) {
            (Some(palette), _) => palette[index],
            (None, 8) => [index as u8; 3],
            // 1-bit monochrome files commonly leave the header palette black.
            (None, 1) if planes == 1 && header_palette[..2] == [[0; 3], [0; 3]] => if index == 0 { [0; 3] } else { [255; 3] },
            _ => header_palette[index % 16],
        }
    };
    let mut image = RgbaImage::new(width, height);
    for y in 0..height as usize {
        let row = &pixels[y * line..(y + 1) * line];
        for x in 0..width as usize {
            let rgba = if bits == 8 && planes >= 3 {
                let alpha = if planes == 4 { row[3 * stride + x] } else { 255 };
                [row[x], row[stride + x], row[2 * stride + x], alpha]
            } else if planes == 1 {
                let bit = x * usize::from(bits);
                let byte = row[bit / 8];
                let shift = 8 - usize::from(bits) - bit % 8;
                let index = (usize::from(byte) >> shift) & ((1 << bits) - 1);
                let [r, g, b] = index_color(index);
                [r, g, b, 255]
            } else {
                let index = (0..usize::from(planes))
                    .map(|plane| ((row[plane * stride + x / 8] >> (7 - x % 8)) & 1) as usize)
                    .enumerate().fold(0, |acc, (plane, bit)| acc | (bit << plane));
                let [r, g, b] = index_color(index);
                [r, g, b, 255]
            };
            image.put_pixel(x as u32, y as u32, Rgba(rgba));
        }
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(bits: u8, planes: u8, width: u16, height: u16, stride: u16, rle: bool) -> Vec<u8> {
        let mut h = vec![0; 128];
        h[0] = 0x0a;
        h[1] = 5;
        h[2] = u8::from(rle);
        h[3] = bits;
        h[8..10].copy_from_slice(&(width - 1).to_le_bytes());
        h[10..12].copy_from_slice(&(height - 1).to_le_bytes());
        h[65] = planes;
        h[66..68].copy_from_slice(&stride.to_le_bytes());
        h
    }

    #[test]
    fn decodes_rle_palette_and_rgb_planes() {
        let mut file = header(8, 1, 3, 2, 4, true);
        file.extend([0xc3, 1, 2, 0xc2, 2, 0, 0]); // row 0: 1 1 1 2, row 1: 2 2 0 0 (RLE)
        file.push(0x0c);
        let mut palette = vec![0; 768];
        palette[3..6].copy_from_slice(&[255, 0, 0]);
        palette[6..9].copy_from_slice(&[0, 0, 255]);
        file.extend(palette);
        let image = decode(&file).unwrap();
        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(0, 1).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(2, 1).0, [0, 0, 0, 255]);

        let mut rgb = header(8, 3, 2, 1, 2, false);
        rgb.extend([10, 11, 20, 21, 30, 31]);
        let image = decode(&rgb).unwrap();
        assert_eq!(image.get_pixel(1, 0).0, [11, 21, 31, 255]);

        let mut mono = header(1, 1, 9, 1, 2, false);
        mono.extend([0b1000_0000, 0b1000_0000]);
        let image = decode(&mono).unwrap();
        assert_eq!((image.get_pixel(0, 0)[0], image.get_pixel(1, 0)[0], image.get_pixel(8, 0)[0]), (255, 0, 255));
    }

    #[test]
    fn rejects_truncated_and_unsupported() {
        let mut file = header(8, 1, 4, 4, 4, true);
        file.extend([0xc4, 1]);
        assert!(decode(&file).is_err());
        assert!(decode(&header(16, 1, 1, 1, 2, false)).is_err());
        assert!(decode(&[0; 10]).is_err());
    }
}
