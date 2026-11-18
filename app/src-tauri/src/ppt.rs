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
    IDispatch, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPPARAMS,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Ole::GetActiveObject;
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_DISPATCH, VT_I4};
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn get_id(disp: &IDispatch, name: &str) -> windows::core::Result<i32> {
    let w = wide(name);
    let names = [PCWSTR(w.as_ptr())];
    let mut ids = [0i32];
    disp.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0x0409, ids.as_mut_ptr())?;
    Ok(ids[0])
}

/// 晚绑定调用/取属性：args 为位置参数（自动按 Invoke 要求倒序）
unsafe fn invoke(
    disp: &IDispatch,
    name: &str,
    args: &[VARIANT],
    _want_result: bool,
) -> windows::core::Result<VARIANT> {
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
        DISPATCH_FLAGS(DISPATCH_PROPERTYGET.0 | DISPATCH_METHOD.0)
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
}

unsafe fn vt(v: &VARIANT) -> VARENUM {
    v.Anonymous.Anonymous.vt
}

unsafe fn to_i32(v: &VARIANT) -> windows::core::Result<i32> {
    if vt(v) == VT_I4 {
        Ok(v.Anonymous.Anonymous.Anonymous.lVal)
    } else {
        Err(windows::core::Error::from(windows::core::HRESULT(-1)))
    }
}

unsafe fn to_dispatch(v: &VARIANT) -> windows::core::Result<IDispatch> {
    let err = || windows::core::Error::from(windows::core::HRESULT(-1));
    if vt(v) == VT_DISPATCH {
        let d = &v.Anonymous.Anonymous.Anonymous.pdispVal;
        (**d).clone().ok_or_else(err)
    } else {
        Err(err())
    }
}

fn var_i32(val: i32) -> VARIANT {
    let mut x = VARIANT::default();
    unsafe {
        (*x.Anonymous.Anonymous).vt = VT_I4;
        (*x.Anonymous.Anonymous).Anonymous.lVal = val;
    }
    x
}

unsafe fn prop_obj(disp: &IDispatch, name: &str) -> windows::core::Result<IDispatch> {
    to_dispatch(&invoke(disp, name, &[], true)?)
}

unsafe fn prop_i32(disp: &IDispatch, name: &str) -> windows::core::Result<i32> {
    to_i32(&invoke(disp, name, &[], true)?)
}

unsafe fn item_i32(coll: &IDispatch, idx: i32) -> windows::core::Result<IDispatch> {
    let arg = var_i32(idx);
    to_dispatch(&invoke(coll, "Item", &[arg], true)?)
}

/// 创建/附着 PowerPoint：GetActiveObject 优先（真实机器标准路径），
/// 沙箱里 ROT 不可见则 CoCreateInstance 兜底（PowerPoint 单例；沙箱例外见 poll_loop 注释）
unsafe fn create_ppt() -> windows::core::Result<IDispatch> {
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
}

// ---- 窗口探测（PPTFrameClass = PowerPoint 主窗口；screenClass = 放映窗） ----

struct FindCtx {
    target: &'static str,
    found: bool,
    show_rect: Option<ShowRect>,
}

unsafe extern "system" fn find_cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let ctx = &mut *(lparam.0 as *mut FindCtx);
    let mut class = [0u16; 32];
    let n = GetClassNameW(hwnd, &mut class);
    let name = String::from_utf16_lossy(&class[..n as usize]);
    if name == ctx.target && IsWindowVisible(hwnd).as_bool() {
        ctx.found = true;
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
}

fn find_window(target: &'static str) -> FindCtx {
    let mut ctx = FindCtx { target, found: false, show_rect: None };
    unsafe {
        let _ = EnumWindows(Some(find_cb), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx
}

fn find_show_monitor() -> Option<ShowRect> {
    find_window("screenClass").show_rect
}

/// 读放映状态：SlideShowWindows(1).View.Slide.SlideIndex
/// 最后页再 Next 后 View.Slide 抛"Invalid request"（PRD 4.4），此时返回 (true, None) 保持上一次页码
unsafe fn ppt_walk(ppt: &IDispatch) -> windows::core::Result<(bool, Option<u32>)> {
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
}

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
