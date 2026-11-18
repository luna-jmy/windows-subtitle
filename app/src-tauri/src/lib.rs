use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

// ---------------------------------------------------------------- 数据模型

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub font_size: u32,
    pub en_size: u32,
    pub bottom_ratio: f64,
    pub width_ratio: f64,
    pub bg_alpha: u32,
    pub show_en: bool,
    pub click_through: bool,
    pub always_on_top: bool,
    pub screen: String,
    pub follow_ppt: bool,
    pub align: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 40,
            en_size: 24,
            bottom_ratio: 0.08,
            width_ratio: 0.86,
            bg_alpha: 232,
            show_en: false,
            click_through: true,
            always_on_top: true,
            screen: "auto".into(),
            follow_ppt: true,
            align: "center".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Store {
    pub settings: Settings,
    pub script: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct CuePayload {
    pub zh: String,
    pub en: String,
    pub index: usize,
    pub total: usize,
    pub page: u32,
    pub visible: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct MonitorInfo {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub primary: bool,
}

const SAMPLE_SCRIPT: &str = "=== 1 ===
各位同事，早上好。感谢大家在百忙之中参加今天的沟通会。
今天我想用二十分钟，讲三件事。

=== 2 ===
第一件事：我们今年走到了哪一步。

=== 3 ===
第二件事：接下来三个季度，公司最看重的三项工作。
";

// ---------------------------------------------------------------- 存储

fn data_dir(app: &AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn store_path(app: &AppHandle) -> PathBuf {
    data_dir(app).join("store.json")
}

#[tauri::command]
fn load_store(app: AppHandle) -> Store {
    let path = store_path(&app);
    let mut store = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Store>(&text).ok())
        .unwrap_or_default();
    if store.script.trim().is_empty() {
        store.script = SAMPLE_SCRIPT.to_string();
    }
    store
}

#[tauri::command]
fn save_store(app: AppHandle, store: Store) -> Result<(), String> {
    let path = store_path(&app);
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())
}

#[tauri::command]
fn store_location(app: AppHandle) -> String {
    store_path(&app).to_string_lossy().to_string()
}

// ---------------------------------------------------------------- 浮层窗口

fn target_monitor(
    app: &AppHandle,
    settings: &Settings,
) -> Option<tauri::Monitor> {
    let main = app.get_webview_window("main");
    let monitors: Vec<tauri::Monitor> = match &main {
        Some(w) => w.available_monitors().unwrap_or_default(),
        None => return None,
    };
    if monitors.is_empty() {
        return main.and_then(|w| w.primary_monitor().ok().flatten());
    }
    let pick = settings.screen.trim();
    if pick.is_empty() || pick.eq_ignore_ascii_case("auto") || pick.eq_ignore_ascii_case("ppt") {
        return main
            .and_then(|w| w.primary_monitor().ok().flatten())
            .or_else(|| monitors.first().cloned());
    }
    if let Ok(idx) = pick.parse::<usize>() {
        return monitors.get(idx.saturating_sub(1)).cloned().or_else(|| monitors.first().cloned());
    }
    monitors.first().cloned()
}

fn overlay_height(settings: &Settings, scale: f64) -> u32 {
    let base = settings.font_size as f64 * 1.9 + 56.0;
    let extra = if settings.show_en {
        settings.en_size as f64 * 1.7
    } else {
        0.0
    };
    ((base + extra) * scale).round().max(60.0) as u32
}

fn place_overlay(app: &AppHandle, settings: &Settings) {
    let Some(win) = app.get_webview_window("overlay") else {
        return;
    };
    let Some(mon) = target_monitor(app, settings) else {
        return;
    };
    let scale = mon.scale_factor();
    let size = mon.size();
    let pos = mon.position();
    let width = (size.width as f64 * settings.width_ratio).round() as u32;
    let height = overlay_height(settings, scale);
    let x = pos.x + ((size.width as i64 - width as i64) / 2) as i32;
    let y = pos.y + (size.height as f64 * (1.0 - settings.bottom_ratio)).round() as i32 - height as i32;
    let _ = win.set_size(PhysicalSize::new(width.max(200), height));
    let _ = win.set_position(PhysicalPosition::new(x, y));
}

fn sync_window_flags(app: &AppHandle, settings: &Settings) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.set_always_on_top(settings.always_on_top);
        let _ = win.set_ignore_cursor_events(settings.click_through);
    }
    place_overlay(app, settings);
}

#[tauri::command]
fn ensure_overlay(app: AppHandle, settings: Settings) -> Result<(), String> {
    if app.get_webview_window("overlay").is_some() {
        sync_window_flags(&app, &settings);
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("subtitle-overlay")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(settings.always_on_top)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = win.set_ignore_cursor_events(settings.click_through);
    sync_window_flags(&app, &settings);
    Ok(())
}

#[tauri::command]
fn show_overlay(app: AppHandle, settings: Settings) -> Result<(), String> {
    ensure_overlay(app.clone(), settings.clone())?;
    if let Some(win) = app.get_webview_window("overlay") {
        win.show().map_err(|e| e.to_string())?;
        sync_window_flags(&app, &settings);
    }
    Ok(())
}

#[tauri::command]
fn hide_overlay(app: AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.hide();
    }
}

#[tauri::command]
fn overlay_visible(app: AppHandle) -> bool {
    app.get_webview_window("overlay")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

#[tauri::command]
fn push_cue(app: AppHandle, payload: CuePayload) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.emit("cue", payload.clone());
    }
    let _ = app.emit("cue-mirror", payload);
}

#[tauri::command]
fn push_style(app: AppHandle, settings: Settings) {
    let _ = app.emit("style", settings.clone());
    sync_window_flags(&app, &settings);
}

#[tauri::command]
fn apply_style(app: AppHandle, settings: Settings) {
    sync_window_flags(&app, &settings);
    let _ = app.emit("style", settings);
}

#[tauri::command]
fn toggle_console(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(true);
        if visible {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn list_monitors(app: AppHandle) -> Vec<MonitorInfo> {
    let Some(win) = app.get_webview_window("main") else {
        return vec![];
    };
    let primary = win.primary_monitor().ok().flatten();
    let primary_pos = primary.as_ref().map(|m| m.position().clone());
    let mut out = vec![];
    for (i, m) in win.available_monitors().unwrap_or_default().iter().enumerate() {
        out.push(MonitorInfo {
            index: i as u32 + 1,
            name: m.name().cloned().unwrap_or_else(|| format!("显示器 {}", i + 1)),
            width: m.size().width,
            height: m.size().height,
            x: m.position().x,
            y: m.position().y,
            scale: m.scale_factor(),
            primary: primary_pos.as_ref().map(|p| p == m.position()).unwrap_or(false),
        });
    }
    out
}

// ---------------------------------------------------------------- 全局热键

fn hotkey_specs() -> Vec<(Shortcut, &'static str)> {
    let ctrl_alt = Modifiers::CONTROL | Modifiers::ALT;
    vec![
        (Shortcut::new(None, Code::F9), "hk:next"),
        (Shortcut::new(None, Code::F8), "hk:prev"),
        (Shortcut::new(None, Code::F7), "hk:toggle"),
        (Shortcut::new(None, Code::F6), "hk:en"),
        (Shortcut::new(Some(ctrl_alt), Code::KeyQ), "hk:quit"),
        (Shortcut::new(Some(ctrl_alt), Code::KeyP), "hk:panel"),
        (Shortcut::new(Some(ctrl_alt), Code::Equal), "hk:font-up"),
        (Shortcut::new(Some(ctrl_alt), Code::Minus), "hk:font-down"),
    ]
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    for (sc, name) in hotkey_specs() {
                        if sc.key == shortcut.key && sc.mods == shortcut.mods {
                            let _ = app.emit(name, ());
                            break;
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            for (sc, _) in hotkey_specs() {
                if let Err(e) = handle.global_shortcut().register(sc) {
                    eprintln!("[hotkey] 注册失败: {e}");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_store,
            save_store,
            store_location,
            ensure_overlay,
            show_overlay,
            hide_overlay,
            overlay_visible,
            push_cue,
            push_style,
            apply_style,
            toggle_console,
            quit_app,
            list_monitors
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
