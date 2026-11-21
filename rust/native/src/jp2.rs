#[cfg(test)]
use std::io::{self, Read};

use image::RgbaImage;
use jpeg2000_decoder::{
    ChannelAssociation, ConvertedChannelType, ConvertedColorSpace, ConvertedImage, DecodeOptions,
    ResourceLimits, TypedSamples, codestream, decode_codestream, decode_jp2_color, jp2,
};

pub const MAX_INPUT_BYTES: usize = 64 << 20;
// The decoder accounts for stages separately; this is not a process memory cap.
const MAX_DECODED_BYTES: u64 = 256 << 20;
const MAX_DIMENSION: u32 = 65_535;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const JP2_SIGNATURE: &[u8] = b"\x00\x00\x00\x0cjP  \r\n\x87\n";
const J2K_SIGNATURE: &[u8] = b"\xff\x4f\xff\x51";
const INPUT_LIMIT: &str = "JPEG 2000 input exceeds the byte limit";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum J2kColor {
    #[default]
    Reject,
    /// One unsigned, full-resolution plane with the sRGB transfer function.
    #[cfg(test)]
    AssumeGray,
    /// Three unsigned, full-resolution planes in RGB order after codestream MCT.
    #[cfg(test)]
    AssumeSrgb,
    /// Infer grayscale or RGB from the component count; raw codestreams carry no color profile.
    AssumeAuto,
}

/// Decode one JP2 or raw J2K image, selected by signature, to straight RGBA8.
/// The raw color policy is never used to override JP2 metadata.
pub fn decode(bytes: &[u8], raw_color: J2kColor) -> Result<RgbaImage, String> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(INPUT_LIMIT.into());
    }
    let limits = ResourceLimits::default()
        .with_maximum_input_bytes(MAX_INPUT_BYTES as u64)
        .and_then(|l| l.with_maximum_decoded_bytes(MAX_DECODED_BYTES))
        .and_then(|l| l.with_maximum_metadata_bytes(4 << 20))
        .and_then(|l| l.with_maximum_codestreams(1))
        .and_then(|l| l.with_maximum_tile_parts(4096))
        .and_then(|l| l.with_maximum_packet_header_bytes(16 << 20))
        .and_then(|l| l.with_maximum_packets(1 << 20))
        .and_then(|l| l.with_maximum_code_blocks(1 << 18))
        .and_then(|l| l.with_maximum_work_units(250_000_000))
        .map_err(decoder_error)?;
    let options = DecodeOptions::new(limits);
    if bytes.starts_with(JP2_SIGNATURE) {
        decode_container(bytes, &options)
    } else if bytes.starts_with(J2K_SIGNATURE) {
        decode_raw(bytes, raw_color, &options)
    } else {
        Err("Unrecognized JP2/J2K signature".into())
    }
}

/// Read at most the input cap plus one probe byte, even for non-seekable sources.
#[cfg(test)]
pub fn decode_reader(reader: impl Read, raw_color: J2kColor) -> Result<RgbaImage, String> {
    let bytes = read_bounded(reader, MAX_INPUT_BYTES)?;
    decode(&bytes, raw_color)
}

#[cfg(test)]
fn read_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut block = [0; 8192];
    loop {
        let count = block.len().min(limit - bytes.len() + 1);
        let read = match reader.read(&mut block[..count]) {
            Ok(0) => return Ok(bytes),
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(format!("JPEG 2000 input: {err}")),
        };
        let needed = bytes.len() + read;
        if needed > limit {
            return Err(INPUT_LIMIT.into());
        }
        if needed > bytes.capacity() {
            let capacity = needed.max(bytes.capacity().saturating_mul(2).min(limit));
            bytes
                .try_reserve_exact(capacity - bytes.len())
                .map_err(|_| "Cannot allocate JPEG 2000 input")?;
        }
        bytes.extend_from_slice(&block[..read]);
    }
}

fn decode_container(bytes: &[u8], options: &DecodeOptions) -> Result<RgbaImage, String> {
    let file = jp2::parse(bytes, options.limits()).map_err(decoder_error)?;
    if file.file_type().brand != jp2::BoxType::JP2_BRAND
        || file.file_type().is_jpx_compatible()
        || file.file_type().is_jph_compatible()
    {
        return Err("Only baseline JP2 containers are supported".into());
    }
    let header = file.header();
    let (width, height, components) = (
        header.image.width,
        header.image.height,
        header.image.components,
    );
    pixel_count(width, height)?;
    if !(1..=4).contains(&components)
        || header.palette.is_some()
        || !header.component_mappings.is_empty()
    {
        return Err("Unsupported JP2 component layout or palette".into());
    }
    if header.image.unknown_color_space
        || !matches!(
            header.color.method,
            jp2::ColorMethod::Enumerated {
                color_space: jp2::ENUMERATED_SRGB
                    | jp2::ENUMERATED_GRAYSCALE
                    | jp2::ENUMERATED_SYCC
            }
        )
    {
        return Err("Unsupported or unspecified JP2 color space (including ICC)".into());
    }
    drop(file);
    // Strict conversion honors cdef and handles sYCC sampling. No recovery/fallback.
    let converted = decode_jp2_color(bytes, options).map_err(decoder_error)?;
    if (converted.width(), converted.height()) != (width, height)
        || converted.channels().len() != usize::from(components)
    {
        return Err("JP2 display channels do not match the source layout".into());
    }
    pack_converted(&converted)
}

fn pack_converted(image: &ConvertedImage) -> Result<RgbaImage, String> {
    use ConvertedChannelType::{Alpha, Blue, Gray, Green, Red};

    let mut planes = [None; 4];
    for channel in image.channels() {
        let index = match (image.color_space(), channel.channel_type()) {
            (ConvertedColorSpace::Grayscale, Gray) => 0,
            (ConvertedColorSpace::Srgb, Red) => 0,
            (ConvertedColorSpace::Srgb, Green) => 1,
            (ConvertedColorSpace::Srgb, Blue) => 2,
            (
                _,
                Alpha {
                    premultiplied: false,
                    association: ChannelAssociation::WholeImage,
                },
            ) => 3,
            _ => return Err("Unsupported JP2 color or alpha association".into()),
        };
        if planes[index].replace(channel.samples()).is_some() {
            return Err("Duplicate JP2 display channel".into());
        }
    }
    if image.color_space() == ConvertedColorSpace::Grayscale {
        planes[1] = planes[0];
        planes[2] = planes[0];
    }
    let (width, height) = (image.width(), image.height());
    let count = pixel_count(width, height)?;
    if planes[..3].iter().any(Option::is_none) || planes.iter().flatten().any(|p| p.len() != count)
    {
        return Err("Incomplete JP2 display planes".into());
    }
    pack_rgba(width, height, |i| {
        planes.map(|p| p.map_or(255, |p| ((u32::from(p[i]) + 128) / 257) as u8))
    })
}

fn decode_raw(bytes: &[u8], color: J2kColor, options: &DecodeOptions) -> Result<RgbaImage, String> {
    let info = codestream::parse(bytes, options.limits()).map_err(decoder_error)?;
    let size = info.image_size;
    let channels = match color {
        J2kColor::Reject => return Err("Raw J2K requires an explicit color assumption".into()),
        #[cfg(test)]
        J2kColor::AssumeGray => 1,
        #[cfg(test)]
        J2kColor::AssumeSrgb => 3,
        J2kColor::AssumeAuto if matches!(size.components, 1 | 3) => size.components,
        J2kColor::AssumeAuto => return Err("Raw J2K component count cannot identify grayscale or RGB".into()),
    };
    let (width, height) = (size.width, size.height);
    let count = pixel_count(width, height)?;
    if size.components != channels {
        return Err("J2K component count does not match the color assumption".into());
    }
    for index in 0..channels {
        validate_raw_component(size.component(index).map_err(decoder_error)?)?;
    }
    let image = decode_codestream(bytes, options).map_err(decoder_error)?;
    if (image.width(), image.height()) != (width, height)
        || image.components().len() != usize::from(channels)
    {
        return Err("J2K decoded geometry does not match the codestream".into());
    }
    let mut planes = [(&[][..], 1); 3];
    for (plane, component) in planes.iter_mut().zip(image.components()) {
        let info = component.info();
        validate_raw_component(info)?;
        let TypedSamples::Unsigned(samples) = component.samples() else {
            return Err("Signed raw J2K components are unsupported".into());
        };
        if (component.width(), component.height()) != (width, height) || samples.len() != count {
            return Err("J2K decoded plane geometry does not match the codestream".into());
        }
        *plane = (samples, (1_u64 << info.precision) - 1);
    }
    if channels == 1 {
        planes = [planes[0]; 3];
    }
    pack_rgba(width, height, |i| {
        let [r, g, b] = planes.map(|(p, max)| ((p[i].min(max) * 255 + max / 2) / max) as u8);
        [r, g, b, 255]
    })
}

fn validate_raw_component(info: codestream::ComponentInfo) -> Result<(), String> {
    if info.signed {
        return Err("Signed raw J2K components are unsupported".into());
    }
    if info.x_subsampling != 1
        || info.y_subsampling != 1
        || info.x_registration != 0
        || info.y_registration != 0
        || !(1..=38).contains(&info.precision)
    {
        return Err("Unsupported raw J2K component geometry or precision".into());
    }
    Ok(())
}

fn pixel_count(width: u32, height: u32) -> Result<usize, String> {
    let count = u64::from(width) * u64::from(height);
    if count == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION || count > MAX_PIXELS {
        return Err("JPEG 2000 dimensions exceed the display limit".into());
    }
    Ok(count as usize)
}

fn pack_rgba(
    width: u32,
    height: u32,
    mut pixel: impl FnMut(usize) -> [u8; 4],
) -> Result<RgbaImage, String> {
    let count = pixel_count(width, height)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(count * 4)
        .map_err(|_| "Cannot allocate JPEG 2000 RGBA output")?;
    for i in 0..count {
        output.extend_from_slice(&pixel(i));
    }
    RgbaImage::from_raw(width, height, output).ok_or_else(|| "Invalid JPEG 2000 RGBA output".into())
}

fn decoder_error(error: jpeg2000_decoder::DecodeError) -> String {
    format!("JPEG 2000 decoder: {error}")
}

#[cfg(test)]
#[path = "jp2_tests.rs"]
mod tests;
