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
        assert_eq!(original, decode_url(&assets, &render(&assets, &item, &Value::Null).unwrap()), "{name}");
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
                    let hidden = decode_url(&assets, &render(&assets, &item, &overrides).unwrap());
                    assert_eq!(hidden.dimensions(), original.dimensions(), "{name}: {kind}");
                    let restored = json!({node.id.to_string(): true});
                    assert_eq!(decode_url(&assets, &render(&assets, &item, &restored).unwrap()), original, "{name}: {kind}");
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
