use std::{
    io::{Cursor, Write},
    process::{Command, Stdio},
};

use super::*;

#[test]
fn input_and_output_limits() {
    assert_eq!(read_bounded(&b"abc"[..], 3).unwrap(), b"abc");
    let mut source = Cursor::new(b"abcde");
    assert_eq!(read_bounded(&mut source, 3).unwrap_err(), INPUT_LIMIT);
    assert_eq!(source.position(), 4);
    assert!(pixel_count(4096, 4096).is_ok());
    assert!(pixel_count(4096, 4097).is_err());
    assert!(pixel_count(65_536, 1).is_err());
    assert!(pixel_count(0, 1).is_err());
    assert!(decode(b"not an image", J2kColor::Reject).is_err());
}

fn ffmpeg(pix_fmt: &str, pixels: &[u8], codec: &str, format: &str) -> Vec<u8> {
    let mut command = Command::new("ffmpeg");
    command.args([
        "-nostdin",
        "-v",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        pix_fmt,
        "-s",
        "16x16",
        "-i",
        "pipe:0",
        "-frames:v",
        "1",
        "-c:v",
        codec,
        "-threads",
        "1",
        "-format",
        format,
    ]);
    if codec == "libopenjpeg" {
        command.args(["-numresolution", "3"]);
    } else {
        command.args(["-pred", "dwt53"]);
    }
    let mut child = command
        .args(["-f", "image2pipe", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let write = child.stdin.take().unwrap().write_all(pixels);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    write.unwrap();
    output.stdout
}

fn assert_pixels(image: RgbaImage, expected: &[u8]) {
    assert_eq!(image.dimensions(), (16, 16));
    assert_eq!(image.as_raw(), expected);
}

#[test]
#[ignore = "requires native ffmpeg with jpeg2000 and libopenjpeg encoders"]
fn ffmpeg_rgb_jp2_and_j2k() {
    let rgb: Vec<u8> = (0..=255).flat_map(|i| [i, 255 - i, i / 2]).collect();
    let rgba: Vec<u8> = rgb
        .chunks_exact(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    let jp2 = ffmpeg("rgb24", &rgb, "libopenjpeg", "jp2");
    let j2k = ffmpeg("rgb24", &rgb, "jpeg2000", "j2k");
    assert!(jp2.starts_with(JP2_SIGNATURE));
    assert!(j2k.starts_with(J2K_SIGNATURE));
    assert_pixels(decode_reader(&jp2[..], J2kColor::Reject).unwrap(), &rgba);
    assert_pixels(decode(&j2k, J2kColor::AssumeSrgb).unwrap(), &rgba);
    assert_pixels(decode(&j2k, J2kColor::AssumeAuto).unwrap(), &rgba);
    assert!(
        decode(&j2k, J2kColor::Reject)
            .unwrap_err()
            .contains("explicit")
    );
    assert!(
        decode(&j2k, J2kColor::AssumeGray)
            .unwrap_err()
            .contains("count")
    );
    assert!(decode(&jp2[..jp2.len() - 3], J2kColor::Reject).is_err());
    assert!(decode(&j2k[..j2k.len() - 3], J2kColor::AssumeSrgb).is_err());

    let mut oversized = j2k.clone();
    // SIZ Xsiz and Ysiz (origin is zero in this fixture).
    oversized[8..12].copy_from_slice(&4096_u32.to_be_bytes());
    oversized[12..16].copy_from_slice(&4097_u32.to_be_bytes());
    assert!(
        decode(&oversized, J2kColor::AssumeSrgb)
            .unwrap_err()
            .contains("display limit")
    );

    let limits = ResourceLimits::default()
        .with_maximum_decoded_bytes(128)
        .unwrap();
    let options = DecodeOptions::new(limits);
    assert!(
        decode_raw(&j2k, J2kColor::AssumeSrgb, &options)
            .unwrap_err()
            .contains("DecodedBytes")
    );
    assert!(
        decode_container(&jp2, &options)
            .unwrap_err()
            .contains("DecodedBytes")
    );

    let colr = jp2.windows(4).position(|w| w == b"colr").unwrap();
    let mut oversized = jp2.clone();
    let ihdr = oversized.windows(4).position(|w| w == b"ihdr").unwrap();
    oversized[ihdr + 4..ihdr + 8].copy_from_slice(&65_536_u32.to_be_bytes());
    assert!(
        decode(&oversized, J2kColor::Reject)
            .unwrap_err()
            .contains("display limit")
    );

    let mut unsupported = jp2;
    unsupported[colr + 7..colr + 11].copy_from_slice(&99_u32.to_be_bytes());
    assert!(
        decode(&unsupported, J2kColor::AssumeSrgb)
            .unwrap_err()
            .contains("color space")
    );
}

#[test]
#[ignore = "requires native ffmpeg with libopenjpeg encoder"]
fn ffmpeg_alpha_metadata() {
    let rgba: Vec<u8> = (0..=255).flat_map(|a| [210, 90, 30, a]).collect();
    let mut jp2 = ffmpeg("rgba", &rgba, "libopenjpeg", "jp2");
    if !jp2.windows(4).any(|w| w == b"cdef") {
        // Some FFmpeg builds encode four planes without identifying the fourth.
        assert!(decode(&jp2, J2kColor::Reject).is_err());
        let header = jp2.windows(4).position(|w| w == b"jp2h").unwrap() - 4;
        let length = u32::from_be_bytes(jp2[header..header + 4].try_into().unwrap());
        let mut cdef = b"\x00\x00\x00\x22cdef\x00\x04".to_vec();
        for record in [[0_u16, 0, 1], [1, 0, 2], [2, 0, 3], [3, 1, 0]] {
            cdef.extend(record.into_iter().flat_map(u16::to_be_bytes));
        }
        let end = header + length as usize;
        jp2[header..header + 4].copy_from_slice(&(length + cdef.len() as u32).to_be_bytes());
        jp2.splice(end..end, cdef);
    }
    assert_pixels(decode(&jp2, J2kColor::Reject).unwrap(), &rgba);
    let cdef = jp2.windows(4).position(|w| w == b"cdef").unwrap();
    let mut premultiplied = jp2.clone();
    // The fixture marks its fourth channel as straight, whole-image alpha.
    let alpha_type = cdef + 4 + 2 + 3 * 6 + 2;
    assert_eq!(&premultiplied[alpha_type..alpha_type + 2], &[0, 1]);
    premultiplied[alpha_type + 1] = 2;
    assert!(
        decode(&premultiplied, J2kColor::Reject)
            .unwrap_err()
            .contains("alpha")
    );
    let mut reordered = jp2;
    reordered.swap(cdef + 11, cdef + 23);
    let bgra: Vec<u8> = (0..=255).flat_map(|a| [30, 90, 210, a]).collect();
    assert_pixels(decode(&reordered, J2kColor::Reject).unwrap(), &bgra);
}

#[test]
#[ignore = "requires native ffmpeg with libopenjpeg encoder"]
fn ffmpeg_gray_and_sycc() {
    let gray: Vec<u8> = (0..=255_u16)
        .flat_map(|i| (i * 257).to_le_bytes())
        .collect();
    let expected: Vec<u8> = (0..=255).flat_map(|g| [g, g, g, 255]).collect();
    assert_pixels(
        decode(
            &ffmpeg("gray16le", &gray, "libopenjpeg", "jp2"),
            J2kColor::Reject,
        )
        .unwrap(),
        &expected,
    );
    assert_pixels(
        decode(
            &ffmpeg("gray16le", &gray, "libopenjpeg", "j2k"),
            J2kColor::AssumeGray,
        )
        .unwrap(),
        &expected,
    );

    let yuv = [vec![100; 256], vec![128; 64], vec![192; 64]].concat();
    let jp2 = ffmpeg("yuv420p", &yuv, "libopenjpeg", "jp2");
    let rendered = decode(&jp2, J2kColor::Reject).unwrap();
    assert_pixels(rendered, &[190, 54, 100, 255].repeat(256));
    let j2k = ffmpeg("yuv420p", &yuv, "libopenjpeg", "j2k");
    assert!(
        decode(&j2k, J2kColor::AssumeSrgb)
            .unwrap_err()
            .contains("geometry")
    );
}
