#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
#![recursion_limit = "256"]

mod asset;
mod associations;
mod bpg;
mod clip;
mod collection;
mod jxl;
mod jxl_animation;
mod jp2;
mod libraw;
mod pcx;
mod picture;
mod psd;
mod raw_preview;
mod subtitles;
mod updater;
mod window_state;
#[cfg(windows)]
mod wic;

use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WebviewEvent, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;

use crate::{asset::Assets, collection::{Item, extension, media_kind}, window_state::WindowState};

type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String { e.to_string() }

struct Backend {
    assets: Assets,
    foreground: tokio::sync::Semaphore,
    background: tokio::sync::Semaphore,
    // Serializing allocation-heavy image operations bounds the number of decoded canvases.
    decode: Mutex<()>,
    active: Mutex<Option<String>>,
    image_cache: Mutex<VecDeque<(String, Value)>>,
    startup: Option<PathBuf>,
    window: WindowState,
    smoke: Option<PathBuf>,
    updater: updater::Updater,
}

impl Backend {
    fn new() -> Self {
        let smoke = std::env::var_os("CLIPVIEW_RUST_SMOKE").filter(|value| !value.is_empty()).map(PathBuf::from);
        Self {
            assets: Assets::default(), foreground: tokio::sync::Semaphore::new(1), background: tokio::sync::Semaphore::new(1),
            decode: Mutex::new(()), active: Mutex::new(None), image_cache: Mutex::new(VecDeque::new()), window: WindowState::default(),
            startup: startup_path(std::env::args_os().skip(1).map(PathBuf::from)), smoke,
            updater: updater::Updater::new(env!("CARGO_PKG_VERSION")),
        }
    }
}

fn startup_path(args: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    args.filter(|path| !path.to_string_lossy().starts_with('-'))
        .find_map(|path| collection::display_absolute(&path).ok().filter(|abs| abs.exists()))
}

fn dropped_path(event: &DragDropEvent) -> Option<&PathBuf> {
    match event {
        DragDropEvent::Drop { paths, .. } => paths.first(),
        _ => None,
    }
}

fn forward_drop(window: &WebviewWindow, event: &DragDropEvent) -> Result<()> {
    if let Some(path) = dropped_path(event) {
        window.emit("open-external-path", path).map_err(error)?;
    }
    Ok(())
}

fn forward_window_drop(window: &WebviewWindow, event: &WindowEvent) -> Result<()> {
    if let WindowEvent::DragDrop(drop) = event {
        forward_drop(window, drop)?;
    }
    Ok(())
}

fn argument<T: DeserializeOwned>(args: &[Value], index: usize) -> Result<T> {
    serde_json::from_value(args.get(index).cloned().unwrap_or(Value::Null)).map_err(error)
}

fn item_key(item: &Item) -> String {
    match item {
        Item::ArchiveEntry { archive_path, entry_name } => format!("{}\0{entry_name}", archive_path.display()),
        _ => item.disk_path().to_string_lossy().into_owned(),
    }
}

fn association_list(group: &str) -> Vec<String> {
    let mut extensions: Vec<_> = group.split_whitespace().map(|ext| format!(".{ext}"))
        .filter(|ext| !collection::BASIC.contains(&ext.as_str())).collect();
    extensions.sort();
    extensions
}

fn runtime_info(state: &Backend, version: &str) -> Value {
    let image = association_list(&format!("{} zip cbz cmc", collection::IMAGE_EXTENSIONS));
    let video = association_list(collection::VIDEO_EXTENSIONS);
    let audio = association_list(collection::AUDIO_EXTENSIONS);
    let mut all: Vec<_> = image.iter().chain(&video).chain(&audio).collect();
    all.sort();
    json!({
        "productName": associations::PRODUCT, "version": version,
        "platform": if cfg!(windows) {"win32"} else {std::env::consts::OS},
        "isPortable": associations::portable(), "associationSupported": associations::supported(),
        "basicAssociations": collection::BASIC, "optionalImageAssociations": image,
        "optionalVideoAssociations": video, "optionalAudioAssociations": audio, "optionalAssociations": all,
        "associationsEnabled": associations::preference(),
        "updateSupported": updater::Updater::supported(), "updateState": state.updater.state(),
        "startupPath": state.startup, "smokeTest": state.smoke.is_some(),
        "smokeDropPath": if state.smoke.is_some() { std::env::var("CLIPVIEW_RUST_DROP_TEST").ok() } else { None },
        "smokeVideoThumbnail": state.smoke.is_some() && std::env::var_os("CLIPVIEW_RUST_VIDEO_THUMB_TEST").is_some(),
        "smokeExport": state.smoke.is_some() && std::env::var_os("CLIPVIEW_RUST_EXPORT_TEST").is_some(),
        "smokeLayer": state.smoke.is_some() && std::env::var_os("CLIPVIEW_RUST_LAYER_TEST").is_some(),
    })
}

fn media_file(state: &Backend, item: Item) -> Result<Value> {
    let Item::File { path, .. } = item else { return Err("미디어 파일을 열 수 없습니다.".into()) };
    let ext = extension(&path);
    let kind = media_kind(&ext);
    if !matches!(kind, "video" | "audio") { return Err("동영상 또는 오디오 파일이 아닙니다.".into()); }
    let stat = fs::metadata(&path).map_err(error)?;
    let mime = collection::mime(&ext).ok_or("지원하지 않는 미디어 형식입니다.")?;
    let modified = stat.modified().ok().and_then(|ts| ts.duration_since(UNIX_EPOCH).ok()).map(|ts| ts.as_secs_f64() * 1000.);
    Ok(json!({"url": state.assets.file(path, mime, true)?, "mime": mime, "metadata": {
        "format": ext.to_ascii_uppercase(), "byteSize": stat.len(), "modifiedAt": modified,
        "source": if kind == "audio" { "오디오 파일" } else { "동영상 파일" }
    }}))
}

fn png_data(state: &Backend, source: &str) -> Result<Vec<u8>> {
    if let Some(encoded) = source.strip_prefix("data:image/png;base64,") {
        if encoded.len() > 128 << 20 { return Err("이미지 크기가 안전 제한을 초과합니다.".into()); }
        return STANDARD.decode(encoded).map_err(error);
    }
    let bytes = state.assets.encoded_image(source)?;
    picture::encode(&picture::decode(&bytes, None)?.into_rgba8())
}

fn image_command(state: &Backend, method: &str, args: &[Value]) -> Result<Value> {
    let item = argument::<Item>(args, 0)?;
    let is_foreground = method == "loadImage" && args.get(2).and_then(Value::as_bool) != Some(true);
    let _permit = state.decode.lock().map_err(error)?;
    if is_foreground && state.active.lock().map_err(error)?.as_ref().is_some_and(|key| *key != item_key(&item)) {
        return Err("ERR_IMAGE_REQUEST_CANCELLED: 더 최신 이미지 작업으로 대체되었습니다.".into());
    }
    if matches!(item.ext().as_str(), "psd" | "psb") {
        match method {
            "loadLayerThumbnail" => return psd::layers::thumbnail(&state.assets, &item, &argument::<String>(args, 1)?, args.get(2).and_then(Value::as_f64)),
            "renderLayeredImage" => return psd::layers::render(&state.assets, &item, args.get(1).unwrap_or(&Value::Null), args.get(2).unwrap_or(&Value::Null)),
            "prepareLayeredImage" => return psd::layers::prepare(&item),
            "pickLayer" => return psd::layers::pick(&item, argument(args, 1)?, argument(args, 2)?, args.get(3).unwrap_or(&Value::Null)),
            _ => {}
        }
    }
    match method {
        "loadImage" => {
            let mode = args.get(1).and_then(Value::as_str).unwrap_or("full");
            let metadata = fs::metadata(item.disk_path()).map_err(error)?;
            let stamp = metadata.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|t| t.as_nanos());
            if stamp.is_none() { return picture::load(&state.assets, &item, mode); }
            let key = format!("{}\0{mode}\0{}\0{:?}", args[0], metadata.len(), stamp);
            {
                let mut cache = state.image_cache.lock().map_err(error)?;
                if let Some(index) = cache.iter().position(|(cached, _)| cached == &key) {
                    let entry = cache.remove(index).expect("existing cache entry");
                    if entry.1["dataUrl"].as_str().is_some_and(|url| state.assets.contains_url(url)) {
                        let result = entry.1.clone();
                        cache.push_back(entry);
                        return Ok(result);
                    }
                }
            }
            let result = picture::load(&state.assets, &item, mode)?;
            let mut cache = state.image_cache.lock().map_err(error)?;
            if cache.len() >= 3 { cache.pop_front(); }
            cache.push_back((key, result.clone()));
            Ok(result)
        },
        "loadThumbnail" => picture::thumbnail(&state.assets, &item).map(Value::String),
        "loadLayerThumbnail" => clip::thumbnail(&state.assets, &item, &argument::<String>(args, 1)?),
        "renderLayeredImage" => clip::render(&state.assets, &item, args.get(1).unwrap_or(&Value::Null)),
        "prepareLayeredImage" => clip::prepare(&item),
        "pickLayer" => clip::pick(&item, argument(args, 1)?, argument(args, 2)?, args.get(3).unwrap_or(&Value::Null)),
        _ => unreachable!(),
    }
}

fn file_command(state: &Backend, app: &AppHandle, method: &str, args: &[Value]) -> Result<Value> {
    let window = app.get_webview_window("main").ok_or("앱 창을 찾지 못했습니다.")?;
    let native_dialog = || rfd::FileDialog::new().set_parent(&window);
    match method {
        "openDialog" => {
            let result = if args.first().and_then(Value::as_str) == Some("folder") {
                native_dialog().pick_folder()
            } else {
                native_dialog().add_filter("지원 미디어", &collection::extensions()).add_filter("모든 파일", &["*"]).pick_file()
            };
            Ok(json!(result))
        }
        "openPath" => collection::open(Path::new(&argument::<String>(args, 0)?)),
        "mediaFileUrl" => media_file(state, argument(args, 0)?),
        "findSubtitles" => subtitles::find(&argument(args, 0)?),
        "copyImage" => {
            let png = png_data(state, &argument::<String>(args, 0)?)?;
            let image = picture::decode(&png, Some(image::ImageFormat::Png))?.into_rgba8();
            let data = arboard::ImageData {
                width: image.width() as usize, height: image.height() as usize,
                bytes: std::borrow::Cow::Owned(image.into_raw()),
            };
            arboard::Clipboard::new().map_err(error)?.set_image(data).map_err(error)?;
            Ok(json!(true))
        }
        "saveImageCopy" => {
            let source: String = argument(args, 0)?;
            let name = collection::name(Path::new(&argument::<String>(args, 1)?));
            let destination = native_dialog().set_file_name(name).add_filter("PNG 이미지", &["png"]).save_file();
            let Some(path) = destination else { return Ok(json!(false)) };
            fs::write(path, png_data(state, &source)?).map_err(error)?;
            Ok(json!(true))
        }
        "showInFolder" => {
            let item: Item = argument(args, 0)?;
            app.opener().reveal_item_in_dir(item.disk_path()).map_err(error)?;
            Ok(Value::Null)
        }
        "openOriginal" => {
            let item: Item = argument(args, 0)?;
            app.opener().open_path(item.disk_path().to_string_lossy(), None::<&str>).map_err(error)?;
            Ok(json!(""))
        }
        "registerAssociations" | "syncAssociations" => {
            let register = method == "registerAssociations";
            associations::register(argument(args, 0)?, register)?;
            if register && args.get(1).and_then(Value::as_bool) == Some(true) {
                app.opener().open_url("ms-settings:defaultapps?registeredAppUser=Clip%20Image%20Viewer", None::<&str>).map_err(error)?;
            }
            Ok(json!(true))
        }
        _ => Err(format!("Rust 실행부에서 지원하지 않는 명령: {method}")),
    }
}

#[tauri::command]
async fn dispatch(app: AppHandle, window: WebviewWindow, state: State<'_, Arc<Backend>>, method: String, args: Vec<Value>) -> Result<Value> {
    match method.as_str() {
        "getRuntimeInfo" => return Ok(runtime_info(&state, &app.package_info().version.to_string())),
        "getStartupPath" => return Ok(json!(state.startup)),
        "testDrop" if state.smoke.is_some() => {
            let path: PathBuf = argument(&args, 0)?;
            let event = WindowEvent::DragDrop(DragDropEvent::Drop { paths: vec![path], position: tauri::PhysicalPosition::new(0., 0.) });
            forward_window_drop(&window, &event)?;
            return Ok(Value::Null);
        }
        "setActiveItem" => {
            *state.active.lock().map_err(error)? = argument::<Option<Item>>(&args, 0)?.as_ref().map(item_key);
            return Ok(Value::Null);
        }
        "toggleFullscreen" => {
            let enabled = !window.is_fullscreen().map_err(error)?;
            window.set_fullscreen(enabled).map_err(error)?;
            window.emit("fullscreen-state", enabled).map_err(error)?;
            return Ok(json!(enabled));
        }
        "setAlwaysOnTop" => {
            let enabled: bool = argument(&args, 0)?;
            window.set_always_on_top(enabled).map_err(error)?;
            return Ok(json!(window.is_always_on_top().map_err(error)?));
        }
        "setWindowTitle" => {
            window.set_title(&argument::<String>(&args, 0)?).map_err(error)?;
            return Ok(Value::Null);
        }
        "checkForUpdates" => {
            let state = state.inner().clone();
            return tauri::async_runtime::spawn_blocking(move || {
                state.updater.check(env!("CARGO_PKG_VERSION"), &|update| { let _ = app.emit("update-state", update); })
            }).await.map_err(error);
        }
        "restartAndUpdate" => {
            state.updater.launch()?;
            app.exit(0);
            return Ok(json!(true));
        }
        "reportDiagnostic" => {
            let path = state.smoke.as_ref().ok_or("진단 모드가 아닙니다.")?;
            let data = args.first().ok_or("진단 결과가 비어 있습니다.")?;
            fs::write(path, serde_json::to_vec_pretty(data).map_err(error)?).map_err(error)?;
            if std::env::var_os("CLIPVIEW_RUST_PROBE").is_none() {
                app.exit(if data["loaded"] == true { 0 } else { 1 });
            }
            return Ok(Value::Null);
        }
        _ => {}
    }

    let image_task = matches!(method.as_str(), "loadImage" | "loadThumbnail" | "loadLayerThumbnail" | "renderLayeredImage" | "prepareLayeredImage" | "pickLayer");
    let background = method == "loadThumbnail" || method == "loadLayerThumbnail" || method == "prepareLayeredImage"
        || (method == "loadImage" && args.get(2).and_then(Value::as_bool) == Some(true));
    let _permit = if image_task {
        Some(if background { &state.background } else { &state.foreground }.acquire().await.map_err(error)?)
    } else { None };
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if image_task { image_command(&state, &method, &args) } else { file_command(&state, &app, &method, &args) }
    }).await.map_err(error)?
}

fn main() {
    // The isolated LibRaw worker re-runs this executable; it must exit before any app setup.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == libraw::WORKER_FLAG) {
        std::process::exit(libraw::worker_main());
    }
    // A downloaded portable update runs this (new) executable as its installer helper.
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments.get(1).is_some_and(|arg| arg == updater::APPLY_FLAG) {
        std::process::exit(updater::apply_main(&arguments[2..]));
    }
    let state = Arc::new(Backend::new());
    let isolated = std::env::var_os("CLIPVIEW_USER_DATA_DIR");
    let mut builder = tauri::Builder::default();
    if isolated.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let path = startup_path(args.into_iter().skip(1).map(|arg| Path::new(&cwd).join(arg)));
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
                if let Some(path) = path { let _ = window.emit("open-external-path", path); }
            }
        }));
    }
    builder
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .register_asynchronous_uri_scheme_protocol("clipmedia", |context, request, responder| {
            let state = context.app_handle().state::<Arc<Backend>>().inner().clone();
            tauri::async_runtime::spawn_blocking(move || responder.respond(state.assets.serve(request)));
        })
        .invoke_handler(tauri::generate_handler![dispatch])
        .setup(move |app| {
            let config = &app.config().app.windows[0];
            let mut builder = WebviewWindowBuilder::from_config(app, config)?.visible(false);
            if let Some(dir) = isolated { builder = builder.data_directory(PathBuf::from(dir).join("webview")); }
            let window = builder.build()?;
            app.state::<Arc<Backend>>().window.restore(&window);
            window.show()?;
            // Remove finished update staging folders, then check for updates shortly after start.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                updater::clean_old_staging();
                if !updater::Updater::supported() { return; }
                std::thread::sleep(std::time::Duration::from_millis(1800));
                let backend = handle.state::<Arc<Backend>>().inner().clone();
                backend.updater.check(env!("CARGO_PKG_VERSION"), &|update| { let _ = handle.emit("update-state", update); });
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            let Some(webview) = window.get_webview_window(window.label()) else { return };
            let state = window.state::<Arc<Backend>>();
            match event {
                WindowEvent::Resized(_) | WindowEvent::Moved(_) => state.window.record(&webview),
                WindowEvent::CloseRequested { .. } => state.window.save(&webview),
                WindowEvent::DragDrop(_) => {
                    if let Err(error) = forward_window_drop(&webview, event) { eprintln!("파일 드롭 전달 실패: {error}"); }
                }
                _ => {}
            }
        })
        .on_webview_event(|webview, event| {
            if let WebviewEvent::DragDrop(drop) = event
                && let Some(path) = dropped_path(drop)
            { let _ = webview.emit("open-external-path", path); }
        })
        .run(tauri::generate_context!())
        .expect("Clip Image Viewer could not start");
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn prefetched_image_is_reused_and_invalidated_when_file_changes() {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/sample.png");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("한글.png");
        fs::copy(original, &path).unwrap();
        let state = Backend::new();
        let args = vec![json!({"kind":"file", "path":path}), json!("full"), json!(true)];
        let first = image_command(&state, "loadImage", &args).unwrap();
        let second = image_command(&state, "loadImage", &args).unwrap();
        assert_eq!(first["dataUrl"], second["dataUrl"]);
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"\0").unwrap();
        let updated = image_command(&state, "loadImage", &args).unwrap();
        assert_ne!(first["dataUrl"], updated["dataUrl"]);
    }
}
