//! Experimental conversion only: decoder allocations and individual render calls are not
//! hard-limited by jxl-oxide. Run in an OS-limited worker before exposing to untrusted input.
use std::{io::{Cursor, Read, Write}, time::{Duration, Instant}};

use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use jxl_oxide::{AllocTracker, InitializeResult, JxlImage};
use lcms2::{Flags, Intent, PixelFormat, Profile, Transform};
use crate::{Result, error};

const MAX_INPUT: usize = 128 << 20;
const MAX_TRACKED: usize = 256 << 20;
const MAX_OUTPUT: usize = 128 << 20;
const MAX_PIXELS: u64 = 4_194_304;
const MAX_FRAME_PIXELS: u64 = 67_108_864;
const MAX_FRAMES: usize = 120;
const TIME_LIMIT: Duration = Duration::from_secs(15);

fn check_time(start: Instant) -> Result<()> {
    if start.elapsed() > TIME_LIMIT { Err("JPEG XL 애니메이션 시간 제한 초과".into()) } else { Ok(()) }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 { (a, b) = (b, a % b); }
    a
}

fn delay(ticks: u32, numerator: u32, denominator: u32) -> Result<(u16, u16)> {
    let n = u64::from(ticks) * u64::from(denominator);
    let d = u64::from(numerator);
    if n == 0 || d == 0 { return Err("JPEG XL 프레임 지연이 유효하지 않습니다.".into()); }
    let div = gcd(n, d);
    Ok((u16::try_from(n / div).map_err(error)?, u16::try_from(d / div).map_err(error)?))
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<()> {
    if out.len().checked_add(data.len() + 12).is_none_or(|n| n > MAX_OUTPUT) {
        return Err("APNG 출력 크기 제한 초과".into());
    }
    out.extend_from_slice(&u32::try_from(data.len()).map_err(error)?.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = !0u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 { crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1))); }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
    Ok(())
}

struct Limited(Vec<u8>);
impl Write for Limited {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.0.len().checked_add(data.len()).is_none_or(|n| n > MAX_OUTPUT) {
            return Err(std::io::Error::other("PNG 크기 제한 초과"));
        }
        self.0.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// Convert a complete animated JXL to APNG. Still images are rejected.
pub fn convert(bytes: &[u8]) -> Result<Vec<u8>> {
    let start = Instant::now();
    if bytes.len() > MAX_INPUT { return Err("JPEG XL 입력 크기 제한 초과".into()); }
    let mut input = Cursor::new(bytes);
    let mut buffer = [0u8; 4096];
    let mut available = 0usize;
    let mut uninit = JxlImage::builder().alloc_tracker(AllocTracker::with_limit(MAX_TRACKED)).build_uninit();
    let mut image = loop {
        check_time(start)?;
        let count = input.read(&mut buffer[available..]).map_err(error)?;
        if count == 0 { return Err("JPEG XL 헤더가 잘렸습니다.".into()); }
        available += count;
        let consumed = uninit.feed_bytes(&buffer[..available]).map_err(error)?;
        buffer.copy_within(consumed..available, 0);
        available -= consumed;
        match uninit.try_init().map_err(error)? {
            InitializeResult::NeedMoreData(next) => uninit = next,
            InitializeResult::Initialized(image) => break image,
        }
    };
    let (width, height) = (image.width(), image.height());
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_PIXELS { return Err("JPEG XL 프레임 크기 제한 초과".into()); }
    let anim = image.image_header().metadata.animation.as_ref().ok_or("정지 JPEG XL입니다.")?;
    let (tps_num, tps_den, loops) = (anim.tps_numerator, anim.tps_denominator, anim.num_loops);
    while !image.is_loading_done() {
        check_time(start)?;
        let count = input.read(&mut buffer[available..]).map_err(error)?;
        if count == 0 { break; }
        available += count;
        let consumed = image.feed_bytes(&buffer[..available]).map_err(error)?;
        buffer.copy_within(consumed..available, 0);
        available -= consumed;
        if image.num_loaded_frames() > MAX_FRAMES { return Err("JPEG XL 프레임 수 제한 초과".into()); }
    }
    image.finalize().map_err(error)?;
    let frames = image.num_loaded_keyframes();
    if !image.is_loading_done() || frames < 2 || frames > MAX_FRAMES || pixels * frames as u64 > MAX_FRAME_PIXELS {
        return Err("JPEG XL 프레임이 불완전하거나 제한을 초과합니다.".into());
    }
    // Associated alpha needs a verified unpremultiplication path; reject it for now.
    let alpha = match image.pixel_format() {
        jxl_oxide::PixelFormat::Rgb => false,
        jxl_oxide::PixelFormat::Rgba => true,
        _ => return Err("RGB JPEG XL 애니메이션만 지원합니다.".into()),
    };
    if image.image_header().metadata.ec_info.iter().any(|ec|
        matches!(ec.ty, jxl_oxide::ExtraChannelType::Alpha { alpha_associated: true })) {
        return Err("결합 알파 JPEG XL은 지원하지 않습니다.".into());
    }
    let profile = Profile::new_icc(&image.rendered_icc()).map_err(error)?;
    if profile.color_space() != lcms2::ColorSpaceSignature::RgbData { return Err("지원하지 않는 색상 프로필".into()); }
    let transform = Transform::<u8, u8>::new_flags(&profile, PixelFormat::RGBA_8, &Profile::new_srgb(),
        PixelFormat::RGBA_8, Intent::Perceptual, Flags::COPY_ALPHA).map_err(error)?;
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes()); ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr)?;
    chunk(&mut out, b"sRGB", &[0])?;
    let mut actl = Vec::new();
    actl.extend_from_slice(&(frames as u32).to_be_bytes()); actl.extend_from_slice(&loops.to_be_bytes());
    chunk(&mut out, b"acTL", &actl)?;
    let mut sequence = 0u32;
    for index in 0..frames {
        check_time(start)?;
        let render = image.render_frame(index).map_err(error)?;
        let (num, den) = delay(render.duration(), tps_num, tps_den)?;
        let mut stream = render.stream();
        let channels = if alpha { 4 } else { 3 };
        if (stream.width(), stream.height(), stream.channels()) != (width, height, channels) {
            return Err("지원하지 않는 JPEG XL 프레임 채널".into());
        }
        let mut samples = vec![0u8; pixels as usize * channels as usize];
        if stream.write_to_buffer(&mut samples) != samples.len() { return Err("JPEG XL 프레임이 불완전합니다.".into()); }
        let mut rgba = if alpha { samples } else {
            let mut rgba = Vec::with_capacity(pixels as usize * 4);
            for p in samples.chunks_exact(3) { rgba.extend_from_slice(&[p[0], p[1], p[2], 255]); }
            rgba
        };
        transform.transform_in_place(&mut rgba);
        let mut fctl = Vec::new();
        fctl.extend_from_slice(&sequence.to_be_bytes()); sequence += 1;
        fctl.extend_from_slice(&width.to_be_bytes()); fctl.extend_from_slice(&height.to_be_bytes());
        fctl.extend_from_slice(&0u32.to_be_bytes()); fctl.extend_from_slice(&0u32.to_be_bytes());
        fctl.extend_from_slice(&num.to_be_bytes()); fctl.extend_from_slice(&den.to_be_bytes());
        fctl.extend_from_slice(&[0, 0]);
        chunk(&mut out, b"fcTL", &fctl)?;
        let mut png = Limited(Vec::new());
        PngEncoder::new(&mut png).write_image(&rgba, width, height, ExtendedColorType::Rgba8).map_err(error)?;
        let mut pos = 8usize;
        while pos + 12 <= png.0.len() {
            let size = u32::from_be_bytes(png.0[pos..pos+4].try_into().unwrap()) as usize;
            let end = pos.checked_add(12 + size).ok_or("PNG 크기 오류")?;
            if end > png.0.len() { return Err("PNG 데이터 오류".into()); }
            if &png.0[pos+4..pos+8] == b"IDAT" {
                let data = &png.0[pos+8..pos+8+size];
                if index == 0 { chunk(&mut out, b"IDAT", data)?; }
                else {
                    let mut fdat = Vec::with_capacity(size + 4);
                    fdat.extend_from_slice(&sequence.to_be_bytes()); sequence += 1;
                    fdat.extend_from_slice(data);
                    chunk(&mut out, b"fdAT", &fdat)?;
                }
            }
            pos = end;
        }
    }
    chunk(&mut out, b"IEND", &[])?;
    check_time(start)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::AnimationDecoder;

    #[test]
    fn rejects_bad_input_and_unrepresentable_delay() {
        assert!(convert(b"invalid").is_err());
        assert!(convert(&vec![0; MAX_INPUT + 1]).is_err());
        assert_eq!(delay(1, 2, 1).unwrap(), (1, 2));
        assert!(delay(1, 0, 1).is_err());
        assert!(delay(1, 1_000_000, 1).is_err());
    }

    #[test]
    #[ignore = "requires JXL_ANIMATION_SAMPLE and JXL_STILL_SAMPLE"]
    fn ffmpeg_animation_roundtrip() {
        let bytes = std::fs::read(std::env::var_os("JXL_ANIMATION_SAMPLE").unwrap()).unwrap();
        let apng = convert(&bytes).unwrap();
        let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&apng)).unwrap();
        assert!(decoder.is_apng().unwrap());
        let frames = decoder.apng().unwrap().into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].buffer().dimensions(), (16, 16));
        assert_eq!(frames[0].delay().numer_denom_ms(), (500, 1));
        assert_eq!(frames[1].delay().numer_denom_ms(), (500, 1));
        let red = frames[0].buffer().get_pixel(8, 8).0;
        let blue = frames[1].buffer().get_pixel(8, 8).0;
        assert!(red[0] > 230 && red[1] < 30 && red[2] < 30 && red[3] == 255, "{red:?}");
        assert!(blue[2] > 230 && blue[0] < 30 && blue[1] < 30 && blue[3] == 255, "{blue:?}");
        let image = JxlImage::builder().read(Cursor::new(&bytes)).unwrap();
        let loops = image.image_header().metadata.animation.as_ref().unwrap().num_loops;
        let actl = apng.windows(4).position(|w| w == b"acTL").unwrap();
        assert_eq!(u32::from_be_bytes(apng[actl+8..actl+12].try_into().unwrap()), loops);
        assert!(convert(&bytes[..bytes.len()/2]).is_err());
        let still = std::fs::read(std::env::var_os("JXL_STILL_SAMPLE").unwrap()).unwrap();
        assert!(convert(&still).is_err());
    }

    #[test]
    #[ignore = "requires JXL_ALPHA_SAMPLE"]
    fn ffmpeg_alpha_roundtrip() {
        let bytes = std::fs::read(std::env::var_os("JXL_ALPHA_SAMPLE").unwrap()).unwrap();
        let apng = convert(&bytes).unwrap();
        let decoder = image::codecs::png::PngDecoder::new(Cursor::new(apng)).unwrap();
        let frames = decoder.apng().unwrap().into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 2);
        let first = frames[0].buffer().get_pixel(8, 8).0;
        let second = frames[1].buffer().get_pixel(8, 8).0;
        assert!((125..=130).contains(&first[3]), "{first:?}");
        assert!((62..=65).contains(&second[3]), "{second:?}");
        assert!(first[0] > 230 && first[2] < 30, "{first:?}");
        assert!(second[2] > 230 && second[0] < 30, "{second:?}");
    }
}
