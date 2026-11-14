use crate::{GameWindowDetector, GameWindowInfo};
use iris_core::models::IntRect;
use iris_core::overlay::{StarCitizenDisplaySettings, WindowMode};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetForegroundWindow, GetSystemMetrics, GetWindowLongW,
    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE,
    SM_CXSCREEN, SM_CYSCREEN, WS_CAPTION, WS_EX_TOPMOST, WS_THICKFRAME,
};

struct CachedProc {
    is_star_citizen: bool,
    exe_path: Option<PathBuf>,
    checked_at: Instant,
}

pub struct Win32GameDetector {
    proc_cache: HashMap<u32, CachedProc>,
    cached_xml_mode: Option<(PathBuf, Instant, Option<WindowMode>)>,
}

impl Win32GameDetector {
    pub fn new() -> Self {
        Self {
            proc_cache: HashMap::new(),
            cached_xml_mode: None,
        }
    }

    fn inspect_pid(&mut self, pid: u32) -> (bool, Option<PathBuf>) {
        if pid == 0 {
            return (false, None);
        }
        let now = Instant::now();
        if let Some(entry) = self.proc_cache.get(&pid) {
            let ttl = if entry.is_star_citizen {
                Duration::from_secs(60)
            } else {
                Duration::from_secs(5)
            };
            if now.duration_since(entry.checked_at) < ttl {
                return (entry.is_star_citizen, entry.exe_path.clone());
            }
        }

        let mut exe_path: Option<PathBuf> = None;
        let mut is_sc = false;

        unsafe {
            if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut buf = [0u16; 1024];
                let mut size = buf.len() as u32;
                if QueryFullProcessImageNameW(
                    handle,
                    PROCESS_NAME_WIN32,
                    windows::core::PWSTR(buf.as_mut_ptr()),
                    &mut size,
                )
                .is_ok()
                {
                    let s = String::from_utf16_lossy(&buf[..size as usize]);
                    let p = PathBuf::from(&s);
                    if p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case("StarCitizen.exe"))
                    {
                        is_sc = true;
                    }
                    exe_path = Some(p);
                }
                let _ = CloseHandle(handle);
            }
        }

        if self.proc_cache.len() > 512 {
            self.proc_cache.clear();
        }
        self.proc_cache.insert(
            pid,
            CachedProc {
                is_star_citizen: is_sc,
                exe_path: exe_path.clone(),
                checked_at: now,
            },
        );
        (is_sc, exe_path)
    }

    fn read_xml_window_mode(&mut self, exe_path: &PathBuf) -> Option<WindowMode> {
        let now = Instant::now();
        if let Some((cached_exe, ts, mode)) = &self.cached_xml_mode {
            if cached_exe == exe_path && now.duration_since(*ts) < Duration::from_secs(2) {
                return *mode;
            }
        }
        let mode = StarCitizenDisplaySettings::attributes_path_for_exe(exe_path)
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|xml| StarCitizenDisplaySettings::parse_window_mode(&xml));
        self.cached_xml_mode = Some((exe_path.clone(), now, mode));
        mode
    }

    fn window_bounds(hwnd: HWND) -> (IntRect, IntRect, bool, bool) {
        unsafe {
            let mut client = RECT::default();
            let mut client_rect = IntRect::default();
            if GetClientRect(hwnd, &mut client).is_ok()
                && client.right > client.left
                && client.bottom > client.top
            {
                let mut tl = POINT {
                    x: client.left,
                    y: client.top,
                };
                if ClientToScreen(hwnd, &mut tl).as_bool() {
                    client_rect = IntRect::new(
                        tl.x,
                        tl.y,
                        client.right - client.left,
                        client.bottom - client.top,
                    );
                }
            }

            if client_rect.is_empty() {
                let mut ext = RECT::default();
                if DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    &mut ext as *mut _ as *mut _,
                    std::mem::size_of::<RECT>() as u32,
                )
                .is_ok()
                {
                    client_rect =
                        IntRect::new(ext.left, ext.top, ext.right - ext.left, ext.bottom - ext.top);
                } else if GetWindowRect(hwnd, &mut ext).is_ok() {
                    client_rect =
                        IntRect::new(ext.left, ext.top, ext.right - ext.left, ext.bottom - ext.top);
                }
            }

            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let mon_rect = if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                IntRect::new(
                    mi.rcMonitor.left,
                    mi.rcMonitor.top,
                    mi.rcMonitor.right - mi.rcMonitor.left,
                    mi.rcMonitor.bottom - mi.rcMonitor.top,
                )
            } else {
                IntRect::new(
                    0,
                    0,
                    GetSystemMetrics(SM_CXSCREEN).max(1920),
                    GetSystemMetrics(SM_CYSCREEN).max(1080),
                )
            };

            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let has_frame = (style & (WS_CAPTION.0 | WS_THICKFRAME.0)) != 0;
            let is_topmost = (ex_style & WS_EX_TOPMOST.0) != 0;
            (client_rect, mon_rect, has_frame, is_topmost)
        }
    }
}

impl Default for Win32GameDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GameWindowDetector for Win32GameDetector {
    fn poll(&mut self) -> GameWindowInfo {
        unsafe {
            let fg = GetForegroundWindow();
            let mut fg_pid = 0u32;
            if !fg.0.is_null() {
                GetWindowThreadProcessId(fg, Some(&mut fg_pid));
            }
            if fg_pid != 0 {
                let (is_sc, exe_path) = self.inspect_pid(fg_pid);
                if is_sc && IsWindowVisible(fg).as_bool() {
                    let minimized = IsIconic(fg).as_bool();
                    let (bounds, mon_bounds, has_frame, _is_topmost) = Self::window_bounds(fg);
                    let xml_mode = exe_path.as_ref().and_then(|p| self.read_xml_window_mode(p));
                    let window_mode = Some(xml_mode.unwrap_or_else(|| {
                        let covers_mon = (bounds.x - mon_bounds.x).abs() <= 2
                            && (bounds.y - mon_bounds.y).abs() <= 2
                            && (bounds.width - mon_bounds.width).abs() <= 4
                            && (bounds.height - mon_bounds.height).abs() <= 4;
                        if covers_mon && !has_frame {
                            WindowMode::Borderless
                        } else {
                            WindowMode::Windowed
                        }
                    }));
                    return GameWindowInfo {
                        present: true,
                        focused: true,
                        minimized,
                        bounds,
                        monitor_bounds: mon_bounds,
                        window_mode,
                        native_handle: fg.0 as usize as u64,
                        process_dir: exe_path.and_then(|p| p.parent().map(|d| d.to_path_buf())),
                    };
                }
            }

            // Enumerate top-level visible windows to see if Star Citizen is running in background
            let mut candidates: Vec<(HWND, u32)> = Vec::new();
            unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
                if IsWindowVisible(hwnd).as_bool() {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    if pid != 0 {
                        let vec = &mut *(lparam.0 as *mut Vec<(HWND, u32)>);
                        vec.push((hwnd, pid));
                    }
                }
                BOOL(1)
            }
            let _ = EnumWindows(Some(enum_cb), LPARAM(&mut candidates as *mut _ as isize));

            for (hwnd, pid) in candidates {
                let (is_sc, exe_path) = self.inspect_pid(pid);
                if !is_sc {
                    continue;
                }
                let minimized = IsIconic(hwnd).as_bool();
                let (bounds, mon_bounds, has_frame, _) = Self::window_bounds(hwnd);
                if !minimized && (bounds.width < 320 || bounds.height < 200) {
                    continue;
                }
                let xml_mode = exe_path.as_ref().and_then(|p| self.read_xml_window_mode(p));
                let window_mode = Some(xml_mode.unwrap_or(if has_frame {
                    WindowMode::Windowed
                } else {
                    WindowMode::Borderless
                }));
                return GameWindowInfo {
                    present: true,
                    focused: false,
                    minimized,
                    bounds,
                    monitor_bounds: mon_bounds,
                    window_mode,
                    native_handle: hwnd.0 as usize as u64,
                    process_dir: exe_path.and_then(|p| p.parent().map(|d| d.to_path_buf())),
                };
            }

            let mon_w = GetSystemMetrics(SM_CXSCREEN).max(1920);
            let mon_h = GetSystemMetrics(SM_CYSCREEN).max(1080);
            GameWindowInfo {
                present: false,
                focused: false,
                minimized: false,
                bounds: IntRect::new(0, 0, 0, 0),
                monitor_bounds: IntRect::new(0, 0, mon_w, mon_h),
                window_mode: None,
                native_handle: 0,
                process_dir: None,
            }
        }
    }
}
