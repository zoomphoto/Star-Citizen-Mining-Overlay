use crate::{TrayAction, TrayBackend, TRAY_OFF_ICO, TRAY_ON_ICO};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, GetDC, ReleaseDC,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, HICON, ICONINFO,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WM_APP, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP,
    WNDCLASSW,
};

const WM_TRAY_CALLBACK: u32 = WM_APP + 42;
const WM_TRAY_UPDATE_ICON: u32 = WM_APP + 43;
const WM_TRAY_SHUTDOWN: u32 = WM_APP + 44;

const ID_SETTINGS: usize = 1001;
const ID_TOGGLE: usize = 1002;
const ID_LOGS: usize = 1003;
const ID_CONFIG: usize = 1004;
const ID_QUIT: usize = 1005;
const ID_DEBUG: usize = 1006;
const ID_CLEAR_BREADCRUMBS: usize = 1007;

struct TrayThreadShared {
    overlay_enabled: Arc<AtomicBool>,
    debug_enabled: Arc<AtomicBool>,
    tx: Sender<TrayAction>,
    icon_on: HICON,
    icon_off: HICON,
}

unsafe impl Send for TrayThreadShared {}
unsafe impl Sync for TrayThreadShared {}

static TRAY_SHARED: OnceLock<TrayThreadShared> = OnceLock::new();

pub struct Win32TrayBackend {
    hwnd: isize,
    overlay_enabled: Arc<AtomicBool>,
    debug_enabled: Arc<AtomicBool>,
    rx: Receiver<TrayAction>,
}

impl Win32TrayBackend {
    pub fn new(overlay_enabled: bool, debug_enabled: bool) -> anyhow::Result<Self> {
        let enabled_flag = Arc::new(AtomicBool::new(overlay_enabled));
        let debug_flag = Arc::new(AtomicBool::new(debug_enabled));
        let (tx, rx) = unbounded();
        let (hwnd_tx, hwnd_rx) = crossbeam_channel::bounded::<anyhow::Result<isize>>(1);

        let flag_clone = Arc::clone(&enabled_flag);
        let debug_clone = Arc::clone(&debug_flag);
        thread::Builder::new()
            .name("sc-overlay-win32-tray".to_string())
            .spawn(move || unsafe {
                let icon_on = decode_ico_to_hicon(TRAY_ON_ICO).unwrap_or_default();
                let icon_off = decode_ico_to_hicon(TRAY_OFF_ICO).unwrap_or_default();

                let _ = TRAY_SHARED.set(TrayThreadShared {
                    overlay_enabled: Arc::clone(&flag_clone),
                    debug_enabled: Arc::clone(&debug_clone),
                    tx,
                    icon_on,
                    icon_off,
                });

                let Ok(hinstance) = GetModuleHandleW(None) else {
                    let _ = hwnd_tx.send(Err(anyhow::anyhow!("GetModuleHandleW failed")));
                    return;
                };
                let class_name: Vec<u16> = "ScOverlayTrayMsgWnd\0".encode_utf16().collect();
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(tray_wnd_proc),
                    hInstance: hinstance.into(),
                    lpszClassName: PCWSTR(class_name.as_ptr()),
                    ..Default::default()
                };
                let _ = RegisterClassW(&wc);

                let Ok(hwnd) = CreateWindowExW(
                    Default::default(),
                    PCWSTR(class_name.as_ptr()),
                    PCWSTR(class_name.as_ptr()),
                    Default::default(),
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    hinstance,
                    None,
                ) else {
                    let _ = hwnd_tx.send(Err(anyhow::anyhow!("CreateWindowExW for tray failed")));
                    return;
                };

                let mut nid = build_nid(
                    hwnd,
                    if flag_clone.load(Ordering::Relaxed) {
                        icon_on
                    } else {
                        icon_off
                    },
                    flag_clone.load(Ordering::Relaxed),
                );
                let _ = Shell_NotifyIconW(NIM_ADD, &nid);

                let _ = hwnd_tx.send(Ok(hwnd.0 as isize));

                let mut msg = MSG::default();
                while windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0)
                    .as_bool()
                {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                nid.uFlags = Default::default();
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                if !icon_on.0.is_null() {
                    let _ = DestroyIcon(icon_on);
                }
                if !icon_off.0.is_null() {
                    let _ = DestroyIcon(icon_off);
                }
            })?;

        let hwnd = hwnd_rx.recv()??;
        Ok(Self {
            hwnd,
            overlay_enabled: enabled_flag,
            debug_enabled: debug_flag,
            rx,
        })
    }
}

impl TrayBackend for Win32TrayBackend {
    fn set_overlay_enabled(&mut self, enabled: bool) {
        self.overlay_enabled.store(enabled, Ordering::Relaxed);
        unsafe {
            let _ = PostMessageW(
                HWND(self.hwnd as *mut _),
                WM_TRAY_UPDATE_ICON,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    fn set_debug_enabled(&mut self, enabled: bool) {
        self.debug_enabled.store(enabled, Ordering::Relaxed);
    }

    fn try_recv(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Win32TrayBackend {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(
                HWND(self.hwnd as *mut _),
                WM_TRAY_SHUTDOWN,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

fn build_nid(hwnd: HWND, icon: HICON, enabled: bool) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY_CALLBACK,
        hIcon: icon,
        ..Default::default()
    };
    let tip = if enabled {
        "SC Overlay (Active)"
    } else {
        "SC Overlay (Disabled)"
    };
    for (i, ch) in tip.encode_utf16().take(nid.szTip.len() - 1).enumerate() {
        nid.szTip[i] = ch;
    }
    nid
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY_UPDATE_ICON => {
            if let Some(shared) = TRAY_SHARED.get() {
                let enabled = shared.overlay_enabled.load(Ordering::Relaxed);
                let icon = if enabled {
                    shared.icon_on
                } else {
                    shared.icon_off
                };
                let nid = build_nid(hwnd, icon, enabled);
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
            LRESULT(0)
        }
        WM_TRAY_SHUTDOWN => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_TRAY_CALLBACK => {
            let mouse_msg = (lparam.0 & 0xFFFF) as u32;
            if let Some(shared) = TRAY_SHARED.get() {
                if mouse_msg == WM_LBUTTONDBLCLK {
                    let _ = shared.tx.send(TrayAction::OpenSettings);
                } else if mouse_msg == WM_RBUTTONUP {
                    show_context_menu(hwnd, shared);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_context_menu(hwnd: HWND, shared: &TrayThreadShared) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    let enabled = shared.overlay_enabled.load(Ordering::Relaxed);
    let toggle_label = if enabled {
        "Disable Overlay\0"
    } else {
        "Enable Overlay\0"
    };
    let debug_on = shared.debug_enabled.load(Ordering::Relaxed);
    let debug_label = if debug_on {
        "Hide Debug HUD\0"
    } else {
        "Show Debug HUD\0"
    };

    let w_settings: Vec<u16> = "Settings…\0".encode_utf16().collect();
    let w_toggle: Vec<u16> = toggle_label.encode_utf16().collect();
    let w_debug: Vec<u16> = debug_label.encode_utf16().collect();
    let w_clear_trail: Vec<u16> = "Clear Breadcrumb Trail\0".encode_utf16().collect();
    let w_logs: Vec<u16> = "Open log folder\0".encode_utf16().collect();
    let w_cfg: Vec<u16> = "Open config location\0".encode_utf16().collect();
    let w_quit: Vec<u16> = "Quit\0".encode_utf16().collect();

    let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, PCWSTR(w_settings.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_TOGGLE, PCWSTR(w_toggle.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_DEBUG, PCWSTR(w_debug.as_ptr()));
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        ID_CLEAR_BREADCRUMBS,
        PCWSTR(w_clear_trail.as_ptr()),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_LOGS, PCWSTR(w_logs.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_CONFIG, PCWSTR(w_cfg.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(w_quit.as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);

    let cmd = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);

    match cmd.0 as usize {
        ID_SETTINGS => {
            let _ = shared.tx.send(TrayAction::OpenSettings);
        }
        ID_TOGGLE => {
            let _ = shared.tx.send(TrayAction::ToggleOverlay);
        }
        ID_DEBUG => {
            let _ = shared.tx.send(TrayAction::ToggleDebugOverlay);
        }
        ID_CLEAR_BREADCRUMBS => {
            let _ = shared.tx.send(TrayAction::ClearBreadcrumbs);
        }
        ID_LOGS => {
            let _ = shared.tx.send(TrayAction::OpenLogs);
        }
        ID_CONFIG => {
            let _ = shared.tx.send(TrayAction::OpenConfigDir);
        }
        ID_QUIT => {
            let _ = shared.tx.send(TrayAction::Quit);
        }
        _ => {}
    }
}

unsafe fn decode_ico_to_hicon(ico_bytes: &[u8]) -> Option<HICON> {
    let img = image::load_from_memory(ico_bytes).ok()?.to_rgba8();
    let w = img.width() as i32;
    let h = img.height() as i32;
    if w <= 0 || h <= 0 {
        return None;
    }

    let screen_dc = GetDC(None);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let color_bmp = CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    ReleaseDC(None, screen_dc);

    if bits.is_null() {
        let _ = DeleteObject(color_bmp);
        return None;
    }

    let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
    for (i, px) in img.pixels().enumerate() {
        dst[i * 4] = px[2];
        dst[i * 4 + 1] = px[1];
        dst[i * 4 + 2] = px[0];
        dst[i * 4 + 3] = px[3];
    }

    let mask_bytes = vec![0u8; (((w + 15) / 16) * 2 * h) as usize];
    let mask_bmp = CreateBitmap(w, h, 1, 1, Some(mask_bytes.as_ptr() as *const _));

    let info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask_bmp,
        hbmColor: color_bmp,
    };
    let hicon = CreateIconIndirect(&info).ok();
    let _ = DeleteObject(color_bmp);
    if !mask_bmp.0.is_null() {
        let _ = DeleteObject(mask_bmp);
    }
    hicon
}

