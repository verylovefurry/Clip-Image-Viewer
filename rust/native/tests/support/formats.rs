//! Opt-in checks with local format samples kept outside the repository.
//! `CLIPVIEW_FORMAT_SAMPLES` points to a folder; every supported file in it is opened through
//! the same `load` path as the app. Where Windows codecs can also decode a file, the two
//! results are compared, so Rust decoders are checked against an independent implementation.

use std::path::PathBuf;

use super::*;

fn rmse(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let sum: f64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2)).sum();
    (sum / a.as_raw().len() as f64).sqrt()
}

#[test]
#[ignore = "requires CLIPVIEW_FORMAT_SAMPLES"]
fn opens_local_format_samples() {
    let dir = PathBuf::from(std::env::var_os("CLIPVIEW_FORMAT_SAMPLES").expect("CLIPVIEW_FORMAT_SAMPLES"));
    let mut paths: Vec<_> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_file() && !p.to_string_lossy().ends_with(".ref.png")).collect();
    paths.sort();
    let mut failures = Vec::new();
    for path in paths {
        let item = Item::File { path: path.clone(), crop: None };
        let assets = Assets::default();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match load(&assets, &item, "full") {
            Ok(result) => {
                let bytes = assets.encoded_image(result["dataUrl"].as_str().unwrap()).unwrap();
                let ours = decode(&bytes, None).unwrap().into_rgba8();
                let mut line = format!("format {name}: {}x{} {} pages={}", ours.width(), ours.height(),
                    result["metadata"]["source"], result["metadata"]["pages"]);
                #[cfg(windows)]
                if let Ok(windows) = crate::wic::decode(&path) {
                    if windows.dimensions() == ours.dimensions() {
                        line += &format!(" | WIC RMSE {:.2}", rmse(&ours, &windows));
                    } else {
                        line += &format!(" | WIC size {:?}", windows.dimensions());
                    }
                }
                // `<file>.ref.png` holds an independent decoder's output (for example Pillow).
                let reference = PathBuf::from(format!("{}.ref.png", path.display()));
                if let Ok(bytes) = fs::read(&reference) {
                    let expected = decode(&bytes, None).unwrap().into_rgba8();
                    let error = rmse(&ours, &expected);
                    line += &format!(" | reference RMSE {error:.2}");
                    if error > 0. { failures.push(format!("{name} differs from reference")); }
                }
                println!("{line}");
            }
            Err(error) => { println!("format {name}: ERROR {error}"); failures.push(name); }
        }
    }
    assert!(failures.is_empty(), "failed: {failures:?}");
}
