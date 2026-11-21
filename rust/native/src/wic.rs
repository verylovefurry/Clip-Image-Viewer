//! Optional fallback to Windows-installed image codecs for formats not decoded by `image`.
use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path};

use image::RgbaImage;
use lcms2::{Flags, Intent, PixelFormat, Profile, Transform};
use windows::{
    core::{PCWSTR, w},
    Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::{CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
            WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand},
        System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
            StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToUInt32}},
    },
};
use crate::{Result, error, picture::check_dimensions};

struct Apartment(bool);
impl Drop for Apartment {
    fn drop(&mut self) { if self.0 { unsafe { CoUninitialize(); } } }
}

pub fn decode(path: &Path) -> Result<RgbaImage> {
    if path.metadata().map_err(error)?.len() > 128 << 20 {
        return Err("이미지 파일 크기가 안전 제한을 초과합니다.".into());
    }
    let name: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    if name[..name.len()-1].contains(&0) { return Err("잘못된 파일 경로입니다.".into()); }
    unsafe {
        let status = CoInitializeEx(None, COINIT_MULTITHREADED);
        // RPC_E_CHANGED_MODE means the thread is already initialized in another apartment.
        let owns_com = status.is_ok();
        if !owns_com && status.0 as u32 != 0x80010106 { return Err(error(status)); }
        let _apartment = Apartment(owns_com);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(error)?;
        let decoder = factory.CreateDecoderFromFilename(PCWSTR(name.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).map_err(error)?;
        let frame = decoder.GetFrame(0).map_err(error)?;
        let orientation = frame.GetMetadataQueryReader().ok().and_then(|reader| {
            for name in [w!("/ifd/{ushort=274}"), w!("/app1/ifd/{ushort=274}")] {
                let mut value = PROPVARIANT::default();
                let status = reader.GetMetadataByName(name, &mut value);
                let number = status.ok().and_then(|_| PropVariantToUInt32(&value).ok());
                let _ = PropVariantClear(&mut value);
                if let Some(number) = number.and_then(|n| u8::try_from(n).ok())
                    && let Some(orientation) = image::metadata::Orientation::from_exif(number) { return Some(orientation); }
            }
            None
        });
        let profile = (|| {
            let mut count = 0;
            frame.GetColorContexts(&mut [], &mut count).ok()?;
            if count == 0 { return None; }
            let mut contexts = [Some(factory.CreateColorContext().ok()?)];
            frame.GetColorContexts(&mut contexts, &mut count).ok()?;
            let context = contexts[0].as_ref()?;
            let mut size = 0;
            context.GetProfileBytes(&mut [], &mut size).ok()?;
            if size == 0 || size > 16 << 20 { return None; }
            let mut bytes = vec![0; size as usize];
            context.GetProfileBytes(&mut bytes, &mut size).ok()?;
            bytes.truncate(size as usize);
            Profile::new_icc(&bytes).ok()
        })();
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height).map_err(error)?;
        check_dimensions(width, height)?;
        let converter = factory.CreateFormatConverter().map_err(error)?;
        converter.Initialize(&frame, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0., WICBitmapPaletteTypeCustom).map_err(error)?;
        let stride = width.checked_mul(4).ok_or("이미지 크기가 안전 제한을 초과합니다.")?;
        let mut rgba = vec![0; stride as usize * height as usize];
        converter.CopyPixels(std::ptr::null(), stride, &mut rgba).map_err(error)?;
        if let Some(profile) = profile.filter(|p| p.color_space() == lcms2::ColorSpaceSignature::RgbData)
            && let Ok(transform) = Transform::<u8, u8>::new_flags(&profile, PixelFormat::RGBA_8,
                &Profile::new_srgb(), PixelFormat::RGBA_8, Intent::Perceptual, Flags::COPY_ALPHA) {
            transform.transform_in_place(&mut rgba);
        }
        let image = RgbaImage::from_raw(width, height, rgba).ok_or("이미지 픽셀 데이터가 손상되었습니다.")?;
        let mut image = image::DynamicImage::ImageRgba8(image);
        if let Some(orientation) = orientation { image.apply_orientation(orientation); }
        Ok(image.into_rgba8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_png_and_rejects_missing_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/sample.png");
        let decoded = decode(&path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (900, 600));
        assert!(decode(&path.with_extension("missing")).is_err());
    }
}
