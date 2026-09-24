//! Target Mode: the user points at any window on screen and the app
//! identifies the process (and installed program) behind it.
//!
//! A nearly transparent, topmost window covering the whole virtual screen
//! receives the mouse (crosshair cursor); a hollow frame highlights the
//! window under the cursor and a label shows which process it belongs to.
//! Left click picks it; right click, Esc or two minutes without a choice
//! cancel. Nothing is changed on the target window.

use crate::util::wide;
use crate::{AppError, Result};
use serde::Serialize;
use std::cell::RefCell;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub hwnd: u64,
    pub title: String,
    pub class_name: String,
    /// Process that owns the window's content (the real app for UWP windows).
    pub pid: u32,
    /// ApplicationFrameHost PID when the window is a UWP frame.
    pub host_pid: Option<u32>,
    pub rect: [i32; 4],
    /// "taskbar" | "desktop" | "trayOverflow" for Windows shell surfaces.
    pub shell: Option<&'static str>,
    /// Tray icon under the click when the window is the taskbar / overflow area.
    pub tray_icon: Option<TrayIcon>,
    /// False when this Windows draws the tray without a readable toolbar (Windows 11).
    pub tray_readable: bool,
}

fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn title_of(h: HWND) -> String {
    let mut buf = vec![0u16; 512];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn pid_of(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, &mut pid) };
    pid
}

/// Visible bounds (without the invisible resize borders of Windows 10/11).
fn bounds(h: HWND) -> Option<RECT> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let ok = unsafe { DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&mut r as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32) } == 0
        || unsafe { GetWindowRect(h, &mut r) } != 0;
    (ok && r.right > r.left && r.bottom > r.top).then_some(r)
}

fn is_candidate(h: HWND) -> bool {
    unsafe {
        if IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, (&mut cloaked as *mut u32).cast(), 4) == 0 && cloaked != 0 {
            return false;
        }
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TRANSPARENT != 0 {
            return false; // click-through overlays are not what the user points at
        }
        if ex & WS_EX_LAYERED != 0 {
            let (mut key, mut alpha, mut flags) = (0u32, 255u8, 0u32);
            if GetLayeredWindowAttributes(h, &mut key, &mut alpha, &mut flags) != 0 && flags & LWA_ALPHA != 0 && alpha == 0 {
                return false;
            }
        }
    }
    true
}

/// Topmost top-level window under a screen point (physical pixels),
/// ignoring windows of `exclude_pid`.
pub(crate) fn window_at(x: i32, y: i32, exclude_pid: u32) -> Option<HWND> {
    struct Find {
        x: i32,
        y: i32,
        exclude: u32,
        found: HWND,
    }
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows_sys::core::BOOL {
        let f = &mut *(l as *mut Find);
        if !is_candidate(h) || pid_of(h) == f.exclude {
            return 1;
        }
        match bounds(h) {
            Some(r) if f.x >= r.left && f.x < r.right && f.y >= r.top && f.y < r.bottom => {
                f.found = h;
                0
            }
            _ => 1,
        }
    }
    let mut f = Find { x, y, exclude: exclude_pid, found: std::ptr::null_mut() };
    // EnumWindows walks top-level windows in Z order, topmost first.
    unsafe { EnumWindows(Some(cb), &mut f as *mut Find as LPARAM) };
    (!f.found.is_null()).then_some(f.found)
}

/// UWP apps live in a child window owned by the real app process.
fn uwp_content_pid(frame: HWND, host: u32) -> Option<u32> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows_sys::core::BOOL {
        let (host, found) = &mut *(l as *mut (u32, u32));
        let p = pid_of(h);
        if p != *host && p != 0 {
            *found = p;
            return 0;
        }
        1
    }
    let mut st = (host, 0u32);
    unsafe { EnumChildWindows(frame, Some(cb), &mut st as *mut (u32, u32) as LPARAM) };
    (st.1 != 0).then_some(st.1)
}

pub(crate) fn describe(h: HWND) -> Option<WindowInfo> {
    if unsafe { IsWindow(h) } == 0 {
        return None;
    }
    let class_name = class_of(h);
    let mut pid = pid_of(h);
    let mut host_pid = None;
    let exe = crate::processes::image_path(pid).unwrap_or_default().to_lowercase();
    if exe.ends_with(r"\applicationframehost.exe") {
        if let Some(real) = uwp_content_pid(h, pid) {
            host_pid = Some(pid);
            pid = real;
        }
    }
    let shell = match class_name.as_str() {
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" => Some("taskbar"),
        "Progman" | "WorkerW" => Some("desktop"),
        "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland" => Some("trayOverflow"),
        _ => None,
    };
    let r = bounds(h).unwrap_or(RECT { left: 0, top: 0, right: 0, bottom: 0 });
    Some(WindowInfo {
        hwnd: h as usize as u64,
        title: title_of(h),
        class_name,
        pid,
        host_pid,
        rect: [r.left, r.top, r.right, r.bottom],
        shell,
        tray_icon: None,
        tray_readable: false,
    })
}

/// [describe] plus, on the taskbar / overflow area, the tray icon at the
/// point (its owner becomes the window's process).
pub(crate) fn describe_at(h: HWND, x: i32, y: i32) -> Option<WindowInfo> {
    let mut info = describe(h)?;
    if matches!(info.shell, Some("taskbar" | "trayOverflow")) {
        info.tray_readable = tray_supported(h);
        if let Some(icon) = tray_icon_at(h, x, y) {
            info.pid = icon.pid;
            info.tray_icon = Some(icon);
        }
    }
    Some(info)
}

// ---------------------------------------------------------------------------
// Notification-area (tray) icons
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayIcon {
    /// Tooltip text of the icon.
    pub tooltip: String,
    /// Process that owns the icon (through the window registered with it).
    pub pid: u32,
    pub owner_hwnd: u64,
}

const TB_GETBUTTON: u32 = WM_USER + 23;
const TB_BUTTONCOUNT: u32 = WM_USER + 24;
const TB_GETITEMRECT: u32 = WM_USER + 29;
const TB_GETBUTTONTEXTW: u32 = WM_USER + 75;

fn find_child(parent: HWND, class: &str) -> Option<HWND> {
    let c = wide(class);
    let h = unsafe { FindWindowExW(parent, std::ptr::null_mut(), c.as_ptr(), std::ptr::null()) };
    (!h.is_null()).then_some(h)
}

/// The toolbar holding the tray icons for a taskbar / overflow window
/// (Windows 10 layout; Windows 11 draws the tray with XAML and has none).
fn tray_toolbar(top: HWND) -> Option<HWND> {
    match class_of(top).as_str() {
        "Shell_TrayWnd" => find_child(find_child(find_child(top, "TrayNotifyWnd")?, "SysPager")?, "ToolbarWindow32"),
        "NotifyIconOverflowWindow" => find_child(top, "ToolbarWindow32"),
        _ => None,
    }
}

fn send(h: HWND, msg: u32, w: usize, l: isize) -> Option<isize> {
    let mut out = 0usize;
    let ok = unsafe { SendMessageTimeoutW(h, msg, w, l, SMTO_ABORTIFHUNG, 500, &mut out) } != 0;
    ok.then_some(out as isize)
}

/// Buffer inside explorer.exe: toolbar messages that return data need a
/// pointer valid in the toolbar's process. Only read access is used.
struct RemoteBuf {
    process: crate::util::OwnedHandle,
    ptr: *mut std::ffi::c_void,
}

impl RemoteBuf {
    fn new(pid: u32, size: usize) -> Option<Self> {
        use windows_sys::Win32::System::Memory::*;
        use windows_sys::Win32::System::Threading::*;
        let process = crate::util::OwnedHandle::new(unsafe { OpenProcess(PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
        let ptr = unsafe { VirtualAllocEx(process.raw(), std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        (!ptr.is_null()).then_some(RemoteBuf { process, ptr })
    }
    fn read(&self, addr: usize, out: &mut [u8]) -> bool {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        let mut n = 0usize;
        unsafe { ReadProcessMemory(self.process.raw(), addr as *const _, out.as_mut_ptr().cast(), out.len(), &mut n) != 0 && n == out.len() }
    }
}

impl Drop for RemoteBuf {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{VirtualFreeEx, MEM_RELEASE};
        unsafe { VirtualFreeEx(self.process.raw(), self.ptr, 0, MEM_RELEASE) };
    }
}

/// The tray icon under a screen point, if `top` is the taskbar or the
/// overflow area and the icon can be read.
pub(crate) fn tray_icon_at(top: HWND, x: i32, y: i32) -> Option<TrayIcon> {
    let tb = tray_toolbar(top)?;
    let buf = RemoteBuf::new(pid_of(tb), 4096)?;
    let count = send(tb, TB_BUTTONCOUNT, 0, 0)?.clamp(0, 512) as usize;
    for i in 0..count {
        // Button rectangle (toolbar client coordinates) → screen.
        send(tb, TB_GETITEMRECT, i, buf.ptr as isize)?;
        let mut raw = [0u8; 16];
        if !buf.read(buf.ptr as usize, &mut raw) {
            return None;
        }
        let v = |o: usize| i32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let mut pts = [POINT { x: v(0), y: v(4) }, POINT { x: v(8), y: v(12) }];
        unsafe { MapWindowPoints(tb, std::ptr::null_mut(), pts.as_mut_ptr(), 2) };
        if !(x >= pts[0].x && x < pts[1].x && y >= pts[0].y && y < pts[1].y) {
            continue;
        }
        // TBBUTTON (64-bit layout): dwData at offset 16 points to the tray
        // record, whose first field is the owner window.
        send(tb, TB_GETBUTTON, i, buf.ptr as isize)?;
        let mut tbb = [0u8; 32];
        if !buf.read(buf.ptr as usize, &mut tbb) {
            return None;
        }
        let data = usize::from_le_bytes(tbb[16..24].try_into().unwrap());
        let mut owner = [0u8; 8];
        if data == 0 || !buf.read(data, &mut owner) {
            return None;
        }
        let owner_hwnd = u64::from_le_bytes(owner) as usize as HWND;
        let pid = pid_of(owner_hwnd);
        // Tooltip text.
        let id = i32::from_le_bytes(tbb[4..8].try_into().unwrap());
        let len = send(tb, TB_GETBUTTONTEXTW, id as usize, buf.ptr as isize).unwrap_or(-1);
        let tooltip = if len > 0 && len < 1000 {
            let mut t = vec![0u8; len as usize * 2];
            if buf.read(buf.ptr as usize, &mut t) {
                let units: Vec<u16> = t.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
                String::from_utf16_lossy(&units).trim().to_string()
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        return (pid != 0).then_some(TrayIcon { tooltip, pid, owner_hwnd: owner_hwnd as usize as u64 });
    }
    None
}

/// Whether this Windows exposes tray icons as a readable toolbar.
pub(crate) fn tray_supported(top: HWND) -> bool {
    tray_toolbar(top).is_some()
}

// ---------------------------------------------------------------------------
// Picker UI (runs on its own thread with its own message loop)
// ---------------------------------------------------------------------------

struct PickState {
    frame: HWND,
    label: HWND,
    current: HWND,
    /// Last hovered tray icon (the label is refreshed when it changes).
    current_icon: u64,
    click: (i32, i32),
    result: Option<Option<HWND>>,
    hint: Vec<u16>,
    text: Vec<u16>,
    own_pid: u32,
}

thread_local! {
    static STATE: RefCell<Option<PickState>> = const { RefCell::new(None) };
}

const FRAME: i32 = 4;
const TIMER_ID: usize = 1;
const TIMEOUT_MS: u32 = 120_000;

fn finish(result: Option<HWND>) {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.result.get_or_insert(result);
        }
    });
    unsafe { PostQuitMessage(0) };
}

fn hover(x: i32, y: i32) {
    let Some((own, frame, label, current, current_icon)) = STATE.with(|s| s.borrow().as_ref().map(|st| (st.own_pid, st.frame, st.label, st.current, st.current_icon))) else { return };
    let h = window_at(x, y, own).unwrap_or(std::ptr::null_mut());
    // On the taskbar the hovered tray icon matters, not only the window.
    let icon = if h.is_null() { None } else { tray_icon_at(h, x, y) };
    let icon_id = icon.as_ref().map(|i| i.owner_hwnd).unwrap_or(0);
    if h == current && icon_id == current_icon {
        return;
    }
    let text = if h.is_null() {
        String::new()
    } else {
        let info = describe(h);
        let pid = icon.as_ref().map(|i| i.pid).or(info.as_ref().map(|i| i.pid)).unwrap_or(0);
        let exe = crate::processes::image_path(pid).and_then(|p| p.rsplit('\\').next().map(str::to_string)).unwrap_or_else(|| format!("PID {pid}"));
        let title = match &icon {
            Some(i) => i.tooltip.clone(),
            None => info.map(|i| i.title).unwrap_or_default(),
        };
        if title.is_empty() { exe } else { format!("{exe}  —  {title}") }
    };
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.current = h;
            st.current_icon = icon_id;
            st.text = text.encode_utf16().collect();
        }
    });
    unsafe {
        match (h.is_null(), bounds(h)) {
            (false, Some(r)) => {
                let (w, hgt) = (r.right - r.left + 2 * FRAME, r.bottom - r.top + 2 * FRAME);
                SetWindowPos(frame, HWND_TOPMOST, r.left - FRAME, r.top - FRAME, w, hgt, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                let outer = CreateRectRgn(0, 0, w, hgt);
                let inner = CreateRectRgn(FRAME, FRAME, w - FRAME, hgt - FRAME);
                CombineRgn(outer, outer, inner, RGN_DIFF);
                DeleteObject(inner);
                SetWindowRgn(frame, outer, 1); // the system owns `outer` now
            }
            _ => {
                ShowWindow(frame, SW_HIDE);
            }
        }
        InvalidateRect(label, std::ptr::null(), 1);
    }
}

unsafe extern "system" fn overlay_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            hover(p.x, p.y);
            0
        }
        WM_LBUTTONDOWN => {
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.click = (p.x, p.y);
                }
            });
            let cur = STATE.with(|s| s.borrow().as_ref().map(|st| st.current)).unwrap_or(std::ptr::null_mut());
            if !cur.is_null() {
                finish(Some(cur));
            }
            0
        }
        WM_RBUTTONDOWN | WM_TIMER => {
            finish(None);
            0
        }
        WM_KEYDOWN if w == windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE as usize => {
            finish(None);
            0
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_CROSS));
            1
        }
        WM_CLOSE => {
            finish(None);
            0
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

unsafe extern "system" fn label_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg != WM_PAINT {
        return DefWindowProcW(h, msg, w, l);
    }
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(h, &mut ps);
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetClientRect(h, &mut rc);
    let bg = CreateSolidBrush(0x0024_1b16); // #161b24
    FillRect(dc, &rc, bg);
    DeleteObject(bg);
    SetBkMode(dc, TRANSPARENT as i32);
    let face = wide("Segoe UI");
    let font = CreateFontW(-17, 0, 0, 0, 600, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, face.as_ptr());
    let old = SelectObject(dc, font);
    let (hint, text) = STATE.with(|s| s.borrow().as_ref().map(|st| (st.hint.clone(), st.text.clone()))).unwrap_or_default();
    let half = (rc.bottom - rc.top) / 2;
    let mut top = RECT { left: rc.left + 12, top: rc.top + 4, right: rc.right - 12, bottom: rc.top + half };
    SetTextColor(dc, 0x00bf_d42d); // teal #2dd4bf
    DrawTextW(dc, hint.as_ptr(), hint.len() as i32, &mut top, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    let mut bottom = RECT { left: rc.left + 12, top: rc.top + half, right: rc.right - 12, bottom: rc.bottom - 4 };
    SetTextColor(dc, 0x00f0_f0f0);
    DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut bottom, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    SelectObject(dc, old);
    DeleteObject(font);
    EndPaint(h, &ps);
    0
}

/// Let the user point at a window. Blocks until a click, a cancel or the
/// timeout. `hint` is shown on screen (already translated by the caller).
pub fn pick(hint: &str) -> Result<Option<WindowInfo>> {
    let hint = hint.to_string();
    std::thread::spawn(move || pick_thread(&hint))
        .join()
        .unwrap_or_else(|_| Err(AppError::Helper("target mode thread panicked".into())))
}

fn pick_thread(hint: &str) -> Result<Option<WindowInfo>> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let inst = GetModuleHandleW(std::ptr::null());
        let names = [wide("NexusTargetOverlay"), wide("NexusTargetFrame"), wide("NexusTargetLabel")];
        let frame_brush = CreateSolidBrush(0x00bf_d42d);
        let procs: [WNDPROC; 3] = [Some(overlay_proc), Some(DefWindowProcW), Some(label_proc)];
        let brushes = [GetStockObject(BLACK_BRUSH), frame_brush, GetStockObject(BLACK_BRUSH)];
        for i in 0..3 {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: procs[i],
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_CROSS),
                hbrBackground: brushes[i],
                lpszMenuName: std::ptr::null(),
                lpszClassName: names[i].as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            RegisterClassExW(&wc);
        }
        let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let base = WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
        let passive = base | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE;
        let create = |ex: u32, i: usize, x: i32, y: i32, w: i32, h: i32| {
            CreateWindowExW(ex, names[i].as_ptr(), std::ptr::null(), WS_POPUP, x, y, w, h, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null())
        };
        let overlay = create(base | WS_EX_LAYERED, 0, vx, vy, vw, vh);
        let frame = create(passive, 1, 0, 0, 0, 0);
        // Label at the top centre of the primary monitor.
        let (pw, lw, lh) = (GetSystemMetrics(SM_CXSCREEN), 640, 64);
        let label = create(passive, 2, (pw - lw) / 2, 24, lw, lh);
        let cleanup = || {
            for h in [label, frame, overlay] {
                if !h.is_null() {
                    DestroyWindow(h);
                }
            }
            for n in &names {
                UnregisterClassW(n.as_ptr(), inst);
            }
            DeleteObject(frame_brush);
        };
        if overlay.is_null() || frame.is_null() || label.is_null() {
            let e = AppError::last_win32("creating target mode windows", None);
            cleanup();
            return Err(e);
        }
        // Nearly invisible but still receives the mouse.
        SetLayeredWindowAttributes(overlay, 0, 1, LWA_ALPHA);
        SetLayeredWindowAttributes(frame, 0, 255, LWA_ALPHA);
        SetLayeredWindowAttributes(label, 0, 235, LWA_ALPHA);
        STATE.with(|s| {
            *s.borrow_mut() = Some(PickState {
                frame,
                label,
                current: std::ptr::null_mut(),
                current_icon: 0,
                click: (0, 0),
                result: None,
                hint: hint.encode_utf16().collect(),
                text: Vec::new(),
                own_pid: std::process::id(),
            })
        });
        ShowWindow(overlay, SW_SHOW);
        ShowWindow(label, SW_SHOWNOACTIVATE);
        SetForegroundWindow(overlay);
        windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(overlay);
        SetTimer(overlay, TIMER_ID, TIMEOUT_MS, None);
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        hover(p.x, p.y);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        KillTimer(overlay, TIMER_ID);
        let st = STATE.with(|s| s.borrow_mut().take());
        cleanup();
        Ok(st.and_then(|s| s.result.flatten().map(|h| (h, s.click))).and_then(|(h, (x, y))| describe_at(h, x, y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tray_icons_on_the_taskbar() {
        let c = wide("Shell_TrayWnd");
        let bar = unsafe { FindWindowW(c.as_ptr(), std::ptr::null()) };
        if bar.is_null() {
            return; // no desktop session
        }
        let Some(tb) = tray_toolbar(bar) else {
            eprintln!("tray toolbar not available (Windows 11 layout)");
            return;
        };
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(tb, &mut r) };
        let y = (r.top + r.bottom) / 2;
        let mut found = Vec::new();
        let mut x = r.left + 2;
        while x < r.right {
            if let Some(i) = tray_icon_at(bar, x, y) {
                if !found.iter().any(|f: &TrayIcon| f.owner_hwnd == i.owner_hwnd) {
                    found.push(i);
                }
            }
            x += 6;
        }
        for i in &found {
            eprintln!("tray icon: pid {} {:?} ({:?})", i.pid, crate::processes::image_path(i.pid), i.tooltip);
            assert!(i.pid != 0);
        }
        // Outside the toolbar nothing is reported.
        assert!(tray_icon_at(bar, r.left - 50, y).is_none());
    }

    #[test]
    fn finds_and_describes_a_window() {
        std::thread::spawn(|| unsafe {
            use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
            let inst = GetModuleHandleW(std::ptr::null());
            let cls = wide("NexusTargetTestWindow");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(DefWindowProcW),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: GetStockObject(WHITE_BRUSH),
                lpszMenuName: std::ptr::null(),
                lpszClassName: cls.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            RegisterClassExW(&wc);
            let title = wide("Nexus target test");
            let h = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, cls.as_ptr(), title.as_ptr(), WS_POPUP | WS_VISIBLE, 40, 40, 220, 160, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
            assert!(!h.is_null());
            let info = describe(h).unwrap();
            assert_eq!(info.pid, std::process::id());
            assert_eq!(info.title, "Nexus target test");
            assert_eq!(info.class_name, "NexusTargetTestWindow");
            assert!(info.shell.is_none());
            // Topmost window under its centre is itself (unless another
            // topmost window covers it) and never one of the excluded pid.
            let found = window_at(150, 120, 0);
            assert!(found.is_some());
            assert_ne!(window_at(150, 120, std::process::id()), Some(h));
            DestroyWindow(h);
            UnregisterClassW(cls.as_ptr(), inst);
        })
        .join()
        .unwrap();
    }
}
