//! Run with `cargo test --release -p clip-image-viewer-native benchmark_initial_open -- --ignored --nocapture`.
//! Set `CLIPVIEW_OPEN_PSD` to include a local PSD/PSB and `CLIPVIEW_OPEN_RUNS` to change the repeat count.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    time::Instant,
};

use super::*;

fn digest(bytes: &[u8]) -> u64 {
    let mut hash = DefaultHasher::new();
    bytes.hash(&mut hash);
    hash.finish()
}

#[test]
#[ignore = "manual release benchmark; reads local fixtures"]
fn benchmark_initial_open() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut inputs = Vec::new();
    if let Some(path) = std::env::var_os("CLIPVIEW_OPEN_PSD") {
        inputs.push(("psd".into(), PathBuf::from(path), "full"));
    }
    for page in 6..=19 {
        let name = format!("page{page:04}");
        inputs.push((name.clone(), root.join(format!("samples/RecWild1_KR/{name}.clip")), "full"));
    }
    inputs.extend([
        ("legacy".into(), root.join("sample.clip"), "full"),
        ("trim".into(), root.join("samples/RecWild1_KR/page0006.clip"), "trim"),
        ("png".into(), root.join("sample.png"), "full"),
    ]);
    let runs = std::env::var("CLIPVIEW_OPEN_RUNS").map_or(5, |s| s.parse::<usize>().unwrap());
    assert!(runs > 0);
    for (label, path, mode) in inputs {
        let item = Item::File { path, crop: None };
        let mut samples = Vec::new();
        let mut fingerprint = None;
        for run in 0..=runs {
            let assets = Assets::default();
            let start = Instant::now();
            let mut result = load(&assets, &item, mode).unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.;
            let bytes = assets.encoded_image(result["dataUrl"].as_str().unwrap()).unwrap();
            result.as_object_mut().unwrap().remove("dataUrl");
            let pixels = if label == "psd" || label == "trim" {
                digest(decode(&bytes, None).unwrap().into_rgba8().as_raw())
            } else {
                digest(&bytes)
            };
            let next = (pixels, digest(&serde_json::to_vec(&result).unwrap()));
            assert_eq!(*fingerprint.get_or_insert(next), next, "{label}");
            if run > 0 { samples.push(ms); }
            println!("open {label} {run} {ms:.3} {} {:016x} {:016x}", bytes.len(), next.0, next.1);
        }
        samples.sort_by(f64::total_cmp);
        println!("median {label} {:.3}", samples[samples.len() / 2]);
    }
    if let Some(path) = std::env::var_os("CLIPVIEW_OPEN_PSD") {
        let item = Item::File { path: path.into(), crop: None };
        for _ in 0..runs {
            let start = Instant::now();
            let data = item.read(MAX_SOURCE).unwrap();
            let read = start.elapsed();
            let start = Instant::now();
            let image = psd::merged(&data).unwrap();
            let merged = start.elapsed();
            let start = Instant::now();
            let png = encode(&image).unwrap();
            println!("psd breakdown {}x{} {} {:.3} {:.3} {:.3}", image.width(), image.height(),
                png.len(), read.as_secs_f64() * 1000., merged.as_secs_f64() * 1000.,
                start.elapsed().as_secs_f64() * 1000.);
        }
    }
}
