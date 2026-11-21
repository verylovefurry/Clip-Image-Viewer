use std::{fs, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Bounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    bounds: Bounds,
    is_maximized: bool,
}

#[derive(Default)]
pub struct WindowState {
    normal: Mutex<Option<Bounds>>,
}

fn state_path(window: &WebviewWindow) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLIPVIEW_USER_DATA_DIR") {
        return Some(PathBuf::from(dir).join("rust-window-state.json"));
    }
    Some(window.path().app_config_dir().ok()?.join("window-state.json"))
}

fn electron_state_path(_window: &WebviewWindow) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLIPVIEW_USER_DATA_DIR") {
        return Some(PathBuf::from(dir).join("window-state.json"));
    }
    #[cfg(windows)]
    {
        return Some(PathBuf::from(std::env::var_os("APPDATA")?)
            .join("clip-image-viewer")
            .join("window-state.json"));
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn read_saved_state(window: &WebviewWindow) -> Option<(Saved, bool)> {
    let destination = state_path(window)?;
    if let Ok(data) = fs::read(&destination) {
        if let Some(saved) = parse_saved_state(&data) {
            return Some((saved, false));
        }
    }

    let legacy = electron_state_path(window)?;
    // The override case can place both paths in the same directory, but the
    // legacy file remains read-only and is never used as the Rust save target.
    if legacy == destination {
        return None;
    }
    fs::read(legacy).ok().and_then(|data| parse_saved_state(&data)).map(|saved| (saved, true))
}

fn parse_saved_state(data: &[u8]) -> Option<Saved> {
    let saved: Saved = serde_json::from_slice(data).ok()?;
    let b = saved.bounds;
    (b.width > 0 && b.height > 0).then_some(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_electron_window_state_shape() {
        let saved = parse_saved_state(
            br#"{"bounds":{"x":-1200,"y":40,"width":900,"height":700},"isMaximized":true}"#,
        )
        .unwrap();
        assert_eq!(saved.bounds.x, -1200);
        assert_eq!(saved.bounds.width, 900);
        assert!(saved.is_maximized);
    }

    #[test]
    fn rejects_damaged_or_incomplete_state() {
        assert!(parse_saved_state(b"{").is_none());
        assert!(parse_saved_state(br#"{"bounds":{"x":0,"y":0,"width":0,"height":10},"isMaximized":false}"#).is_none());
        assert!(parse_saved_state(br#"{"bounds":{"x":0,"y":0,"width":10,"height":10}}"#).is_none());
    }
}

impl WindowState {
    pub fn restore(&self, window: &WebviewWindow) {
        let saved = read_saved_state(window).map(|(mut saved, legacy)| {
            // Electron stores device-independent pixels; Rust state stores physical pixels.
            if legacy {
                if let Ok(Some(monitor)) = window.primary_monitor() {
                    let scale = monitor.scale_factor();
                    saved.bounds.x = (saved.bounds.x as f64 * scale).round() as i32;
                    saved.bounds.y = (saved.bounds.y as f64 * scale).round() as i32;
                    saved.bounds.width = (saved.bounds.width as f64 * scale).round() as u32;
                    saved.bounds.height = (saved.bounds.height as f64 * scale).round() as u32;
                }
            }
            saved
        });
        let monitor = saved.as_ref().and_then(|s| {
            let b = s.bounds;
            window.monitor_from_point(b.x as f64 + b.width as f64 / 2., b.y as f64 + b.height as f64 / 2.)
                .ok().flatten()
        }).or_else(|| window.primary_monitor().ok().flatten());
        let Some(monitor) = monitor else { return };
        let work = monitor.work_area();
        let (w, h) = (work.size.width, work.size.height);
        let dpi = monitor.scale_factor();
        let from_saved = saved.as_ref().map(|saved| saved.bounds);
        let min = (320. * dpi).round() as u32;
        let (width, height) = if let Some(bounds) = from_saved {
            (bounds.width.clamp(min.min(w), w), bounds.height.clamp(min.min(h), h))
        } else {
            let scale = dpi.min(w as f64 / 720.).min(h as f64 / 960.);
            ((720. * scale).round() as u32, (960. * scale).round() as u32)
        };
        let x = work.position.x;
        let y = work.position.y;
        let left = from_saved.map_or(x + (w - width) as i32 / 2, |b| b.x).clamp(x, x + (w - width) as i32);
        let top = from_saved.map_or(y + (h - height) as i32 / 2, |b| b.y).clamp(y, y + (h - height) as i32);
        let inner = window.inner_size().ok();
        let outer = window.outer_size().ok();
        let (pad_w, pad_h) = inner.zip(outer).map_or((0, 0), |(i, o)| (
            o.width.saturating_sub(i.width), o.height.saturating_sub(i.height),
        ));
        let _ = window.set_size(PhysicalSize::new(width.saturating_sub(pad_w), height.saturating_sub(pad_h)));
        let _ = window.set_position(PhysicalPosition::new(left, top));
        self.record(window);
        if saved.is_some_and(|s| s.is_maximized) { let _ = window.maximize(); }
    }

    pub fn record(&self, window: &WebviewWindow) {
        if window.is_minimized().unwrap_or(false) || window.is_maximized().unwrap_or(false)
            || window.is_fullscreen().unwrap_or(false) { return; }
        if let (Ok(p), Ok(s), Ok(mut normal)) = (window.outer_position(), window.outer_size(), self.normal.lock()) {
            *normal = Some(Bounds { x: p.x, y: p.y, width: s.width, height: s.height });
        }
    }

    pub fn save(&self, window: &WebviewWindow) {
        self.record(window);
        let Ok(normal) = self.normal.lock() else { return };
        let (Some(path), Some(bounds)) = (state_path(window), *normal) else { return };
        let Some(dir) = path.parent() else { return };
        let saved = Saved { bounds, is_maximized: window.is_maximized().unwrap_or(false) };
        let result = (|| -> crate::Result<()> {
            fs::create_dir_all(dir).map_err(crate::error)?;
            let mut file = tempfile::NamedTempFile::new_in(dir).map_err(crate::error)?;
            serde_json::to_writer(&mut file, &saved).map_err(crate::error)?;
            file.persist(&path).map_err(crate::error)?;
            Ok(())
        })();
        if let Err(e) = result { eprintln!("창 상태를 저장하지 못했습니다: {e}"); }
    }
}
