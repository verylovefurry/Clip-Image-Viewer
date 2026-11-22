use super::*;
use std::path::PathBuf;

fn visible_leaves(nodes: &[Node]) -> Box<dyn Iterator<Item = &Node> + '_> {
    Box::new(
        nodes
            .iter()
            .filter(|node| node.visible && node.opacity > 0.)
            .flat_map(|node| {
                if node.folder {
                    visible_leaves(&node.child)
                } else {
                    Box::new(std::iter::once(node))
                }
            }),
    )
}

#[test]
fn offscreen_chunk_ids_match_by_bytes_not_sqlite_storage_class() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "
        CREATE TABLE Offscreen (MainId, LayerId, Attribute, BlockData);
        CREATE TABLE ExternalChunk (ExternalID, Offset);
        INSERT INTO ExternalChunk VALUES ('ext-a', 100), (x'6578742D62', 200), ('EXT-C', 300);
    ",
    )
    .unwrap();
    let mut attribute = [0; 160];
    for (offset, value) in [
        (0, 16u32),
        (4, 102),
        (8, 42),
        (16, 9),
        (38, 256),
        (42, 256),
        (46, 1),
        (50, 1),
        (58, 1),
        (62, 4),
        (118, 9),
    ] {
        attribute[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    attribute[20..38].copy_from_slice(&tile::utf16_marker("Parameter"));
    attribute[122..140].copy_from_slice(&tile::utf16_marker("InitColor"));
    db.execute(
        "INSERT INTO Offscreen VALUES
        (1, 10, ?1, x'6578742D61'), (2, 10, ?1, x'6578742D62'), (3, 10, ?1, x'6578742D63')",
        [&attribute[..]],
    )
    .unwrap();

    let store = load_storage(&db).unwrap();
    assert_eq!(store.levels.len(), 2);
    assert_eq!(store.levels[&1].offset, 100);
    assert_eq!(store.levels[&2].offset, 200);
}

#[test]
#[ignore = "requires private local CLIP samples"]
fn separates_real_text_and_raster_layers_without_writing_sources() {
    let samples = std::env::var_os("CLIPVIEW_CLIP_SAMPLE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/samples/RecWild1_KR")
        });
    for name in ["page0006.clip", "page0007.clip"] {
        let path = samples.join(name);
        let before = std::fs::read(&path).unwrap();
        let item = Item::File {
            path: path.clone(),
            crop: None,
        };
        let mut doc = Document::read(&item).unwrap();
        assert_eq!(doc.description()["toggleSupported"], true, "{name}");
        prepare(&item).unwrap();
        let decode_url = |assets: &Assets, result: &Value| {
            let bytes = assets.encoded_image(result["dataUrl"].as_str().unwrap()).unwrap();
            picture::decode(&bytes, None).unwrap().into_rgba8()
        };
        // The initial display is the independent composite, not CSP's merged preview.
        let assets = Assets::default();
        let opened = picture::load(&assets, &item, "full").unwrap();
        assert_eq!(opened["metadata"]["source"], "CLIP 레이어 합성", "{name}");
        assert_eq!(opened["layerDocument"]["independentRendering"], true, "{name}");
        let original = decode_url(&assets, &opened);
        assert_eq!(original, decode_url(&assets, &render(&assets, &item, &Value::Null, "full").unwrap()), "{name}");
        let brushes = std::rc::Rc::clone(&doc.brushes);
        let frame = doc.frame(&brushes);
        let mut ids = Vec::new();
        for kind in ["text", "raster"] {
            let changed = visible_leaves(&doc.nodes)
                .filter(|node| node.kind == kind && node.toggleable())
                .find_map(|node| {
                    let isolated = leaf(&mut *doc.source, node, frame)
                        .unwrap()
                        .unwrap();
                    if !isolated.pixels().any(|pixel| pixel[3] > 8) {
                        return None;
                    }
                    let overrides = json!({node.id.to_string(): false});
                    let hidden = decode_url(&assets, &render(&assets, &item, &overrides, "full").unwrap());
                    assert_eq!(hidden.dimensions(), original.dimensions(), "{name}: {kind}");
                    let restored = json!({node.id.to_string(): true});
                    assert_eq!(decode_url(&assets, &render(&assets, &item, &restored, "full").unwrap()), original, "{name}: {kind}");
                    // A layer can have its own pixels and still be covered by another layer.
                    (hidden != original).then_some(node.id)
                });
            ids.push(
                changed.unwrap_or_else(|| {
                    panic!("{name}: no independent {kind} changed the composite")
                }),
            );
        }
        assert_ne!(ids[0], ids[1]);
        assert!(
            std::fs::read(&path).unwrap() == before,
            "{name}: source bytes changed"
        );
    }
}

/// Survey of many local CLIP files listed (one path per line) in `CLIPVIEW_CLIP_LIST`:
/// sizes, rendering coverage, why layers or folders cannot be toggled, and the error of the
/// independent composite against CSP's stored preview. Prints one line per file and totals.
#[test]
#[ignore = "requires CLIPVIEW_CLIP_LIST"]
fn survey_clip_list() {
    let list = std::fs::read_to_string(std::env::var_os("CLIPVIEW_CLIP_LIST").unwrap()).unwrap();
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    let (mut total, mut total_blur, mut count) = (0f64, 0f64, 0f64);
    fn why(node: &Node) -> Option<String> {
        if node.toggleable() { return None; }
        let kind = node.detail["kind"].as_str().unwrap_or("?");
        Some(if node.folder {
            if node.can_render { format!("folder-child:{kind}") } else { format!("folder-blocked:{kind}:maskflag={}:visible={}:levels={}:vectors={}", node.detail["mask"], node.mask_visible, node.mask.len(), node.vectors.len()) }
        } else if !node.has_source() {
            format!("nosource:{kind}")
        } else {
            let text = node.detail["textObjectCount"].as_u64().unwrap_or(0);
            let cached = node.detail["textRasterCount"].as_u64().unwrap_or(0);
            format!("blocked:{kind}{}{}", if text > cached { ":text-cache-missing" } else { "" },
                if node.mask_visible && node.mask.is_empty() && node.detail["mask"] == true { ":mask-missing" } else { "" })
        })
    }
    for path in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let item = Item::File { path: PathBuf::from(path), crop: None };
        let start = std::time::Instant::now();
        let Ok(mut doc) = Document::read(&item) else { println!("survey ERROR read {path}"); continue };
        let mut file_reasons: std::collections::BTreeMap<String, usize> = Default::default();
        for node in visit(&doc.nodes) {
            if let Some(reason) = why(node) { *file_reasons.entry(reason).or_default() += 1; }
        }
        let missing = doc.describe(None)["unrenderedLayers"].as_u64().unwrap_or(0);
        let brushes = std::rc::Rc::clone(&doc.brushes);
        let frame = doc.frame(&brushes);
        let rmse = if doc.layered() {
            let hide = std::env::var("CLIPVIEW_HIDE_KIND").unwrap_or_default();
            let overrides: serde_json::Map<String, Value> = visit(&doc.nodes)
                .filter(|n| !hide.is_empty() && hide.split('|').any(|k| n.detail["kind"] == k))
                .map(|n| (n.id.to_string(), Value::Bool(false))).collect();
            if !hide.is_empty() && overrides.is_empty() { println!("survey skip {path}"); continue; }
            match compose(&mut *doc.source, &doc.nodes, frame, &Value::Object(overrides)) {
                Ok(image) => {
                    let preview = picture::decode(&doc.preview, None).unwrap().into_rgba8();
                    // Compare at the preview's size; the composite is now at canvas resolution.
                    let image = image::imageops::resize(&image, preview.width(), preview.height(), image::imageops::FilterType::Triangle);
                    let (mut sum, mut n) = (0f64, 0f64);
                    for (a, b) in image.pixels().zip(preview.pixels()) {
                        for c in 0..3 {
                            let over = |p: &image::Rgba<u8>| f64::from(p[c]) * f64::from(p[3]) / 255. + 255. * (1. - f64::from(p[3]) / 255.);
                            sum += (over(a) - over(b)).powi(2);
                            n += 1.;
                        }
                    }
                    let e = (sum / n).sqrt();
                    // Halftone dots differ in phase even when the tone matches; blur before comparing.
                    let flat = |img: &RgbaImage| {
                        let mut out = RgbaImage::from_pixel(img.width(), img.height(), image::Rgba([255; 4]));
                        image::imageops::overlay(&mut out, img, 0, 0);
                        image::imageops::blur(&out, 3.)
                    };
                    let (fa, fb) = (flat(&image), flat(&preview));
                    let (mut sum2, mut n2) = (0f64, 0f64);
                    for (a, b) in fa.pixels().zip(fb.pixels()) {
                        for c in 0..3 { sum2 += (f64::from(a[c]) - f64::from(b[c])).powi(2); n2 += 1.; }
                    }
                    let blurred = (sum2 / n2).sqrt();
                    total += e;
                    total_blur += blurred;
                    count += 1.;
                    format!("{e:.1} blur {blurred:.1}")
                }
                Err(e) => format!("ERR {e}"),
            }
        } else { "no-layers".into() };
        let name: String = path.chars().rev().take(48).collect::<Vec<_>>().into_iter().rev().collect();
        println!("survey {name} | prev {}x{} native {}x{} | {}ms | unrendered {missing} | rmse {rmse} | {:?}",
            doc.size.0, doc.size.1, doc.native.0, doc.native.1, start.elapsed().as_millis(), file_reasons);
        for (k, v) in file_reasons { *reasons.entry(k).or_default() += v; }
    }
    println!("survey mean rmse {:.2} blurred {:.2} over {count} files", total / count.max(1.), total_blur / count.max(1.));
    for (k, v) in reasons { println!("survey reason {v:5} {k}"); }
}

/// Finds layers drawn wrongly: hides each visible layer in turn and reports those whose removal
/// brings the composite closest to CSP's stored preview. Files come from `CLIPVIEW_CLIP_LIST`;
/// composites are made at the preview size to keep the run short.
#[test]
#[ignore = "requires CLIPVIEW_CLIP_LIST"]
fn ablate_clip_layers() {
    let list = std::fs::read_to_string(std::env::var_os("CLIPVIEW_CLIP_LIST").unwrap()).unwrap();
    fn error(image: &RgbaImage, preview: &RgbaImage) -> f64 {
        let (mut sum, mut n) = (0f64, 0f64);
        for (a, b) in image.pixels().zip(preview.pixels()) {
            for c in 0..3 {
                let over = |p: &image::Rgba<u8>| f64::from(p[c]) * f64::from(p[3]) / 255. + 255. * (1. - f64::from(p[3]) / 255.);
                sum += (over(a) - over(b)).powi(2);
                n += 1.;
            }
        }
        (sum / n).sqrt()
    }
    fn walk<'a>(nodes: &'a [Node], depth: usize, out: &mut Vec<(&'a Node, usize)>) {
        for n in nodes.iter().filter(|n| n.visible) { out.push((n, depth)); if depth < 3 { walk(&n.child, depth + 1, out); } }
    }
    let mut kinds: std::collections::BTreeMap<String, (usize, f64)> = Default::default();
    for path in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let item = Item::File { path: PathBuf::from(path), crop: None };
        let Ok(mut doc) = Document::read(&item) else { continue };
        if !doc.layered() { continue; }
        doc.size = doc.preview_size;
        let preview = picture::decode(&doc.preview, None).unwrap().into_rgba8();
        let brushes = std::rc::Rc::clone(&doc.brushes);
        let frame = doc.frame(&brushes);
        let base = error(&compose(&mut *doc.source, &doc.nodes, frame, &Value::Null).unwrap(), &preview);
        let mut targets = Vec::new();
        walk(&doc.nodes, 0, &mut targets);
        let mut gains = Vec::new();
        for (node, depth) in targets.iter().take(120) {
            let overrides = json!({ node.id.to_string(): false });
            let Ok(image) = compose(&mut *doc.source, &doc.nodes, frame, &overrides) else { continue };
            gains.push((base - error(&image, &preview), node, *depth));
        }
        gains.sort_by(|a, b| b.0.total_cmp(&a.0));
        let name: String = path.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
        println!("ablate {name} base {base:.1}");
        for (gain, node, depth) in gains.iter().take(4).filter(|g| g.0 > 0.5) {
            let kind = format!("{}{}{}", node.detail["kind"].as_str().unwrap_or("?"), if node.blend != 0 { format!(":blend{}", node.blend) } else { String::new() },
                if node.clipping { ":clip" } else { "" });
            println!("ablate   -{gain:.1} d{depth} [{kind}] {} op={:.2} raster={} vec={} tone={} edge={} mask={}/{}",
                node.name.chars().take(18).collect::<String>(), node.opacity, node.raster.len(), node.vectors.len(), node.tone.is_some(), node.edge.is_some(), node.mask_visible, node.mask.len());
            let entry = kinds.entry(kind).or_default();
            entry.0 += 1;
            entry.1 += gain;
        }
    }
    for (kind, (count, gain)) in kinds { println!("ablate kind {count:4} gain {gain:7.1} {kind}"); }
}

/// Open-time breakdown (read, compose, PNG encode) for files listed in `CLIPVIEW_CLIP_LIST`,
/// following the viewer's `picture::load` path.
#[test]
#[ignore = "requires CLIPVIEW_CLIP_LIST"]
fn bench_clip_open() {
    let list = std::fs::read_to_string(std::env::var_os("CLIPVIEW_CLIP_LIST").unwrap()).unwrap();
    let (mut sums, mut all) = ([0u128; 3], Vec::new());
    for path in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let item = Item::File { path: PathBuf::from(path), crop: None };
        let t0 = std::time::Instant::now();
        let Ok(mut doc) = Document::read(&item) else { continue };
        let t1 = std::time::Instant::now();
        let (display, _) = doc.display();
        let t2 = std::time::Instant::now();
        let mut digest = 0u64;
        let bytes = match display {
            Display::Layers(image) => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                image.as_raw().hash(&mut hasher);
                digest = hasher.finish();
                picture::encode(&image).unwrap().len()
            }
            Display::Preview => 0,
        };
        let t3 = std::time::Instant::now();
        let phases = [t1 - t0, t2 - t1, t3 - t2].map(|d| d.as_millis());
        for (s, p) in sums.iter_mut().zip(phases) { *s += p; }
        all.push(phases.iter().sum::<u128>());
        let name: String = path.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
        println!("bench {name} | {}x{} | read {} compose {} encode {} ms | png {} KB | hash {digest:016x}", doc.size.0, doc.size.1, phases[0], phases[1], phases[2], bytes / 1024);
    }
    all.sort_unstable();
    let n = all.len().max(1);
    println!("bench total read {} compose {} encode {} ms | median {} p90 {} max {} ms over {n} files",
        sums[0], sums[1], sums[2], all[n / 2], all[n * 9 / 10], all.last().copied().unwrap_or(0));
}
