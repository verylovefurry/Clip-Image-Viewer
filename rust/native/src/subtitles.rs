use std::{fs, path::Path, sync::LazyLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    Result, error,
    collection::{self, Item, extension, name, read_limited},
};

const MAX_BYTES: u64 = 20 << 20;
const MAX_TRACKS: usize = 32;
static TIMECODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):([0-9]{1,2}):([0-9]{1,2})(?:[,.]([0-9]{1,3}))?$").unwrap());
static ASS_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[^}]*\}").unwrap());

fn timestamp(text: &str) -> Option<String> {
    let capture = TIMECODE.captures(text.trim())?;
    let hour: u32 = capture[1].parse().ok()?;
    let min: u32 = capture[2].parse().ok()?;
    let sec: u32 = capture[3].parse().ok()?;
    let fraction = capture.get(4).map_or("0", |m| m.as_str());
    if min >= 60 || sec >= 60 { return None; }
    Some(format!("{hour:02}:{min:02}:{sec:02}.{fraction:0<3}"))
}

fn srt_to_vtt(content: &str) -> String {
    let mut output = String::from("WEBVTT\n\n");
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.trim().is_empty() && line.trim().bytes().all(|c| c.is_ascii_digit())
            && lines.peek().is_some_and(|l| l.contains("-->"))
        { continue; }
        if let Some((start, end)) = line.split_once("-->")
            && let Some(start) = timestamp(start)
            && let Some(end_time) = end.split_whitespace().next()
            && let Some(end_time) = timestamp(end_time)
        {
            output.push_str(&format!("{start} --> {end_time}"));
            let settings = end.trim_start().split_once(char::is_whitespace).map(|(_, rest)| rest.trim());
            if let Some(settings) = settings.filter(|s| !s.is_empty()) {
                output.push(' ');
                output.push_str(settings);
            }
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    output
}

fn ass_to_vtt(content: &str) -> String {
    let mut output = String::from("WEBVTT\n\n");
    let mut events = false;
    let mut fields = vec![];
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !events { continue; }
        let Some((key, body)) = line.split_once(':') else { continue };
        if key.eq_ignore_ascii_case("format") {
            fields = body.split(',').map(|field| field.trim().to_ascii_lowercase()).collect();
            continue;
        }
        if !key.eq_ignore_ascii_case("dialogue") || fields.is_empty() { continue; }
        let Some(text_index) = fields.iter().position(|name| name == "text") else { continue };
        let mut values: Vec<_> = body.trim_start().splitn(text_index + 1, ',').collect();
        if values.len() != text_index + 1 { continue; }
        let tail = values.pop().unwrap();
        let mut suffix: Vec<_> = tail.rsplitn(fields.len() - text_index, ',').collect();
        if suffix.len() != fields.len() - text_index { continue; }
        suffix.reverse();
        values.extend(suffix);
        let field = |key| fields.iter().position(|name| name == key).and_then(|i| values.get(i)).copied();
        if let (Some(start), Some(end), Some(text)) = (
            field("start").and_then(timestamp), field("end").and_then(timestamp), field("text")
        ) {
            let text = ASS_TAG.replace_all(text, "").replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ");
            if !text.trim().is_empty() {
                output.push_str(&format!("{start} --> {end}\n{}\n\n", text.trim()));
            }
        }
    }
    output
}

fn decode(bytes: &[u8]) -> Result<std::borrow::Cow<'_, str>> {
    let decoded = if bytes.starts_with(&[0xff, 0xfe]) {
        encoding_rs::UTF_16LE.decode_without_bom_handling_and_without_replacement(&bytes[2..])
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        encoding_rs::UTF_16BE.decode_without_bom_handling_and_without_replacement(&bytes[2..])
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        Some(text.into())
    } else {
        encoding_rs::EUC_KR.decode_without_bom_handling_and_without_replacement(bytes)
    };
    decoded.ok_or_else(|| "자막의 인코딩이 올바르지 않습니다. UTF-8, UTF-16 또는 EUC-KR 자막인지 확인하세요.".into())
}

fn convert(path: &Path) -> Result<String> {
    let bytes = read_limited(fs::File::open(path).map_err(error)?, MAX_BYTES)?;
    let text = decode(&bytes)?;
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    Ok(match extension(path).as_str() {
        "srt" => srt_to_vtt(&text),
        "ass" | "ssa" => ass_to_vtt(&text),
        _ if text.starts_with("WEBVTT") => text,
        _ => format!("WEBVTT\n\n{text}"),
    })
}

pub fn find(item: &Item) -> Result<Value> {
    let Item::File { path, .. } = item else { return Ok(json!([])) };
    if collection::media_kind(&extension(path)) != "video" { return Ok(json!([])); }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let folder = path.parent().ok_or("동영상의 폴더를 찾지 못했습니다.")?;
    let mut matches = Vec::new();
    for entry in fs::read_dir(folder).map_err(error)? {
        let entry = entry.map_err(error)?;
        let path = entry.path();
        if !matches!(extension(&path).as_str(), "srt" | "ass" | "ssa" | "vtt")
            || !entry.file_type().map_err(error)?.is_file() { continue; }
        let other = path.file_stem().unwrap_or_default().to_string_lossy();
        let label = if other == stem {
            "자막".into()
        } else if let Some(suffix) = other.strip_prefix(stem.as_ref()).and_then(|s| s.strip_prefix('.')) {
            suffix.to_uppercase()
        } else { continue };
        matches.push((path, label));
    }
    matches.sort_by(|a, b| collection::natural_cmp(&name(&a.0), &name(&b.0)));
    let mut result = Vec::new();
    let mut total = 0;
    for (path, label) in matches.into_iter().take(MAX_TRACKS) {
        let vtt = convert(&path)?;
        total += vtt.len() as u64;
        if total > MAX_BYTES { return Err("자막 전체 크기가 안전 제한을 초과합니다.".into()); }
        let lang = match label.to_lowercase().split(['.', '_', '-']).next().unwrap_or("") {
            "ko" | "kor" | "kr" | "korean" | "한국어" => "ko",
            "ja" | "jpn" | "jp" | "japanese" | "日本語" => "ja",
            "en" | "eng" | "english" => "en",
            _ => "und",
        };
        result.push(json!({"name": name(&path), "label": label, "srclang": lang, "vtt": vtt}));
    }
    Ok(json!(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_timestamps_and_utf8_text() {
        assert_eq!(srt_to_vtt("1\n0:00:02,4 --> 0:00:03,021\n한글"), "WEBVTT\n\n00:00:02.400 --> 00:00:03.021\n한글\n");
    }

    #[test]
    fn ass_dialogue_preserves_text_commas() {
        assert_eq!(ass_to_vtt("[Events]\nFormat: Layer,Start,End,Text\nDialogue: 0,0:00:01.05,0:00:02.00,{\\i1}a,b\\Nc"),
                   "WEBVTT\n\n00:00:01.050 --> 00:00:02.000\na,b\nc\n\n");
    }

    #[test]
    fn srt_accepts_whitespace_and_preserves_cue_settings() {
        assert_eq!(srt_to_vtt(" 1 \n0:00:02,4\t-->  0:00:03,021 line:90%\n한😀"),
                   "WEBVTT\n\n00:00:02.400 --> 00:00:03.021 line:90%\n한😀\n");
        assert!(timestamp("١:00:00.000").is_none());
        assert!(timestamp("0:60:00.000").is_none());
    }

    #[test]
    fn ass_text_can_precede_other_fields() {
        assert_eq!(ass_to_vtt("[Events]\nFormat: Start, Text, End\nDialogue: 0:00:01.00,한,😀,0:00:02.00"),
                   "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n한,😀\n\n");
    }

    #[test]
    fn decodes_supported_encodings_and_rejects_corruption() {
        assert_eq!(decode("한😀".as_bytes()).unwrap(), "한😀");
        let (korean, _, errors) = encoding_rs::EUC_KR.encode("한글");
        assert!(!errors);
        assert_eq!(decode(&korean).unwrap(), "한글");
        for little in [true, false] {
            let mut bytes = if little { vec![0xff, 0xfe] } else { vec![0xfe, 0xff] };
            for unit in "한😀".encode_utf16() {
                bytes.extend(if little { unit.to_le_bytes() } else { unit.to_be_bytes() });
            }
            assert_eq!(decode(&bytes).unwrap(), "한😀");
        }
        assert!(decode(&[0xff, 0xfe, 0x00]).is_err());
        assert!(decode(&[0xfe, 0xff, 0xd8, 0x00]).is_err());
        assert!(decode(&[0x81]).is_err());
    }

    #[test]
    fn finds_matching_subtitles_and_converts_files() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("영상.mp4");
        fs::write(dir.path().join("영상.ko.SRT"), "\u{feff}1\r\n00:00:01,000 --> 00:00:02,000\r\n한글").unwrap();
        fs::write(dir.path().join("영상.en.vtt"), "WEBVTT\r\n\r\n00:00:01.000 --> 00:00:02.000\r\nHello").unwrap();
        fs::write(dir.path().join("영상다른.srt"), "ignored").unwrap();
        let item = Item::File { path: video, crop: None };
        let tracks = find(&item).unwrap();
        let tracks = tracks.as_array().unwrap();
        assert_eq!(tracks.len(), 2);
        let korean = tracks.iter().find(|v| v["srclang"] == "ko").unwrap();
        assert!(korean["vtt"].as_str().unwrap().contains("00:00:01.000 --> 00:00:02.000\n한글"));
        let english = tracks.iter().find(|v| v["srclang"] == "en").unwrap();
        assert!(english["vtt"].as_str().unwrap().starts_with("WEBVTT\n\n"));
        fs::write(dir.path().join("영상.bad.srt"), [0xff, 0xfe, 0]).unwrap();
        assert!(find(&item).is_err());
    }
}
