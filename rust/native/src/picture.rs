use std::{fs::{self, File}, io::{BufReader, Cursor}, path::Path};

use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, RgbaImage,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use lcms2::{Flags, Intent, PixelFormat, Profile, Transform};
use serde_json::{Value, json};
use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};

use crate::{
    Result, asset::Assets, clip,
    collection::{Crop, Item, mime},
    error, psd,
};

const MAX_SOURCE: u64 = 128 << 20;
const MAX_PSD_FILE: u64 = 2 << 30;
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

pub fn check_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("이미지 크기가 안전 제한을 초과합니다.".into());
    }
    Ok(())
}

pub fn encode(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgba8)
        .map_err(error)?;
    Ok(png)
}

fn format(ext: &str) -> Option<ImageFormat> {
    match ext {
        "jfif" => Some(ImageFormat::Jpeg),
        "apng" => Some(ImageFormat::Png),
        _ => ImageFormat::from_extension(ext),
    }
}

fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(384 << 20);
    limits.max_image_width = Some(65_535);
    limits.max_image_height = Some(65_535);
    limits
}

pub fn decode(bytes: &[u8], hint: Option<ImageFormat>) -> Result<DynamicImage> {
    let kind = image::guess_format(bytes).ok().or(hint)
        .ok_or("원본 이미지 형식을 인식할 수 없습니다.")?;
    if kind == ImageFormat::Jpeg && let Some(image) = decode_cmyk_jpeg(bytes)? {
        return Ok(image);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), kind);
    reader.limits(limits());
    let mut decoder = reader.into_decoder().map_err(error)?;
    let (width, height) = decoder.dimensions();
    check_dimensions(width, height)?;
    let orientation = decoder.orientation().map_err(error)?;
    let profile = decoder.icc_profile().ok().flatten();
    let mut image = DynamicImage::from_decoder(decoder).map_err(error)?;
    if let Some(profile) = profile.and_then(|data| Profile::new_icc(&data).ok())
        && profile.color_space() == lcms2::ColorSpaceSignature::RgbData
    {
        let mut rgba = image.into_rgba8();
        if let Ok(transform) = Transform::<u8, u8>::new_flags(
            &profile, PixelFormat::RGBA_8, &Profile::new_srgb(), PixelFormat::RGBA_8,
            Intent::Perceptual, Flags::COPY_ALPHA,
        ) {
            transform.transform_in_place(rgba.as_mut());
        }
        image = DynamicImage::ImageRgba8(rgba);
    }
    image.apply_orientation(orientation);
    Ok(image)
}

fn decode_cmyk_jpeg(bytes: &[u8]) -> Result<Option<DynamicImage>> {
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(
        ZCursor::new(bytes),
        DecoderOptions::default().set_max_width(65_535).set_max_height(65_535).set_strict_mode(false),
    );
    decoder.decode_headers().map_err(error)?;
    let Some(space @ (ColorSpace::CMYK | ColorSpace::YCCK)) = decoder.input_colorspace() else { return Ok(None) };
    let (width, height) = decoder.dimensions().ok_or("JPEG 크기를 찾지 못했습니다.")?;
    let (width, height) = (width as u32, height as u32);
    check_dimensions(width, height)?;
    let orientation = decoder.exif().and_then(|bytes| image::metadata::Orientation::from_exif_chunk(bytes));
    let profile = decoder.icc_profile();
    decoder.set_options(decoder.options().jpeg_set_out_colorspace(space));
    let mut cmyk = decoder.decode().map_err(error)?;
    // Adobe JPEG stores inverted CMYK. In YCCK, YCbCr remaps to ordinary C/M/Y,
    // while the K component is still inverted.
    let adobe = has_adobe_marker(bytes);
    for pixel in cmyk.chunks_exact_mut(4) {
        if space == ColorSpace::YCCK {
            let y = f32::from(pixel[0]);
            let cb = f32::from(pixel[1]) - 128.;
            let cr = f32::from(pixel[2]) - 128.;
            let chan = |v: f32| v.round().clamp(0., 255.) as u8;
            *pixel.first_mut().unwrap() = chan(y + 1.402 * cr);
            pixel[1] = chan(y - 0.344136 * cb - 0.714136 * cr);
            pixel[2] = chan(y + 1.772 * cb);
            pixel[3] = 255 - pixel[3];
        } else if adobe {
            for c in pixel { *c = 255 - *c; }
        }
    }
    let packed = cmyk_to_rgb(&cmyk, profile.as_deref())?;
    let mut image = DynamicImage::ImageRgb8(image::RgbImage::from_raw(width, height, packed).ok_or("JPEG 픽셀 수가 올바르지 않습니다.")?);
    if let Some(orientation) = orientation { image.apply_orientation(orientation); }
    Ok(Some(image))
}

fn has_adobe_marker(mut bytes: &[u8]) -> bool {
    let Some(rest) = bytes.strip_prefix(&[0xff, 0xd8]) else { return false };
    bytes = rest;
    while let Some(rest) = bytes.strip_prefix(&[0xff]) {
        bytes = rest;
        if bytes.first() == Some(&0xff) { continue; }
        let [marker, low, high, ..] = bytes else { break };
        let size = u16::from_be_bytes([*low, *high]) as usize;
        if *marker == 0xda || size < 2 || size >= bytes.len() { break; }
        if *marker == 0xee && bytes[3..=size].starts_with(b"Adobe") { return true; }
        bytes = &bytes[size + 1..];
    }
    false
}

pub fn cmyk_to_rgb(cmyk: &[u8], icc: Option<&[u8]>) -> Result<Vec<u8>> {
    if cmyk.len() % 4 != 0 { return Err("CMYK 픽셀 데이터가 손상되었습니다.".into()); }
    let mut rgb = vec![0; cmyk.len() / 4 * 3];
    if let Some(icc) = icc {
        let profile = Profile::new_icc(icc).map_err(error)?;
        let conversion = Transform::<u8, u8>::new(
            &profile, PixelFormat::CMYK_8, &Profile::new_srgb(), PixelFormat::RGB_8, Intent::Perceptual,
        ).map_err(error)?;
        conversion.transform_pixels(cmyk, &mut rgb);
    } else {
        for (src, dst) in cmyk.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) {
            for index in 0..3 {
                dst[index] = (((255 - u16::from(src[index])) * (255 - u16::from(src[3])) + 127) / 255) as u8;
            }
        }
    }
    Ok(rgb)
}

fn metadata(width: u32, height: u32, format: &str, channels: u8) -> Value {
    json!({"width": width, "height": height, "format": format.to_ascii_uppercase(),
        "space": if channels <= 2 && channels > 0 { "b-w" } else { "srgb" },
        "channels": channels, "hasAlpha": channels == 4 || channels == 2,
        "pages": 1, "pageHeight": height, "density": 0,
    })
}

fn headers_file(path: &Path, ext: &str) -> Result<(Value, bool)> {
    let format = format(ext).ok_or("이미지 형식을 인식할 수 없습니다.")?;
    let file = File::open(path).map_err(error)?;
    if file.metadata().map_err(error)?.len() > MAX_SOURCE {
        return Err("이미지 파일 크기가 안전 제한을 초과합니다.".into());
    }
    let mut reader = ImageReader::with_format(BufReader::new(file), format);
    reader.limits(limits());
    let decoder = reader.into_decoder().map_err(error)?;
    let (width, height) = decoder.dimensions();
    check_dimensions(width, height)?;
    let color = decoder.original_color_type();
    let cmyk = color == ExtendedColorType::Cmyk8;
    Ok((metadata(width, height, if ext == "jpg" || ext == "jfif" { "jpeg" } else { ext }, color.channel_count()), cmyk))
}

fn crop_image(image: DynamicImage, crop: Option<Crop>, mode: &str) -> DynamicImage {
    let Some(crop) = crop.filter(|_| matches!(mode, "trim" | "bleed")) else { return image };
    if crop.canvas_width <= 0. || crop.canvas_height <= 0. { return image; }
    let extra = if mode == "bleed" { crop.bleed.max(0.) * 2. } else { 0. };
    let w = (crop.trim_width + extra) * crop.page_count.max(1.);
    let h = crop.trim_height + extra;
    let x = (crop.canvas_width - w) / 2. + crop.offset_x;
    let y = (crop.canvas_height - h) / 2. + crop.offset_y;
    let scale_x = image.width() as f64 / crop.canvas_width;
    let scale_y = image.height() as f64 / crop.canvas_height;
    let l = (x.max(0.) * scale_x).round() as u32;
    let t = (y.max(0.) * scale_y).round() as u32;
    let r = ((x + w).min(crop.canvas_width) * scale_x).round() as u32;
    let b = ((y + h).min(crop.canvas_height) * scale_y).round() as u32;
    if r > l && b > t && r <= image.width() && b <= image.height() { image.crop_imm(l, t, r - l, b - t) }
    else { image }
}

pub fn result(item: &Item, data_url: String, mut meta: Value, doc: Value, source: &str, crop: bool, encoded_size: Option<usize>) -> Result<Value> {
    let stat = fs::metadata(item.disk_path()).map_err(error)?;
    let modified = stat.modified().ok().map(time::OffsetDateTime::from)
        .and_then(|time| time.format(&time::format_description::well_known::Rfc3339).ok()).unwrap_or_default();
    meta["source"] = json!(source);
    meta["cropAvailable"] = json!(crop);
    meta["byteSize"] = json!(if matches!(item, Item::ArchiveEntry { .. }) { encoded_size.map_or(stat.len(), |n| n as u64) } else { stat.len() });
    meta["modifiedAt"] = json!(modified);
    Ok(json!({"dataUrl": data_url, "metadata": meta, "layerDocument": doc}))
}

/// Serves an encoded APNG unchanged so the web view animates it.
fn animation(assets: &Assets, item: &Item, apng: Vec<u8>, format: &str, frames: usize, source: &str) -> Result<Value> {
    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&apng)).map_err(error)?;
    let (width, height) = decoder.dimensions();
    let mut meta = metadata(width, height, format, 4);
    meta["pages"] = json!(frames);
    let size = apng.len();
    result(item, assets.bytes(apng, "image/png")?, meta, Value::Null, source, false, Some(size))
}

pub fn rendered(assets: &Assets, item: &Item, image: RgbaImage, doc: Value, source: &str) -> Result<Value> {
    let meta = metadata(image.width(), image.height(), "PNG", 4);
    let bytes = encode(&image)?;
    let count = bytes.len();
    result(item, assets.bytes(bytes, "image/png")?, meta, doc, source, false, Some(count))
}

fn psd_composite(item: &Item) -> Result<RgbaImage> {
    match item {
        Item::ArchiveEntry { .. } => psd::merged(&item.read(MAX_SOURCE)?),
        _ => psd::merged_file(item.disk_path(), MAX_PSD_FILE, MAX_SOURCE),
    }
}

const RAW_EXTENSIONS: &str = "3fr ari arw bay cap cr2 cr3 dcr dcs dng drf eip erf fff gpr iiq k25 kdc mdc mef mos mrw nef nrw obm orf pef ptx pxn r3d raf raw rwl rw2 rwz sr2 srf srw sti";

/// Camera JPEG preview: the strict extractor for formats it verifies, then the generic
/// container scan. `None` leaves the file to Windows codecs.
fn raw_preview(item: &Item) -> Result<Option<crate::raw_preview::RawPreview>> {
    use crate::raw_preview::{PreviewError, extract_any_from, extract_from};
    let ext = item.ext();
    if !RAW_EXTENSIONS.split_whitespace().any(|raw| raw == ext) { return Ok(None); }
    let strict = matches!(ext.as_str(), "dng" | "cr2" | "nef" | "arw" | "rw2" | "orf");
    let run = |mut input: &mut dyn ReadSeekRaw, len: u64| {
        if strict {
            // The generic scan accepts only validated JPEGs, so it is a safe second attempt
            // after the strict reader rejects a variant; the strict error is kept otherwise.
            match extract_from(&mut input, len) {
                Err(PreviewError::Io(error)) => Err(PreviewError::Io(error)),
                Err(strict_error) => extract_any_from(&mut input, len).map_err(|generic| match generic {
                    PreviewError::Io(error) => PreviewError::Io(error),
                    _ => strict_error,
                }),
                ok => ok,
            }
        } else { extract_any_from(&mut input, len) }
    };
    let preview = match item {
        Item::File { path, .. } => {
            let mut file = File::open(path).map_err(error)?;
            let len = file.metadata().map_err(error)?.len();
            run(&mut file, len)
        }
        Item::ArchiveEntry { .. } => {
            let bytes = item.read(MAX_SOURCE)?;
            let len = bytes.len() as u64;
            run(&mut Cursor::new(bytes), len)
        }
        _ => return Ok(None),
    };
    match preview {
        Ok(preview) => Ok(Some(preview)),
        Err(PreviewError::Io(error)) => Err(format!("RAW 미리보기를 읽을 수 없습니다: {error}")),
        Err(PreviewError::Malformed | PreviewError::LimitExceeded) if strict => Err("RAW 미리보기 구조가 손상되었거나 안전 제한을 초과합니다.".into()),
        Err(_) => Ok(None),
    }
}

trait ReadSeekRaw: std::io::Read + std::io::Seek {}
impl<T: std::io::Read + std::io::Seek> ReadSeekRaw for T {}

enum RawSource {
    Preview(crate::raw_preview::RawPreview, &'static str),
    Image(DynamicImage),
    /// LibRaw could not read the file; the message is kept for the final error.
    Failed(String),
    NotRaw,
}

const MAX_RAW_FILE: u64 = 512 << 20;
const MIN_RAW_PREVIEW_EDGE: u16 = 1024;

/// Embedded camera JPEG (parsed in this process, no decoding of sensor data) when it is large
/// enough; otherwise LibRaw in the isolated worker (its thumbnail or a development).
fn raw_source(item: &Item) -> Result<RawSource> {
    let ext = item.ext();
    if !RAW_EXTENSIONS.split_whitespace().any(|raw| raw == ext) { return Ok(RawSource::NotRaw); }
    let embedded = raw_preview(item)?;
    if let Some(preview) = embedded.as_ref().filter(|p| p.width.max(p.height) >= MIN_RAW_PREVIEW_EDGE) {
        let _ = preview;
        return Ok(RawSource::Preview(embedded.expect("checked"), "RAW 내장 JPEG 미리보기"));
    }
    let libraw = item.read(MAX_RAW_FILE).and_then(|bytes| crate::libraw::decode(&bytes));
    let embedded_edge = embedded.as_ref().map_or(0, |p| u32::from(p.width.max(p.height)));
    Ok(match libraw {
        Ok(crate::libraw::Output::Jpeg { data, width, height, orientation }) if width.max(height) > embedded_edge => {
            let (width, height) = (u16::try_from(width).map_err(error)?, u16::try_from(height).map_err(error)?);
            RawSource::Preview(crate::raw_preview::RawPreview { jpeg: data, width, height, container_orientation: orientation }, "LibRaw 내장 미리보기")
        }
        Ok(crate::libraw::Output::Image(image)) if image.width().max(image.height()) > embedded_edge => RawSource::Image(image),
        result => match embedded {
            Some(preview) => RawSource::Preview(preview, "RAW 내장 JPEG 미리보기"),
            None => RawSource::Failed(result.err().unwrap_or_else(|| "LibRaw 결과가 없습니다.".into())),
        },
    })
}

fn preview_needs_conversion(preview: &crate::raw_preview::RawPreview) -> Result<(bool, Option<image::metadata::Orientation>)> {
    let mut decoder = ImageReader::with_format(Cursor::new(&preview.jpeg), ImageFormat::Jpeg).into_decoder().map_err(error)?;
    let jpeg_orientation = decoder.exif_metadata().map_err(error)?.as_deref()
        .and_then(image::metadata::Orientation::from_exif_chunk);
    let container_orientation = preview.container_orientation.and_then(image::metadata::Orientation::from_exif);
    let extra = if jpeg_orientation.is_none() { container_orientation } else { None };
    Ok((decoder.original_color_type() == ExtendedColorType::Cmyk8
        || extra.is_some_and(|o| o != image::metadata::Orientation::NoTransforms), extra))
}

fn decode_raw_preview(preview: &crate::raw_preview::RawPreview, extra: Option<image::metadata::Orientation>) -> Result<DynamicImage> {
    let mut image = decode(&preview.jpeg, Some(ImageFormat::Jpeg))?;
    if let Some(orientation) = extra { image.apply_orientation(orientation); }
    Ok(image)
}

/// HEIF/HEIC through the pure-Rust decoder, with Windows codecs as a second chance.
fn decode_heif(bytes: &[u8]) -> Result<DynamicImage> {
    let image = heif_oxide::decode_bytes(bytes).map_err(|e| format!("HEIF 이미지를 열 수 없습니다: {e}"))?;
    check_dimensions(image.width, image.height)?;
    RgbaImage::from_raw(image.width, image.height, image.to_rgba8())
        .map(DynamicImage::ImageRgba8).ok_or_else(|| "HEIF 픽셀 수가 올바르지 않습니다.".into())
}

/// First frame of an animated JPEG XL, decoded from its APNG conversion.
fn jxl_first_frame(bytes: &[u8]) -> Result<DynamicImage> {
    match crate::jxl::decode(bytes) {
        Err(e) if e == crate::jxl::ANIMATED => decode(&crate::jxl_animation::convert(bytes)?, Some(ImageFormat::Png)),
        other => other,
    }
}

fn decode_item(item: &Item, ext: &str) -> Result<DynamicImage> {
    let bytes = item.read(MAX_SOURCE)?;
    if ext == "jxl" { return jxl_first_frame(&bytes); }
    if ext == "pcx" { return crate::pcx::decode(&bytes).map(DynamicImage::ImageRgba8); }
    if ext == "bpg" {
        let (_, frames) = crate::bpg::decode(&bytes)?;
        return frames.into_iter().next().map(|f| DynamicImage::ImageRgba8(f.image)).ok_or_else(|| "BPG 프레임이 없습니다.".into());
    }
    if matches!(ext, "jp2" | "j2k" | "j2c" | "jpc" | "jpf" | "jpx") {
        return crate::jp2::decode(&bytes, crate::jp2::J2kColor::AssumeAuto)
            .map(DynamicImage::ImageRgba8)
            .map_err(|e| format!("JPEG 2000 이미지를 열 수 없습니다: {e}"));
    }
    let decoded = if matches!(ext, "heic" | "heif" | "hif") { decode_heif(&bytes) } else { decode(&bytes, format(ext)) };
    #[cfg(windows)]
    if let Err(native) = &decoded {
        let native = native.clone();
        use std::io::Write;
        let converted = match item {
            Item::File { path, .. } => crate::wic::decode(path),
            Item::ArchiveEntry { .. } => {
                let mut temp = tempfile::Builder::new().suffix(&format!(".{ext}")).tempfile().map_err(error)?;
                temp.write_all(&bytes).map_err(error)?;
                let path = temp.into_temp_path();
                crate::wic::decode(&path)
            }
            _ => return decoded,
        };
        return converted.map(DynamicImage::ImageRgba8)
            .map_err(|e| format!("이 형식을 열 수 없습니다. ({native}; Windows 이미지 코덱: {e})"));
    }
    decoded
}

pub fn load(assets: &Assets, item: &Item, mode: &str) -> Result<Value> {
    let ext = item.ext();
    if matches!(ext.as_str(), "clip" | "csp") {
        let mut clip = clip::Document::read(item)?;
        let area = clip.crop.as_ref().or(item.crop().as_ref()).cloned();
        let (display, doc) = clip.display();
        let label = if matches!(display, clip::Display::Layers(_)) { "CLIP 레이어 합성" } else { "CLIP 저장 미리보기" };
        let source = match mode {
            "trim" if area.is_some() => format!("{label} · 재단선"),
            "bleed" if area.is_some() => format!("{label} · 재단 여백"),
            _ => label.to_owned(),
        };
        let source = source.as_str();
        if let clip::Display::Layers(image) = display {
            let image = crop_image(DynamicImage::ImageRgba8(image), area.clone(), mode).into_rgba8();
            let meta = metadata(image.width(), image.height(), "PNG", 4);
            let data = encode(&image)?;
            let size = data.len();
            return result(item, assets.bytes(data, "image/png")?, meta, doc, source, area.is_some(), Some(size));
        }
        let (data, meta) = if area.is_some() && matches!(mode, "trim" | "bleed") {
            let image = crop_image(decode(&clip.preview, None)?, area.clone(), mode).into_rgba8();
            (encode(&image)?, metadata(image.width(), image.height(), "PNG", 4))
        } else {
            let preview_format = image::guess_format(&clip.preview).map_err(error)?;
            let (width, height) = clip.dimensions()?;
            (clip.preview, metadata(width, height, preview_format.extensions_str()[0], 4))
        };
        let size = data.len();
        let kind = data_mime(&data)?;
        return result(item, assets.bytes(data, kind)?, meta, doc, source, area.is_some(), Some(size));
    }
    if matches!(ext.as_str(), "psd" | "psb") {
        return psd::layers::load(assets, item);
    }
    match raw_source(item)? {
        RawSource::Preview(preview, source) => {
            let (convert, orientation) = preview_needs_conversion(&preview)?;
            if convert {
                return rendered(assets, item, decode_raw_preview(&preview, orientation)?.into_rgba8(), Value::Null, source);
            }
            let meta = metadata(preview.width as u32, preview.height as u32, "JPEG", 3);
            let size = preview.jpeg.len();
            return result(item, assets.bytes(preview.jpeg, "image/jpeg")?, meta, Value::Null, source, false, Some(size));
        }
        RawSource::Image(image) => return rendered(assets, item, image.into_rgba8(), Value::Null, "LibRaw RAW 현상"),
        RawSource::Failed(libraw) => {
            return match decode_item(item, &ext) {
                Ok(image) => rendered(assets, item, image.into_rgba8(), Value::Null, "Windows RAW 코덱"),
                Err(wic) => Err(format!("RAW 이미지를 열 수 없습니다. ({libraw}; {wic})")),
            };
        }
        RawSource::NotRaw => {}
    }
    if let Some(mime) = mime(&ext)
        && mime.starts_with("image/")
    {
        if matches!(ext.as_str(), "svg" | "avif") {
            let meta = metadata(0, 0, &ext, 0);
            return match item {
                Item::File { path, .. } => result(item, assets.file(path.clone(), mime, false)?, meta, Value::Null, "원본", false, None),
                _ => {
                    let data = item.read(MAX_SOURCE)?;
                    let len = data.len();
                    result(item, assets.bytes(data, mime)?, meta, Value::Null, "원본", false, Some(len))
                }
            };
        }
        if let Item::File { path, .. } = item {
            let (meta, cmyk) = headers_file(path, &ext)?;
            if !cmyk {
                return result(item, assets.file(path.clone(), mime, false)?, meta, Value::Null, "원본", false, None);
            }
            let image = decode(&item.read(MAX_SOURCE)?, format(&ext))?.into_rgba8();
            return rendered(assets, item, image, Value::Null, "원본 · CMYK → sRGB");
        }
        let data = item.read(MAX_SOURCE)?;
        let decoder = ImageReader::with_format(Cursor::new(&data), format(&ext).ok_or("이미지 형식 오류")?)
            .into_decoder().map_err(error)?;
        let (w, h) = decoder.dimensions();
        check_dimensions(w, h)?;
        let color = decoder.original_color_type();
        drop(decoder);
        if color != ExtendedColorType::Cmyk8 {
            let len = data.len();
            return result(item, assets.bytes(data, mime)?, metadata(w, h, &ext, color.channel_count()), Value::Null, "원본", false, Some(len));
        }
        let image = decode(&data, format(&ext))?.into_rgba8();
        return rendered(assets, item, image, Value::Null, "원본 · CMYK → sRGB");
    }
    if ext == "bpg" {
        let bytes = item.read(MAX_SOURCE)?;
        let (header, frames) = crate::bpg::decode(&bytes)?;
        if frames.len() == 1 {
            let image = frames.into_iter().next().expect("one frame").image;
            return rendered(assets, item, image, Value::Null, "BPG 변환");
        }
        return animation(assets, item, crate::bpg::encode_animation(&frames, header.animation.map_or(0, |a| a.loops))?,
            "BPG", frames.len(), "BPG 애니메이션 변환");
    }
    if ext == "jxl" {
        let bytes = item.read(MAX_SOURCE)?;
        if let Err(e) = crate::jxl::decode(&bytes) && e == crate::jxl::ANIMATED {
            let apng = crate::jxl_animation::convert(&bytes)?;
            let frames = apng.windows(4).position(|w| w == b"acTL")
                .and_then(|at| apng.get(at + 4..at + 8)).map_or(1, |n| u32::from_be_bytes(n.try_into().unwrap()) as usize);
            return animation(assets, item, apng, "JXL", frames, "JPEG XL 애니메이션 변환");
        }
    }
    let image = decode_item(item, &ext)?.into_rgba8();
    let source = if matches!(ext.as_str(), "j2k" | "j2c" | "jpc") {
        "JPEG 2000 · 색상 정보가 없는 경우 RGB/회색조 가정"
    } else if matches!(ext.as_str(), "heic" | "heif" | "hif") {
        "HEIF 변환"
    } else if ext == "pcx" { "PCX 변환" } else { "변환 이미지" };
    rendered(assets, item, image, Value::Null, source)
}

pub fn thumbnail(assets: &Assets, item: &Item) -> Result<String> {
    let ext = item.ext();
    let image = match ext.as_str() {
        "svg" | "avif" => return load(assets, item, "full")?.get("dataUrl").and_then(Value::as_str).map(str::to_owned)
            .ok_or("이미지 미리보기를 불러올 수 없습니다.".into()),
        "clip" | "csp" => decode(&clip::read_preview(item)?, None)?,
        "psd" | "psb" => DynamicImage::ImageRgba8(psd_composite(item)?),
        _ => match raw_source(item)? {
            RawSource::Preview(preview, _) => {
                let (_, orientation) = preview_needs_conversion(&preview)?;
                decode_raw_preview(&preview, orientation)?
            }
            RawSource::Image(image) => image,
            RawSource::Failed(_) | RawSource::NotRaw => decode_item(item, &ext)?,
        },
    };
    let preview = if image.width() <= 180 && image.height() <= 110 { image.into_rgba8() }
        else { image.thumbnail(180, 110).into_rgba8() };
    assets.bytes(encode(&preview)?, "image/png")
}

pub fn data_mime(buffer: &[u8]) -> Result<&'static str> {
    Ok(match image::guess_format(buffer).map_err(error)? {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        _ => return Err("저장된 미리보기 형식을 지원하지 않습니다.".into()),
    })
}

#[cfg(test)]
#[path = "../tests/support/image_open.rs"]
mod image_open_tests;

#[cfg(test)]
#[path = "../tests/support/formats.rs"]
mod format_tests;
