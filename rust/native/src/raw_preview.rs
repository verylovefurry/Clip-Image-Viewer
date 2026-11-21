//! Read-only extraction of a standalone camera JPEG from classic DNG, CR2,
//! compressed Nikon NEF and Sony ARW, Panasonic RW2, or OM SYSTEM ORF.
//! JPEG bytes (including any embedded ICC/EXIF) are returned unchanged. No
//! demosaic, color conversion, resizing, rotation, or TIFF metadata copying.
//! The container orientation is reported separately; do not apply both it and
//! the JPEG's own EXIF orientation. The JPEG is probed, not fully decoded.

use std::fmt;
#[cfg(test)]
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
#[path = "raw_preview_tests.rs"]
mod tests;

#[path = "raw_preview_generic.rs"]
mod generic;
pub(crate) use generic::extract_any_from;

const MAX_IFDS: usize = 64;
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 512;
const MAX_SUB_IFDS: usize = 16;
const MAX_JPEG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EDGE: u32 = 16_384;
const MAX_PIXELS: u32 = 64_000_000;
const RW2_HEADER_LEN: u64 = 24;
const RW2_SIGNATURE: [u8; 16] =
    *b"\x88\xe7\x74\xd8\xf8\x25\x1d\x4d\x94\x7a\x6e\x77\x82\x2b\x5d\x6a";

#[derive(Debug)]
pub enum PreviewError {
    Io(io::Error),
    UnsupportedFormat,
    NoSupportedPreview,
    Malformed,
    LimitExceeded,
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "RAW preview I/O: {error}"),
            Self::UnsupportedFormat => f.write_str("unsupported RAW format or variant"),
            Self::NoSupportedPreview => f.write_str("no supported standalone JPEG preview"),
            Self::Malformed => f.write_str("malformed RAW preview structure"),
            Self::LimitExceeded => f.write_str("RAW preview resource limit exceeded"),
        }
    }
}

impl std::error::Error for PreviewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for PreviewError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            Self::Malformed
        } else {
            Self::Io(error)
        }
    }
}

#[derive(Debug)]
pub struct RawPreview {
    pub jpeg: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub container_orientation: Option<u8>,
}

type Result<T> = std::result::Result<T, PreviewError>;

#[cfg(test)]
pub fn extract(path: &Path) -> Result<RawPreview> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(PreviewError::UnsupportedFormat);
    }
    extract_from(&mut file, metadata.len())
}

struct Tiff<'a, R> {
    input: &'a mut R,
    len: u64,
    little: bool,
    header_len: u64,
}

#[derive(Clone, Copy)]
struct Entry {
    kind: u16,
    count: u32,
    value: [u8; 4],
}

#[derive(Default)]
struct Ifd {
    make: Option<Entry>,
    rw2_jpeg: Option<Entry>,
    subtype: Option<Entry>,
    compression: Option<Entry>,
    photometric: Option<Entry>,
    strip_offset: Option<Entry>,
    strip_length: Option<Entry>,
    orientation: Option<Entry>,
    children: Option<Entry>,
    jpeg_offset: Option<Entry>,
    jpeg_length: Option<Entry>,
    dng_version: Option<Entry>,
    exif: Option<Entry>,
    next: u32,
}

impl<R: Read + Seek> Tiff<'_, R> {
    fn u16(&self, b: [u8; 2]) -> u16 {
        if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    }

    fn u32(&self, b: [u8; 4]) -> u32 {
        if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(PreviewError::Malformed);
        }
        self.input.seek(SeekFrom::Start(offset))?;
        self.input.read_exact(buf)?;
        Ok(())
    }

    fn ifd(&mut self, offset: u32) -> Result<Ifd> {
        let offset = u64::from(offset);
        if offset < self.header_len || offset % 2 != 0 {
            return Err(PreviewError::Malformed);
        }
        let mut count = [0; 2];
        self.read(offset, &mut count)?;
        let count = usize::from(self.u16(count));
        if count > MAX_ENTRIES {
            return Err(PreviewError::LimitExceeded);
        }
        let mut table = [0; MAX_ENTRIES * 12 + 4];
        let size = count * 12;
        self.read(offset + 2, &mut table[..size + 4])?;
        let mut ifd = Ifd::default();
        for &[a, b, c, d, e, f, g, h, i, j, k, l] in table[..size].as_chunks::<12>().0 {
            let slot = match self.u16([a, b]) {
                46 if self.header_len == RW2_HEADER_LEN => &mut ifd.rw2_jpeg,
                254 => &mut ifd.subtype,
                259 => &mut ifd.compression,
                262 => &mut ifd.photometric,
                271 => &mut ifd.make,
                273 => &mut ifd.strip_offset,
                274 => &mut ifd.orientation,
                279 => &mut ifd.strip_length,
                330 => &mut ifd.children,
                513 => &mut ifd.jpeg_offset,
                514 => &mut ifd.jpeg_length,
                50706 => &mut ifd.dng_version,
                34665 => &mut ifd.exif,
                _ => continue,
            };
            let entry = Entry {
                kind: self.u16([c, d]),
                count: self.u32([e, f, g, h]),
                value: [i, j, k, l],
            };
            if slot.replace(entry).is_some() {
                return Err(PreviewError::Malformed);
            }
        }
        let [a, b, c, d] = table[size..][..4] else {
            unreachable!()
        };
        ifd.next = self.u32([a, b, c, d]);
        Ok(ifd)
    }

    fn scalar(&self, entry: Option<Entry>) -> Result<Option<u32>> {
        entry
            .map(|e| match (e.kind, e.count, e.value) {
                (3, 1, [a, b, _, _]) => Ok(u32::from(self.u16([a, b]))),
                (4, 1, value) => Ok(self.u32(value)),
                _ => Err(PreviewError::Malformed),
            })
            .transpose()
    }

    fn has_make(&mut self, entry: Option<Entry>, expected: &[u8]) -> Result<bool> {
        let mut make = [0; 32];
        let Some(Entry {
            kind: 2,
            count,
            value,
        }) = entry
        else {
            return Ok(false);
        };
        let count = count as usize;
        if count != expected.len() || !(5..=make.len()).contains(&count) {
            return Ok(false);
        }
        let offset = u64::from(self.u32(value));
        if offset < self.header_len || offset % 2 != 0 {
            return Err(PreviewError::Malformed);
        }
        self.read(offset, &mut make[..count])?;
        Ok(&make[..count] == expected)
    }

    // Read only named entries from a single directory inside the given region.
    fn selected_entries<const N: usize>(
        &mut self,
        offset: u64,
        min: u64,
        end: u64,
        tags: [u16; N],
    ) -> Result<[Option<Entry>; N]> {
        if offset < min || offset % 2 != 0 || offset.checked_add(2).is_none_or(|v| v > end) {
            return Err(PreviewError::Malformed);
        }
        let mut count = [0; 2];
        self.read(offset, &mut count)?;
        let count = usize::from(self.u16(count));
        if count > MAX_ENTRIES {
            return Err(PreviewError::LimitExceeded);
        }
        let size = count * 12 + 4;
        if offset.checked_add(2 + size as u64).is_none_or(|v| v > end) {
            return Err(PreviewError::Malformed);
        }
        let mut table = [0; MAX_ENTRIES * 12 + 4];
        self.read(offset + 2, &mut table[..size])?;
        let mut entries = [None; N];
        for item in table[..count * 12].as_chunks::<12>().0 {
            let tag = self.u16([item[0], item[1]]);
            if let Some(index) = tags.iter().position(|&wanted| wanted == tag) {
                let entry = Entry {
                    kind: self.u16([item[2], item[3]]),
                    count: self.u32([item[4], item[5], item[6], item[7]]),
                    value: [item[8], item[9], item[10], item[11]],
                };
                if entries[index].replace(entry).is_some() {
                    return Err(PreviewError::Malformed);
                }
            }
        }
        Ok(entries)
    }

    fn om_system_preview(&mut self, root: &Ifd, orientation: Option<u8>) -> Result<RawPreview> {
        if !self.has_make(root.make, b"OM Digital Solutions   \0")? {
            return Err(PreviewError::UnsupportedFormat);
        }
        let exif = match root.exif {
            Some(Entry {
                kind: 4,
                count: 1,
                value,
            }) => u64::from(self.u32(value)),
            None => return Err(PreviewError::NoSupportedPreview),
            _ => return Err(PreviewError::Malformed),
        };
        let [maker] = self.selected_entries(exif, 8, self.len, [0x927c])?;
        let Entry {
            kind: 7,
            count,
            value,
        } = maker.ok_or(PreviewError::NoSupportedPreview)?
        else {
            return Err(PreviewError::Malformed);
        };
        let base = u64::from(self.u32(value));
        let end = base
            .checked_add(u64::from(count))
            .ok_or(PreviewError::Malformed)?;
        if base < 8 || base % 2 != 0 || count < 16 || end > self.len {
            return Err(PreviewError::Malformed);
        }
        let mut header = [0; 16];
        self.read(base, &mut header)?;
        if &header != b"OM SYSTEM\0\0\0II\x04\0" {
            return Err(PreviewError::UnsupportedFormat);
        }
        let [settings] = self.selected_entries(base + 16, base + 16, end, [0x2020])?;
        let Entry {
            kind: 13,
            count: 1,
            value,
        } = settings.ok_or(PreviewError::NoSupportedPreview)?
        else {
            return Err(PreviewError::Malformed);
        };
        let settings_offset = base + u64::from(self.u32(value));
        let [valid, offset, length] =
            self.selected_entries(settings_offset, base + 16, end, [0x100, 0x101, 0x102])?;
        let long = |entry: Option<Entry>| match entry {
            Some(Entry {
                kind: 4,
                count: 1,
                value,
            }) => Ok(u64::from(self.u32(value))),
            None => Err(PreviewError::NoSupportedPreview),
            _ => Err(PreviewError::Malformed),
        };
        if long(valid)? != 1 {
            return Err(PreviewError::NoSupportedPreview);
        }
        let start = base + long(offset)?;
        let length = long(length)?;
        if start < base + 16 || start.checked_add(length).is_none_or(|v| v > end) {
            return Err(PreviewError::Malformed);
        }
        let size = self
            .jpeg_dimensions(start, length)?
            .ok_or(PreviewError::NoSupportedPreview)?;
        load_preview(self, start, length, size, orientation)
    }

    fn has_compressed_cfa(&mut self, root: &Ifd, compression: u32) -> Result<bool> {
        let Some(children) = root.children else {
            return Ok(false);
        };
        for offset in self.children(children)? {
            let raw = self.ifd(offset)?;
            if self.scalar(raw.compression)? == Some(compression)
                && self.scalar(raw.photometric)? == Some(32803)
                && self.scalar(raw.subtype)? == Some(0)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn children(&mut self, entry: Entry) -> Result<impl Iterator<Item = u32> + use<R>> {
        let count = entry.count as usize;
        if count > MAX_SUB_IFDS {
            return Err(PreviewError::LimitExceeded);
        }
        if count == 0 || !matches!(entry.kind, 4 | 13) {
            return Err(PreviewError::Malformed);
        }
        let mut offsets = [0; MAX_SUB_IFDS];
        if count == 1 {
            offsets[0] = self.u32(entry.value);
        } else {
            let offset = u64::from(self.u32(entry.value));
            if offset < self.header_len || offset % 2 != 0 {
                return Err(PreviewError::Malformed);
            }
            let mut buf = [0; MAX_SUB_IFDS * 4];
            self.read(offset, &mut buf[..count * 4])?;
            for (out, bytes) in offsets.iter_mut().zip(buf.as_chunks::<4>().0) {
                *out = self.u32(*bytes);
            }
        }
        Ok(offsets.into_iter().take(count))
    }

    fn jpeg_dimensions(&mut self, start: u64, length: u64) -> Result<Option<(u16, u16)>> {
        let end = start + length;
        if start < self.header_len || length < 4 || end > self.len {
            return Err(PreviewError::Malformed);
        }
        if length > MAX_JPEG_BYTES {
            return Err(PreviewError::LimitExceeded);
        }
        let mut pair = [0; 2];
        self.read(start, &mut pair)?;
        if pair != [0xff, 0xd8] {
            return Err(PreviewError::Malformed);
        }
        let mut cursor = start + 2;
        let mut dimensions = None;
        // Only walk length-delimited JPEG headers, never entropy data or RAW bytes.
        for _ in 0..512 {
            if cursor + 2 > end - 2 {
                return Err(PreviewError::Malformed);
            }
            self.read(cursor, &mut pair)?;
            let marker = pair[1];
            if pair[0] != 0xff {
                return Err(PreviewError::Malformed);
            }
            if marker == 0xff {
                cursor += 1;
                continue;
            }
            if matches!(marker, 0x00 | 0x01 | 0xd0..=0xd9) || cursor + 4 > end - 2 {
                return Err(PreviewError::Malformed);
            }
            self.read(cursor + 2, &mut pair)?;
            let size = u64::from(u16::from_be_bytes(pair));
            let next = cursor + 2 + size;
            if size < 2 || next > end - 2 {
                return Err(PreviewError::Malformed);
            }
            match marker {
                0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                    if !matches!(marker, 0xc0..=0xc2) {
                        return Ok(None);
                    }
                    if size < 8 || dimensions.is_some() {
                        return Err(PreviewError::Malformed);
                    }
                    let mut frame = [0; 6];
                    self.read(cursor + 4, &mut frame)?;
                    let [precision, h1, h2, w1, w2, components] = frame;
                    if !matches!(components, 1 | 3) || precision != 8 {
                        return Ok(None);
                    }
                    if size != 8 + 3 * u64::from(components) {
                        return Err(PreviewError::Malformed);
                    }
                    let (w, h) = (u16::from_be_bytes([w1, w2]), u16::from_be_bytes([h1, h2]));
                    let (width, height) = (u32::from(w), u32::from(h));
                    if w == 0 || h == 0 {
                        return Err(PreviewError::Malformed);
                    }
                    if width > MAX_EDGE || height > MAX_EDGE || width * height > MAX_PIXELS {
                        return Err(PreviewError::LimitExceeded);
                    }
                    dimensions = Some((w, h, components));
                }
                0xda => {
                    let (w, h, components) = dimensions.ok_or(PreviewError::Malformed)?;
                    if size < 8 || next >= end - 2 {
                        return Err(PreviewError::Malformed);
                    }
                    let mut scans = [0];
                    self.read(cursor + 4, &mut scans)?;
                    if scans[0] == 0 || scans[0] > components || size != 6 + 2 * u64::from(scans[0])
                    {
                        return Err(PreviewError::Malformed);
                    }
                    self.read(end - 2, &mut pair)?;
                    return if pair == [0xff, 0xd9] {
                        Ok(Some((w, h)))
                    } else {
                        Err(PreviewError::Malformed)
                    };
                }
                _ => {}
            }
            cursor = next;
        }
        Err(PreviewError::LimitExceeded)
    }
}

pub(crate) fn extract_from<R: Read + Seek>(input: &mut R, len: u64) -> Result<RawPreview> {
    let mut tiff = Tiff {
        input,
        len,
        little: false,
        header_len: 8,
    };
    let mut header = [0; 8];
    if len < 4 {
        return Err(PreviewError::UnsupportedFormat);
    }
    tiff.read(0, &mut header[..4])?;
    let rw2 = &header[..4] == b"II\x55\0";
    let orf = &header[..4] == b"IIRO";
    tiff.little = match &header[..4] {
        b"II\x55\0" => {
            tiff.header_len = RW2_HEADER_LEN;
            true
        }
        b"II\x2a\0" => true,
        b"IIRO" => true,
        b"MM\0\x2a" => false,
        _ => return Err(PreviewError::UnsupportedFormat),
    };
    tiff.read(4, &mut header[4..])?;
    let first = tiff.u32([header[4], header[5], header[6], header[7]]);
    let mut cr2_raw_ifd = None;
    if rw2 {
        let mut signature = [0; 16];
        tiff.read(8, &mut signature)?;
        if signature != RW2_SIGNATURE {
            return Err(PreviewError::UnsupportedFormat);
        }
    } else if !orf && first >= 16 {
        tiff.read(8, &mut header)?;
        if &header[..2] == b"CR" {
            if header[2..4] != [2, 0] {
                return Err(PreviewError::UnsupportedFormat);
            }
            let raw = tiff.u32([header[4], header[5], header[6], header[7]]);
            if raw < 16 || raw == first || raw % 2 != 0 || u64::from(raw) + 6 > len {
                return Err(PreviewError::Malformed);
            }
            cr2_raw_ifd = Some(raw);
            tiff.header_len = 16;
        }
    }
    if first == 0 {
        return Err(PreviewError::UnsupportedFormat);
    }
    let root = tiff.ifd(first)?;
    if orf {
        let orientation = tiff
            .scalar(root.orientation)?
            .filter(|v| (1..=8).contains(v))
            .map(|v| v as u8);
        return tiff.om_system_preview(&root, orientation);
    }
    let dng = !rw2 && cr2_raw_ifd.is_none() && root.dng_version.is_some();
    if dng {
        if !matches!(
            root.dng_version,
            Some(Entry {
                kind: 1,
                count: 4,
                value: [1, ..]
            })
        ) {
            return Err(PreviewError::UnsupportedFormat);
        }
    } else if cr2_raw_ifd.is_none() {
        let supported = if rw2 {
            tiff.has_make(root.make, b"Panasonic\0")?
        } else {
            // Make alone also occurs in ordinary TIFFs. Require the matching
            // proprietary compression on a non-reduced CFA SubIFD.
            (tiff.has_make(root.make, b"NIKON CORPORATION\0")?
                && tiff.has_compressed_cfa(&root, 34713)?)
                || (tiff.has_make(root.make, b"SONY\0")?
                    && tiff.has_compressed_cfa(&root, 32767)?)
        };
        if !supported {
            return Err(PreviewError::UnsupportedFormat);
        }
    }
    let container_orientation = tiff
        .scalar(root.orientation)?
        .filter(|v| (1..=8).contains(v))
        .map(|v| v as u8);
    if rw2 {
        // IFD0 JpgFromRaw (UNDEFINED): byte count and absolute data offset.
        let Entry { kind, count, value } = root.rw2_jpeg.ok_or(PreviewError::NoSupportedPreview)?;
        if kind != 7 {
            return Err(PreviewError::Malformed);
        }
        let (start, length) = (u64::from(tiff.u32(value)), u64::from(count));
        let size = tiff
            .jpeg_dimensions(start, length)?
            .ok_or(PreviewError::NoSupportedPreview)?;
        return load_preview(&mut tiff, start, length, size, container_orientation);
    }
    let mut pending = vec![(first, 0)];
    let mut seen = Vec::with_capacity(MAX_IFDS);
    let mut best = None;
    let mut candidate_error = None;
    while let Some((offset, depth)) = pending.pop() {
        if depth > MAX_DEPTH || seen.len() == MAX_IFDS {
            return Err(PreviewError::LimitExceeded);
        }
        if offset == 0 || seen.contains(&offset) {
            return Err(PreviewError::Malformed);
        }
        seen.push(offset);
        if Some(offset) == cr2_raw_ifd {
            continue;
        }
        let ifd = if offset == first {
            &root
        } else {
            &tiff.ifd(offset)?
        };
        if ifd.next != 0 {
            pending.push((ifd.next, depth));
        }
        if let Some(children) = ifd.children {
            pending.extend(tiff.children(children)?.map(|offset| (offset, depth + 1)));
        }
        if pending.len() + seen.len() > MAX_IFDS {
            return Err(PreviewError::LimitExceeded);
        }
        let compression = tiff.scalar(ifd.compression)?;
        let photo = tiff.scalar(ifd.photometric)?;
        if matches!(photo, Some(32803 | 34892)) {
            continue;
        }
        let preview_strip = if dng {
            compression == Some(7)
                && matches!(photo, Some(2 | 6))
                && tiff.scalar(ifd.subtype)? == Some(1)
        } else if cr2_raw_ifd.is_some() {
            matches!(compression, Some(6 | 7))
        } else {
            if compression != Some(6) || tiff.scalar(ifd.subtype)? != Some(1) {
                continue;
            }
            false
        };
        let strip = if preview_strip
            && matches!(
                (ifd.strip_offset, ifd.strip_length),
                (Some(Entry { count: 1, .. }), Some(Entry { count: 1, .. }))
            ) {
            (ifd.strip_offset, ifd.strip_length)
        } else {
            (None, None)
        };
        for (start, length) in [(ifd.jpeg_offset, ifd.jpeg_length), strip] {
            let (start, length) = match (tiff.scalar(start)?, tiff.scalar(length)?) {
                (Some(start), Some(length)) => (u64::from(start), u64::from(length)),
                (None, None) => continue,
                _ => return Err(PreviewError::Malformed),
            };
            match tiff.jpeg_dimensions(start, length) {
                Ok(Some((w, h))) => {
                    let area = u32::from(w) * u32::from(h);
                    if best.is_none_or(|(old, _, _, _, _)| area > old) {
                        best = Some((area, start, length, w, h));
                    }
                }
                Ok(None) => {}
                Err(error @ (PreviewError::Malformed | PreviewError::LimitExceeded)) => {
                    candidate_error.get_or_insert(error);
                }
                Err(error) => return Err(error),
            }
        }
    }
    let (_, start, length, width, height) =
        best.ok_or_else(|| candidate_error.unwrap_or(PreviewError::NoSupportedPreview))?;
    load_preview(
        &mut tiff,
        start,
        length,
        (width, height),
        container_orientation,
    )
}

fn load_preview<R: Read + Seek>(
    tiff: &mut Tiff<'_, R>,
    start: u64,
    length: u64,
    (width, height): (u16, u16),
    container_orientation: Option<u8>,
) -> Result<RawPreview> {
    let mut jpeg = vec![0; length as usize];
    tiff.read(start, &mut jpeg)?;
    Ok(RawPreview {
        jpeg,
        width,
        height,
        container_orientation,
    })
}
