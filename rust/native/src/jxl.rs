use std::io::{Cursor, Read};

use image::{DynamicImage, ImageDecoder};
use jxl_oxide::{AllocTracker, InitializeResult, JxlImage, integration::JxlDecoder};
use lcms2::{Flags, Intent, PixelFormat, Profile, Transform};

use crate::{Result, error};

const MAX_SOURCE: usize = 128 << 20;
const MAX_DIMENSION: u32 = 65_535;
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_ALLOC: usize = 384 << 20;
/// Returned by [`decode`] for animated files, which `jxl_animation::convert` handles.
pub const ANIMATED: &str = "애니메이션 JPEG XL은 정지 이미지 경로에서 지원하지 않습니다.";

fn check_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("JPEG XL 이미지 크기가 안전 제한을 초과합니다.".into());
    }
    Ok(())
}

// Validate with a tracker before using the image integration, whose constructor
// starts with an unlimited tracker and does not expose the animation header.
fn validate(bytes: &[u8]) -> Result<()> {
    validate_with_limit(bytes, MAX_ALLOC)
}

fn validate_with_limit(bytes: &[u8], limit: usize) -> Result<()> {
    if bytes.len() > MAX_SOURCE { return Err("JPEG XL 파일 크기가 안전 제한을 초과합니다.".into()); }
    let mut input = Cursor::new(bytes);
    let mut buf = [0u8; 4096];
    let mut available = 0;
    let mut uninit = JxlImage::builder().alloc_tracker(AllocTracker::with_limit(limit)).build_uninit();
    let mut image = loop {
        let count = input.read(&mut buf[available..]).map_err(error)?;
        if count == 0 { return Err("JPEG XL 헤더가 잘렸습니다.".into()); }
        available += count;
        let consumed = uninit.feed_bytes(&buf[..available]).map_err(error)?;
        buf.copy_within(consumed..available, 0);
        available -= consumed;
        match uninit.try_init().map_err(error)? {
            InitializeResult::NeedMoreData(next) => uninit = next,
            InitializeResult::Initialized(image) => break image,
        }
    };
    check_dimensions(image.width(), image.height())?;
    if image.image_header().metadata.animation.is_some() {
        return Err(ANIMATED.into());
    }
    while !image.is_loading_done() {
        let count = input.read(&mut buf[available..]).map_err(error)?;
        if count == 0 { break; }
        available += count;
        let consumed = image.feed_bytes(&buf[..available]).map_err(error)?;
        buf.copy_within(consumed..available, 0);
        available -= consumed;
    }
    image.finalize().map_err(error)?;
    if !image.is_loading_done() { return Err("JPEG XL 이미지 데이터가 잘렸습니다.".into()); }
    Ok(())
}

pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    validate(bytes)?;
    let mut decoder = JxlDecoder::new(Cursor::new(bytes)).map_err(error)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC as u64);
    decoder.set_limits(limits).map_err(error)?;
    check_dimensions(decoder.dimensions().0, decoder.dimensions().1)?;
    if decoder.total_bytes() > MAX_ALLOC as u64 {
        return Err("JPEG XL 출력 버퍼가 안전 제한을 초과합니다.".into());
    }
    let icc = decoder.icc_profile().map_err(error)?.ok_or("JPEG XL 색상 프로필이 없습니다.")?;
    let profile = Profile::new_icc(&icc).map_err(error)?;
    let mut rgba = DynamicImage::from_decoder(decoder).map_err(error)?.into_rgba8();
    match profile.color_space() {
        lcms2::ColorSpaceSignature::RgbData => {
            let transform = Transform::<u8, u8>::new_flags(&profile, PixelFormat::RGBA_8,
                &Profile::new_srgb(), PixelFormat::RGBA_8, Intent::Perceptual, Flags::COPY_ALPHA).map_err(error)?;
            transform.transform_in_place(rgba.as_mut());
        }
        lcms2::ColorSpaceSignature::GrayData => {
            let gray: Vec<u8> = rgba.pixels().map(|p| p[0]).collect();
            let mut rgb = vec![0; gray.len() * 3];
            let transform = Transform::<u8, u8>::new(&profile, PixelFormat::GRAY_8,
                &Profile::new_srgb(), PixelFormat::RGB_8, Intent::Perceptual).map_err(error)?;
            transform.transform_pixels(&gray, &mut rgb);
            for (pixel, color) in rgba.pixels_mut().zip(rgb.chunks_exact(3)) {
                pixel.0[..3].copy_from_slice(color);
            }
        }
        _ => return Err("지원하지 않는 JPEG XL 색상 프로필입니다.".into()),
    }
    Ok(DynamicImage::ImageRgba8(rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_inputs_and_limits() {
        assert!(decode(b"not jpeg xl").is_err());
        assert!(decode(&vec![0; MAX_SOURCE + 1]).is_err());
        assert!(check_dimensions(0, 1).is_err());
        assert!(check_dimensions(MAX_DIMENSION + 1, 1).is_err());
        assert!(check_dimensions(8193, 8192).is_err());
        assert!(check_dimensions(8192, 8192).is_ok());
    }

    #[test]
    #[ignore = "requires JXL_TEST_STILL and JXL_TEST_ANIMATION fixtures"]
    fn ffmpeg_samples_and_truncation() {
        let bytes = std::fs::read(std::env::var_os("JXL_TEST_STILL").unwrap()).unwrap();
        assert!(validate_with_limit(&bytes, 1).is_err());
        let image = decode(&bytes).unwrap().into_rgba8();
        assert_eq!((image.width(), image.height()), (16, 16));
        let pixel = image.get_pixel(0, 0).0;
        assert!(pixel[0] > 200 && pixel[1] < 50 && pixel[2] < 50 && pixel[3] == 255, "{pixel:?}");
        for length in [0, 2, bytes.len() / 2] {
            assert!(decode(&bytes[..length]).is_err(), "truncated length {length}");
        }
        let animated = std::fs::read(std::env::var_os("JXL_TEST_ANIMATION").unwrap()).unwrap();
        assert!(decode(&animated).unwrap_err().contains("애니메이션"));
    }
}
