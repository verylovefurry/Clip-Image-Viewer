use super::{self as raw_preview, PreviewError, extract};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn sample(name: &str) -> PathBuf {
    std::env::var_os("CLIP_RAW_SAMPLE_DIR")
        .map(PathBuf::from)
        .unwrap()
        .join(name)
}

fn check_sample(name: &str, offset: u64, length: usize, size: (u16, u16), orientation: u8) {
    let path = sample(name);
    let result = extract(&path).unwrap();
    assert_eq!((result.width, result.height), size);
    assert_eq!(result.container_orientation, Some(orientation));
    let mut expected = vec![0; length];
    let mut file = fs::File::open(path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.read_exact(&mut expected).unwrap();
    assert!(result.jpeg == expected, "preview bytes changed");
    let decoded = image::load_from_memory_with_format(&result.jpeg, image::ImageFormat::Jpeg)
        .expect("embedded JPEG must decode to pixels");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (size.0.into(), size.1.into())
    );
}

#[test]
#[ignore = "requires CLIP_RAW_SAMPLE_DIR fixtures"]
fn ricoh_dng() {
    check_sample("Ricoh-GR-III.DNG", 30_425_920, 2_949_020, (6000, 4000), 1);
    check_sample(
        "Ricoh-GR-III-orientation6.DNG",
        30_425_920,
        2_949_020,
        (6000, 4000),
        6,
    );
}

#[test]
#[ignore = "requires CLIP_RAW_SAMPLE_DIR fixtures"]
fn canon_cr2() {
    check_sample("Canon-5D-Mark-IV.CR2", 69_984, 2_145_029, (6720, 4480), 1);
}

#[test]
#[ignore = "requires CLIP_RAW_SAMPLE_DIR fixtures"]
fn nikon_nef() {
    check_sample("Nikon-Z6.NEF", 3_284_480, 1_298_401, (6048, 4024), 1);
}

#[test]
#[ignore = "requires CLIP_RAW_SAMPLE_DIR fixtures"]
fn sony_arw() {
    check_sample("Sony-a7III.ARW", 151_714, 1_150_105, (1616, 1080), 1);
}

#[test]
#[ignore = "requires CLIP_RAW_SAMPLE_DIR fixtures"]
fn panasonic_rw2() {
    check_sample("Panasonic-G9.RW2", 4_608, 773_354, (1920, 1440), 1);
}

#[test]
#[ignore = "requires CLIP_RAW_SAMPLE_DIR fixtures"]
fn om_system_orf() {
    check_sample("OM-System-OM-1.ORF", 52_224, 962_481, (3200, 2400), 1);
}

struct Fixture(PathBuf);

impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        static INDEX: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "raw-preview-{}-{}",
            std::process::id(),
            INDEX.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let fixture = Self(dir);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(fixture.path())
            .unwrap()
            .write_all(bytes)
            .unwrap();
        fixture
    }

    fn path(&self) -> PathBuf {
        self.0
            .join("\u{bbf8}\u{b9ac}\u{bcf4}\u{ae30} \u{1f4f7}.DNG")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.path());
        let _ = fs::remove_dir(&self.0);
    }
}

// Structural JPEG fixture: the extractor does not validate entropy coding.
const JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0, 2, 0, 3, 1, 1, 0x11, 0, 0xff, 0xda, 0, 8, 1, 1, 0, 0, 63,
    0, 0, 0xff, 0xd9,
];

fn dng(big: bool, offset: u32, length: u32, next: u32, jpeg: &[u8]) -> Vec<u8> {
    let u16 = |v: u16| {
        if big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32 = |v: u32| {
        if big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let mut bytes = if big { b"MM".to_vec() } else { b"II".to_vec() };
    bytes.extend(u16(42));
    bytes.extend(u32(8));
    bytes.extend(u16(3));
    for (tag, kind, count, value) in [
        (513, 4, 1, u32(offset)),
        (514, 4, 1, u32(length)),
        (50706, 1, 4, [1, 2, 0, 0]),
    ] {
        bytes.extend(u16(tag));
        bytes.extend(u16(kind));
        bytes.extend(u32(count));
        bytes.extend(value);
    }
    bytes.extend(u32(next));
    assert_eq!(bytes.len(), 50);
    bytes.extend(jpeg);
    bytes
}

fn parse(bytes: &[u8]) -> Result<raw_preview::RawPreview, PreviewError> {
    extract(&Fixture::new(bytes).path())
}

fn append_ifd(bytes: &mut Vec<u8>, entries: &[(u16, u16, u32, u32)]) {
    bytes.extend((entries.len() as u16).to_le_bytes());
    for &(tag, kind, count, value) in entries {
        bytes.extend(tag.to_le_bytes());
        bytes.extend(kind.to_le_bytes());
        bytes.extend(count.to_le_bytes());
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend(0u32.to_le_bytes());
}

fn om_system_fixture() -> Vec<u8> {
    let mut bytes = b"IIRO\x08\0\0\0".to_vec();
    append_ifd(&mut bytes, &[(271, 2, 24, 38), (34665, 4, 1, 62)]);
    bytes.extend(b"OM Digital Solutions   \0");
    append_ifd(&mut bytes, &[(0x927c, 7, (76 + JPEG.len()) as u32, 80)]);
    bytes.extend(b"OM SYSTEM\0\0\0II\x04\0");
    append_ifd(&mut bytes, &[(0x2020, 13, 1, 34)]);
    append_ifd(
        &mut bytes,
        &[
            (0x100, 4, 1, 1),
            (0x101, 4, 1, 76),
            (0x102, 4, 1, JPEG.len() as u32),
        ],
    );
    assert_eq!(bytes.len(), 156);
    bytes.extend(JPEG);
    bytes
}

#[test]
fn om_system_orf_rejects_forgery_and_bad_references() {
    let bytes = om_system_fixture();
    assert_eq!(parse(&bytes).unwrap().jpeg, JPEG);
    for len in 0..bytes.len() {
        assert!(parse(&bytes[..len]).is_err(), "accepted prefix {len}");
    }
    for (pos, value) in [
        (4, u32::MAX - 1),              // root IFD
        (30, u32::MAX - 1),             // Exif IFD
        (68, u32::MAX),                 // MakerNote byte count
        (68, (75 + JPEG.len()) as u32), // JPEG outside declared MakerNote
        (72, u32::MAX - 1),             // MakerNote offset
        (106, u32::MAX - 1),            // settings offset
        (136, u32::MAX),                // JPEG offset
        (148, u32::MAX),                // JPEG length
        (136, 0),                       // reference into MakerNote header
        (136, 77),                      // end extends past MakerNote
    ] {
        let mut bad = bytes.clone();
        bad[pos..pos + 4].copy_from_slice(&value.to_le_bytes());
        assert!(parse(&bad).is_err(), "accepted corruption at {pos}");
    }
    for pos in [38, 80, 92] {
        let mut bad = bytes.clone();
        bad[pos] ^= 1;
        assert!(matches!(parse(&bad), Err(PreviewError::UnsupportedFormat)));
    }
    for pos in [66, 100, 118, 130, 142] {
        let mut bad = bytes.clone();
        bad[pos..pos + 2].copy_from_slice(&3u16.to_le_bytes());
        assert!(
            matches!(parse(&bad), Err(PreviewError::Malformed)),
            "bad type at {pos}"
        );
    }
    let mut bad = bytes.clone();
    bad[114..116].copy_from_slice(&513u16.to_le_bytes());
    assert!(matches!(parse(&bad), Err(PreviewError::LimitExceeded)));
    let mut bad = bytes.clone();
    bad[140..142].copy_from_slice(&0x101u16.to_le_bytes());
    assert!(matches!(parse(&bad), Err(PreviewError::Malformed)));
    let mut bad = bytes.clone();
    bad[128..130].copy_from_slice(&0x999u16.to_le_bytes());
    assert!(matches!(parse(&bad), Err(PreviewError::NoSupportedPreview)));
    let mut bad = bytes.clone();
    bad[124..128].copy_from_slice(&0u32.to_le_bytes());
    assert!(matches!(parse(&bad), Err(PreviewError::NoSupportedPreview)));
}

fn compressed_cfa(make: &[u8], raw_compression: u32, preview_tag: u16, subtype: u32) -> Vec<u8> {
    let mut bytes = b"II\x2a\0\x08\0\0\0".to_vec();
    let raw_ifd = 86;
    let make_offset = 128;
    let jpeg_offset = make_offset + make.len() as u32;
    append_ifd(
        &mut bytes,
        &[
            (254, 4, 1, subtype),
            (259, 3, 1, 6),
            (271, 2, make.len() as u32, make_offset),
            (330, 4, 1, raw_ifd),
            (preview_tag, 4, 1, jpeg_offset),
            (
                if preview_tag == 513 { 514 } else { 60001 },
                4,
                1,
                JPEG.len() as u32,
            ),
        ],
    );
    assert_eq!(bytes.len(), raw_ifd as usize);
    append_ifd(
        &mut bytes,
        &[
            (254, 4, 1, 0),
            (259, 3, 1, raw_compression),
            (262, 3, 1, 32803),
        ],
    );
    assert_eq!(bytes.len(), make_offset as usize);
    bytes.extend(make);
    bytes.extend(JPEG);
    bytes
}

#[test]
fn compressed_cfa_requires_matching_make_and_raw_sub_ifd() {
    for (make, compression) in [
        (b"NIKON CORPORATION\0".as_slice(), 34713),
        (b"SONY\0", 32767),
    ] {
        let bytes = compressed_cfa(make, compression, 513, 1);
        assert_eq!(parse(&bytes).unwrap().jpeg, JPEG);
        for wrong_compression in [1, 7, if compression == 34713 { 32767 } else { 34713 }] {
            assert!(matches!(
                parse(&compressed_cfa(make, wrong_compression, 513, 1)),
                Err(PreviewError::UnsupportedFormat)
            ));
        }
        let mut wrong_make = bytes.clone();
        wrong_make[128] = b'X';
        assert!(matches!(
            parse(&wrong_make),
            Err(PreviewError::UnsupportedFormat)
        ));
        let mut damaged = bytes.clone();
        damaged[54..58].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        assert!(matches!(parse(&damaged), Err(PreviewError::Malformed)));
        for (tag, subtype) in [(60000, 1), (513, 0)] {
            assert!(matches!(
                parse(&compressed_cfa(make, compression, tag, subtype)),
                Err(PreviewError::NoSupportedPreview)
            ));
        }
    }
}

fn rw2(tag: u16, kind: u16, offset: u32, length: u32) -> Vec<u8> {
    let mut bytes = b"II\x55\0\x18\0\0\0".to_vec();
    bytes.extend(super::RW2_SIGNATURE);
    append_ifd(&mut bytes, &[(tag, kind, length, offset), (271, 2, 10, 54)]);
    bytes.extend(b"Panasonic\0");
    assert_eq!(bytes.len(), 64);
    bytes.extend(JPEG);
    bytes
}

#[test]
fn rw2_requires_private_header_and_typed_ifd0_reference() {
    let len = JPEG.len() as u32;
    let mut bytes = rw2(46, 7, 64, len);
    assert_eq!(parse(&bytes).unwrap().jpeg, JPEG);
    bytes[8] ^= 1;
    assert!(matches!(
        parse(&bytes),
        Err(PreviewError::UnsupportedFormat)
    ));
    assert!(matches!(
        parse(&rw2(47, 7, 64, len)),
        Err(PreviewError::NoSupportedPreview)
    ));
    for (kind, offset, len) in [
        (1, 64, len),
        (7, 8, len),
        (7, u32::MAX, len),
        (7, 64, len + 1),
    ] {
        assert!(matches!(
            parse(&rw2(46, kind, offset, len)),
            Err(PreviewError::Malformed)
        ));
    }
}

#[test]
fn unicode_path_and_both_byte_orders() {
    for big in [false, true] {
        let result = parse(&dng(big, 50, JPEG.len() as u32, 0, JPEG)).unwrap();
        assert_eq!(result.jpeg, JPEG);
        assert_eq!((result.width, result.height), (3, 2));
    }
}

#[test]
fn rejects_invalid_offsets_and_cycles() {
    for (offset, length, next) in [
        (u32::MAX, 40, 0),
        (50, u32::MAX, 0),
        (0, 28, 0),
        (50, JPEG.len() as u32, 8),
        (50, JPEG.len() as u32, u32::MAX - 1),
    ] {
        assert!(matches!(
            parse(&dng(false, offset, length, next, JPEG)),
            Err(PreviewError::Malformed)
        ));
    }
    let mut bytes = dng(false, 50, JPEG.len() as u32, 0, JPEG);
    bytes[10..12].copy_from_slice(&330u16.to_le_bytes());
    bytes[14..18].copy_from_slice(&2u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
    assert!(matches!(parse(&bytes), Err(PreviewError::Malformed)));
}

#[test]
fn rejects_truncation_and_does_not_scan() {
    let bytes = dng(false, 50, JPEG.len() as u32, 0, JPEG);
    for len in 0..bytes.len() {
        assert!(
            parse(&bytes[..len]).is_err(),
            "accepted prefix of length {len}"
        );
    }
    let mut corrupt = bytes.clone();
    *corrupt.last_mut().unwrap() = 0;
    assert!(matches!(parse(&corrupt), Err(PreviewError::Malformed)));
    let mut unreferenced = bytes;
    unreferenced[10..12].copy_from_slice(&60000u16.to_le_bytes());
    unreferenced[22..24].copy_from_slice(&60001u16.to_le_bytes());
    assert!(matches!(
        parse(&unreferenced),
        Err(PreviewError::NoSupportedPreview)
    ));
    unreferenced[34..36].copy_from_slice(&60002u16.to_le_bytes());
    assert!(matches!(
        parse(&unreferenced),
        Err(PreviewError::UnsupportedFormat)
    ));
    assert!(matches!(
        parse(b"FUJIFILMCCD-RAW"),
        Err(PreviewError::UnsupportedFormat)
    ));
}

#[test]
fn enforces_resource_limits() {
    let mut jpeg = JPEG.to_vec();
    jpeg[9..11].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(matches!(
        parse(&dng(false, 50, jpeg.len() as u32, 0, &jpeg)),
        Err(PreviewError::LimitExceeded)
    ));
    let mut bytes = dng(false, 50, JPEG.len() as u32, 0, JPEG);
    bytes[8..10].copy_from_slice(&513u16.to_le_bytes());
    assert!(matches!(parse(&bytes), Err(PreviewError::LimitExceeded)));

    let mut chain = dng(false, 50, JPEG.len() as u32, 78, JPEG);
    assert_eq!(chain.len(), 78);
    for offset in (78u32..).step_by(18).take(9) {
        chain.extend(1u16.to_le_bytes());
        chain.extend(330u16.to_le_bytes());
        chain.extend(4u16.to_le_bytes());
        chain.extend(1u32.to_le_bytes());
        chain.extend((offset + 18).to_le_bytes());
        chain.extend(0u32.to_le_bytes());
    }
    assert!(matches!(parse(&chain), Err(PreviewError::LimitExceeded)));

    let oversized = dng(false, 50, 32 * 1024 * 1024 + 1, 0, JPEG);
    let fixture = Fixture::new(&oversized);
    OpenOptions::new()
        .write(true)
        .open(fixture.path())
        .unwrap()
        .set_len(50 + 32 * 1024 * 1024 + 1)
        .unwrap();
    assert!(matches!(
        extract(&fixture.path()),
        Err(PreviewError::LimitExceeded)
    ));
}
