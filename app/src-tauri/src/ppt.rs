// PowerPoint COM 轮询（PRD 4.4：250ms 拉 SlideIndex 比事件订阅稳）
// 独立 STA 线程：GetActiveObject 附着运行中的 PowerPoint，读放映状态 + screenClass 定位
#![allow(dead_code)]

use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use windows::core::{GUID, Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IDispatch, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT,
    DISPPARAMS, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Ole::GetActiveObject;
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_BSTR, VT_DISPATCH, VT_I4};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowRect, IsWindowVisible,
};

/// 放映窗所在显示器矩形（物理像素）；screen:auto 时 overlay 跟过去（R12）
#[derive(Clone, Copy, Debug)]
pub struct ShowRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub static PPT_SHOW: Mutex<Option<ShowRect>> = Mutex::new(None);
/// 最近导入/打开的 pptx（「打开 PPT」/「断开挂载」用）
pub static LAST_PPTX: Mutex<Option<String>> = Mutex::new(None);
static DETACH_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 断开挂载：轮询线程下个周期关掉我们挂的 deck、停止跟随
pub fn request_detach() {
    DETACH_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn get_id(disp: &IDispatch, name: &str) -> windows::core::Result<i32> { unsafe {
    let w = wide(name);
    let names = [PCWSTR(w.as_ptr())];
    let mut ids = [0i32];
    disp.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0x0409, ids.as_mut_ptr())?;
    Ok(ids[0])
}}

/// 晚绑定调用/取属性：args 为位置参数（自动按 Invoke 要求倒序）
unsafe fn invoke(
    disp: &IDispatch,
    name: &str,
    args: &[VARIANT],
    _want_result: bool,
) -> windows::core::Result<VARIANT> { unsafe {
    let id = get_id(disp, name)?;
    let mut store: Vec<VARIANT> = args.to_vec();
    store.reverse();
    let mut dp = DISPPARAMS::default();
    if !store.is_empty() {
        dp.rgvarg = store.as_mut_ptr();
        dp.cArgs = store.len() as u32;
    }
    let flags = if args.is_empty() {
        DISPATCH_PROPERTYGET
    } else {
        // 带参数 = 方法调用；混入 PROPERTYGET 会让部分 COM 服务端（如 PowerPoint）抛 0x80020009
        DISPATCH_FLAGS(DISPATCH_METHOD.0)
    };
    let mut result = VARIANT::default();
    disp.Invoke(
        id,
        &GUID::zeroed(),
        0x0409,
        flags,
        &dp,
        Some(&mut result),
        None,
        None,
    )?;
    Ok(result)
}}

unsafe fn vt(v: &VARIANT) -> VARENUM { unsafe {
    v.Anonymous.Anonymous.vt
}}

unsafe fn to_i32(v: &VARIANT) -> windows::core::Result<i32> { unsafe {
    if vt(v) == VT_I4 {
        Ok(v.Anonymous.Anonymous.Anonymous.lVal)
    } else {
        Err(windows::core::Error::from(windows::core::HRESULT(-1)))
    }
}}

unsafe fn to_dispatch(v: &VARIANT) -> windows::core::Result<IDispatch> { unsafe {
    let err = || windows::core::Error::from(windows::core::HRESULT(-1));
    if vt(v) == VT_DISPATCH {
        let d = &v.Anonymous.Anonymous.Anonymous.pdispVal;
        (**d).clone().ok_or_else(err)
    } else {
        Err(err())
    }
}}

fn var_i32(val: i32) -> VARIANT {
    let mut x = VARIANT::default();
    unsafe {
        (*x.Anonymous.Anonymous).vt = VT_I4;
        (*x.Anonymous.Anonymous).Anonymous.lVal = val;
    }
    x
}

fn var_bstr(s: &str) -> VARIANT {
    let mut x = VARIANT::default();
    unsafe {
        (*x.Anonymous.Anonymous).vt = VT_BSTR;
        (*x.Anonymous.Anonymous).Anonymous.bstrVal =
            std::mem::ManuallyDrop::new(windows::core::BSTR::from(s));
    }
    x
}

/// 可见地打开最近导入的 PPT（用户显式操作；不动跟随逻辑）
pub fn open_pptx_blocking(pptx: &str) -> Result<(), String> {
    let pptx = pptx.replace('/', "\\");
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let ppt = create_ppt().map_err(|e| format!("无法启动/附着 PowerPoint：{e}"))?;
        let _ = invoke_put(&ppt, "Visible", var_i32(-1));
        let pres_coll = prop_obj(&ppt, "Presentations").map_err(|e| e.to_string())?;
        invoke(
            &pres_coll,
            "Open",
            &[var_bstr(&pptx), var_i32(-1), var_i32(0), var_i32(-1)],
            false,
        )
        .map_err(|e| format!("PowerPoint 打不开 {pptx}：{e}"))?;
        let _ = invoke_put(&ppt, "WindowState", var_i32(3)); // ppWindowMaximized
        Ok(())
    }
}

/// 备注长段拆成字幕条：按句末标点切分、短句合并到 ~60 字符（中英混排通用）
fn split_note_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let push = |buf: &mut String, out: &mut Vec<String>| {
        let t = buf.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
        buf.clear();
    };
    for ch in text.chars() {
        buf.push(ch);
        let sentence_end = matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；');
        if sentence_end && buf.chars().count() >= 20 {
            push(&mut buf, &mut out);
        } else if buf.chars().count() >= 60 {
            push(&mut buf, &mut out);
        }
    }
    push(&mut buf, &mut out);
    out
}

/// 读一页标题（没有标题占位符的页返回空串）
unsafe fn slide_title(slide: &IDispatch) -> String { unsafe {
    let chain = prop_obj(slide, "Shapes")
        .and_then(|sh| prop_obj(&sh, "Title"))
        .and_then(|t| prop_obj(&t, "TextFrame"))
        .and_then(|tf| prop_obj(&tf, "TextRange"))
        .and_then(|tr| invoke(&tr, "Text", &[], true));
    match chain {
        Ok(v) => {
            let b = &v.Anonymous.Anonymous.Anonymous.bstrVal;
            b.to_string()
        }
        Err(_) => String::new(),
    }
}}

/// 读一页的演讲者备注（NotesPage 正文占位符；内容源是它，不是标题）
unsafe fn slide_notes(slide: &IDispatch) -> String { unsafe {
    let chain = prop_obj(slide, "NotesPage")
        .and_then(|np| prop_obj(&np, "Shapes"))
        .and_then(|sh| prop_obj(&sh, "Placeholders"))
        .and_then(|ph| item_i32(&ph, 2))
        .and_then(|p2| prop_obj(&p2, "TextFrame"))
        .and_then(|tf| prop_obj(&tf, "TextRange"))
        .and_then(|tr| invoke(&tr, "Text", &[], true));
    match chain {
        Ok(v) => {
            let b = &v.Anonymous.Anonymous.Anonymous.bstrVal;
            b.to_string().replace('\r', "\n")
        }
        Err(_) => String::new(),
    }
}}

#[derive(serde::Serialize)]
pub struct ImportResult {
    pub pages: u32,
    pub titles: Vec<String>,
    pub script: String, // 骨架（=== N === + 标题注释）
    pub dir: String,    // 预览图目录
}

/// 导入 pptx：COM 打开（只读、无窗口）读页数/标题生成骨架 + 每页导出 PNG（PRD R9）
/// 导入用的实例：PowerPoint 原本没开就干完活 Quit，原本开着（附着）则不动它
pub fn import_pptx_blocking(pptx: &str, out_dir: &std::path::Path) -> Result<ImportResult, String> {
    // PowerPoint 的 COM Open 只认反斜杠路径
    let pptx = pptx.replace('/', "\\");
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let was_running = find_window("PPTFrameClass").found;
        let ppt = create_ppt().map_err(|e| format!("无法启动/附着 PowerPoint：{e}"))?;
        let result = (|| -> Result<ImportResult, String> {
            let pres_coll = prop_obj(&ppt, "Presentations").map_err(|e| e.to_string())?;
            // 先无窗口打开（干净）；失败退回带窗口（部分版本/受保护视图不支持无窗口）
            let opened_invisible = invoke(
                &pres_coll,
                "Open",
                &[var_bstr(&pptx), var_i32(-1), var_i32(0), var_i32(0)],
                true,
            )
            .and_then(|v| to_dispatch(&v));
            let pres = match opened_invisible {
                Ok(p) => p,
                Err(_) => {
                    let v = invoke(
                        &pres_coll,
                        "Open",
                        &[var_bstr(&pptx), var_i32(-1), var_i32(0), var_i32(-1)],
                        true,
                    )
                    .map_err(|e| format!("PowerPoint 打不开 {pptx}：{e}"))?;
                    let p = to_dispatch(&v).map_err(|e| format!("PowerPoint 打不开 {pptx}：{e}"))?;
                    // 只最小化我们自己拉起的实例；附着到用户已开的 PowerPoint 时不碰他窗口
                    if !was_running {
                        let _ = invoke_put(&ppt, "WindowState", var_i32(2)); // ppWindowMinimized
                    }
                    p
                }
            };
            let slides = prop_obj(&pres, "Slides").map_err(|e| e.to_string())?;
            let count = prop_i32(&slides, "Count").map_err(|e| e.to_string())?;
            let mut titles = Vec::new();
            let mut script = format!(
                "# 导入自 {} · {} 页\n",
                std::path::Path::new(&pptx)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default(),
                count
            );
            for i in 1..=count {
                let slide = item_i32(&slides, i).map_err(|e| e.to_string())?;
                let title = slide_title(&slide);
                titles.push(title.clone());
                script.push_str(&format!("\n=== {} ===\n", i));
                if !title.is_empty() {
                    script.push_str(&format!("# {}\n", title));
                }
                // 内容源 = 演讲者备注；长段按句拆成多条字幕（一段一句，~60 字/条）
                for line in split_note_lines(&slide_notes(&slide)) {
                    script.push_str(&line);
                    script.push('\n');
                }
                let png = out_dir.join(format!("slide-{:02}.png", i));
                invoke(
                    &slide,
                    "Export",
                    &[
                        var_bstr(&png.to_string_lossy()),
                        var_bstr("PNG"),
                        var_i32(1920),
                        var_i32(1080),
                    ],
                    false,
                )
                .map_err(|e| format!("第 {} 页导出失败：{}", i, e))?;
            }
            let _ = invoke(&pres, "Close", &[], false);
            *LAST_PPTX.lock().unwrap() = Some(pptx.clone());
            Ok(ImportResult { pages: count as u32, titles, script, dir: out_dir.to_string_lossy().to_string() })
        })();
        if !was_running {
            let _ = invoke(&ppt, "Quit", &[], false);
        }
        result
    }
}

unsafe fn prop_obj(disp: &IDispatch, name: &str) -> windows::core::Result<IDispatch> { unsafe {
    to_dispatch(&invoke(disp, name, &[], true)?)
}}

unsafe fn prop_i32(disp: &IDispatch, name: &str) -> windows::core::Result<i32> { unsafe {
    to_i32(&invoke(disp, name, &[], true)?)
}}

/// 属性赋值（DISPID_PROPERTYPUT）
unsafe fn invoke_put(disp: &IDispatch, name: &str, arg: VARIANT) -> windows::core::Result<()> { unsafe {
    let id = get_id(disp, name)?;
    let mut store = [arg];
    let mut dispid_put = -3i32; // DISPID_PROPERTYPUT
    let mut dp = DISPPARAMS::default();
    dp.rgvarg = store.as_mut_ptr();
    dp.cArgs = 1;
    dp.rgdispidNamedArgs = &mut dispid_put;
    dp.cNamedArgs = 1;
    disp.Invoke(
        id,
        &GUID::zeroed(),
        0x0409,
        DISPATCH_FLAGS(DISPATCH_PROPERTYPUT.0),
        &dp,
        None,
        None,
        None,
    )?;
    Ok(())
}}

unsafe fn item_i32(coll: &IDispatch, idx: i32) -> windows::core::Result<IDispatch> { unsafe {
    let arg = var_i32(idx);
    to_dispatch(&invoke(coll, "Item", &[arg], true)?)
}}

/// 创建/附着 PowerPoint：GetActiveObject 优先（真实机器标准路径），
/// 沙箱里 ROT 不可见则 CoCreateInstance 兜底（PowerPoint 单例；沙箱例外见 poll_loop 注释）
unsafe fn create_ppt() -> windows::core::Result<IDispatch> { unsafe {
    let prog = wide("PowerPoint.Application");
    let clsid = CLSIDFromProgID(PCWSTR(prog.as_ptr()))?;
    let mut punk: Option<windows::core::IUnknown> = None;
    if GetActiveObject(&clsid, None, &mut punk).is_ok() {
        if let Some(unk) = punk {
            if let Ok(d) = unk.cast() {
                return Ok(d);
            }
        }
    }
    let unk: windows::core::IUnknown = CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)?;
    unk.cast()
}}

// ---- 窗口探测（PPTFrameClass = PowerPoint 主窗口；screenClass = 放映窗） ----

struct FindCtx {
    target: &'static str,
    found: bool,
    hwnd: HWND,
    show_rect: Option<ShowRect>,
}

unsafe extern "system" fn find_cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL { unsafe {
    let ctx = &mut *(lparam.0 as *mut FindCtx);
    let mut class = [0u16; 32];
    let n = GetClassNameW(hwnd, &mut class);
    let name = String::from_utf16_lossy(&class[..n as usize]);
    if name == ctx.target && IsWindowVisible(hwnd).as_bool() {
        ctx.found = true;
        ctx.hwnd = hwnd;
        if name == "screenClass" {
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() {
                // 放映窗所在显示器（放映窗通常就是整屏，但双屏下要拿对那块屏）
                let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(mon, &mut mi).as_bool() {
                    ctx.show_rect = Some(ShowRect {
                        x: mi.rcMonitor.left,
                        y: mi.rcMonitor.top,
                        w: mi.rcMonitor.right - mi.rcMonitor.left,
                        h: mi.rcMonitor.bottom - mi.rcMonitor.top,
                    });
                }
            }
        }
        return windows::core::BOOL(0);
    }
    windows::core::BOOL(1)
}}

fn find_window(target: &'static str) -> FindCtx {
    let mut ctx = FindCtx { target, found: false, hwnd: HWND::default(), show_rect: None };
    unsafe {
        let _ = EnumWindows(Some(find_cb), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx
}

fn find_show_monitor() -> Option<ShowRect> {
    find_window("screenClass").show_rect
}

/// 首个可见目标窗口句柄（screenClass 或其他）
pub fn find_hwnd(target: &'static str) -> Option<HWND> {
    let ctx = find_window(target);
    if ctx.found {
        Some(ctx.hwnd)
    } else {
        None
    }
}

// ---- 贴边布局（R16/R17，路线①：SetWindowPos 缩目标窗 + 字幕条贴边） ----

use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE;

static DOCK_RESTORE: Mutex<Option<(isize, i32, i32, i32, i32)>> = Mutex::new(None);

/// 贴边几何：返回 (目标窗新矩形, 字幕条矩形)，物理像素
pub fn dock_geometry(
    mon: (i32, i32, i32, i32),
    side: &str,
    ratio: f64,
) -> ((i32, i32, i32, i32), (i32, i32, i32, i32)) {
    let (mx, my, mw, mh) = mon;
    let r = ratio.clamp(0.08, 0.5);
    match side {
        "bottom" => {
            let bar = (mh as f64 * r).round() as i32;
            ((mx, my, mw, mh - bar), (mx, my + mh - bar, mw, bar))
        }
        "left" => {
            let bar = (mw as f64 * r).round() as i32;
            ((mx + bar, my, mw - bar, mh), (mx, my, bar, mh))
        }
        "right" => {
            let bar = (mw as f64 * r).round() as i32;
            ((mx, my, mw - bar, mh), (mx + mw - bar, my, bar, mh))
        }
        // 默认 top（PRD：字幕条贴放映窗上方）
        _ => {
            let bar = (mh as f64 * r).round() as i32;
            ((mx, my + bar, mw, mh - bar), (mx, my, mw, bar))
        }
    }
}

unsafe fn set_win_rect(h: HWND, x: i32, y: i32, w: i32, hh: i32) { unsafe {
    let _ = SetWindowPos(h, None, x, y, w, hh, SWP_NOZORDER | SWP_NOACTIVATE);
}}

/// 应用布局：dock＝目标窗（放映窗优先，其次排练窗；都没有则桌面即舞台）让位 + overlay 贴边整条；
/// 切回 overlay＝把目标窗恢复全屏。返回字幕条新矩形（物理像素）。
/// 注意：无目标窗（无 PPT、未开排练窗）也要返回贴边矩形——贴边是字幕条自身的布局，
/// 不能因为没有可缩的窗就不生效（2026-11-19 修：此前无 PPT 时贴边完全失效）。
pub fn apply_dock(app: &tauri::AppHandle, settings: &crate::Settings) -> Option<(i32, i32, i32, i32)> {
    use tauri::Manager;
    let mon: (i32, i32, i32, i32);
    let target: Option<HWND>;

    if let (Some(rect), Some(h)) = (find_show_monitor(), find_hwnd("screenClass")) {
        mon = (rect.x, rect.y, rect.w, rect.h);
        target = Some(h);
    } else if let Some(pw) = app.get_webview_window("preview") {
        let m = pw.current_monitor().ok().flatten()?;
        let p = m.position();
        let s = m.size();
        mon = (p.x, p.y, s.width as i32, s.height as i32);
        target = pw.hwnd().ok();
    } else {
        let m = crate::target_monitor(app, settings)?;
        let p = m.position();
        let s = m.size();
        mon = (p.x, p.y, s.width as i32, s.height as i32);
        target = None;
    }

    if settings.layout != "dock" {
        // 覆盖布局：恢复目标窗全屏
        if let Some(t) = target {
            let mut rest = DOCK_RESTORE.lock().unwrap();
            if let Some((h, x, y, w, hh)) = *rest {
                if h == t.0 as isize {
                    unsafe { set_win_rect(t, x, y, w, hh) };
                    *rest = None;
                }
            }
        }
        return None;
    }

    let (target_rect, bar) = dock_geometry(mon, &settings.dock_side, settings.dock_ratio);
    if let Some(t) = target {
        if settings.ppt_resize {
            // 记住目标窗原矩形（换目标时重记），缩窗让位；ppt_resize 关掉时只贴条不缩窗
            {
                let mut rest = DOCK_RESTORE.lock().unwrap();
                if rest.is_none() || rest.unwrap().0 != t.0 as isize {
                    let mut r = RECT::default();
                    if unsafe { GetWindowRect(t, &mut r) }.is_ok() {
                        *rest = Some((t.0 as isize, r.left, r.top, r.right - r.left, r.bottom - r.top));
                    }
                }
            }
            unsafe { set_win_rect(t, target_rect.0, target_rect.1, target_rect.2, target_rect.3) };
        }
    }
    Some(bar)
}

/// 读放映状态：SlideShowWindows(1).View.Slide.SlideIndex
/// 最后页再 Next 后 View.Slide 抛"Invalid request"（PRD 4.4），此时返回 (true, None) 保持上一次页码
unsafe fn ppt_walk(ppt: &IDispatch) -> windows::core::Result<(bool, Option<u32>)> { unsafe {
    let wins = prop_obj(ppt, "SlideShowWindows")?;
    let count = to_i32(&invoke(&wins, "Count", &[], true)?)?;
    if count <= 0 {
        return Ok((false, None));
    }
    let w1 = item_i32(&wins, 1)?;
    let view = prop_obj(&w1, "View")?;
    match prop_obj(&view, "Slide") {
        Ok(s) => {
            let idx = prop_i32(&s, "SlideIndex")?;
            Ok((true, Some(idx.max(0) as u32)))
        }
        Err(_) => Ok((true, None)),
    }
}}

/// 轮询主循环：状态变化时发事件；放映窗矩形持续更新全局（place_overlay 用）
fn poll_loop(app: AppHandle) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let mut last_slide: Option<u32> = None;
    let mut was_showing = false;
    let mut cached: Option<IDispatch> = None;
    let mut wrong_instance_note = false;
    loop {
        std::thread::sleep(Duration::from_millis(250));

        // 断开挂载请求：关掉我们挂的 deck（按路径匹配，不动用户其他文件）、停止跟随
        if DETACH_REQUESTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let target = LAST_PPTX.lock().unwrap().clone().map(|p| p.replace('/', "\\").to_lowercase());
            if let Some(ppt) = cached.take() {
                unsafe {
                    if let (Some(want), Ok(coll)) = (target.clone(), prop_obj(&ppt, "Presentations")) {
                        if let Ok(count) = prop_i32(&coll, "Count") {
                            for i in (1..=count).rev() {
                                if let Ok(p) = item_i32(&coll, i) {
                                    if let Ok(v) = invoke(&p, "FullName", &[], true) {
                                        let b = &v.Anonymous.Anonymous.Anonymous.bstrVal;
                                        if b.to_string().replace('/', "\\").to_lowercase() == want {
                                            let _ = invoke(&p, "Close", &[], false);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            *LAST_PPTX.lock().unwrap() = None;
            *PPT_SHOW.lock().unwrap() = None;
            eprintln!("[ppt] detached");
            let _ = app.emit(
                "ppt-state",
                serde_json::json!({ "showing": false, "slide": null }),
            );
            let _ = app.emit("ppt-detached", ());
            was_showing = false;
            last_slide = None;
            continue;
        }

        // PowerPoint 主窗口不在了就丢弃缓存实例（放映结束/关掉 PowerPoint）
        if !find_window("PPTFrameClass").found {
            cached = None;
        }
        if cached.is_none() && find_window("PPTFrameClass").found {
            match unsafe { create_ppt() } {
                Ok(p) => {
                    eprintln!("[ppt] attached");
                    cached = Some(p);
                    wrong_instance_note = false;
                }
                Err(e) => eprintln!("[ppt] create fail: {e}"),
            }
        }

        let mut showing = false;
        let mut slide: Option<u32> = None;
        if let Some(ppt) = &cached {
            match unsafe { ppt_walk(ppt) } {
                Ok((sh, sl)) => {
                    showing = sh;
                    slide = sl;
                    // 附着的实例里没有放映、但桌面上明明有放映窗 → 附着错了实例
                    // （DSH 沙箱会给每个 COM 客户端独立 POWERPNT，真实机器不会发生）
                    if !sh && find_window("screenClass").found && !wrong_instance_note {
                        wrong_instance_note = true;
                        eprintln!("[ppt] 警告：附着的 PowerPoint 实例里没有放映（疑似沙箱实例隔离），跟随不可用");
                    }
                }
                Err(e) => {
                    // 服务端断开等错误：丢弃缓存，下个周期重连
                    eprintln!("[ppt] walk err: {e}");
                    cached = None;
                }
            }
        }

        let rect = if showing { find_show_monitor() } else { None };
        *PPT_SHOW.lock().unwrap() = rect;

        if showing != was_showing || slide != last_slide {
            eprintln!(
                "[ppt] showing={} slide={:?} monitor={:?}",
                showing, slide, rect
            );
            let _ = app.emit(
                "ppt-state",
                serde_json::json!({ "showing": showing, "slide": slide }),
            );
        }
        if was_showing && !showing {
            let _ = app.emit("ppt-show-end", ());
        }
        if showing {
            last_slide = slide.or(last_slide);
        } else {
            last_slide = None;
        }
        was_showing = showing;
    }
    #[allow(unreachable_code)]
    unsafe {
        CoUninitialize();
    }
}

/// 在 setup 里调用一次，启动轮询线程
pub fn start_polling(app: AppHandle) {
    std::thread::spawn(move || poll_loop(app));
}
