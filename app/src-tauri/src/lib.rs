use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

mod script;
mod ppt;

// ---------------------------------------------------------------- 数据模型

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub font_size: u32,
    pub en_size: u32,
    pub font_family: String,
    pub fg: String,
    pub bg_color: String,
    pub bg_alpha: u32,
    pub radius: u32,
    pub padding: u32,
    pub bottom_ratio: f64,
    pub width_ratio: f64,
    pub lines: u32,
    pub advance: String, // "scroll" 整段滚动 / "step" 逐条
    pub scroll_speed: f64,
    pub scroll_delay: u64,
    pub stay_at_end: bool,
    pub layout: String,  // "overlay" 覆盖 / "dock" 贴边
    pub dock_side: String,
    pub dock_ratio: f64,
    pub ppt_resize: bool,
    pub show_en: bool,
    pub click_through: bool,
    pub always_on_top: bool,
    pub screen: String,
    pub follow_ppt: bool,
    pub minimize_to_tray: bool,
    pub align: String, // UI 额外项（文字对齐），PRD 5.0 之外
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 36,
            en_size: 24,
            font_family: "Microsoft YaHei".into(),
            fg: "#FFFFFF".into(),
            bg_color: "#111318".into(),
            bg_alpha: 235,
            radius: 14,
            padding: 18,
            bottom_ratio: 0.055,
            width_ratio: 0.86,
            lines: 1,
            advance: "scroll".into(),
            scroll_speed: 0.5,
            scroll_delay: 1000,
            stay_at_end: true,
            layout: "overlay".into(),
            dock_side: "top".into(),
            dock_ratio: 0.18,
            ppt_resize: true,
            show_en: false,
            click_through: true,
            always_on_top: true,
            screen: "auto".into(),
            follow_ppt: true,
            minimize_to_tray: true,
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

/// 校验字幕稿（PRD 5.1 格式），返回条数/页数摘要；格式错误时报"第 N 行：原因"
#[tauri::command]
fn validate_script(text: String) -> Result<script::ScriptSummary, String> {
    let cues = script::parse_script(&text).map_err(|e| format!("第 {} 行：{}", e.line_no, e.message))?;
    Ok(script::summarize(&cues))
}

/// 导入预览图目录：沙箱/开发用仓库内，正常 release 用应用数据目录
fn imports_dir(app: &AppHandle) -> PathBuf {
    let base = dev_webview_data_dir()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| data_dir(app));
    let d = base.join("imports");
    let _ = fs::create_dir_all(&d);
    d
}

/// 导入 pptx：骨架 + 每页 PNG（R9）。COM 调用放独立线程（自己的 STA）
#[tauri::command]
fn import_pptx(app: AppHandle, path: String) -> Result<ppt::ImportResult, String> {
    let dir = imports_dir(&app);
    std::thread::spawn(move || ppt::import_pptx_blocking(&path, &dir))
        .join()
        .map_err(|_| "导入线程崩溃".to_string())?
}

/// 读译稿文本文件（UTF-8 宽松解码）
#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    let bytes = fs::read(&path).map_err(|e| format!("读不了 {}: {e}", path))?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// 预览排练（R14）：幻灯片预览窗，最大化、无边框，放导入的页面图片
#[tauri::command]
fn open_preview(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("preview") {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    let mut b = WebviewWindowBuilder::new(&app, "preview", WebviewUrl::App("preview.html".into()))
        .title("slide-preview")
        .decorations(false)
        .maximized(true);
    if let Some(dir) = dev_webview_data_dir() {
        b = b.data_directory(dir);
    }
    b.build().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn close_preview(app: AppHandle) {
    if let Some(w) = app.get_webview_window("preview") {
        let _ = w.close();
    }
}

/// 预览图：imports/slide-NN.png → data URL
#[tauri::command]
fn read_preview_image(app: AppHandle, page: u32) -> Option<String> {
    let path = imports_dir(&app).join(format!("slide-{:02}.png", page));
    let bytes = fs::read(path).ok()?;
    use base64::Engine;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
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
    let lines = settings.lines.clamp(1, 5) as f64;
    let base = settings.padding as f64 * 2.0 + lines * settings.font_size as f64 * 1.5;
    let extra = if settings.show_en {
        settings.en_size as f64 * 1.6
    } else {
        0.0
    };
    ((base + extra) * scale).round().max(60.0) as u32
}

fn place_overlay(app: &AppHandle, settings: &Settings) {
    let Some(win) = app.get_webview_window("overlay") else {
        return;
    };
    if settings.layout == "dock" {
        return; // 贴边模式的几何由 apply_dock 负责
    }
    // screen:auto 且 PPT 在放映 → 跟随放映窗所在的屏（R12，旧原型 screenClass 方案）
    let pick = settings.screen.trim();
    let auto = pick.is_empty() || pick.eq_ignore_ascii_case("auto") || pick.eq_ignore_ascii_case("ppt");
    if auto {
        let show = ppt::PPT_SHOW.lock().unwrap();
        if let Some(r) = *show {
            let mons = win.available_monitors().unwrap_or_default();
            let scale = mons
                .iter()
                .find(|m| m.position().x == r.x && m.position().y == r.y)
                .map(|m| m.scale_factor())
                .unwrap_or(1.0);
            let width = (r.w as f64 * settings.width_ratio).round() as u32;
            let height = overlay_height(settings, scale);
            let x = r.x + ((r.w as i64 - width as i64) / 2) as i32;
            let y = r.y + (r.h as f64 * (1.0 - settings.bottom_ratio)).round() as i32 - height as i32;
            let _ = win.set_size(PhysicalSize::new(width.max(200), height));
            let _ = win.set_position(PhysicalPosition::new(x, y));
            return;
        }
    }
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
    *LAST_LAYOUT.lock().unwrap() = settings.layout.clone();
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.set_always_on_top(settings.always_on_top);
        let _ = win.set_ignore_cursor_events(settings.click_through);
    }
    place_overlay(app, settings);
    // 贴边布局：缩目标窗（放映窗/预览窗）让位，字幕条贴边整条（R16，路线①）
    if let Some(bar) = ppt::apply_dock(app, settings) {
        if let Some(win) = app.get_webview_window("overlay") {
            let _ = win.set_size(PhysicalSize::new(bar.2.max(200) as u32, bar.3.max(60) as u32));
            let _ = win.set_position(PhysicalPosition::new(bar.0, bar.1));
        }
    }
}

/// 最近一次应用的布局（set_overlay_height 在贴边模式下不自行改高度）
static LAST_LAYOUT: Mutex<String> = Mutex::new(String::new());
use std::sync::Mutex;

// ---------------------------------------------------------------- 窗口创建

/// dev 构建把 WebView2 数据目录指到仓库内（DSH 沙箱拒写 AppData\Local，
/// 否则 create_dir_all(AppData\...) 直接 os error 5 崩在启动）。
/// release 在正常机器上跑，保持默认（AppData）；
/// 也可用环境变量 DESKTOP_SUBTITLE_DATA_DIR 强制指定（沙箱里测 release 用）。
fn dev_webview_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("DESKTOP_SUBTITLE_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(debug_assertions)]
    {
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.webview2-data"))
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn build_main_window(app: &AppHandle) -> tauri::Result<()> {
    let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("字幕控制台 · Windows Subtitle")
        .inner_size(1040.0, 760.0)
        .min_inner_size(880.0, 600.0)
        .resizable(true)
        .center();
    if let Some(dir) = dev_webview_data_dir() {
        b = b.data_directory(dir);
    }
    b.build()?;
    Ok(())
}

#[tauri::command]
fn ensure_overlay(app: AppHandle, settings: Settings) -> Result<(), String> {
    if app.get_webview_window("overlay").is_some() {
        sync_window_flags(&app, &settings);
        return Ok(());
    }
    let mut b = WebviewWindowBuilder::new(&app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("subtitle-overlay")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(settings.always_on_top)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false);
    if let Some(dir) = dev_webview_data_dir() {
        b = b.data_directory(dir);
    }
    let win = b.build().map_err(|e| e.to_string())?;
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

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PageCue {
    pub zh: String,
    pub en: Option<String>,
}

/// 把当前页的字幕段推给 overlay（滚动/逐条的播放状态机都在 overlay 里，PRD 6.2）
#[tauri::command]
fn push_page(app: AppHandle, page: u32, cues: Vec<PageCue>, settings: Settings) {
    if let Some(win) = app.get_webview_window("overlay") {
        let payload = serde_json::json!({ "page": page, "cues": cues });
        let _ = win.emit("page", payload);
        let _ = win.emit("style", settings);
    }
    // 预览排练窗同步翻页（R14）
    if let Some(pw) = app.get_webview_window("preview") {
        let _ = pw.emit("preview-page", page);
    }
}

/// 主界面按钮 → overlay 控制命令（step ±1 / restart / pause 等）
#[tauri::command]
fn send_ctrl(app: AppHandle, kind: String, arg: i32) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.emit("ctrl", serde_json::json!({ "kind": kind, "arg": arg }));
    }
}

/// overlay 内容折行超行时自报高度，窗口跟着长（R18，封顶逻辑在 JS 侧）
/// 贴边模式下高度由 dockRatio 决定，不自行调整
#[tauri::command]
fn set_overlay_height(app: AppHandle, height: u32) {
    if *LAST_LAYOUT.lock().unwrap() == "dock" {
        return;
    }
    if let Some(win) = app.get_webview_window("overlay") {
        let w = win.inner_size().map(|s| s.width).unwrap_or(800);
        let _ = win.set_size(PhysicalSize::new(w, height.clamp(60, 1200)));
    }
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
        (Shortcut::new(Some(ctrl_alt), Code::KeyX), "hk:passthrough"),
        (Shortcut::new(Some(ctrl_alt), Code::Space), "hk:pause"),
        (Shortcut::new(Some(ctrl_alt), Code::ArrowUp), "hk:nudge-up"),
        (Shortcut::new(Some(ctrl_alt), Code::ArrowDown), "hk:nudge-down"),
        (Shortcut::new(Some(ctrl_alt), Code::ArrowRight), "hk:page-next"),
        (Shortcut::new(Some(ctrl_alt), Code::ArrowLeft), "hk:page-prev"),
        (Shortcut::new(Some(ctrl_alt), Code::KeyR), "hk:restart"),
        (Shortcut::new(Some(ctrl_alt), Code::Equal), "hk:font-up"),
        (Shortcut::new(Some(ctrl_alt), Code::Minus), "hk:font-down"),
    ]
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
            // 窗口在代码里创建（dev 时 WebView2 数据目录指到仓库内，绕开沙箱 AppData 限制）
            build_main_window(app.handle())?;
            // PowerPoint COM 轮询（R4 跟随翻页 / R12 多屏跟随）
            ppt::start_polling(app.handle().clone());

            // S1 自动化实测用：--dev-show-overlay 启动即创建并显示字幕条
            #[cfg(debug_assertions)]
            if std::env::args().any(|a| a == "--dev-show-overlay") {
                let settings = Settings::default();
                if let Err(e) = ensure_overlay(app.handle().clone(), settings.clone()) {
                    eprintln!("[dev-show-overlay] {e}");
                }
                if let Some(win) = app.get_webview_window("overlay") {
                    let _ = win.show();
                    sync_window_flags(app.handle(), &settings);
                }
            }
            // S7 自动化验证用：--dev-import <pptx> 启动即导入并打印结果
            #[cfg(debug_assertions)]
            if let Some(path) = std::env::args().find(|a| a.ends_with(".pptx")) {
                let handle = app.handle().clone();
                let dir = imports_dir(&handle);
                std::thread::spawn(move || match ppt::import_pptx_blocking(&path, &dir) {
                    Ok(r) => eprintln!(
                        "[dev-import] pages={} titles={:?} dir={} script:\n{}",
                        r.pages,
                        r.titles.iter().map(|t| t.chars().take(12).collect::<String>()).collect::<Vec<_>>(),
                        r.dir,
                        r.script
                    ),
                    Err(e) => eprintln!("[dev-import] FAILED: {e}"),
                });
            }
            let handle = app.handle().clone();
            for (sc, _) in hotkey_specs() {
                if let Err(e) = handle.global_shortcut().register(sc) {
                    eprintln!("[hotkey] 注册失败: {e}");
                }
            }

            // 托盘（R21）：最小化不占任务栏，菜单召回/退出
            {
                use tauri::menu::{Menu, MenuItem};
                let show = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &quit])?;
                tauri::tray::TrayIconBuilder::with_id("main-tray")
                    .icon(app.default_window_icon().cloned().unwrap())
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let tauri::tray::TrayIconEvent::Click {
                            button: tauri::tray::MouseButton::Left,
                            button_state: tauri::tray::MouseButtonState::Up,
                            ..
                        } = event
                        {
                            let app = tray.app_handle().clone();
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    })
                    .build(app)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口关闭 → 按 minimizeToTray 决定收托盘还是真退出（R7/R21）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let app = window.app_handle().clone();
                    let minimize = load_store(app).settings.minimize_to_tray;
                    if minimize {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_store,
            save_store,
            store_location,
            validate_script,
            import_pptx,
            read_text_file,
            open_preview,
            close_preview,
            read_preview_image,
            ensure_overlay,
            show_overlay,
            hide_overlay,
            overlay_visible,
            push_page,
            send_ctrl,
            set_overlay_height,
            push_style,
            apply_style,
            toggle_console,
            quit_app,
            list_monitors
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
