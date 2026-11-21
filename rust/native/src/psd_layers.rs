use super::*;
use std::io::SeekFrom;

use clipview_core::compositor::{BlendMode, composite_rgba};
use image::{Rgba, imageops::FilterType};
use serde_json::{Value, json};

use crate::{asset::Assets, collection::Item, picture};

const LIMIT: u64 = 128 << 20;
const FILE_LIMIT: u64 = 2 << 30;
const WORK_LIMIT: u64 = 384 << 20;
const MAX_LAYERS: usize = 8192;
const MAX_DEPTH: usize = 32;
const UNAVAILABLE: &str = "독립 합성을 지원하지 않는 PSD/PSB 레이어입니다.";

trait Source: BufRead + Seek {}
impl<T: BufRead + Seek> Source for T {}
type Input = Sections<Box<dyn Source>>;

#[derive(Clone, Copy, Default)]
struct Rect {
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
}

impl Rect {
    fn read(data: &mut Bytes<'_>) -> Result<Self> {
        Ok(Self {
            top: i32::from_be_bytes(data.array()?),
            left: i32::from_be_bytes(data.array()?),
            bottom: i32::from_be_bytes(data.array()?),
            right: i32::from_be_bytes(data.array()?),
        })
    }

    fn size(self) -> (u32, u32) {
        (
            (i64::from(self.right) - i64::from(self.left)).max(0) as u32,
            (i64::from(self.bottom) - i64::from(self.top)).max(0) as u32,
        )
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            top: self.top.max(other.top),
            left: self.left.max(other.left),
            bottom: self.bottom.min(other.bottom),
            right: self.right.min(other.right),
        }
    }
}

struct Channel {
    id: i16,
    length: u64,
    offset: u64,
}

struct Mask {
    rect: Rect,
    default: u8,
    flags: u8,
}

struct Node {
    id: String,
    name: String,
    rect: Rect,
    channels: Vec<Channel>,
    blend: [u8; 4],
    opacity: u8,
    fill: u8,
    visible: bool,
    clipping: bool,
    divider: u32,
    mask: Option<Mask>,
    vector_mask: bool,
    effects: Vec<&'static str>,
    adjustment: bool,
    children: Vec<Node>,
}

fn record(input: &mut Input, large: bool) -> Result<Node> {
    let rect = Rect::read(&mut Bytes(&input.array::<16>()?))?;
    let count = u16::from_be_bytes(input.array()?);
    if count > 56 {
        return Err("PSD/PSB 레이어 채널 수가 올바르지 않습니다.".into());
    }
    let mut channels = Vec::with_capacity(count.into());
    for _ in 0..count {
        channels.push(Channel {
            id: i16::from_be_bytes(input.array()?),
            length: input.length(large)?,
            offset: 0,
        });
    }
    if &input.array::<4>()? != b"8BIM" {
        return Err("PSD/PSB 레이어 혼합 모드 서명이 올바르지 않습니다.".into());
    }
    let mut blend = input.array()?;
    let [opacity, clipping, flags, _] = input.array()?;
    let mut fill = 255;
    let mut divider = 0;
    let mut vector_mask = false;
    let mut adjustment = false;
    let mut effects = Vec::new();
    let (name, mask) = input.section(false, |extra| {
        let mask = extra.section(false, |mask| {
            if mask.remaining() == 0 {
                return Ok(None);
            }
            let rect = Rect::read(&mut Bytes(&mask.array::<16>()?))?;
            let [default, flags] = mask.array()?;
            if flags & 16 != 0 {
                effects.push("마스크 밀도/페더 미재현");
            }
            Ok(Some(Mask {
                rect,
                default,
                flags,
            }))
        })?;
        extra.section(false, |ranges| {
            // Default blend ranges use 0/0/255/255 for each source/destination.
            while ranges.remaining() >= 4 {
                if ranges.array::<4>()? != [0, 0, 255, 255] {
                    effects.push("혼합 범위 미재현");
                    break;
                }
            }
            Ok(())
        })?;
        let len = usize::from(extra.array::<1>()?[0]);
        let mut name = vec![0; len];
        extra.read(&mut name)?;
        extra.skip(((4 - (len + 1) % 4) % 4) as u64)?;
        let mut name = encoding_rs::MACINTOSH.decode(&name).0.into_owned();
        let mut blocks = 0;
        while extra.remaining() >= 12 {
            blocks += 1;
            if blocks > 4096 {
                return Err("PSD/PSB 레이어 추가 정보 수가 안전 제한을 초과합니다.".into());
            }
            let signature = extra.array::<4>()?;
            if !matches!(&signature, b"8BIM" | b"8B64") {
                return Err("PSD/PSB 레이어 추가 정보 서명이 올바르지 않습니다.".into());
            }
            let key = extra.array::<4>()?;
            extra.section(wide_key(large, &signature, &key), |tag| {
                match &key {
                    b"luni" => {
                        let count = u32::from_be_bytes(tag.array()?);
                        if u64::from(count) * 2 > tag.remaining() || count > 32768 {
                            return Err(TRUNCATED.into());
                        }
                        let mut utf16 = Vec::with_capacity(count as usize);
                        for _ in 0..count {
                            utf16.push(u16::from_be_bytes(tag.array()?));
                        }
                        name = String::from_utf16_lossy(&utf16)
                            .trim_end_matches('\0')
                            .to_owned();
                    }
                    b"lsct" | b"lsdk" => {
                        divider = u32::from_be_bytes(tag.array()?);
                        if tag.remaining() >= 8 && &tag.array::<4>()? == b"8BIM" {
                            blend = tag.array()?;
                        }
                    }
                    b"iOpa" => fill = tag.array::<1>()?[0],
                    b"vmsk" | b"vsms" => vector_mask = true,
                    b"lrFX" | b"lfx2" | b"lmfx" => effects.push("레이어 스타일 미재현"),
                    b"lfxs" | b"knko" | b"clbl" | b"infx" | b"tsly" => {
                        if tag.remaining() > 0
                            && tag.array::<1>()?[0] != 0
                            && &key != b"clbl"
                            && &key != b"tsly"
                        {
                            effects.push("고급 혼합 옵션 미재현");
                        }
                    }
                    b"brit" | b"levl" | b"curv" | b"expA" | b"vibA" | b"hue " | b"hue2"
                    | b"blnc" | b"blwh" | b"phfl" | b"mixr" | b"clrL" | b"nvrt" | b"post"
                    | b"thrs" | b"grdm" | b"selc" => adjustment = true,
                    b"TySh" | b"tySh" | b"SoLd" | b"SoLE" | b"PlLd" | b"plLd" | b"SoCo"
                    | b"GdFl" | b"PtFl" | b"vscg" => {
                        effects.push("저장 래스터만 사용");
                    }
                    _ => {}
                }
                Ok(())
            })?;
            extra.skip_padding()?;
        }
        Ok((name, mask))
    })?;
    Ok(Node {
        id: String::new(),
        name,
        rect,
        channels,
        blend,
        opacity,
        fill,
        visible: flags & 2 == 0,
        clipping: clipping != 0,
        divider,
        mask,
        vector_mask,
        effects,
        adjustment,
        children: Vec::new(),
    })
}

fn read_records(input: &mut Input, large: bool) -> Result<Vec<Node>> {
    if input.remaining() == 0 {
        return Ok(Vec::new());
    }
    let count = usize::from(i16::from_be_bytes(input.array()?).unsigned_abs());
    if count > MAX_LAYERS {
        return Err("PSD/PSB 레이어 수가 안전 제한을 초과합니다.".into());
    }
    let mut nodes = Vec::with_capacity(count);
    for _ in 0..count {
        nodes.push(record(input, large)?);
    }
    for node in &mut nodes {
        for channel in &mut node.channels {
            channel.offset = input.position;
            input.skip(channel.length)?;
        }
    }
    // Records are bottom-to-top. A bounding divider precedes the group's children.
    let mut stack = vec![Vec::new()];
    for mut node in nodes {
        match node.divider {
            3 if stack.len() < MAX_DEPTH => stack.push(Vec::new()),
            1 | 2 if stack.len() > 1 => {
                node.children = stack.pop().unwrap();
                stack.last_mut().unwrap().push(node);
            }
            0 => stack.last_mut().unwrap().push(node),
            _ => return Err("PSD/PSB 레이어 그룹 구조가 올바르지 않습니다.".into()),
        }
    }
    if stack.len() != 1 {
        return Err("PSD/PSB 레이어 그룹이 닫히지 않았습니다.".into());
    }
    fn assign(nodes: &mut [Node], parent: &str) {
        for (index, node) in nodes.iter_mut().enumerate() {
            node.id = format!("{parent}{index}");
            assign(&mut node.children, &format!("{}.", node.id));
        }
    }
    let mut nodes = stack.pop().unwrap();
    assign(&mut nodes, "");
    Ok(nodes)
}

fn seek(input: &mut Input, pos: u64) -> Result<()> {
    input.source.seek(SeekFrom::Start(pos)).map_err(error)?;
    input.position = pos;
    Ok(())
}

fn read_tree(input: &mut Input, header: &Composite) -> Result<Vec<Node>> {
    let large = header.large;
    seek(input, header.layer_offset)?;
    let location = input.section(large, |section| {
        if section.remaining() == 0 {
            return Ok(None);
        }
        let primary = section.position;
        let count = section.section(large, |info| {
            if info.remaining() == 0 {
                Ok(0)
            } else {
                Ok(i16::from_be_bytes(info.array()?))
            }
        })?;
        let mut selected = Some((primary, large));
        if section.remaining() < 4 {
            return Ok(selected);
        }
        let mask_len = section.length(false)?;
        section.skip(mask_len)?;
        while section.remaining() >= 12 {
            let signature = section.array::<4>()?;
            if !matches!(&signature, b"8BIM" | b"8B64") {
                break;
            }
            let key = section.array::<4>()?;
            let wide = wide_key(large, &signature, &key);
            let offset = section.position;
            let len = section.length(wide)?;
            if len >= 2
                && ((header.depth == 16 && &key == b"Lr16")
                    || (count == 0 && &key == b"Layr" && selected == Some((primary, large))))
            {
                selected = Some((offset, wide));
            }
            section.skip(len)?;
            section.skip_padding()?;
        }
        Ok(selected)
    })?;
    if let Some((offset, wide)) = location {
        seek(input, offset)?;
        input.section(wide, |info| read_records(info, large))
    } else {
        Ok(Vec::new())
    }
}

fn blend(key: &[u8; 4]) -> (&'static str, &'static str, Option<u8>) {
    let (name, label, code) = match key {
        b"pass" => ("pass through", "통과", 0),
        b"norm" => ("normal", "표준", 0),
        b"dark" => ("darken", "어둡게", 1),
        b"mul " => ("multiply", "곱하기", 2),
        b"idiv" => ("color burn", "색상 번", 3),
        b"lbrn" => ("linear burn", "선형 번", 4),
        b"fsub" => ("subtract", "빼기", 5),
        b"lite" => ("lighten", "밝게", 7),
        b"scrn" => ("screen", "스크린", 8),
        b"div " => ("color dodge", "색상 닷지", 9),
        b"lddg" => ("linear dodge", "선형 닷지(추가)", 11),
        b"over" => ("overlay", "오버레이", 14),
        b"sLit" => ("soft light", "소프트 라이트", 15),
        b"hLit" => ("hard light", "하드 라이트", 16),
        b"vLit" => ("vivid light", "선명한 라이트", 17),
        b"lLit" => ("linear light", "선형 라이트", 18),
        b"pLit" => ("pin light", "핀 라이트", 19),
        b"hMix" => ("hard mix", "하드 혼합", 20),
        b"diff" => ("difference", "차이", 21),
        b"smud" => ("exclusion", "제외", 22),
        b"hue " => ("hue", "색조", 23),
        b"sat " => ("saturation", "채도", 24),
        b"colr" => ("color", "색상", 25),
        b"lum " => ("luminosity", "광도", 26),
        b"fdiv" => ("divide", "나누기", 36),
        b"diss" => return ("dissolve", "디졸브", None),
        b"dkCl" => return ("darker color", "어두운 색상", None),
        b"lgCl" => return ("lighter color", "밝은 색상", None),
        _ => return ("unknown", "알 수 없는 혼합", None),
    };
    (name, label, Some(code))
}

impl Node {
    fn group(&self) -> bool {
        matches!(self.divider, 1 | 2)
    }

    fn shown(&self, visibility: &Value) -> bool {
        visibility
            .get(&self.id)
            .and_then(Value::as_bool)
            .unwrap_or(self.visible)
    }

    fn channel(&self, id: i16) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|channel| channel.id == id && channel.length > 2)
    }

    fn raster(&self, header: &Composite) -> bool {
        let (w, h) = self.rect.size();
        !self.group()
            && !self.adjustment
            && w != 0
            && h != 0
            && u64::from(w) * u64::from(h) <= LIMIT / 4
            && (0..header.color_channels).all(|i| self.channel(i as i16).is_some())
    }

    fn toggleable(&self, header: &Composite) -> bool {
        self.raster(header) || (self.group() && !self.children.is_empty()
            && self.children.iter().all(|n| n.toggleable(header)))
    }

    fn description(&self, header: &Composite, supported: bool) -> Value {
        let (name, label, code) = blend(&self.blend);
        let mut effects = self.effects.clone();
        if self.vector_mask {
            effects.push("벡터 마스크 미재현");
        }
        if self.adjustment {
            effects.push("조정 레이어 미재현");
        }
        if code.is_none() {
            effects.push("이 혼합 모드는 표준으로 근사");
        }
        if self.channel(-3).is_some() {
            effects.push("실제 사용자 마스크 미재현");
        }
        if self
            .mask
            .as_ref()
            .is_some_and(|m| m.flags & 2 == 0 && m.rect.size() != (0, 0))
            && self.channel(-2).is_none()
        {
            effects.push("픽셀 마스크 데이터 없음");
        }
        if self.group() && self.blend == *b"pass" && !self.pass_through() {
            effects.push("통과 그룹을 격리 합성으로 근사");
        }
        effects.sort_unstable();
        effects.dedup();
        json!({
            "id": self.id,
            "name": if self.name.is_empty() { if self.group() { "그룹" } else { "이름 없는 레이어" } } else { &self.name },
            "type": if self.group() { "group" } else { "layer" },
            "visible": self.visible, "opacity": (u32::from(self.opacity) * 100 + 127) / 255,
            "blendMode": name, "blendModeLabel": label, "clipping": self.clipping,
            "mask": self.mask.is_some() || self.vector_mask || self.channel(-3).is_some(),
            "effects": effects, "toggleAvailable": supported && self.toggleable(header),
            "thumbnailAvailable": self.raster(header),
            "previewAccuracy": if self.group() { "레이어 그룹" }
                else if !self.raster(header) { "독립 레이어 래스터 없음" }
                else if !effects.is_empty() { "저장 래스터 · 일부 효과 미재현" }
                else { "저장 레이어 래스터" },
            "children": self.children.iter().rev().map(|n| n.description(header, supported)).collect::<Vec<_>>(),
        })
    }

    fn pass_through(&self) -> bool {
        self.group()
            && &self.blend == b"pass"
            && self.opacity == 255
            && self.fill == 255
            && self.mask.is_none()
            && !self.clipping
    }
}

struct Document {
    source: Input,
    header: Composite,
    nodes: Vec<Node>,
}

fn visit(nodes: &[Node]) -> impl Iterator<Item = &Node> {
    fn nested(nodes: &[Node]) -> Box<dyn Iterator<Item = &Node> + '_> {
        Box::new(
            nodes
                .iter()
                .flat_map(|node| std::iter::once(node).chain(nested(&node.children))),
        )
    }
    nested(nodes)
}

impl Document {
    fn open(item: &Item) -> Result<Input> {
        let (reader, len): (Box<dyn Source>, u64) = match item {
            Item::ArchiveEntry { .. } => {
                let data = item.read(LIMIT)?;
                let len = data.len() as u64;
                (Box::new(Cursor::new(data)), len)
            }
            _ => {
                let file = File::open(item.disk_path()).map_err(error)?;
                let len = file.metadata().map_err(error)?.len();
                (Box::new(BufReader::new(file)), len)
            }
        };
        if len > FILE_LIMIT {
            return Err("이미지 파일 크기가 안전 제한을 초과합니다.".into());
        }
        Ok(Sections::new(reader, len))
    }

    fn read(item: &Item) -> Result<Self> {
        Self::parse(Self::open(item)?)
    }

    fn parse(mut source: Input) -> Result<Self> {
        let header = composite(&mut source)?;
        let nodes = read_tree(&mut source, &header)?;
        Ok(Self {
            source,
            header,
            nodes,
        })
    }

    fn bounds(&self) -> Rect {
        Rect {
            top: 0,
            left: 0,
            bottom: self.header.height as i32,
            right: self.header.width as i32,
        }
    }

    fn supported(&self) -> bool {
        supported(&self.header, &self.nodes)
    }

    fn description(&self) -> Value {
        describe(&self.header, &self.nodes)
    }
}

fn supported(header: &Composite, nodes: &[Node]) -> bool {
    fn depth(nodes: &[Node]) -> u64 {
        nodes
            .iter()
            .map(|n| if n.group() { 1 + depth(&n.children) } else { 0 })
            .max()
            .unwrap_or(0)
    }
    let canvas = u64::from(header.width) * u64::from(header.height) * 4;
    let mut max_channel = 0u64;
    for node in visit(nodes) {
        max_channel = max_channel.max(node.channels.iter().map(|c| c.length).max().unwrap_or(0));
    }
    // Raster layers are decoded and discarded one at a time, not retained for the
    // entire document. Bound the simultaneously live canvases and encoded channel.
    canvas.saturating_mul(3 + 2 * depth(nodes)).saturating_add(max_channel) <= WORK_LIMIT
        && visit(nodes).any(|n| n.toggleable(header))
}

fn describe(header: &Composite, nodes: &[Node]) -> Value {
    if nodes.is_empty() {
        return Value::Null;
    }
    let supported = supported(header, nodes);
    json!({
        "format": if header.large { "PSB" } else { "PSD" },
        "width": header.width, "height": header.height,
        "toggleSupported": supported, "pickSupported": supported,
        "thumbnailSupported": visit(nodes).any(|n| n.raster(header)),
        "approximateRendering": true,
        "note": if supported {
            "기본 가시성은 저장 합성을 사용하며, 변경되지 않은 영역은 보존합니다. 변경 영역은 근사 합성이므로 레이어 스타일, 조정, 벡터 마스크 및 일부 특수 혼합이 원본과 다를 수 있습니다."
        } else {
            "저장 합성과 레이어 정보를 표시합니다. 이 파일에서는 독립 레이어 합성을 지원하지 않습니다."
        },
        "layers": nodes.iter().rev().map(|n| n.description(header, supported)).collect::<Vec<_>>(),
    })
}

pub fn load(assets: &Assets, item: &Item) -> Result<Value> {
    let mut source = Document::open(item)?;
    let header = composite(&mut source)?;
    let merged_start = source.position;
    if source.remaining() > LIMIT {
        return Err("PSD/PSB 합성 이미지 데이터가 안전 제한을 초과합니다.".into());
    }
    let (width, height) = (header.width, header.height);
    let description = match read_tree(&mut source, &header) {
        Ok(nodes) => describe(&header, &nodes),
        Err(message) => json!({
            "format": item.ext().to_ascii_uppercase(),
            "width": width, "height": height,
            "toggleSupported": false, "pickSupported": false, "thumbnailSupported": false,
            "approximateRendering": true, "layers": [],
            "note": format!("저장 합성만 표시합니다. 레이어를 읽지 못했습니다: {message}"),
        }),
    };
    seek(&mut source, merged_start)?;
    let image = decode_composite(header, &source.rest()?)?;
    picture::rendered(assets, item, image, description, "PSD 합성 이미지")
}

fn plane(
    source: &mut Input,
    channel: &Channel,
    rect: Rect,
    header: &Composite,
    mut emit: impl FnMut(u32, &[u8]),
) -> Result<()> {
    let (w, h) = rect.size();
    let row_bytes = (u64::from(w) * u64::from(header.depth)).div_ceil(8);
    if channel.length > LIMIT || row_bytes.saturating_mul(u64::from(h)) > LIMIT {
        return Err("PSD/PSB 레이어 채널 크기가 안전 제한을 초과합니다.".into());
    }
    if w == 0 || h == 0 {
        return Ok(());
    }
    seek(source, channel.offset)?;
    let mut encoded = vec![0; channel.length as usize];
    source.read(&mut encoded)?;
    let mut data = Bytes(&encoded);
    let method = data.u16()?;
    let mut rows = match method {
        0 => Rows::Raw(data),
        1 => Rows::Rle {
            sizes: Bytes(data.take(h as usize * if header.large { 4 } else { 2 })?),
            data,
            large: header.large,
        },
        2 | 3 if method == 2 || header.depth != 1 => Rows::Zip(Zip {
            input: data.0,
            decoder: Decompress::new(true),
            ended: false,
        }),
        _ => {
            return Err(format!(
                "지원하지 않는 PSD/PSB 레이어 압축 방식입니다 ({method})."
            ));
        }
    };
    let mut scratch = vec![0; row_bytes as usize];
    for y in 0..h {
        let row = rows.read(&mut scratch)?;
        if method == 3 {
            undo_prediction(row, header.depth);
        }
        emit(y, row);
    }
    rows.finish()
}

fn sample(row: &[u8], x: usize, depth: u16) -> u8 {
    match depth {
        1 => {
            if row[x / 8] & (0x80 >> (x % 8)) == 0 {
                255
            } else {
                0
            }
        }
        8 => row[x],
        16 => ((u32::from(u16::from_be_bytes([row[2 * x], row[2 * x + 1]])) + 128) / 257) as u8,
        _ => unreachable!(),
    }
}

fn mask_image(
    source: &mut Input,
    header: &Composite,
    node: &Node,
    area: Rect,
    image: &mut RgbaImage,
) -> Result<()> {
    let Some(mask) = node.mask.as_ref().filter(|m| m.flags & 2 == 0) else {
        return Ok(());
    };
    let Some(channel) = node.channel(-2) else {
        // A zero-area mask can be represented by its default value alone.
        if mask.rect.size() == (0, 0) {
            for pixel in image.pixels_mut() {
                pixel[3] = scale_alpha(
                    pixel[3],
                    if mask.flags & 4 == 0 {
                        mask.default
                    } else {
                        255 - mask.default
                    },
                );
            }
        }
        return Ok(());
    };
    let mut rect = mask.rect;
    if mask.flags & 1 != 0 {
        let shift = |v: i32, offset: i32| v.checked_add(offset).ok_or_else(|| TRUNCATED.to_owned());
        rect.top = shift(rect.top, node.rect.top)?;
        rect.bottom = shift(rect.bottom, node.rect.top)?;
        rect.left = shift(rect.left, node.rect.left)?;
        rect.right = shift(rect.right, node.rect.left)?;
    }
    let invert = |value| {
        if mask.flags & 4 == 0 {
            value
        } else {
            255 - value
        }
    };
    let overlap = area.intersect(rect);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let (x, y) = (area.left + x as i32, area.top + y as i32);
        if x < overlap.left || x >= overlap.right || y < overlap.top || y >= overlap.bottom {
            pixel[3] = scale_alpha(pixel[3], invert(mask.default));
        }
    }
    if overlap.size().0 != 0 && overlap.size().1 != 0 {
        plane(source, channel, rect, header, |y, row| {
            let y = i64::from(rect.top) + i64::from(y);
            if y < i64::from(overlap.top) || y >= i64::from(overlap.bottom) {
                return;
            }
            for x in overlap.left..overlap.right {
                let pixel =
                    image.get_pixel_mut((x - area.left) as u32, (y - i64::from(area.top)) as u32);
                pixel[3] = scale_alpha(
                    pixel[3],
                    invert(sample(
                        row,
                        (i64::from(x) - i64::from(rect.left)) as usize,
                        header.depth,
                    )),
                );
            }
        })?;
    }
    Ok(())
}

fn scale_alpha(a: u8, b: u8) -> u8 {
    ((u16::from(a) * u16::from(b) + 127) / 255) as u8
}

fn raster(source: &mut Input, header: &Composite, node: &Node, area: Rect) -> Result<RgbaImage> {
    let (width, height) = area.size();
    let mut image = RgbaImage::from_pixel(width, height, Rgba([255; 4]));
    if width == 0 || height == 0 {
        return Ok(image);
    }
    let resources = Resources::read(Bytes(&header.resources))?;
    let mut alpha = if header.mode == 4 && node.channel(-1).is_some() {
        vec![255; width as usize * height as usize]
    } else {
        Vec::new()
    };
    for channel in node.channels.iter().filter(|c| {
        c.length > 2 && (c.id == -1 || (0..header.color_channels as i16).contains(&c.id))
    }) {
        plane(source, channel, node.rect, header, |y, row| {
            let y = i64::from(node.rect.top) + i64::from(y);
            if y < i64::from(area.top) || y >= i64::from(area.bottom) {
                return;
            }
            let y = (y - i64::from(area.top)) as u32;
            for x in 0..width {
                let value = sample(
                    row,
                    (i64::from(area.left) - i64::from(node.rect.left) + i64::from(x)) as usize,
                    header.depth,
                );
                let pixel = image.get_pixel_mut(x, y);
                if channel.id == -1 {
                    if header.mode == 4 {
                        alpha[(y * width + x) as usize] = value;
                    } else {
                        pixel[3] = value;
                    }
                } else {
                    match header.mode {
                        0 | 1 => pixel.0[..3].fill(value),
                        2 => {
                            let i = usize::from(value);
                            pixel.0[..3].copy_from_slice(&[
                                header.palette[i],
                                header.palette[i + 256],
                                header.palette[i + 512],
                            ]);
                            if resources.transparent_index == Some(u16::from(value)) {
                                pixel[3] = 0;
                            }
                        }
                        3 => pixel[channel.id as usize] = value,
                        4 => pixel[channel.id as usize] = 255 - value,
                        _ => unreachable!(),
                    }
                }
            }
        })?;
    }
    // Unlike the saved composite, individual layer channels are not white-matted.
    convert_colors(
        image.as_flat_samples_mut().samples.as_chunks_mut::<4>().0,
        resources.icc,
        header.mode,
        &alpha,
    )?;
    mask_image(source, header, node, area, &mut image)?;
    Ok(image)
}

fn blend_at(
    dst: &mut RgbaImage,
    canvas: Rect,
    src: &RgbaImage,
    area: Rect,
    node: &Node,
    clipped: bool,
) -> Result<()> {
    let dx = (area.left - canvas.left) as usize * 4;
    let dy = (area.top - canvas.top) as usize;
    let stride = dst.width() as usize * 4;
    let mode = BlendMode::new(blend(&node.blend).2.unwrap_or(0)).map_err(error)?;
    let opacity = f64::from(node.opacity) * f64::from(node.fill) / (255. * 255.);
    let len = src.width() as usize * 4;
    if len == 0 {
        return Ok(());
    }
    for (y, row) in src.as_raw().chunks_exact(len).enumerate() {
        let start = (dy + y) * stride + dx;
        composite_rgba(
            &mut dst.as_mut()[start..start + len],
            row,
            mode,
            opacity,
            clipped,
        )
        .map_err(error)?;
    }
    Ok(())
}

struct Render<'a> {
    source: &'a mut Input,
    header: &'a Composite,
    visibility: &'a Value,
    picking: bool,
}

impl Render<'_> {
    fn layer(
        &mut self,
        node: &Node,
        canvas: Rect,
    ) -> Result<Option<(Rect, RgbaImage, Option<String>)>> {
        if !node.shown(self.visibility) || node.opacity == 0 || node.fill == 0 {
            return Ok(None);
        }
        let (area, image, picked) = if node.group() {
            let (w, h) = canvas.size();
            let mut image = RgbaImage::new(w, h);
            let picked = self.list(&node.children, canvas, &mut image)?;
            mask_image(self.source, self.header, node, canvas, &mut image)?;
            (canvas, image, picked)
        } else if node.raster(self.header) {
            let area = canvas.intersect(node.rect);
            let (w, h) = area.size();
            if w == 0 || h == 0 {
                return Ok(None);
            }
            let image = raster(self.source, self.header, node, area)?;
            let picked = self.picking.then(|| node.id.clone());
            (area, image, picked)
        } else {
            return Ok(None);
        };
        let picked = picked.filter(|_| {
            f64::from(image.get_pixel(0, 0)[3]) * f64::from(node.opacity) * f64::from(node.fill)
                / (255. * 255.)
                > 2.
        });
        Ok(Some((area, image, picked)))
    }

    fn list(
        &mut self,
        mut nodes: &[Node],
        canvas: Rect,
        dst: &mut RgbaImage,
    ) -> Result<Option<String>> {
        let mut hit = None;
        while let Some((base, rest)) = nodes.split_first() {
            let clipped_count = rest.iter().take_while(|n| n.clipping).count();
            nodes = &rest[clipped_count..];
            if base.clipping {
                continue;
            }
            if clipped_count == 0 && base.pass_through() && base.shown(self.visibility) {
                hit = self.list(&base.children, canvas, dst)?.or(hit);
                continue;
            }
            let Some((area, mut layer, mut picked)) = self.layer(base, canvas)? else {
                continue;
            };
            // Clip to the immediate base, not the accumulated backdrop.
            for node in &rest[..clipped_count] {
                if let Some((region, image, selected)) = self.layer(node, area)? {
                    if !self.picking || layer.get_pixel(0, 0)[3] > 2 {
                        picked = selected.or(picked);
                    }
                    blend_at(&mut layer, area, &image, region, node, true)?;
                }
            }
            blend_at(dst, canvas, &layer, area, base, false)?;
            hit = picked.or(hit);
        }
        Ok(hit)
    }
}

pub fn thumbnail(assets: &Assets, item: &Item, id: &str, size: Option<f64>) -> Result<Value> {
    let mut doc = Document::read(item)?;
    let Some(node) = visit(&doc.nodes).find(|node| node.id == id && node.raster(&doc.header))
    else {
        return Ok(Value::Null);
    };
    let image = raster(&mut doc.source, &doc.header, node, node.rect)?;
    let size = size
        .filter(|n| n.is_finite() && *n != 0.)
        .unwrap_or(58.)
        .round()
        .clamp(24., 160.) as u32;
    let resized = if image.width().max(image.height()) == size {
        image
    } else {
        image::DynamicImage::ImageRgba8(image)
            .resize(size, size, FilterType::Triangle)
            .into_rgba8()
    };
    let mut thumb = RgbaImage::new(size, size);
    image::imageops::replace(
        &mut thumb,
        &resized,
        i64::from((size - resized.width()) / 2),
        i64::from((size - resized.height()) / 2),
    );
    Ok(json!(assets.bytes(picture::encode(&thumb)?, "image/png")?))
}

pub fn render(assets: &Assets, item: &Item, visibility: &Value, options: &Value) -> Result<Value> {
    let mut doc = Document::read(item)?;
    let mut changed = visit(&doc.nodes)
        .filter(|node| node.shown(visibility) != node.visible)
        .peekable();
    if changed.peek().is_none()
        && options.get("preserveOriginal").and_then(Value::as_bool) != Some(false)
    {
        return picture::load(assets, item, "full");
    }
    if !doc.supported() || changed.any(|node| !node.toggleable(&doc.header)) {
        return Err(UNAVAILABLE.into());
    }
    let canvas = doc.bounds();
    let mut image = RgbaImage::new(doc.header.width, doc.header.height);
    Render {
        source: &mut doc.source,
        header: &doc.header,
        visibility,
        picking: false,
    }
    .list(&doc.nodes, canvas, &mut image)?;
    if options.get("preserveOriginal").and_then(Value::as_bool) != Some(false) {
        let mut baseline = RgbaImage::new(doc.header.width, doc.header.height);
        Render {
            source: &mut doc.source,
            header: &doc.header,
            visibility: &Value::Null,
            picking: false,
        }.list(&doc.nodes, canvas, &mut baseline)?;
        let mut original = match item {
            Item::ArchiveEntry { .. } => merged(&item.read(LIMIT)?)?,
            _ => merged_file(item.disk_path(), FILE_LIMIT, LIMIT)?,
        };
        for ((original, before), after) in original.pixels_mut().zip(baseline.pixels()).zip(image.pixels()) {
            if before != after { *original = *after; }
        }
        image = original;
    }
    picture::rendered(
        assets,
        item,
        image,
        doc.description(),
        "PSD/PSB 원본 보존 레이어 근사 합성",
    )
}

pub fn prepare(item: &Item) -> Result<Value> {
    let doc = Document::read(item)?;
    if !doc.supported() {
        return Err(UNAVAILABLE.into());
    }
    Ok(json!(true))
}

pub fn pick(item: &Item, x: f64, y: f64, visibility: &Value) -> Result<Value> {
    let mut doc = Document::read(item)?;
    if !x.is_finite()
        || !y.is_finite()
        || x < 0.
        || y < 0.
        || x >= f64::from(doc.header.width)
        || y >= f64::from(doc.header.height)
        || !doc.supported()
    {
        return Ok(Value::Null);
    }
    let (x, y) = (x as i32, y as i32);
    let area = Rect {
        top: y,
        left: x,
        bottom: y + 1,
        right: x + 1,
    };
    let hit = Render {
        source: &mut doc.source,
        header: &doc.header,
        visibility,
        picking: true,
    }
    .list(&doc.nodes, area, &mut RgbaImage::new(1, 1))?;
    Ok(json!(hit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn length(out: &mut Vec<u8>, len: usize, wide: bool) {
        if wide {
            out.extend((len as u64).to_be_bytes());
        } else {
            out.extend((len as u32).to_be_bytes());
        }
    }

    fn block(out: &mut Vec<u8>, data: &[u8], wide: bool) {
        length(out, data.len(), wide);
        out.extend(data);
    }

    fn tag(out: &mut Vec<u8>, key: &[u8; 4], data: &[u8], wide: bool) {
        out.extend(if wide { b"8B64" } else { b"8BIM" });
        out.extend(key);
        block(out, data, wide);
        if data.len() % 2 != 0 {
            out.push(0);
        }
    }

    fn record_bytes(
        bounds: [i32; 4],
        flags: [u8; 4],
        key: &[u8; 4],
        channels: &[(i16, Vec<u8>)],
        tags: &[u8],
        mask: &[u8],
        wide: bool,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(bounds.into_iter().flat_map(i32::to_be_bytes));
        out.extend((channels.len() as u16).to_be_bytes());
        for (id, data) in channels {
            out.extend(id.to_be_bytes());
            length(&mut out, data.len(), wide);
        }
        out.extend(b"8BIM");
        out.extend(key);
        out.extend(flags);
        let mut extra = Vec::new();
        block(&mut extra, mask, false);
        block(&mut extra, &[], false);
        extra.extend([3, b'P', b'S', b'D']);
        extra.extend(tags);
        block(&mut out, &extra, false);
        out
    }

    fn raw(value: u8) -> Vec<u8> {
        vec![0, 0, value]
    }

    fn fixture(wide: bool, malformed: bool) -> Vec<u8> {
        let base = [
            (0, vec![0, 0, 255, 255]),
            (1, vec![0, 0, 0, 0]),
            (2, vec![0, 0, 0, 0]),
            (
                -2,
                if wide {
                    vec![0, 1, 0, 0, 0, 2, 0, 128]
                } else {
                    raw(128)
                },
            ),
        ];
        let clip = [
            (0, vec![0, 0, 0, 0]),
            (1, vec![0, 0, 255, 255]),
            (2, vec![0, 0, 0, 0]),
        ];
        let mut mask: Vec<_> = [0i32, 0, 1, 1]
            .into_iter()
            .flat_map(i32::to_be_bytes)
            .collect();
        mask.extend([0, 0, 0, 0]);
        let mut nodes = 5i16.to_be_bytes().to_vec();
        let mut tags = Vec::new();
        tag(&mut tags, b"lsct", &3u32.to_be_bytes(), false);
        nodes.extend(record_bytes(
            [0; 4],
            [255, 0, 0, 0],
            b"norm",
            &[],
            &tags,
            &[],
            wide,
        ));
        tags.clear();
        let mut name = 2u32.to_be_bytes().to_vec();
        name.extend("기본".encode_utf16().flat_map(u16::to_be_bytes));
        tag(&mut tags, b"luni", &name, false);
        nodes.extend(record_bytes(
            [0, 0, 1, 2],
            [255, 0, 0, 0],
            b"norm",
            &base,
            &tags,
            &mask,
            wide,
        ));
        nodes.extend(record_bytes(
            [0, 0, 1, 2],
            [255, 1, 0, 0],
            b"norm",
            &clip,
            &[],
            &[],
            wide,
        ));
        tags.clear();
        tag(&mut tags, b"lsct", b"\0\0\0\x02\x38BIMnorm", false);
        nodes.extend(record_bytes(
            [0; 4],
            [255, 0, 0, 0],
            b"norm",
            &[],
            &tags,
            &[],
            wide,
        ));
        tags.clear();
        tag(&mut tags, b"curv", &[], false);
        tag(&mut tags, b"lfx2", &[], false);
        nodes.extend(record_bytes(
            [0; 4],
            [255, 0, 2, 0],
            b"norm",
            &[],
            &tags,
            &[],
            wide,
        ));
        if !malformed {
            for (_, channel) in base.into_iter().chain(clip) {
                nodes.extend(channel);
            }
        }
        if nodes.len() % 2 != 0 {
            nodes.push(0);
        }
        let mut layer = Vec::new();
        block(&mut layer, &nodes, wide);
        block(&mut layer, &[], false);
        let mut file = b"8BPS".to_vec();
        file.extend([0, if wide { 2 } else { 1 }, 0, 0, 0, 0, 0, 0, 0, 3]);
        file.extend(1u32.to_be_bytes());
        file.extend(2u32.to_be_bytes());
        file.extend([0, 8, 0, 3]);
        block(&mut file, &[], false);
        block(&mut file, &[], false);
        block(&mut file, &layer, wide);
        file.extend([0, 0, 14, 15, 24, 25, 34, 35]);
        file
    }

    fn file(data: &[u8], wide: bool) -> (tempfile::NamedTempFile, Item) {
        let mut file = tempfile::Builder::new()
            .suffix(if wide { ".psb" } else { ".psd" })
            .tempfile()
            .unwrap();
        file.write_all(data).unwrap();
        let item = Item::File {
            path: file.path().to_owned(),
            crop: None,
        };
        (file, item)
    }

    fn image(assets: &Assets, url: &Value) -> RgbaImage {
        let png = assets.encoded_image(url.as_str().unwrap()).unwrap();
        image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8()
    }

    #[test]
    fn psd_and_psb_layer_contract_preserves_saved_composite_and_renders_edits() {
        for wide in [false, true] {
            let bytes = fixture(wide, false);
            let (_file, item) = file(&bytes, wide);
            let assets = Assets::default();
            let null = Value::Null;
            let original = picture::load(&assets, &item, "full").unwrap();
            let doc = &original["layerDocument"];
            assert_eq!(doc["format"], if wide { "PSB" } else { "PSD" });
            assert_eq!(doc["layers"][0]["id"], "1");
            assert_eq!(doc["layers"][0]["toggleAvailable"], false);
            assert_eq!(doc["layers"][0]["effects"].as_array().unwrap().len(), 2);
            assert_eq!(doc["layers"][1]["children"][0]["id"], "0.1");
            assert_eq!(doc["layers"][1]["children"][1]["name"], "기본");
            assert_eq!(prepare(&item).unwrap(), true);
            assert_eq!(
                image(&assets, &original["dataUrl"]),
                merged(&bytes).unwrap()
            );
            assert_eq!(
                image(
                    &assets,
                    &render(&assets, &item, &json!({"0": true}), &null).unwrap()["dataUrl"]
                ),
                merged(&bytes).unwrap()
            );
            let edited = render(&assets, &item, &json!({"0.1": false}), &null).unwrap();
            assert_eq!(
                image(&assets, &edited["dataUrl"]).as_raw(),
                &[255, 0, 0, 128, 15, 25, 35, 255]
            );
            let approx =
                render(&assets, &item, &null, &json!({"preserveOriginal": false})).unwrap();
            assert_eq!(
                image(&assets, &approx["dataUrl"]).as_raw(),
                &[0, 255, 0, 128, 0, 0, 0, 0]
            );
            let hidden = render(&assets, &item, &json!({"0": false}), &null).unwrap();
            assert_eq!(image(&assets, &hidden["dataUrl"]).as_raw(), &[0, 0, 0, 0, 15, 25, 35, 255]);
            assert!(render(&assets, &item, &json!({"1": true}), &null).is_err());
            assert_eq!(pick(&item, 0.0, 0.0, &null).unwrap(), "0.1");
            assert_eq!(
                pick(&item, 0.0, 0.0, &json!({"0.1": false})).unwrap(),
                "0.0"
            );
            assert_eq!(pick(&item, 0.0, 0.0, &json!({"0.0": false})).unwrap(), null);
            assert_eq!(pick(&item, 1.0, 0.0, &null).unwrap(), null);
            assert_eq!(pick(&item, f64::NAN, 0.0, &null).unwrap(), null);
            let url = thumbnail(&assets, &item, "0.0", None).unwrap();
            let thumb = image(&assets, &url);
            assert_eq!(thumb.dimensions(), (58, 58));
            assert_eq!(thumb.get_pixel(0, 29).0, [255, 0, 0, 128]);
            assert_eq!(thumb.get_pixel(57, 29)[3], 0);
            assert_eq!(thumb.get_pixel(0, 0)[3], 0);
            assert_eq!(thumbnail(&assets, &item, "0", None).unwrap(), null);
        }
    }

    #[test]
    fn malformed_layer_channel_length_cannot_consume_saved_composite() {
        let bytes = fixture(false, true);
        let (_file, item) = file(&bytes, false);
        let assets = Assets::default();
        let loaded = picture::load(&assets, &item, "full").unwrap();
        assert_eq!(loaded["layerDocument"]["toggleSupported"], false);
        assert_eq!(image(&assets, &loaded["dataUrl"]), merged(&bytes).unwrap());
        assert!(prepare(&item).is_err());
    }

    #[test]
    fn unsupported_child_disables_group_but_not_other_raster_layers() {
        let (_file, item) = file(&fixture(false, false), false);
        let mut doc = Document::read(&item).unwrap();
        let group = doc.nodes.iter_mut().find(|node| node.group()).unwrap();
        assert!(group.toggleable(&doc.header));
        group.children[0].adjustment = true;
        assert!(!group.toggleable(&doc.header));
        assert!(doc.supported());
        let description = doc.description();
        let group = &description["layers"][1];
        assert_eq!(group["toggleAvailable"], false);
        assert!(group["children"].as_array().unwrap().iter().any(|node| node["toggleAvailable"] == true));
    }

    #[test]
    fn psb_nested_16_bit_layer_decodes_zip_prediction() {
        use flate2::{Compression, write::ZlibEncoder};

        fn channel(values: [u16; 2]) -> Vec<u8> {
            let mut zlib = ZlibEncoder::new(vec![0, 3], Compression::fast());
            zlib.write_all(&values[0].to_be_bytes()).unwrap();
            zlib.write_all(&values[1].wrapping_sub(values[0]).to_be_bytes())
                .unwrap();
            zlib.finish().unwrap()
        }
        let channels = [
            (0, channel([0x11ff, 0xe1ff])),
            (1, channel([0x2222, 0x7777])),
            (2, channel([0xeeee, 0x3333])),
            (-1, channel([0xffff, 0x8888])),
        ];
        let mut info = 1i16.to_be_bytes().to_vec();
        info.extend(record_bytes(
            [0, 0, 1, 2],
            [255, 0, 0, 0],
            b"norm",
            &channels,
            &[],
            &[],
            true,
        ));
        for (_, data) in channels {
            info.extend(data);
        }
        let mut layers = Vec::new();
        block(&mut layers, &[], true);
        block(&mut layers, &[], false);
        tag(&mut layers, b"Lr16", &info, true);
        let mut bytes = fixture(true, false);
        bytes[22..24].copy_from_slice(&16u16.to_be_bytes());
        let old_len = u64::from_be_bytes(bytes[34..42].try_into().unwrap()) as usize;
        let mut replacement = Vec::new();
        block(&mut replacement, &layers, true);
        replacement.extend([0; 14]);
        bytes.splice(34..42 + old_len + 8, replacement);
        let (_file, item) = file(&bytes, true);
        let assets = Assets::default();
        let thumb = thumbnail(&assets, &item, "0", None).unwrap();
        let thumb = image(&assets, &thumb);
        assert_eq!(thumb.get_pixel(0, 29).0, [18, 34, 238, 255]);
        assert_eq!(thumb.get_pixel(57, 29).0, [225, 119, 51, 136]);
    }

    #[test]
    #[ignore = "manual interoperability check; requires installed Electron ag-psd dependency"]
    fn measure_ag_psd_layer_interop() {
        use std::{process::Command, time::Instant};
        let generated = Command::new("node")
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .args([
                "-e",
                r#"
                const { writePsdBuffer } = require('ag-psd');
                const width = 1024, height = 768;
                function pixels(width, height, color) {
                    const data = new Uint8ClampedArray(width * height * 4);
                    for (let i = 0; i < data.length; i += 4) data.set(color, i);
                    return { width, height, data };
                }
                const bg = pixels(width, height, [60, 100, 180, 255]);
                const saved = pixels(width, height, [61, 101, 181, 255]);
                const fg = pixels(512, 384, [200, 50, 30, 255]);
                process.stdout.write(writePsdBuffer({
                    width, height, imageData: saved,
                    children: [{ name: 'Background', imageData: bg }, {
                        name: '그룹', children: [{
                            name: '채색', left: 16, top: 12, opacity: 0.5, imageData: fg
                        }]
                    }]
                }, { generateThumbnail: false }));
            "#,
            ])
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let (_file, item) = file(&generated.stdout, false);
        let mut samples = Vec::new();
        for _ in 0..5 {
            let assets = Assets::default();
            let start = Instant::now();
            let original = load(&assets, &item).unwrap();
            let load_time = start.elapsed();
            let start = Instant::now();
            let edited = render(&assets, &item, &json!({"1": false}), &Value::Null).unwrap();
            let edit_time = start.elapsed();
            let start = Instant::now();
            let picked = pick(&item, 17., 13., &Value::Null).unwrap();
            let pick_time = start.elapsed();
            assert_eq!(
                original["layerDocument"]["layers"][0]["children"][0]["name"],
                "채색"
            );
            assert_eq!(
                image(&assets, &original["dataUrl"]).get_pixel(17, 13).0,
                [61, 101, 181, 255]
            );
            assert_eq!(
                image(&assets, &edited["dataUrl"]).get_pixel(17, 13).0,
                [60, 100, 180, 255]
            );
            assert_eq!(picked, "1.0");
            samples.push([load_time, edit_time, pick_time].map(|time| time.as_secs_f64() * 1000.));
        }
        let medians = [0, 1, 2].map(|column| {
            let mut durations: Vec<_> = samples.iter().map(|sample| sample[column]).collect();
            durations.sort_by(f64::total_cmp);
            durations[2]
        });
        println!(
            "ag-psd 1024x768 ({} bytes), median load/edit/pick (ms): {:.2?}",
            generated.stdout.len(),
            medians
        );
    }
}
