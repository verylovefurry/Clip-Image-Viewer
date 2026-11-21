//! Embedded JPEG previews for RAW formats outside the strict DNG/CR2/NEF/ARW/RW2/ORF paths:
//! any TIFF-structured RAW (PEF, SRW, ERF, KDC, DCR, 3FR, MEF, MOS, IIQ, NRW, SR2, SRF, RWL,
//! GPR, ...), Fujifilm RAF, Canon CR3 and Minolta MRW. As in the parent
//! module, files are only parsed: the largest structurally referenced baseline/progressive JPEG
//! is returned unchanged after its headers are validated. Sensor data is never decoded.

use super::*;

const MAX_BOXES: usize = 4096;

/// A `Read + Seek` view of `[base, base + len)` inside another reader.
struct Window<'a, R> { inner: &'a mut R, base: u64, len: u64, pos: u64 }

impl<R: Read + Seek> Read for Window<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.len.saturating_sub(self.pos).min(buf.len() as u64) as usize;
        if available == 0 { return Ok(0); }
        self.inner.seek(SeekFrom::Start(self.base + self.pos))?;
        let count = self.inner.read(&mut buf[..available])?;
        self.pos += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for Window<'_, R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let next = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(n) => self.len.checked_add_signed(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
        }.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek outside window"))?;
        self.pos = next;
        Ok(next)
    }
}

fn read_at<R: Read + Seek>(input: &mut R, len: u64, offset: u64, buf: &mut [u8]) -> Result<()> {
    if offset.checked_add(buf.len() as u64).is_none_or(|end| end > len) { return Err(PreviewError::Malformed); }
    input.seek(SeekFrom::Start(offset))?;
    input.read_exact(buf)?;
    Ok(())
}

fn u32_be(b: &[u8]) -> u32 { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) }

/// Largest valid JPEG candidate seen so far.
#[derive(Default)]
struct Best { found: Option<(u32, u64, u64, u16, u16)>, error: Option<PreviewError> }

impl Best {
    fn consider<R: Read + Seek>(&mut self, input: &mut R, len: u64, start: u64, length: u64) -> Result<()> {
        let mut tiff = Tiff { input, len, little: false, header_len: 0 };
        match tiff.jpeg_dimensions(start, length) {
            Ok(Some((w, h))) => {
                let area = u32::from(w) * u32::from(h);
                if self.found.is_none_or(|(old, ..)| area > old) { self.found = Some((area, start, length, w, h)); }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error @ (PreviewError::Malformed | PreviewError::LimitExceeded)) => { self.error.get_or_insert(error); Ok(()) }
            Err(error) => Err(error),
        }
    }

    fn finish<R: Read + Seek>(self, input: &mut R, len: u64, orientation: Option<u8>) -> Result<RawPreview> {
        let (_, start, length, w, h) = self.found.ok_or_else(|| self.error.unwrap_or(PreviewError::NoSupportedPreview))?;
        let mut tiff = Tiff { input, len, little: false, header_len: 0 };
        load_preview(&mut tiff, start, length, (w, h), orientation)
    }
}

/// Collects JPEG candidates from every IFD reachable through IFD chains, SubIFDs and EXIF.
fn tiff_candidates<R: Read + Seek>(input: &mut R, len: u64, best: &mut Best) -> Result<Option<u8>> {
    let mut header = [0; 8];
    read_at(input, len, 0, &mut header)?;
    let (little, header_len) = match &header[..4] {
        b"II\x2a\0" | b"IIRO" | b"IIRS" => (true, 8),
        b"II\x55\0" => (true, RW2_HEADER_LEN),
        b"MM\0\x2a" => (false, 8),
        _ => return Err(PreviewError::UnsupportedFormat),
    };
    let mut tiff = Tiff { input, len, little, header_len };
    let first = tiff.u32([header[4], header[5], header[6], header[7]]);
    let mut pending = vec![(first, 0usize)];
    let mut seen = Vec::new();
    let mut candidates = Vec::new();
    let mut orientation = None;
    while let Some((offset, depth)) = pending.pop() {
        if offset == 0 || depth > MAX_DEPTH || seen.contains(&offset) { continue; }
        if seen.len() >= MAX_IFDS { return Err(PreviewError::LimitExceeded); }
        seen.push(offset);
        // A damaged directory is skipped; previews elsewhere in the file can still be valid.
        let Ok(ifd) = tiff.ifd(offset) else { continue };
        if offset == first {
            orientation = tiff.scalar(ifd.orientation).ok().flatten().filter(|v| (1..=8).contains(v)).map(|v| v as u8);
        }
        pending.push((ifd.next, depth));
        if let Some(children) = ifd.children && let Ok(list) = tiff.children(children) {
            pending.extend(list.map(|child| (child, depth + 1)));
        }
        if let Some(Entry { kind: 4 | 13, count: 1, value }) = ifd.exif { pending.push((tiff.u32(value), depth + 1)); }
        if let Some(Entry { kind: 7, count, value }) = ifd.rw2_jpeg {
            candidates.push((u64::from(tiff.u32(value)), u64::from(count)));
        }
        let pair = |tiff: &Tiff<'_, R>, a: Option<Entry>, b: Option<Entry>| match (tiff.scalar(a), tiff.scalar(b)) {
            (Ok(Some(start)), Ok(Some(length))) => Some((u64::from(start), u64::from(length))),
            _ => None,
        };
        candidates.extend(pair(&tiff, ifd.jpeg_offset, ifd.jpeg_length));
        let compression = tiff.scalar(ifd.compression).ok().flatten();
        let photometric = tiff.scalar(ifd.photometric).ok().flatten();
        let single = matches!((ifd.strip_offset, ifd.strip_length), (Some(Entry { count: 1, .. }), Some(Entry { count: 1, .. })));
        if matches!(compression, Some(6 | 7)) && !matches!(photometric, Some(32803 | 34892)) && single {
            candidates.extend(pair(&tiff, ifd.strip_offset, ifd.strip_length));
        }
    }
    for (start, length) in candidates { best.consider(tiff.input, len, start, length)?; }
    Ok(orientation)
}

fn tiff_generic<R: Read + Seek>(input: &mut R, len: u64) -> Result<RawPreview> {
    let mut best = Best::default();
    let orientation = tiff_candidates(input, len, &mut best)?;
    best.finish(input, len, orientation)
}

fn raf<R: Read + Seek>(input: &mut R, len: u64) -> Result<RawPreview> {
    let mut header = [0; 92];
    read_at(input, len, 0, &mut header)?;
    let mut best = Best::default();
    best.consider(input, len, u64::from(u32_be(&header[84..])), u64::from(u32_be(&header[88..])))?;
    best.finish(input, len, None)
}

/// ISO BMFF boxes directly inside `[start, end)`: `(type, payload start, end, usertype)`.
fn boxes<R: Read + Seek>(input: &mut R, len: u64, start: u64, end: u64) -> Result<Vec<([u8; 4], u64, u64, [u8; 16])>> {
    let mut result = Vec::new();
    let mut at = start;
    while at + 8 <= end {
        if result.len() >= MAX_BOXES { return Err(PreviewError::LimitExceeded); }
        let mut head = [0; 16];
        read_at(input, len, at, &mut head[..8])?;
        let kind: [u8; 4] = head[4..8].try_into().unwrap();
        let (size, mut payload) = match u32_be(&head) {
            0 => (end - at, at + 8),
            1 => {
                read_at(input, len, at + 8, &mut head[8..])?;
                (u64::from_be_bytes(head[8..16].try_into().unwrap()), at + 16)
            }
            n => (u64::from(n), at + 8),
        };
        let box_end = at.checked_add(size).filter(|&e| size >= payload - at && e <= end).ok_or(PreviewError::Malformed)?;
        let mut usertype = [0; 16];
        if &kind == b"uuid" {
            read_at(input, len, payload, &mut usertype)?;
            payload += 16;
        }
        result.push((kind, payload, box_end, usertype));
        at = box_end;
    }
    Ok(result)
}

fn child<R: Read + Seek>(input: &mut R, len: u64, (start, end): (u64, u64), kind: &[u8; 4]) -> Result<Option<(u64, u64)>> {
    Ok(boxes(input, len, start, end)?.into_iter().find(|b| &b.0 == kind).map(|b| (b.1, b.2)))
}

const CR3_PREVIEW: [u8; 16] = [0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16];
const CR3_METADATA: [u8; 16] = [0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];

fn cr3<R: Read + Seek>(input: &mut R, len: u64) -> Result<RawPreview> {
    let top = boxes(input, len, 0, len)?;
    let mut best = Best::default();
    let mut orientation = None;
    // PRVW: a mid-size JPEG inside the preview uuid box.
    for &(_, start, end, _) in top.iter().filter(|b| &b.0 == b"uuid" && b.3 == CR3_PREVIEW) {
        if let Some((prvw, prvw_end)) = child(input, len, (start + 8, end), b"PRVW")? && prvw + 16 <= prvw_end {
            let mut fields = [0; 16];
            read_at(input, len, prvw, &mut fields)?;
            let declared = u64::from(u32_be(&fields[12..]));
            best.consider(input, len, prvw + 16, declared.min(prvw_end - prvw - 16))?;
        }
    }
    if let Some(&(_, moov, moov_end, _)) = top.iter().find(|b| &b.0 == b"moov") {
        let children = boxes(input, len, moov, moov_end)?;
        // CMT1 is a TIFF IFD0 carrying the camera orientation.
        if let Some(&(_, start, end, _)) = children.iter().find(|b| &b.0 == b"uuid" && b.3 == CR3_METADATA)
            && let Some((cmt1, cmt1_end)) = child(input, len, (start, end), b"CMT1")?
        {
            let mut window = Window { inner: &mut *input, base: cmt1, len: cmt1_end - cmt1, pos: 0 };
            let mut ignored = Best::default();
            orientation = tiff_candidates(&mut window, cmt1_end - cmt1, &mut ignored).ok().flatten();
        }
        // The first track holds the full-size JPEG: one sample located by stsz and stco/co64.
        if let Some(&(_, trak, trak_end, _)) = children.iter().find(|b| &b.0 == b"trak") {
            let mut node = Some((trak, trak_end));
            for kind in [b"mdia", b"minf", b"stbl"] {
                node = match node { Some(range) => child(input, len, range, kind)?, None => None };
            }
            if let Some(stbl) = node {
                let mut size = None;
                // stsz: version/flags, sample_size, sample_count, then sizes only when sample_size is 0.
                if let Some((stsz, stsz_end)) = child(input, len, stbl, b"stsz")? && stsz + 12 <= stsz_end {
                    let mut f = [0; 16];
                    read_at(input, len, stsz, &mut f[..12])?;
                    size = match u32_be(&f[4..]) {
                        0 if u32_be(&f[8..]) > 0 && stsz + 16 <= stsz_end => {
                            read_at(input, len, stsz + 12, &mut f[12..])?;
                            Some(u64::from(u32_be(&f[12..])))
                        }
                        0 => None,
                        n => Some(u64::from(n)),
                    };
                }
                let mut offset = None;
                if let Some((co64, end)) = child(input, len, stbl, b"co64")? && co64 + 16 <= end {
                    let mut f = [0; 16];
                    read_at(input, len, co64, &mut f)?;
                    if u32_be(&f[4..]) > 0 { offset = Some(u64::from_be_bytes(f[8..16].try_into().unwrap())); }
                } else if let Some((stco, end)) = child(input, len, stbl, b"stco")? && stco + 12 <= end {
                    let mut f = [0; 12];
                    read_at(input, len, stco, &mut f)?;
                    if u32_be(&f[4..]) > 0 { offset = Some(u64::from(u32_be(&f[8..]))); }
                }
                if let (Some(offset), Some(size)) = (offset, size) { best.consider(input, len, offset, size)?; }
            }
        }
    }
    best.finish(input, len, orientation)
}

fn mrw<R: Read + Seek>(input: &mut R, len: u64) -> Result<RawPreview> {
    let mut head = [0; 8];
    read_at(input, len, 0, &mut head)?;
    let end = 8 + u64::from(u32_be(&head[4..]));
    let mut at = 8;
    while at + 8 <= end.min(len) {
        read_at(input, len, at, &mut head)?;
        let size = u64::from(u32_be(&head[4..]));
        if &head[..4] == b"\0TTW" {
            // The TTW block is a complete TIFF whose offsets are relative to the block data.
            let length = size.min(len.saturating_sub(at + 8));
            let mut window = Window { inner: &mut *input, base: at + 8, len: length, pos: 0 };
            return tiff_generic(&mut window, length).map(|mut preview| {
                preview.container_orientation = preview.container_orientation.filter(|v| (1..=8).contains(v));
                preview
            });
        }
        at = at.checked_add(8 + size).ok_or(PreviewError::Malformed)?;
    }
    Err(PreviewError::NoSupportedPreview)
}

/// Picks the container from its signature and returns its largest embedded JPEG.
pub(crate) fn extract_any_from<R: Read + Seek>(input: &mut R, len: u64) -> Result<RawPreview> {
    let mut head = [0; 16];
    if len < 16 { return Err(PreviewError::UnsupportedFormat); }
    read_at(input, len, 0, &mut head)?;
    if &head == b"FUJIFILMCCD-RAW " { return raf(input, len); }
    if &head[4..12] == b"ftypcrx " { return cr3(input, len); }
    if &head[..4] == b"\0MRM" { return mrw(input, len); }
    tiff_generic(input, len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A tiny valid baseline JPEG (16×8, grey) produced by the `image` crate.
    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut out)
            .encode(&vec![128u8; (width * height * 3) as usize], width, height, image::ExtendedColorType::Rgb8).unwrap();
        out
    }

    fn run(data: Vec<u8>) -> Result<RawPreview> {
        let len = data.len() as u64;
        extract_any_from(&mut Cursor::new(data), len)
    }

    fn tiff_with_previews(previews: &[Vec<u8>], orientation: u16) -> Vec<u8> {
        // IFD0: orientation + JPEGInterchangeFormat/Length of the first preview; IFD1 the second.
        let mut file = b"II\x2a\0\x08\0\0\0".to_vec();
        let ifd_len = |n: usize| 2 + n * 12 + 4;
        let ifd1 = 8 + ifd_len(3);
        let data = ifd1 + ifd_len(2);
        let second = data + previews[0].len();
        let entry = |file: &mut Vec<u8>, tag: u16, kind: u16, value: u32| {
            file.extend(tag.to_le_bytes()); file.extend(kind.to_le_bytes()); file.extend(1u32.to_le_bytes()); file.extend(value.to_le_bytes());
        };
        file.extend(3u16.to_le_bytes());
        entry(&mut file, 274, 3, u32::from(orientation));
        entry(&mut file, 513, 4, data as u32);
        entry(&mut file, 514, 4, previews[0].len() as u32);
        file.extend((ifd1 as u32).to_le_bytes());
        file.extend(2u16.to_le_bytes());
        entry(&mut file, 513, 4, second as u32);
        entry(&mut file, 514, 4, previews[1].len() as u32);
        file.extend(0u32.to_le_bytes());
        for p in previews { file.extend(p); }
        file
    }

    #[test]
    fn tiff_raw_returns_largest_preview_and_orientation() {
        let preview = run(tiff_with_previews(&[jpeg(16, 8), jpeg(64, 32)], 6)).unwrap();
        assert_eq!((preview.width, preview.height, preview.container_orientation), (64, 32, Some(6)));
        assert_eq!(&preview.jpeg[..2], &[0xff, 0xd8]);
    }

    #[test]
    fn raf_uses_header_offsets() {
        let image = jpeg(32, 16);
        let mut file = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        file.resize(100, 0);
        file[84..88].copy_from_slice(&100u32.to_be_bytes());
        file[88..92].copy_from_slice(&(image.len() as u32).to_be_bytes());
        file.extend(&image);
        let preview = run(file).unwrap();
        assert_eq!((preview.width, preview.height), (32, 16));
        assert_eq!(preview.jpeg, image);
    }

    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(payload);
        out
    }

    #[test]
    fn cr3_prefers_full_size_track_jpeg_over_prvw() {
        let (small, large) = (jpeg(16, 8), jpeg(48, 32));
        let mut prvw = vec![0; 16];
        prvw[12..16].copy_from_slice(&(small.len() as u32).to_be_bytes());
        prvw.extend(&small);
        let mut uuid = CR3_PREVIEW.to_vec();
        uuid.extend([0; 8]);
        uuid.extend(mp4_box(b"PRVW", &prvw));
        let ftyp = mp4_box(b"ftyp", b"crx \0\0\0\x01crx isom");
        let preview_box = mp4_box(b"uuid", &uuid);
        // stsz: version/flags, sample_size, count; co64: version/flags, count, offset.
        let build = |offset: u64| {
            let mut stsz = vec![0; 4];
            stsz.extend((large.len() as u32).to_be_bytes());
            stsz.extend(1u32.to_be_bytes());
            let mut co64 = vec![0; 4];
            co64.extend(1u32.to_be_bytes());
            co64.extend(offset.to_be_bytes());
            let stbl = mp4_box(b"stbl", &[mp4_box(b"stsz", &stsz), mp4_box(b"co64", &co64)].concat());
            mp4_box(b"moov", &mp4_box(b"trak", &mp4_box(b"mdia", &mp4_box(b"minf", &stbl))))
        };
        let head = ftyp.len() + preview_box.len() + build(0).len();
        let moov = build((head + 8) as u64);
        let file = [ftyp, preview_box, moov, mp4_box(b"mdat", &large)].concat();
        let preview = run(file).unwrap();
        assert_eq!((preview.width, preview.height), (48, 32));
    }

    #[test]
    fn mrw_reads_tiff_block_relative_offsets() {
        let ttw = tiff_with_previews(&[jpeg(16, 16), jpeg(32, 24)], 1);
        let mut blocks = b"\0PRD".to_vec();
        blocks.extend(4u32.to_be_bytes());
        blocks.extend([0; 4]);
        blocks.extend(b"\0TTW");
        blocks.extend((ttw.len() as u32).to_be_bytes());
        blocks.extend(&ttw);
        let mut file = b"\0MRM".to_vec();
        file.extend((blocks.len() as u32).to_be_bytes());
        file.extend(blocks);
        let preview = run(file).unwrap();
        assert_eq!((preview.width, preview.height, preview.container_orientation), (32, 24, Some(1)));
    }

    #[test]
    fn rejects_files_without_previews() {
        // Header, one empty IFD and padding to the 16-byte signature probe.
        assert!(matches!(run(b"II\x2a\0\x08\0\0\0\0\0\0\0\0\0\0\0".to_vec()), Err(PreviewError::NoSupportedPreview)));
        assert!(matches!(run(vec![0; 64]), Err(PreviewError::UnsupportedFormat)));
    }
}
