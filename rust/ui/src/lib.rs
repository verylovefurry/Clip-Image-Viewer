use clipview_core::{FitInput, Point, Rect, Viewport, calculate_fit_zoom, image_point};
use js_sys::{Array, Function, Promise, Reflect};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::*;

thread_local! { static APP: RefCell<App> = RefCell::new(App::default()); }
thread_local! { static TIMERS: RefCell<HashMap<i32,Closure<dyn FnMut()>>> = RefCell::new(HashMap::new()); }

struct App {
    items: Vec<Value>,
    index: usize,
    sequence: u64,
    layer_sequence: u64,
    media: String,
    source: String,
    metadata: Value,
    document: Value,
    visibility: HashMap<String, bool>,
    confirmed_visibility: HashMap<String, bool>,
    collapsed: HashSet<String>,
    selected_layer: String,
    width: f64,
    height: f64,
    zoom: f64,
    pan: (f64, f64),
    rotation: i32,
    flip: f64,
    fit: bool,
    drag: Option<(f64, f64, f64, f64)>,
    pointer: Option<(f64, f64)>,
    fullscreen: bool,
    pinned: bool,
    thumbs: bool,
    info: bool,
    layers: bool,
    wheel: String,
    crop: String,
    loop_files: bool,
    playback: String,
    controls_timer: Option<i32>,
    controls_mode: String,
    controls_locked: bool,
    interacting: bool,
    slide_timer: Option<i32>,
    slide_callback: Option<Closure<dyn FnMut()>>,
    video_click: Option<i32>,
    subtitle_urls: Vec<String>,
    video_thumbnail_tasks: usize,
    runtime: Value,
}

impl Default for App {
    fn default() -> Self {
        Self {
            items: vec![],
            index: 0,
            sequence: 0,
            layer_sequence: 0,
            media: String::new(),
            source: String::new(),
            metadata: Value::Null,
            document: Value::Null,
            visibility: HashMap::new(),
            confirmed_visibility: HashMap::new(),
            collapsed: HashSet::new(),
            selected_layer: String::new(),
            width: 0.,
            height: 0.,
            zoom: 1.,
            pan: (0., 0.),
            rotation: 0,
            flip: 1.,
            fit: true,
            drag: None,
            pointer: None,
            fullscreen: false,
            pinned: false,
            thumbs: false,
            info: false,
            layers: false,
            wheel: saved("wheelAction", "zoom"),
            crop: saved("cropMode", "full"),
            loop_files: saved("loop", "true") != "false",
            playback: saved("videoPlaybackMode", "sequential"),
            controls_timer: None,
            controls_mode: "full".into(),
            controls_locked: false,
            interacting: false,
            slide_timer: None,
            slide_callback: None,
            video_click: None,
            subtitle_urls: vec![],
            video_thumbnail_tasks: 0,
            runtime: Value::Null,
        }
    }
}

fn win() -> Window {
    web_sys::window().expect("window")
}
fn doc() -> Document {
    win().document().expect("document")
}
fn el(id: &str) -> Element {
    doc()
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("missing element {id}"))
}
fn html(id: &str) -> HtmlElement {
    el(id).unchecked_into()
}
fn image() -> HtmlImageElement {
    el("viewerImage").unchecked_into()
}
fn video() -> HtmlVideoElement {
    el("viewerVideo").unchecked_into()
}
fn cls(id: &str, name: &str, enabled: bool) {
    let _ = el(id).class_list().toggle_with_force(name, enabled);
}
fn hidden(id: &str, enabled: bool) {
    cls(id, "hidden", enabled);
}
fn text(id: &str, value: &str) {
    el(id).set_text_content(Some(value));
}
fn attr(id: &str, key: &str, value: &str) {
    let _ = el(id).set_attribute(key, value);
}
fn style(id: &str, key: &str, value: &str) {
    let _ = html(id).style().set_property(key, value);
}
fn val(id: &str) -> String {
    Reflect::get(&el(id), &"value".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}
fn set_val(id: &str, value: &str) {
    let _ = Reflect::set(&el(id), &"value".into(), &value.into());
}
fn saved(key: &str, default: &str) -> String {
    win()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item(key).ok().flatten())
        .unwrap_or_else(|| default.into())
}
fn save(key: &str, value: &str) {
    if let Ok(Some(storage)) = win().local_storage() {
        let _ = storage.set_item(key, value);
    }
}
fn s(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
fn num(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(0.)
}
fn current() -> Value {
    APP.with(|a| {
        a.borrow()
            .items
            .get(a.borrow().index)
            .cloned()
            .unwrap_or(Value::Null)
    })
}
fn playback() -> bool {
    APP.with(|a| matches!(a.borrow().media.as_str(), "video" | "audio"))
}
fn err(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}
fn toast(message: &str) {
    text("toast", message);
    hidden("toast", false);
    later(2400, || hidden("toast", true));
}
fn later(ms: i32, callback: impl FnOnce() + 'static) -> i32 {
    let handle = Rc::new(Cell::new(0));
    let completed = handle.clone();
    let cb = Closure::once(move || {
        callback();
        TIMERS.with(|timers| {
            timers.borrow_mut().remove(&completed.get());
        });
    });
    let id = win()
        .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), ms)
        .unwrap_or(0);
    handle.set(id);
    TIMERS.with(|timers| timers.borrow_mut().insert(id, cb));
    id
}
fn clear_timer(id: i32) {
    win().clear_timeout_with_handle(id);
    TIMERS.with(|timers| {
        timers.borrow_mut().remove(&id);
    });
}
fn on(target: &EventTarget, event: &str, callback: impl FnMut(Event) + 'static) {
    let callback = Closure::<dyn FnMut(Event)>::new(callback);
    target
        .add_event_listener_with_callback(event, callback.as_ref().unchecked_ref())
        .expect("event listener");
    callback.forget();
}
fn click(id: &str, mut callback: impl FnMut() + 'static) {
    on(el(id).as_ref(), "click", move |_| callback());
}
async fn call(method: &str, args: Vec<Value>) -> Result<Value, JsValue> {
    let internals = Reflect::get(&win(), &"__TAURI_INTERNALS__".into())?;
    let invoke: Function = Reflect::get(&internals, &"invoke".into())?.dyn_into()?;
    let payload = json!({"method":method,"args":args})
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let promise: Promise = invoke
        .call2(&internals, &"dispatch".into(), &payload)?
        .dyn_into()?;
    let result = JsFuture::from(promise).await?;
    serde_wasm_bindgen::from_value(result).map_err(|e| JsValue::from_str(&e.to_string()))
}
fn command(method: &'static str, args: Vec<Value>) {
    spawn_local(async move {
        if let Err(e) = call(method, args).await {
            toast(&err(e));
        }
    });
}
fn listen(name: &'static str, mut handler: impl FnMut(Value) + 'static) {
    spawn_local(async move {
        let result = async {
            let tauri = Reflect::get(&win(), &"__TAURI__".into())?;
            let events = Reflect::get(&tauri, &"event".into())?;
            let function: Function = Reflect::get(&events, &"listen".into())?.dyn_into()?;
            let callback = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
                if let Ok(value) = serde_wasm_bindgen::from_value::<Value>(value) {
                    handler(value["payload"].clone());
                }
            });
            let promise: Promise = function
                .call2(&events, &name.into(), callback.as_ref().unchecked_ref())?
                .dyn_into()?;
            JsFuture::from(promise).await?;
            callback.forget();
            Ok::<(), JsValue>(())
        }
        .await;
        if let Err(error) = result {
            toast(&err(error));
        }
    });
}

fn fit() {
    let rect = el("stage").get_bounding_client_rect();
    APP.with(|state| {
        let mut a = state.borrow_mut();
        a.fit = true;
        a.pan = (0., 0.);
        a.zoom = calculate_fit_zoom(FitInput {
            natural_width: a.width,
            natural_height: a.height,
            viewport: Rect {
                left: rect.left(),
                top: rect.top(),
                width: rect.width(),
                height: rect.height(),
            },
            rotation_degrees: a.rotation as f64,
        })
        .unwrap_or(1.);
    });
    transform();
}
fn transform() {
    APP.with(|state| {
        let a = state.borrow();
        let id = if a.media == "image" {
            "viewerImage"
        } else {
            "viewerVideo"
        };
        style(
            id,
            "transform",
            &format!(
                "translate({}px, {}px) rotate({}deg) scale({}, {}) translate(-50%, -50%)",
                a.pan.0,
                a.pan.1,
                a.rotation,
                a.zoom * a.flip,
                a.zoom
            ),
        );
        text("zoomLabel", &format!("{}%", (a.zoom * 100.).round()));
        cls("fitBtn", "active", a.fit && !a.source.is_empty());
        cls("actualBtn", "active", !a.fit && a.zoom == 1.);
    });
}
fn zoom(factor: f64) {
    let rect = el("stage").get_bounding_client_rect();
    APP.with(|state| {
        let mut a = state.borrow_mut();
        if a.source.is_empty() || a.media == "audio" {
            return;
        }
        let mut viewport = Viewport {
            zoom: a.zoom,
            pan_x: a.pan.0,
            pan_y: a.pan.1,
            fit_mode: a.fit,
        };
        let mut pan_start = a
            .drag
            .map(|d| Point { x: d.2, y: d.3 })
            .unwrap_or(Point { x: 0., y: 0. });
        let _ = viewport.zoom_at(
            factor,
            a.pointer.map(|p| Point { x: p.0, y: p.1 }),
            Rect {
                left: rect.left(),
                top: rect.top(),
                width: rect.width(),
                height: rect.height(),
            },
            a.media == "image",
            a.drag.is_some(),
            &mut pan_start,
        );
        a.zoom = viewport.zoom;
        a.pan = (viewport.pan_x, viewport.pan_y);
        a.fit = viewport.fit_mode;
        if let Some(ref mut drag) = a.drag {
            drag.2 = pan_start.x;
            drag.3 = pan_start.y;
        }
    });
    transform();
}
fn rotate(angle: i32) {
    APP.with(|a| {
        let mut a = a.borrow_mut();
        a.rotation = (a.rotation + angle).rem_euclid(360);
    });
    if APP.with(|a| a.borrow().fit) {
        fit();
    } else {
        transform();
    }
}
fn fullscreen() {
    spawn_local(async {
        match call("toggleFullscreen", vec![]).await {
            Ok(result) => {
                let enabled = result
                    .as_bool()
                    .unwrap_or_else(|| APP.with(|a| !a.borrow().fullscreen));
                APP.with(|a| a.borrow_mut().fullscreen = enabled);
                let _ = doc()
                    .body()
                    .unwrap()
                    .class_list()
                    .toggle_with_force("fullscreen-ui", enabled);
                later(0, fit);
            }
            Err(e) => toast(&err(e)),
        }
    });
}
fn move_by(delta: i32) {
    let index = APP.with(|state| {
        let a = state.borrow();
        let len = a.items.len() as i32;
        if len < 2 {
            return None;
        }
        let next = if a.loop_files {
            (a.index as i32 + delta).rem_euclid(len)
        } else {
            (a.index as i32 + delta).clamp(0, len - 1)
        };
        (next as usize != a.index).then_some(next as usize)
    });
    if let Some(index) = index {
        select(index);
    }
}
fn select(index: usize) {
    spawn_local(async move {
        load(index).await;
    });
}
fn open(path: String) {
    if path.is_empty() {
        return;
    }
    spawn_local(async move {
        let generation = APP.with(|a| {
            let mut a = a.borrow_mut();
            a.sequence += 1;
            a.sequence
        });
        match call("openPath", vec![json!(path)]).await {
            Ok(collection) => {
                if APP.with(|a| a.borrow().sequence != generation) {
                    return;
                }
                let items = collection["items"].as_array().cloned().unwrap_or_default();
                if items.is_empty() {
                    toast("지원되는 파일이 없습니다.");
                    return;
                }
                let index = collection["index"].as_u64().unwrap_or(0) as usize;
                APP.with(|a| {
                    let mut a = a.borrow_mut();
                    a.items = items;
                    a.index = index;
                });
                el("thumbnailStrip").set_text_content(None);
                load(index).await;
            }
            Err(e) if APP.with(|a| a.borrow().sequence == generation) => toast(&err(e)),
            Err(_) => {}
        }
    });
}
fn dialog(kind: &'static str) {
    spawn_local(async move {
        match call("openDialog", vec![json!(kind)]).await {
            Ok(path) => {
                if let Some(path) = path.as_str() {
                    open(path.into());
                }
            }
            Err(e) => toast(&err(e)),
        }
    });
}

async fn load(index: usize) {
    let pair = APP.with(|state| {
        let mut a = state.borrow_mut();
        let item = a.items.get(index)?.clone();
        a.index = index;
        a.sequence += 1;
        a.layer_sequence += 1;
        a.document = Value::Null;
        a.visibility.clear();
        a.confirmed_visibility.clear();
        a.collapsed.clear();
        a.selected_layer.clear();
        a.layers = false;
        Some((item, a.sequence))
    });
    let Some((item, generation)) = pair else {
        return;
    };
    hidden("loading", false);
    hidden("errorState", true);
    hidden("emptyState", true);
    hidden("imageLayer", true);
    hidden("layerPanel", true);
    hidden("layerDetails", true);
    hidden("layerBtn", true);
    attr("stage", "aria-busy", "true");
    video().pause().ok();
    video().remove_attribute("src").ok();
    video().load();
    APP.with(|a| {
        let mut a = a.borrow_mut();
        for url in a.subtitle_urls.drain(..) {
            Url::revoke_object_url(&url).ok();
        }
        if let Some(timer) = a.video_click.take() {
            clear_timer(timer);
        }
    });
    let media = s(&item, "mediaType");
    let media = if matches!(media.as_str(), "video" | "audio") {
        media
    } else {
        "image".into()
    };
    APP.with(|a| {
        let mut a = a.borrow_mut();
        a.media = media.clone();
        a.source.clear();
        a.rotation = 0;
        a.flip = 1.;
        a.pan = (0., 0.);
        a.fit = true;
    });
    update();
    let _ = call("setActiveItem", vec![item.clone()]).await;
    if APP.with(|a| a.borrow().sequence != generation) { return; }
    let crop = APP.with(|a| a.borrow().crop.clone());
    let result = if media == "image" {
        call("loadImage", vec![item.clone(), json!(crop), json!(false)]).await
    } else {
        call("mediaFileUrl", vec![item.clone()]).await
    };
    if APP.with(|a| a.borrow().sequence != generation) {
        return;
    }
    match result {
        Ok(result) => {
            let source = if media == "image" {
                s(&result, "dataUrl")
            } else {
                s(&result, "url")
            };
            APP.with(|a| {
                let mut a = a.borrow_mut();
                a.source = source.clone();
                a.metadata = result["metadata"].clone();
                a.document = result["layerDocument"].clone();
            });
            if media == "image" {
                let img = image();
                img.set_alt(&s(&item, "name"));
                img.set_cross_origin(Some("anonymous"));
                img.set_src(&source);
                if let Err(error) = JsFuture::from(img.decode()).await {
                    if APP.with(|a| a.borrow().sequence == generation) {
                        failure(&item, &err(error));
                    }
                    return;
                }
                if APP.with(|a| a.borrow().sequence != generation) {
                    return;
                }
                APP.with(|a| {
                    let mut a = a.borrow_mut();
                    a.width = img.natural_width() as f64;
                    a.height = img.natural_height() as f64;
                });
                hidden("viewerImage", false);
                hidden("viewerVideo", true);
                hidden("audioPresentation", true);
                hidden("videoControls", true);
                cls("imageLayer", "video-active", false);
                cls("imageLayer", "audio-active", false);
                let layers = APP.with(|a| a.borrow().document["layers"].clone());
                index_layers(&layers);
                APP.with(|a| { let mut a = a.borrow_mut(); a.confirmed_visibility = a.visibility.clone(); });
                hidden(
                    "layerBtn",
                    !layers.is_array() || layers.as_array().is_some_and(|a| a.is_empty()),
                );
            } else {
                hidden("viewerImage", true);
                hidden("viewerVideo", false);
                hidden("audioPresentation", media != "audio");
                cls("imageLayer", "video-active", true);
                cls("imageLayer", "audio-active", media == "audio");
                text("audioFileName", &s(&item, "name"));
                text("audioFormat", &s(&result["metadata"], "format"));
                video().set_src(&source);
                video().load();
                show_controls("full", false);
                match call("findSubtitles", vec![item.clone()]).await {
                    Ok(subtitles) if APP.with(|a| a.borrow().sequence == generation) => attach_subtitles(&subtitles),
                    Err(error) if APP.with(|a| a.borrow().sequence == generation) => toast(&err(error)),
                    _ => {}
                }
                if APP.with(|a| a.borrow().sequence != generation) {
                    return;
                }
                if let Ok(promise) = video().play() {
                    let _ = JsFuture::from(promise).await;
                }
            }
            hidden("imageLayer", false);
            hidden("loading", true);
            attr("stage", "aria-busy", "false");
            fit();
            update();
            if APP.with(|a| a.borrow().thumbs) {
                render_thumbnails();
            }
            if media == "image" {
                prefetch(generation);
            }
        }
        Err(e) => failure(&item, &err(e)),
    }
}
fn failure(item: &Value, error: &str) {
    hidden("loading", true);
    hidden("imageLayer", true);
    hidden("errorState", false);
    text("errorFile", &s(item, "name"));
    text("errorDetail", error);
    attr("stage", "aria-busy", "false");
    APP.with(|a| a.borrow_mut().source.clear());
    update();
}
fn prefetch(generation: u64) {
    let items = APP.with(|a| {
        let a = a.borrow();
        [-1i32, 1]
            .into_iter()
            .filter_map(|d| {
                let n = a.index as i32 + d;
                if n < 0 {
                    None
                } else {
                    a.items.get(n as usize).cloned()
                }
            })
            .collect::<Vec<_>>()
    });
    spawn_local(async move {
        for item in items {
            if APP.with(|a| a.borrow().sequence != generation) {
                return;
            }
            if s(&item, "mediaType") == "image" {
                let crop = APP.with(|a| a.borrow().crop.clone());
                let _ = call("loadImage", vec![item, json!(crop), json!(true)]).await;
            }
        }
    });
}
fn time(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.) as u64
    } else {
        0
    };
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}
fn update() {
    let item = current();
    let name = s(&item, "name");
    text(
        "fileTitle",
        if name.is_empty() {
            "열린 파일 없음"
        } else {
            &name
        },
    );
    let title = if name.is_empty() {
        "Clip Image Viewer".to_owned()
    } else {
        format!("{name} - Clip Image Viewer")
    };
    doc().set_title(&title);
    command("setWindowTitle", vec![json!(title)]);
    APP.with(|state| {
        let a = state.borrow();
        text(
            "counter",
            &format!(
                "{} / {}",
                if a.items.is_empty() { 0 } else { a.index + 1 },
                a.items.len()
            ),
        );
        for id in ["prevOverlay", "nextOverlay"] {
            hidden(id, a.items.len() < 2);
        }
        for (id, disabled) in [
            (
                "prevBtn",
                a.items.len() < 2 || (!a.loop_files && a.index == 0),
            ),
            (
                "nextBtn",
                a.items.len() < 2 || (!a.loop_files && a.index + 1 == a.items.len()),
            ),
        ] {
            el(id)
                .unchecked_into::<HtmlElement>()
                .set_attribute("aria-disabled", &disabled.to_string())
                .ok();
            let _ = Reflect::set(&el(id), &"disabled".into(), &disabled.into());
        }
        let dimensions = if a.media == "audio" {
            s(&a.metadata, "format")
        } else {
            format!(
                "{} × {} · {}",
                a.width as u64,
                a.height as u64,
                s(&a.metadata, "format")
            )
        };
        text("dimensionLabel", &dimensions);
        if a.info {
            render_info(&item, &a.metadata);
        }
    });
    update_video();
}
fn render_info(item: &Value, metadata: &Value) {
    el("infoList").set_text_content(None);
    for (key, value) in [
        ("파일", s(item, "name")),
        ("경로", s(item, "path")),
        (
            "크기",
            format!("{} × {}", num(metadata, "width"), num(metadata, "height")),
        ),
        ("형식", s(metadata, "format")),
        ("파일 용량", format!("{} bytes", num(metadata, "byteSize"))),
        ("색 공간", s(metadata, "space")),
        ("불러오기", s(metadata, "source")),
    ] {
        for (tag, content) in [("dt", key.to_owned()), ("dd", value)] {
            let child = doc().create_element(tag).unwrap();
            child.set_text_content(Some(&content));
            el("infoList").append_child(&child).ok();
        }
    }
}
fn panel(which: &str) {
    APP.with(|state| {
        let mut a = state.borrow_mut();
        match which {
            "info" => {
                a.info = !a.info;
                if a.info {
                    a.layers = false;
                }
            }
            "layers" => {
                a.layers = !a.layers;
                if a.layers {
                    a.info = false;
                }
            }
            "thumbs" => a.thumbs = !a.thumbs,
            _ => {}
        }
        hidden("infoPanel", !a.info);
        hidden("layerPanel", !a.layers);
        hidden("thumbnailPanel", !a.thumbs);
        cls("infoBtn", "active", a.info);
        cls("layerBtn", "active", a.layers);
        cls("thumbBtn", "active", a.thumbs);
    });
    if APP.with(|a| a.borrow().layers) {
        render_layers();
    }
    if APP.with(|a| a.borrow().thumbs) {
        render_thumbnails();
    }
    update();
    later(0, fit);
}
fn render_thumbnails() {
    if el("thumbnailStrip").child_nodes().length() > 0 {
        let index = APP.with(|a| a.borrow().index);
        let nodes = el("thumbnailStrip").child_nodes();
        for i in 0..nodes.length() {
            let child: Element = nodes.item(i).unwrap().unchecked_into();
            child
                .class_list()
                .toggle_with_force("active", i as usize == index)
                .ok();
        }
        load_visible_thumbnails();
        return;
    }
    el("thumbnailStrip").set_text_content(None);
    let (items, index, generation) = APP.with(|a| {
        let a = a.borrow();
        (a.items.clone(), a.index, a.sequence)
    });
    for (i, item) in items.into_iter().enumerate() {
        let button = doc().create_element("button").unwrap();
        button.set_class_name(if i == index {
            "thumb-item active"
        } else {
            "thumb-item"
        });
        button.set_attribute("type", "button").ok();
        let img: HtmlImageElement = doc().create_element("img").unwrap().unchecked_into();
        img.set_alt("");
        let label = doc().create_element("span").unwrap();
        label.set_text_content(Some(&s(&item, "name")));
        button.append_child(&img).ok();
        button.append_child(&label).ok();
        el("thumbnailStrip").append_child(&button).ok();
        button.set_attribute("data-index", &i.to_string()).ok();
    }
    let _ = generation;
    load_visible_thumbnails();
}
fn load_visible_thumbnails() {
    let view = el("thumbnailStrip").get_bounding_client_rect();
    let nodes = el("thumbnailStrip").child_nodes();
    for index in 0..nodes.length() {
        let node: Element = nodes.item(index).unwrap().unchecked_into();
        let rect = node.get_bounding_client_rect();
        if rect.right() < view.left() - 100. || rect.left() > view.right() + 100. {
            continue;
        }
        let Some(img) = node.query_selector("img").ok().flatten() else {
            continue;
        };
        if img.has_attribute("src") || img.has_attribute("data-loading") || img.has_attribute("data-failed") {
            continue;
        }
        let item = APP.with(|a| a.borrow().items.get(index as usize).cloned());
        let Some(item) = item else {
            continue;
        };
        if s(&item, "mediaType") == "audio" {
            if let Ok(url) = media_placeholder(&item) { img.set_attribute("src", &url).ok(); }
            continue;
        }
        let is_video = s(&item, "mediaType") == "video";
        if is_video {
            let accepted = APP.with(|a| {
                let mut a = a.borrow_mut();
                if a.video_thumbnail_tasks >= 2 { return false; }
                a.video_thumbnail_tasks += 1;
                true
            });
            if !accepted { continue; }
        }
        img.set_attribute("data-loading", "true").ok();
        spawn_local(async move {
            let result = if is_video {
                match video_thumbnail(item.clone()).await {
                    Ok(url) => Ok(Value::String(url)),
                    Err(_) => media_placeholder(&item).map(Value::String),
                }
            } else {
                call("loadThumbnail", vec![item]).await
            };
            img.remove_attribute("data-loading").ok();
            match result {
                Ok(url) if url.as_str().is_some() => { img.set_attribute("src", url.as_str().unwrap()).ok(); }
                _ => { img.set_attribute("data-failed", "true").ok(); }
            };
            if is_video {
                let visible = APP.with(|a| {
                    let mut a = a.borrow_mut();
                    a.video_thumbnail_tasks = a.video_thumbnail_tasks.saturating_sub(1);
                    a.thumbs
                });
                if visible { load_visible_thumbnails(); }
            }
        });
    }
}
fn media_placeholder(item: &Value) -> Result<String, JsValue> {
    let canvas: HtmlCanvasElement = doc().create_element("canvas")?.dyn_into()?;
    canvas.set_width(180); canvas.set_height(110);
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d")?.ok_or("캔버스를 사용할 수 없습니다.")?.dyn_into()?;
    ctx.set_fill_style_str("#242727"); ctx.fill_rect(0., 0., 180., 110.);
    ctx.set_fill_style_str("#ced8d1"); ctx.set_font("18px Segoe UI, sans-serif"); ctx.set_text_align("center");
    let name = s(item, "name");
    let label = if s(item, "mediaType") == "audio" { name.rsplit('.').next().unwrap_or("AUDIO").to_uppercase() } else { "VIDEO".to_owned() };
    ctx.fill_text(&label, 90., 62.)?;
    canvas.to_data_url_with_type("image/png")
}
async fn video_event(video: &HtmlVideoElement, event: &str, start: impl FnOnce()) -> Result<(), JsValue> {
    let mut success: Option<Closure<dyn FnMut(Event)>> = None;
    let mut failure: Option<Closure<dyn FnMut(Event)>> = None;
    let mut timeout: Option<Closure<dyn FnMut()>> = None;
    let mut timer = None;
    let promise = Promise::new(&mut |resolve, reject| {
        let done = resolve.clone();
        let callback = Closure::wrap(Box::new(move |_: Event| { let _ = done.call0(&JsValue::UNDEFINED); }) as Box<dyn FnMut(Event)>);
        let fail = reject.clone();
        let error = Closure::wrap(Box::new(move |_: Event| { let _ = fail.call1(&JsValue::UNDEFINED, &JsValue::from_str("영상 썸네일을 만들 수 없습니다.")); }) as Box<dyn FnMut(Event)>);
        let expired = reject.clone();
        let timeout_cb = Closure::wrap(Box::new(move || { let _ = expired.call1(&JsValue::UNDEFINED, &JsValue::from_str("영상 썸네일 생성 시간이 초과되었습니다.")); }) as Box<dyn FnMut()>);
        video.add_event_listener_with_callback(event, callback.as_ref().unchecked_ref()).ok();
        video.add_event_listener_with_callback("error", error.as_ref().unchecked_ref()).ok();
        timer = win().set_timeout_with_callback_and_timeout_and_arguments_0(timeout_cb.as_ref().unchecked_ref(), 8000).ok();
        success = Some(callback); failure = Some(error); timeout = Some(timeout_cb);
    });
    start();
    let result = JsFuture::from(promise).await.map(|_| ());
    if let Some(callback) = success { video.remove_event_listener_with_callback(event, callback.as_ref().unchecked_ref()).ok(); }
    if let Some(callback) = failure { video.remove_event_listener_with_callback("error", callback.as_ref().unchecked_ref()).ok(); }
    if let Some(timer) = timer { win().clear_timeout_with_handle(timer); }
    drop(timeout);
    result
}
async fn video_thumbnail(item: Value) -> Result<String, JsValue> {
    let media = call("mediaFileUrl", vec![item]).await?;
    let preview: HtmlVideoElement = doc().create_element("video")?.dyn_into()?;
    preview.set_preload("auto");
    preview.set_muted(true);
    preview.set_cross_origin(Some("anonymous"));
    let result = async {
        video_event(&preview, "loadedmetadata", || { preview.set_src(&s(&media, "url")); preview.load(); }).await?;
        let duration = preview.duration();
        let target = if duration.is_finite() && duration > 0.25 { (duration * 0.1).clamp(0.1, 5.).min(duration - 0.1) } else { 0. };
        if target > 0.01 {
            video_event(&preview, "seeked", || preview.set_current_time(target)).await?;
        } else if preview.ready_state() < 2 {
            video_event(&preview, "loadeddata", || {}).await?;
        }
        let (width, height) = (preview.video_width() as f64, preview.video_height() as f64);
        if width <= 0. || height <= 0. { return Err(JsValue::from_str("영상 크기를 읽을 수 없습니다.")); }
        let canvas: HtmlCanvasElement = doc().create_element("canvas")?.dyn_into()?;
        canvas.set_width(300); canvas.set_height(152);
        let context: CanvasRenderingContext2d = canvas.get_context("2d")?.ok_or("캔버스를 사용할 수 없습니다.")?.dyn_into()?;
        context.set_fill_style_str("#151923"); context.fill_rect(0., 0., 300., 152.);
        let scale = (300. / width).min(152. / height);
        let (w, h) = (width * scale, height * scale);
        context.draw_image_with_html_video_element_and_dw_and_dh(&preview, (300. - w) / 2., (152. - h) / 2., w, h)?;
        canvas.to_data_url_with_type_and_encoder_options("image/jpeg", &JsValue::from_f64(0.84))
    }.await;
    preview.remove_attribute("src").ok(); preview.load();
    result
}
fn index_layers(layers: &Value) {
    if let Some(layers) = layers.as_array() {
        for layer in layers {
            let id = s(layer, "id");
            APP.with(|a| {
                a.borrow_mut()
                    .visibility
                    .insert(id, layer["visible"].as_bool().unwrap_or(true));
            });
            index_layers(&layer["children"]);
        }
    }
}
fn render_layers() {
    let scroll = el("layerTree").scroll_top();
    el("layerTree").set_text_content(None);
    let document = APP.with(|a| a.borrow().document.clone());
    text("layerHelp", &s(&document, "note"));
    append_layers(&document["layers"], 0);
    el("layerTree").set_scroll_top(scroll);
    load_visible_layers();
}
fn load_visible_layers() {
    let view = el("layerTree").get_bounding_client_rect();
    if let Ok(nodes) = el("layerTree").query_selector_all("img[data-layer-id]") {
        for i in 0..nodes.length() {
            let img: Element = nodes.item(i).unwrap().unchecked_into();
            let rect = img.get_bounding_client_rect();
            if img.has_attribute("src")
                || img.has_attribute("data-loading")
                || rect.bottom() < view.top() - 60.
                || rect.top() > view.bottom() + 60.
            {
                continue;
            }
            img.set_attribute("data-loading", "true").ok();
            let id = img.get_attribute("data-layer-id").unwrap_or_default();
            let item = current();
            spawn_local(async move {
                let result = call("loadLayerThumbnail", vec![item, json!(id)]).await;
                img.remove_attribute("data-loading").ok();
                if let Ok(result) = result {
                    if let Some(url) = result.as_str().or_else(|| result["dataUrl"].as_str()) {
                        img.set_attribute("src", url).ok();
                    }
                }
            });
        }
    }
}
fn layer_details(id: &str) {
    fn find(layers: &Value, id: &str, ancestors: &mut Vec<String>) -> Option<Value> {
        for layer in layers.as_array()? {
            if s(layer, "id") == id {
                return Some(layer.clone());
            }
            ancestors.push(s(layer, "id"));
            if let Some(result) = find(&layer["children"], id, ancestors) {
                return Some(result);
            }
            ancestors.pop();
        }
        None
    }
    let mut ancestors = Vec::new();
    let layer = APP.with(|a| find(&a.borrow().document["layers"], id, &mut ancestors));
    let Some(layer) = layer else {
        return;
    };
    APP.with(|a| {
        let mut a = a.borrow_mut();
        a.selected_layer = id.into();
        for ancestor in ancestors { a.collapsed.remove(&ancestor); }
    });
    render_layers();
    if let Ok(Some(row)) = el("layerTree").query_selector(".layer-row.selected") {
        row.scroll_into_view();
    }
    text("layerDetailName", &s(&layer, "name"));
    el("layerDetailList").set_text_content(None);
    let yes_no = |key: &str| if layer[key].as_bool() == Some(true) { "있음" } else { "없음" };
    let effects = layer["effects"].as_array().map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty()).unwrap_or_else(|| "없음".into());
    let type_label = layer["typeLabel"].as_str().unwrap_or_else(|| layer["type"].as_str().unwrap_or(""));
    let objects = layer["textObjects"].as_array();
    let mut values = vec![
        ("종류", type_label.to_owned()),
        ("혼합 모드", s(&layer, "blendModeLabel")),
        ("불투명도", format!("{}%", layer["opacityExact"].as_f64().unwrap_or_else(|| num(&layer, "opacity")))),
        ("클리핑", yes_no("clipping").to_owned()),
        ("마스크", yes_no("mask").to_owned()),
        ("효과", effects),
    ];
    if let Some(label) = layer["colorMode"]["label"].as_str() { values.push(("표현색", label.into())); }
    if let Some(objects) = objects { values.push(("텍스트 객체", format!("{}개", objects.len()))); }
    if let Some(value) = layer["previewAccuracy"].as_str() { values.push(("미리보기", value.into())); }
    for (key, value) in values {
        for (tag, t) in [("dt", key.to_owned()), ("dd", value)] {
            let e = doc().create_element(tag).unwrap();
            e.set_text_content(Some(&t));
            el("layerDetailList").append_child(&e).ok();
        }
    }
    let panel = el("layerTextObjects");
    panel.set_text_content(None);
    hidden("layerTextObjects", objects.is_none_or(|objects| objects.is_empty()));
    if let Some(objects) = objects {
        for (index, object) in objects.iter().enumerate() {
            let article = doc().create_element("article").unwrap();
            article.set_class_name("layer-text-object");
            for (class, content) in [("layer-text-object-heading", format!("객체 {}", index + 1)),
                ("layer-text-object-content", object["text"].as_str().filter(|s| !s.is_empty()).unwrap_or("(빈 텍스트)").to_owned())] {
                let element = doc().create_element("div").unwrap();
                element.set_class_name(class); element.set_text_content(Some(&content)); article.append_child(&element).ok();
            }
            let attrs = if object["attributes"].is_object() { &object["attributes"] } else { &object["extendedAttributes"] };
            let details = doc().create_element("dl").unwrap();
            let mut fields = vec![];
            if let Some(font) = attrs["font"].as_str() { fields.push(("글꼴", font.to_owned())); }
            if let Some(size) = attrs["fontSize"].as_f64() { fields.push(("크기", format!("{} pt", size / 100.))); }
            if let Some(vertical) = attrs["vertical"].as_bool() { fields.push(("쓰기 방향", if vertical { "세로" } else { "가로" }.to_owned())); }
            for (key, value) in fields {
                for (tag, content) in [("dt", key.to_owned()), ("dd", value)] {
                    let element = doc().create_element(tag).unwrap(); element.set_text_content(Some(&content)); details.append_child(&element).ok();
                }
            }
            article.append_child(&details).ok(); panel.append_child(&article).ok();
        }
    }
    hidden("layerDetails", false);
}
fn append_layers(layers: &Value, depth: usize) {
    let Some(layers) = layers.as_array() else {
        return;
    };
    for layer in layers {
        let id = s(layer, "id");
        let children = &layer["children"];
        let group = layer["isGroup"].as_bool().unwrap_or(false) || s(layer, "type") == "group";
        let row = doc().create_element("div").unwrap();
        row.set_class_name(if group {
            "layer-row layer-group-row"
        } else {
            "layer-row"
        });
        row.class_list().toggle_with_force("selected", APP.with(|a| a.borrow().selected_layer == id)).ok();
        row.unchecked_ref::<HtmlElement>()
            .style()
            .set_property("--layer-depth", &depth.to_string())
            .ok();
        let eye = doc().create_element("button").unwrap();
        eye.set_class_name("layer-visibility");
        eye.set_attribute("type", "button").ok();
        eye.set_attribute("aria-label", "레이어 표시 전환").ok();
        if !APP.with(|a| a.borrow().document["toggleSupported"].as_bool().unwrap_or(false))
            || layer["toggleAvailable"].as_bool() == Some(false) {
            eye.set_attribute("disabled", "").ok();
        }
        let visible = APP.with(|a| a.borrow().visibility.get(&id).copied().unwrap_or(true));
        eye.class_list().toggle_with_force("active", visible).ok();
        eye.set_attribute("aria-pressed", &visible.to_string()).ok();
        eye.set_attribute("data-layer-toggle", &id).ok();
        row.append_child(&eye).ok();
        if group {
            let folder = doc().create_element("button").unwrap();
            folder.set_class_name("layer-folder-toggle");
            folder.set_attribute("type", "button").ok();
            for class in ["layer-disclosure", "layer-folder-icon"] {
                let icon = doc().create_element("span").unwrap();
                icon.set_class_name(class);
                folder.append_child(&icon).ok();
            }
            let collapsed = APP.with(|a| a.borrow().collapsed.contains(&id));
            folder
                .class_list()
                .toggle_with_force("collapsed", collapsed)
                .ok();
            folder.set_attribute("data-layer-folder", &id).ok();
            row.append_child(&folder).ok();
        } else {
            let frame = doc().create_element("div").unwrap();
            frame.set_class_name("layer-thumb-frame");
            let img: HtmlImageElement = doc().create_element("img").unwrap().unchecked_into();
            img.set_class_name("layer-thumbnail");
            frame.append_child(&img).ok();
            row.append_child(&frame).ok();
            img.set_attribute("data-layer-id", &id).ok();
        }
        let name = doc().create_element("button").unwrap();
        name.set_class_name("layer-name");
        name.set_attribute("type", "button").ok();
        for (class, content) in [
            ("layer-title", s(layer, "name")),
            (
                "layer-meta",
                format!(
                    "{}% · {}",
                    num(layer, "opacity"),
                    s(layer, "blendModeLabel")
                ),
            ),
        ] {
            let span = doc().create_element("span").unwrap();
            span.set_class_name(class);
            span.set_text_content(Some(&content));
            name.append_child(&span).ok();
        }
        name.set_attribute("data-layer-select", &id).ok();
        row.append_child(&name).ok();
        el("layerTree").append_child(&row).ok();
        if group && !APP.with(|a| a.borrow().collapsed.contains(&id)) {
            append_layers(children, depth + 1);
        }
    }
}
fn toggle_layer(id: String) {
    let (item, visibility, generation, selection) = APP.with(|state| {
        let mut a = state.borrow_mut();
        let previous = a.visibility.get(&id).copied().unwrap_or(true);
        let enabled = !previous;
        a.visibility.insert(id.clone(), enabled);
        a.layer_sequence += 1;
        (
            a.items[a.index].clone(),
            a.visibility.clone(),
            a.layer_sequence,
            a.sequence,
        )
    });
    render_layers();
    spawn_local(async move {
        match call("renderLayeredImage", vec![item, json!(visibility)]).await {
            Ok(result) => {
                if APP.with(|a| {
                    let a = a.borrow();
                    a.layer_sequence == generation && a.sequence == selection
                }) {
                    APP.with(|a| a.borrow_mut().confirmed_visibility = visibility);
                    image().set_src(&s(&result, "dataUrl"));
                }
            }
            Err(error) => {
                let current = APP.with(|a| {
                    let mut a = a.borrow_mut();
                    if a.layer_sequence != generation || a.sequence != selection { return false; }
                    a.visibility = a.confirmed_visibility.clone();
                    true
                });
                if current { render_layers(); toast(&err(error)); }
            }
        }
    });
}
fn pick_layer(event: &MouseEvent) {
    let rect = el("stage").get_bounding_client_rect();
    let request = APP.with(|state| {
        let a = state.borrow();
        if a.media != "image" || a.document["pickSupported"].as_bool() != Some(true) { return None; }
        let point = image_point(Point { x: event.client_x() as f64, y: event.client_y() as f64 },
            Rect { left: rect.left(), top: rect.top(), width: rect.width(), height: rect.height() },
            Point { x: a.width, y: a.height }, Point { x: a.pan.0, y: a.pan.1 },
            a.zoom, a.rotation as f64, a.flip)?;
        Some((a.items.get(a.index)?.clone(), a.visibility.clone(), a.sequence, point))
    });
    let Some((item, visibility, sequence, point)) = request else { return; };
    spawn_local(async move {
        let result = call("pickLayer", vec![item, json!(point.x), json!(point.y), json!(visibility)]).await;
        if APP.with(|a| a.borrow().sequence != sequence) { return; }
        match result {
            Ok(id) if id.as_str().is_some_and(|id| !id.is_empty()) => {
                if !APP.with(|a| a.borrow().layers) { panel("layers"); }
                layer_details(id.as_str().unwrap());
            }
            Ok(_) => toast("해당 위치에서 레이어를 찾지 못했습니다."),
            Err(error) => toast(&err(error)),
        }
    });
}
fn exported_png() -> Result<String, JsValue> {
        if playback() || APP.with(|a| a.borrow().source.is_empty()) || el("stage").get_attribute("aria-busy").as_deref() == Some("true") {
            return Err(JsValue::from_str("내보낼 이미지가 없습니다."));
        }
        let image = image();
        let (width, height) = (image.natural_width(), image.natural_height());
        if width == 0 || height == 0 { return Err(JsValue::from_str("이미지를 아직 불러오지 못했습니다.")); }
        let (rotation, flip) = APP.with(|a| { let a = a.borrow(); (a.rotation, a.flip) });
        let canvas: HtmlCanvasElement = doc().create_element("canvas")?.dyn_into()?;
        let swap = rotation.rem_euclid(180) == 90;
        canvas.set_width(if swap { height } else { width });
        canvas.set_height(if swap { width } else { height });
        let context: CanvasRenderingContext2d = canvas.get_context("2d")?.ok_or("캔버스를 사용할 수 없습니다.")?.dyn_into()?;
        context.translate(canvas.width() as f64 / 2., canvas.height() as f64 / 2.)?;
        context.rotate((rotation as f64).to_radians())?;
        context.scale(flip, 1.)?;
        context.draw_image_with_html_image_element(&image, -(width as f64) / 2., -(height as f64) / 2.)?;
        canvas.to_data_url_with_type("image/png")
}
fn export_image(save_file: bool) {
    match exported_png() {
        Ok(data) if save_file => {
            let name = s(&current(), "name");
            let stem = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
            command("saveImageCopy", vec![json!(data), json!(format!("{stem}.png"))]);
        }
        Ok(data) => command("copyImage", vec![json!(data)]),
        Err(error) => toast(&err(error)),
    }
}
fn attach_subtitles(subtitles: &Value) {
    if let Ok(nodes) = video().query_selector_all("track") {
        for i in 0..nodes.length() {
            if let Some(node) = nodes.item(i) {
                node.parent_node().and_then(|p| p.remove_child(&node).ok());
            }
        }
    }
    if let Some(subtitles) = subtitles.as_array() {
        for (i, subtitle) in subtitles.iter().enumerate() {
            let track: HtmlTrackElement = doc().create_element("track").unwrap().unchecked_into();
            track.set_kind("subtitles");
            track.set_label(&s(subtitle, "label"));
            track.set_srclang(&s(subtitle, "srclang"));
            let options = BlobPropertyBag::new();
            options.set_type("text/vtt");
            if let Ok(blob) = Blob::new_with_str_sequence_and_options(
                &Array::of1(&s(subtitle, "vtt").into()),
                &options,
            ) {
                if let Ok(url) = Url::create_object_url_with_blob(&blob) {
                    track.set_src(&url);
                    APP.with(|a| a.borrow_mut().subtitle_urls.push(url));
                }
            }
            track.set_default(i == 0);
            video().append_child(&track).ok();
        }
    }
}
fn subtitles_toggle() {
    let tracks = video().text_tracks();
    let Some(tracks) = tracks else {
        return;
    };
    let showing = (0..tracks.length())
        .filter_map(|i| tracks.get(i))
        .any(|t| t.mode() == TextTrackMode::Showing);
    for i in 0..tracks.length() {
        if let Some(track) = tracks.get(i) {
            track.set_mode(if !showing && i == 0 {
                TextTrackMode::Showing
            } else {
                TextTrackMode::Hidden
            });
        }
    }
    cls("videoSubtitleBtn", "active", !showing);
}
fn play_pause() {
    if !playback() {
        return;
    }
    if video().paused() {
        if let Ok(p) = video().play() {
            spawn_local(async move {
                if let Err(e) = JsFuture::from(p).await {
                    toast(&err(e));
                }
            });
        }
    } else {
        video().pause().ok();
    }
    show_controls("full", false);
}
fn seek(delta: f64) {
    let v = video();
    if playback() && v.duration().is_finite() {
        v.set_current_time((v.current_time() + delta).clamp(0., v.duration()));
        show_controls("progress", true);
    }
}
fn volume(delta: f64) {
    let v = video();
    v.set_volume((v.volume() + delta).clamp(0., 1.));
    v.set_muted(false);
    update_video();
    show_controls("volume", true);
}
fn update_video() {
    if !playback() {
        return;
    }
    let v = video();
    let duration = v.duration();
    text(
        "videoTimeLabel",
        &format!("{} / {}", time(v.current_time()), time(duration)),
    );
    text("videoTitleLabel", &s(&current(), "name"));
    set_val(
        "videoSeek",
        &if duration > 0. && duration.is_finite() {
            (v.current_time() / duration * 1000.).round().to_string()
        } else {
            "0".into()
        },
    );
    style(
        "videoSeek",
        "--range-progress",
        &format!(
            "{}%",
            if duration > 0. {
                v.current_time() / duration * 100.
            } else {
                0.
            }
        ),
    );
    cls("videoPlayBtn", "playing", !v.paused());
    cls("videoMuteBtn", "active", v.muted() || v.volume() == 0.);
    set_val(
        "videoVolume",
        &if v.muted() { 0. } else { v.volume() }.to_string(),
    );
    set_val("videoSpeedSelect", &v.playback_rate().to_string());
    APP.with(|a| {
        let a = a.borrow();
        text(
            "videoModeBtn",
            match a.playback.as_str() {
                "repeatOne" => "한 파일 반복",
                "repeatAll" => "전체반복",
                "shuffle" => "무작위",
                _ => "순서",
            },
        );
    });
    let audio = APP.with(|a| a.borrow().media == "audio");
    hidden("videoSubtitleBtn", audio);
    hidden("videoFullscreenBtn", audio);
}
fn show_controls(mode: &str, lock: bool) {
    if !playback() {
        return;
    }
    let mode = APP.with(|state| {
        let mut a = state.borrow_mut();
        if let Some(timer) = a.controls_timer.take() {
            clear_timer(timer);
        }
        let keep_locked = a.controls_locked && mode == "full";
        let mode = if a.media == "audio" {
            "full".into()
        } else if keep_locked {
            a.controls_mode.clone()
        } else {
            mode.into()
        };
        a.controls_mode = mode.clone();
        a.controls_locked = lock || keep_locked;
        mode
    });
    hidden("videoControls", false);
    cls("videoControls", "controls-hidden", false);
    cls("videoControls", "peek-progress", mode == "progress");
    cls("videoControls", "peek-volume", mode == "volume");
    doc()
        .body()
        .unwrap()
        .class_list()
        .remove_1("video-cursor-hidden")
        .ok();
    if APP.with(|a| a.borrow().media != "audio") {
        let timer = later(2000, hide_controls);
        APP.with(|a| a.borrow_mut().controls_timer = Some(timer));
    }
}
fn hide_controls() {
    if APP.with(|a| {
        let a = a.borrow();
        a.media != "video" || a.interacting
    }) {
        return;
    }
    cls("videoControls", "controls-hidden", true);
    APP.with(|a| a.borrow_mut().controls_locked = false);
    doc()
        .body()
        .unwrap()
        .class_list()
        .toggle_with_force("video-cursor-hidden", !video().paused())
        .ok();
}
fn set_playback(mode: String) {
    save("videoPlaybackMode", &mode);
    APP.with(|a| a.borrow_mut().playback = mode);
    update_video();
}
fn ended() {
    let next = APP.with(|state| {
        let a = state.borrow();
        let indices = a
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| s(item, "mediaType") == a.media)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let position = indices.iter().position(|i| *i == a.index)?;
        match a.playback.as_str() {
            "repeatOne" => Some(a.index),
            "repeatAll" => indices.get((position + 1) % indices.len()).copied(),
            "shuffle" => {
                let choices = indices
                    .into_iter()
                    .filter(|i| *i != a.index)
                    .collect::<Vec<_>>();
                if choices.is_empty() {
                    Some(a.index)
                } else {
                    choices
                        .get((js_sys::Math::random() * choices.len() as f64) as usize)
                        .copied()
                }
            }
            _ => indices.get(position + 1).copied(),
        }
    });
    if let Some(index) = next {
        select(index);
    }
}
fn background(value: &str) {
    for mode in ["dark", "black", "light", "checker"] {
        cls("stage", &format!("bg-{mode}"), mode == value);
    }
}
fn settings() {
    set_val("wheelActionSelect", &APP.with(|a| a.borrow().wheel.clone()));
    set_val("backgroundSelect", &saved("background", "dark"));
    set_val("slideIntervalSelect", &saved("slideInterval", "3000"));
    let _ = Reflect::set(
        &el("loopCheckbox"),
        &"checked".into(),
        &APP.with(|a| a.borrow().loop_files).into(),
    );
    let dialog: HtmlDialogElement = el("settingsDialog").unchecked_into();
    dialog.show_modal().ok();
}
fn settings_page(page: &str) {
    if let Ok(tabs) = doc().query_selector_all("[data-settings-page]") {
        for i in 0..tabs.length() {
            let tab: Element = tabs.item(i).unwrap().unchecked_into();
            let selected = tab.get_attribute("data-settings-page").as_deref() == Some(page);
            tab.set_attribute("aria-selected", &selected.to_string())
                .ok();
            tab.set_attribute("tabindex", if selected { "0" } else { "-1" })
                .ok();
        }
    }
    if let Ok(panels) = doc().query_selector_all("[data-settings-panel]") {
        for i in 0..panels.length() {
            let panel: Element = panels.item(i).unwrap().unchecked_into();
            let show = panel.get_attribute("data-settings-panel").as_deref() == Some(page);
            panel.class_list().toggle_with_force("hidden", !show).ok();
        }
    }
}
fn slideshow() {
    let existing = APP.with(|a| a.borrow_mut().slide_timer.take());
    if let Some(timer) = existing {
        win().clear_interval_with_handle(timer);
        APP.with(|a| a.borrow_mut().slide_callback = None);
        return;
    }
    let callback = Closure::<dyn FnMut()>::new(|| move_by(1));
    let millis = saved("slideInterval", "3000").parse().unwrap_or(3000);
    if let Ok(timer) = win().set_interval_with_callback_and_timeout_and_arguments_0(
        callback.as_ref().unchecked_ref(),
        millis,
    ) {
        APP.with(|a| {
            let mut a = a.borrow_mut();
            a.slide_timer = Some(timer);
            a.slide_callback = Some(callback);
        });
    }
}
fn key(event: KeyboardEvent) {
    if el("settingsDialog")
        .unchecked_into::<HtmlDialogElement>()
        .open()
    {
        return;
    }
    let key = event.key().to_lowercase();
    let target = event.target().and_then(|e| e.dyn_into::<Element>().ok());
    if playback() && key == " " {
        event.prevent_default();
        if !event.repeat() {
            play_pause();
        }
        return;
    }
    if playback() && event.ctrl_key() {
        match key.as_str() {
            "arrowleft" => seek(-5.),
            "arrowright" => seek(5.),
            "arrowup" => volume(0.1),
            "arrowdown" => volume(-0.1),
            "m" => {
                video().set_muted(!video().muted());
                update_video();
                show_controls("volume", true);
            }
            _ => return,
        }
        event.prevent_default();
        return;
    }
    if target
        .as_ref()
        .is_some_and(|t| t.closest("input,select,textarea").ok().flatten().is_some())
    {
        return;
    }
    if event.ctrl_key() {
        match key.as_str() {
            "o" => {
                event.prevent_default();
                dialog(if event.shift_key() { "folder" } else { "file" });
            }
            "c" => export_image(false),
            "s" => export_image(true),
            _ => {}
        }
        return;
    }
    if event.alt_key() || event.meta_key() {
        return;
    }
    match key.as_str() {
        "arrowleft" | "pageup" => {
            event.prevent_default();
            move_by(-1);
        }
        "arrowright" | "pagedown" | " " => {
            event.prevent_default();
            move_by(1);
        }
        "+" | "=" => zoom(1.18),
        "-" => zoom(1. / 1.18),
        "0" => fit(),
        "1" => {
            APP.with(|a| {
                let mut a = a.borrow_mut();
                a.zoom = 1.;
                a.pan = (0., 0.);
                a.fit = false;
            });
            transform();
        }
        "r" => rotate(if event.shift_key() { -90 } else { 90 }),
        "f" => {
            APP.with(|a| a.borrow_mut().flip *= -1.);
            transform();
        }
        "i" => panel("info"),
        "t" => panel("thumbs"),
        "l" => panel("layers"),
        "s" => slideshow(),
        "enter" | "f11" => {
            event.prevent_default();
            fullscreen();
        }
        "escape" => {
            hidden("videoContextMenu", true);
            if APP.with(|a| a.borrow().fullscreen) {
                fullscreen();
            }
        }
        _ => {}
    }
}
fn initialize_runtime(runtime: Value) {
    text(
        "settingsVersion",
        &format!("Clip Image Viewer {}", s(&runtime, "version")),
    );
    text("associationHelp", &s(&runtime, "fileAssociationMessage"));
    let association_supported = runtime["associationSupported"].as_bool().unwrap_or(false);
    text(
        "associationHelp",
        if association_supported {
            "Windows 보안 정책상 기본 앱 지정은 Windows 설정에서 사용자가 직접 확인해야 합니다."
        } else {
            "포터블·개발 버전에서는 확장자 파일 연결을 지원하지 않습니다."
        },
    );
    hidden("associationControls", !association_supported);
    for (id, key) in [
        ("imageAssociationOptions", "optionalImageAssociations"),
        ("videoAssociationOptions", "optionalVideoAssociations"),
        ("audioAssociationOptions", "optionalAudioAssociations"),
    ] {
        if let Some(extensions) = runtime[key].as_array() {
            for extension in extensions {
                let label = doc().create_element("label").unwrap();
                let input: HtmlInputElement =
                    doc().create_element("input").unwrap().unchecked_into();
                input.set_type("checkbox");
                input.set_class_name("association-extension");
                input.set_value(extension.as_str().unwrap_or_default());
                label.append_child(&input).ok();
                let span = doc().create_element("span").unwrap();
                span.set_text_content(Some(
                    &extension
                        .as_str()
                        .unwrap_or_default()
                        .trim_start_matches('.')
                        .to_uppercase(),
                ));
                label.append_child(&span).ok();
                el(id).append_child(&label).ok();
            }
        }
    }
    apply_update(&runtime["updateState"]);
    let startup = s(&runtime, "startupPath");
    let smoke = runtime["smokeTest"].as_bool().unwrap_or(false);
    APP.with(|a| a.borrow_mut().runtime = runtime);
    if !startup.is_empty() {
        open(startup);
    }
    if smoke {
        diagnostic_poll(0);
    }
}
/// Mirrors the Electron `applyUpdateState`: status text, progress, and the restart button.
fn apply_update(update: &Value) {
    if update.is_null() { return; }
    let status = s(update, "status");
    let message = s(update, "message");
    text("updateStatus", if message.is_empty() { "업데이트 정보를 확인하는 중..." } else { &message });
    hidden("updateProgress", status != "downloading");
    el("updateProgress").set_attribute("value", &update["percent"].as_f64().unwrap_or(0.).to_string()).ok();
    let busy = matches!(status.as_str(), "checking" | "available" | "downloading") || status == "disabled";
    if busy { el("checkUpdateBtn").set_attribute("disabled", "").ok(); } else { el("checkUpdateBtn").remove_attribute("disabled").ok(); }
    hidden("restartUpdateBtn", status != "downloaded");
}
fn diagnostic_poll(attempt: u32) {
    later(200, move || {
        let state: Value = serde_json::from_str(&diagnostic_state()).unwrap_or(Value::Null);
        if state["loaded"].as_bool() == Some(true) || attempt >= 100 {
            if APP.with(|a| a.borrow().runtime["smokeLayer"].as_bool() == Some(true)) && attempt < 100 {
                if !APP.with(|a| a.borrow().layers) { panel("layers"); }
                fn eligible(layers: &Value) -> Option<(String, bool)> {
                    for layer in layers.as_array()? {
                        if let Some(found) = eligible(&layer["children"]) { return Some(found); }
                        if layer["toggleAvailable"] == true && s(layer, "type") != "group" {
                            return Some((s(layer, "id"), layer["visible"].as_bool().unwrap_or(true)));
                        }
                    }
                    None
                }
                let layer = APP.with(|a| eligible(&a.borrow().document["layers"]));
                spawn_local(async move {
                    let mut state: Value = serde_json::from_str(&diagnostic_state()).unwrap_or(Value::Null);
                    state["layerRows"] = json!(el("layerTree").query_selector_all(".layer-row").map(|rows| rows.length()).unwrap_or(0));
                    state["layerThumbnails"] = json!(el("layerTree").query_selector_all("img[data-layer-id]").map(|images| images.length()).unwrap_or(0));
                    if let Some((id, visible)) = layer {
                        let item = current();
                        let thumb = call("loadLayerThumbnail", vec![item.clone(), json!(id)]).await;
                        let mut visibility = APP.with(|a| a.borrow().visibility.clone());
                        visibility.insert(id, !visible);
                        let rendered = call("renderLayeredImage", vec![item, json!(visibility)]).await;
                        match (thumb, rendered) {
                            (Ok(thumb), Ok(image)) if thumb.as_str().is_some() && image["dataUrl"].as_str().is_some() => state["layerRendered"] = json!(true),
                            (Err(error), _) | (_, Err(error)) => { state["loaded"] = json!(false); state["error"] = json!(err(error)); },
                            _ => { state["loaded"] = json!(false); state["error"] = json!("레이어 결과가 비어 있습니다."); },
                        }
                    } else {
                        state["layerDocument"] = APP.with(|a| a.borrow().document.clone());
                        state["loaded"] = json!(false); state["error"] = json!("전환 가능한 레이어가 없습니다.");
                    }
                    command("reportDiagnostic", vec![state]);
                });
                return;
            }
            if APP.with(|a| a.borrow().runtime["smokeExport"].as_bool() == Some(true)) && attempt < 100 {
                APP.with(|a| { let mut a = a.borrow_mut(); a.rotation = 90; a.flip = -1.; });
                let mut state = state;
                match exported_png() {
                    Ok(png) if png.starts_with("data:image/png;base64,") => state["exportPng"] = json!(true),
                    Ok(_) => { state["loaded"] = json!(false); state["error"] = json!("Unexpected exported image format"); },
                    Err(error) => { state["loaded"] = json!(false); state["error"] = json!(err(error)); },
                }
                command("reportDiagnostic", vec![state]);
                return;
            }
            let thumbnail = APP.with(|a| a.borrow().runtime["smokeVideoThumbnail"].as_bool() == Some(true));
            if thumbnail && attempt < 100 {
                let item = current();
                spawn_local(async move {
                    let mut state: Value = serde_json::from_str(&diagnostic_state()).unwrap_or(Value::Null);
                    match video_thumbnail(item).await {
                        Ok(source) if source.starts_with("data:image/jpeg") => state["videoThumbnail"] = json!(true),
                        Ok(_) => { state["loaded"] = json!(false); state["error"] = json!("Unexpected thumbnail format"); },
                        Err(error) => { state["loaded"] = json!(false); state["error"] = json!(err(error)); },
                    }
                    command("reportDiagnostic", vec![state]);
                });
                return;
            }
            let drop_path = APP.with(|a| s(&a.borrow().runtime, "smokeDropPath"));
            if !drop_path.is_empty() && attempt < 100 {
                APP.with(|a| a.borrow_mut().runtime["smokeDropPath"] = Value::Null);
                spawn_local(async move {
                    match call("testDrop", vec![json!(drop_path.clone())]).await {
                        Ok(_) => diagnostic_drop_poll(drop_path, 0),
                        Err(error) => command("reportDiagnostic", vec![json!({"loaded":false,"error":err(error)})]),
                    }
                });
                return;
            }
            command("reportDiagnostic", vec![state]);
        } else {
            diagnostic_poll(attempt + 1);
        }
    });
}

fn diagnostic_drop_poll(path: String, attempt: u32) {
    later(200, move || {
        let mut state: Value = serde_json::from_str(&diagnostic_state()).unwrap_or(Value::Null);
        let matched = s(&current(), "path") == path;
        if (matched && state["loaded"].as_bool() == Some(true)) || attempt >= 100 {
            state["dropAccepted"] = json!(matched);
            if !matched { state["loaded"] = json!(false); state["error"] = json!("Dropped file did not open"); }
            command("reportDiagnostic", vec![state]);
        } else { diagnostic_drop_poll(path, attempt + 1); }
    });
}

#[wasm_bindgen]
pub fn diagnostic_state() -> String {
    APP.with(|a|{let a=a.borrow();let stage=el("stage").get_bounding_client_rect();let rect=if a.media=="image"{el("viewerImage").get_bounding_client_rect()}else{el("viewerVideo").get_bounding_client_rect()};
        json!({"runtime":"rust-wasm","items":a.items.len(),"index":a.index,"mediaType":a.media,
            "loaded":!a.source.is_empty()&&el("loading").class_list().contains("hidden")&&!el("imageLayer").class_list().contains("hidden"),
            "width":a.width,"height":a.height,"zoom":a.zoom,"panX":a.pan.0,"panY":a.pan.1,
            "centerXError":rect.left()+rect.width()/2.-stage.left()-stage.width()/2.,
            "centerYError":rect.top()+rect.height()/2.-stage.top()-stage.height()/2.,
            "stageWidth":stage.width(),"stageHeight":stage.height(),"playing":!video().paused(),
            "subtitleTracks":video().text_tracks().map_or(0, |tracks| tracks.length()),
            "subtitleShowing":video().text_tracks().and_then(|tracks| tracks.get(0)).is_some_and(|track| track.mode() == TextTrackMode::Showing),
            "subtitleReady":video().query_selector("track").ok().flatten().and_then(|track| track.dyn_into::<HtmlTrackElement>().ok()).map_or(0, |track| track.ready_state()),
            "error":el("errorDetail").text_content().unwrap_or_default()}).to_string()
    })
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    // The existing DOM, stylesheet, and icons are copied unchanged by the Rust build script.
    for id in ["openFileBtn", "emptyOpenFileBtn"] {
        click(id, || dialog("file"));
    }
    for id in ["openFolderBtn", "emptyOpenFolderBtn"] {
        click(id, || dialog("folder"));
    }
    for id in ["prevBtn", "prevOverlay"] {
        click(id, || move_by(-1));
    }
    for id in ["nextBtn", "nextOverlay", "skipFileBtn"] {
        click(id, || move_by(1));
    }
    click("retryFileBtn", || select(APP.with(|a| a.borrow().index)));
    click("fitBtn", fit);
    click("actualBtn", || {
        APP.with(|a| {
            let mut a = a.borrow_mut();
            a.fit = false;
            a.zoom = 1.;
            a.pan = (0., 0.);
        });
        transform();
    });
    click("zoomInBtn", || zoom(1.18));
    click("zoomOutBtn", || zoom(1. / 1.18));
    click("rotateLeftBtn", || rotate(-90));
    click("rotateRightBtn", || rotate(90));
    click("flipBtn", || {
        APP.with(|a| a.borrow_mut().flip *= -1.);
        transform();
    });
    click("fullscreenBtn", fullscreen);
    click("videoFullscreenBtn", fullscreen);
    click("pinBtn", || {
        let pinned = APP.with(|a| {
            let mut a = a.borrow_mut();
            a.pinned = !a.pinned;
            a.pinned
        });
        command("setAlwaysOnTop", vec![json!(pinned)]);
        cls("pinBtn", "active", pinned);
    });
    click("settingsBtn", settings);
    click("infoBtn", || panel("info"));
    click("closeInfoBtn", || panel("info"));
    click("thumbBtn", || panel("thumbs"));
    click("layerBtn", || panel("layers"));
    click("closeLayerBtn", || panel("layers"));
    on(el("thumbnailStrip").as_ref(), "click", |event| {
        if let Some(button) = event
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|t| t.closest("[data-index]").ok().flatten())
        {
            if let Some(index) = button
                .get_attribute("data-index")
                .and_then(|s| s.parse().ok())
            {
                select(index);
            }
        }
    });
    on(el("thumbnailStrip").as_ref(), "scroll", |_| {
        load_visible_thumbnails()
    });
    on(el("layerTree").as_ref(), "scroll", |_| {
        load_visible_layers()
    });
    on(el("layerTree").as_ref(), "click", |event| {
        let Some(target) = event.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
            return;
        };
        for attribute in [
            "data-layer-toggle",
            "data-layer-folder",
            "data-layer-select",
        ] {
            if let Some(button) = target.closest(&format!("[{attribute}]")).ok().flatten() {
                let id = button.get_attribute(attribute).unwrap_or_default();
                match attribute {
                    "data-layer-toggle" => toggle_layer(id),
                    "data-layer-folder" => {
                        APP.with(|a| {
                            let mut a = a.borrow_mut();
                            if !a.collapsed.remove(&id) {
                                a.collapsed.insert(id);
                            }
                        });
                        render_layers();
                    }
                    _ => layer_details(&id),
                }
                break;
            }
        }
    });
    click("slideshowBtn", slideshow);
    click("revealBtn", || command("showInFolder", vec![current()]));
    click("openOriginalBtn", || {
        command("openOriginal", vec![current()])
    });
    click("copyImageBtn", || export_image(false));
    click("saveImageBtn", || export_image(true));
    click("videoPlayBtn", play_pause);
    click("videoMuteBtn", || {
        video().set_muted(!video().muted());
        update_video();
    });
    click("videoSubtitleBtn", subtitles_toggle);
    on(video().as_ref(), "click", |event| {
        let e: MouseEvent = event.unchecked_into();
        APP.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(timer) = a.video_click.take() {
                clear_timer(timer);
            }
            if e.detail() == 1 {
                a.video_click = Some(later(220, play_pause));
            }
        });
    });
    on(el("videoControls").as_ref(), "mousemove", |event| {
        event.stop_propagation();
        show_controls("full", false);
    });
    on(el("stage").as_ref(), "contextmenu", |event| {
        if !playback() {
            return;
        }
        event.prevent_default();
        let e: MouseEvent = event.unchecked_into();
        hidden("videoContextMenu", false);
        show_controls("full", false);
        let stage = el("stage").get_bounding_client_rect();
        let menu = el("videoContextMenu").get_bounding_client_rect();
        style(
            "videoContextMenu",
            "left",
            &format!(
                "{}px",
                (e.client_x() as f64 - stage.left())
                    .clamp(0., (stage.width() - menu.width()).max(0.))
            ),
        );
        style(
            "videoContextMenu",
            "top",
            &format!(
                "{}px",
                (e.client_y() as f64 - stage.top())
                    .clamp(0., (stage.height() - menu.height()).max(0.))
            ),
        );
    });
    on(el("videoContextMenu").as_ref(), "click", |event| {
        let Some(button) = event
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|t| t.closest("button").ok().flatten())
        else {
            return;
        };
        if let Some(mode) = button.get_attribute("data-video-mode") {
            set_playback(mode);
        }
        if let Some(speed) = button
            .get_attribute("data-video-speed")
            .and_then(|s| s.parse().ok())
        {
            video().set_playback_rate(speed);
        }
        if let Some(action) = button.get_attribute("data-video-action") {
            match action.as_str() {
                "playPause" => play_pause(),
                "restart" => {
                    video().set_current_time(0.);
                    if let Ok(p) = video().play() {
                        spawn_local(async move {
                            let _ = JsFuture::from(p).await;
                        });
                    }
                }
                "seekBack" => seek(-5.),
                "seekForward" => seek(5.),
                "mute" => {
                    video().set_muted(!video().muted());
                    update_video();
                }
                "subtitles" => subtitles_toggle(),
                "fit" => fit(),
                "actual" => {
                    APP.with(|a| a.borrow_mut().zoom = 1.);
                    transform();
                }
                "fullscreen" => fullscreen(),
                "reveal" => command("showInFolder", vec![current()]),
                "openOriginal" => command("openOriginal", vec![current()]),
                _ => {}
            }
        }
        hidden("videoContextMenu", true);
    });
    on(win().as_ref(), "click", |event| {
        if event
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|t| t.closest(".video-context-menu").ok().flatten())
            .is_none()
        {
            hidden("videoContextMenu", true);
        }
    });
    click("videoModeBtn", || {
        let modes = ["sequential", "repeatAll", "repeatOne", "shuffle"];
        let next = APP.with(|a| {
            let a = a.borrow();
            (modes.iter().position(|m| *m == a.playback).unwrap_or(0) + 1) % 4
        });
        set_playback(modes[next].into());
    });
    click("checkUpdateBtn", || {
        spawn_local(async {
            match call("checkForUpdates", vec![]).await {
                Ok(result) => apply_update(&result),
                Err(e) => toast(&err(e)),
            }
        })
    });
    click("restartUpdateBtn", || command("restartAndUpdate", vec![]));
    on(el("wheelActionSelect").as_ref(), "change", |_| {
        let value = val("wheelActionSelect");
        save("wheelAction", &value);
        APP.with(|a| a.borrow_mut().wheel = value);
    });
    on(el("backgroundSelect").as_ref(), "change", |_| {
        let value = val("backgroundSelect");
        save("background", &value);
        background(&value);
    });
    on(el("slideIntervalSelect").as_ref(), "change", |_| {
        save("slideInterval", &val("slideIntervalSelect"))
    });
    on(el("loopCheckbox").as_ref(), "change", |_| {
        let enabled = el("loopCheckbox")
            .unchecked_into::<HtmlInputElement>()
            .checked();
        save("loop", &enabled.to_string());
        APP.with(|a| a.borrow_mut().loop_files = enabled);
    });
    on(el("cropModeSelect").as_ref(), "change", |_| {
        let mode = val("cropModeSelect");
        save("cropMode", &mode);
        APP.with(|a| a.borrow_mut().crop = mode);
        select(APP.with(|a| a.borrow().index));
    });
    for (id, page) in [
        ("settingsViewTab", "view"),
        ("settingsFilesTab", "files"),
        ("settingsUpdateTab", "update"),
    ] {
        click(id, move || settings_page(page));
    }
    for id in ["selectAllAssociationBtn", "clearOptionalAssociationBtn"] {
        click(id, move || {
            if let Ok(inputs) = doc().query_selector_all(".association-extension") {
                for i in 0..inputs.length() {
                    inputs
                        .item(i)
                        .unwrap()
                        .unchecked_into::<HtmlInputElement>()
                        .set_checked(id == "selectAllAssociationBtn");
                }
            }
        });
    }
    click("applyAssociationBtn", || {
        let mut extensions = vec![
            json!(".jpg"),
            json!(".jpeg"),
            json!(".png"),
            json!(".gif"),
            json!(".webp"),
            json!(".bmp"),
        ];
        if let Ok(inputs) = doc().query_selector_all(".association-extension:checked") {
            for i in 0..inputs.length() {
                extensions.push(json!(
                    inputs
                        .item(i)
                        .unwrap()
                        .unchecked_into::<HtmlInputElement>()
                        .value()
                ));
            }
        }
        command("registerAssociations", vec![json!(extensions), json!(true)]);
    });
    click("clearAssociationBtn", || {
        command("registerAssociations", vec![json!([]), json!(false)])
    });
    on(el("videoSeek").as_ref(), "input", |_| {
        let value = val("videoSeek").parse::<f64>().unwrap_or(0.);
        if video().duration().is_finite() {
            video().set_current_time(value / 1000. * video().duration());
        }
        show_controls("progress", true);
    });
    on(el("videoVolume").as_ref(), "input", |_| {
        let volume = val("videoVolume")
            .parse::<f64>()
            .unwrap_or(1.)
            .clamp(0., 1.);
        video().set_volume(volume);
        video().set_muted(volume == 0.);
        update_video();
        show_controls("volume", true);
    });
    on(el("videoSpeedSelect").as_ref(), "change", |_| {
        video().set_playback_rate(val("videoSpeedSelect").parse().unwrap_or(1.))
    });
    for name in [
        "timeupdate",
        "play",
        "pause",
        "durationchange",
        "volumechange",
        "ratechange",
    ] {
        on(video().as_ref(), name, |_| update_video());
    }
    on(video().as_ref(), "loadedmetadata", |_| {
        APP.with(|a| {
            let mut a = a.borrow_mut();
            if a.media == "video" {
                a.width = video().video_width() as f64;
                a.height = video().video_height() as f64;
            } else {
                a.width = 0.;
                a.height = 0.;
            }
        });
        fit();
        update();
    });
    on(video().as_ref(), "error", |_| {
        if playback() {
            failure(&current(), "미디어 파일 또는 코덱을 재생할 수 없습니다.");
        }
    });
    on(video().as_ref(), "ended", |_| ended());
    for id in ["videoSeek", "videoVolume"] {
        on(el(id).as_ref(), "pointerdown", |_| {
            APP.with(|a| a.borrow_mut().interacting = true)
        });
    }
    on(win().as_ref(), "pointerup", |_| {
        APP.with(|a| a.borrow_mut().interacting = false);
        if playback() {
            show_controls("full", false);
        }
    });
    on(el("stage").as_ref(), "wheel", |event| {
        let e: WheelEvent = event.unchecked_into();
        if e.delta_y() == 0. || APP.with(|a| a.borrow().source.is_empty()) {
            return;
        }
        if e.target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|t| t.closest(".video-controls").ok().flatten())
            .is_some()
        {
            return;
        }
        e.prevent_default();
        APP.with(|a| a.borrow_mut().pointer = Some((e.client_x() as f64, e.client_y() as f64)));
        if APP.with(|a| a.borrow().wheel == "navigate") {
            move_by(if e.delta_y() > 0. { 1 } else { -1 });
        } else {
            zoom(if e.delta_y() < 0. { 1.12 } else { 1. / 1.12 });
        }
    });
    on(el("stage").as_ref(), "mousemove", |event| {
        let e: MouseEvent = event.unchecked_into();
        APP.with(|a| {
            let mut a = a.borrow_mut();
            a.pointer = Some((e.client_x() as f64, e.client_y() as f64));
            if let Some((x, y, px, py)) = a.drag {
                a.pan = (px + e.client_x() as f64 - x, py + e.client_y() as f64 - y);
            }
        });
        if playback() {
            show_controls("full", false);
        } else {
            transform();
        }
    });
    on(el("stage").as_ref(), "mouseleave", |_| {
        APP.with(|a| a.borrow_mut().pointer = None)
    });
    on(el("imageLayer").as_ref(), "mousedown", |event| {
        let e: MouseEvent = event.unchecked_into();
        if e.button() != 0 || playback() {
            return;
        }
        if e.target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|t| t.closest("button,input,select").ok().flatten())
            .is_some()
        {
            return;
        }
        if e.ctrl_key() {
            e.prevent_default();
            pick_layer(&e);
            return;
        }
        APP.with(|a| {
            let mut a = a.borrow_mut();
            a.drag = Some((e.client_x() as f64, e.client_y() as f64, a.pan.0, a.pan.1));
        });
    });
    on(win().as_ref(), "mouseup", |_| {
        APP.with(|a| a.borrow_mut().drag = None)
    });
    on(win().as_ref(), "keydown", |event| {
        key(event.unchecked_into())
    });
    on(win().as_ref(), "resize", |_| {
        if APP.with(|a| a.borrow().fit) {
            fit();
        }
    });
    on(el("stage").as_ref(), "dblclick", |event| {
        if event
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .and_then(|t| {
                t.closest("button,input,select,.video-controls")
                    .ok()
                    .flatten()
            })
            .is_none()
        {
            APP.with(|a| {
                if let Some(timer) = a.borrow_mut().video_click.take() {
                    clear_timer(timer);
                }
            });
            fullscreen();
        }
    });
    background(&saved("background", "dark"));
    listen("open-external-path", |path| {
        if let Some(path) = path.as_str() {
            open(path.into());
        }
    });
    listen("update-state", |update| apply_update(&update));
    listen("fullscreen-state", |enabled| {
        if let Some(enabled) = enabled.as_bool() {
            APP.with(|a| a.borrow_mut().fullscreen = enabled);
            doc()
                .body()
                .unwrap()
                .class_list()
                .toggle_with_force("fullscreen-ui", enabled)
                .ok();
            later(0, fit);
        }
    });
    spawn_local(async {
        match call("getRuntimeInfo", vec![]).await {
            Ok(runtime) => initialize_runtime(runtime),
            Err(e) => toast(&err(e)),
        }
    });
    Ok(())
}
