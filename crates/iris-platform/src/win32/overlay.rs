use crate::OverlayWindowBackend;
use crossbeam_channel::{bounded, Sender};
use iris_core::models::IntRect;
use std::ffi::c_void;
use std::sync::Arc;
use std::thread;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP,
    HDC, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_TOPMOST, MSG,
    PeekMessageW, PM_REMOVE, PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowDisplayAffinity,
    SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WINDOW_DISPLAY_AFFINITY, WM_DESTROY, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

enum OverlayCmd {
    Present {
        bounds: IntRect,
        pixels: Arc<Vec<u8>>,
        visible_in_capture: bool,
    },
    Hide,
    Shutdown,
}

pub struct Win32OverlayWindow {
    tx: Sender<OverlayCmd>,
}

impl Win32OverlayWindow {
    pub fn new() -> anyhow::Result<Self> {
        let (tx, rx) = bounded::<OverlayCmd>(2);
        let (ready_tx, ready_rx) = bounded::<anyhow::Result<()>>(1);

        thread::Builder::new()
            .name("sc-overlay-win32-layer".to_string())
            .spawn(move || {
                if let Err(e) = Self::run_window_thread(rx, ready_tx) {
                    tracing::error!("Win32 overlay thread failed: {:#}", e);
                }
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx }),
            Ok(Err(e)) => Err(e),
            Err(_) => anyhow::bail!("Win32 overlay thread exited before initialization"),
        }
    }

    fn run_window_thread(
        rx: crossbeam_channel::Receiver<OverlayCmd>,
        ready_tx: Sender<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let class_name: Vec<u16> = "ScOverlayLayeredWnd\0".encode_utf16().collect();

            unsafe extern "system" fn wnd_proc(
                hwnd: HWND,
                msg: u32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                if msg == WM_DESTROY {
                    PostQuitMessage(0);
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let ex_style = WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE;
            let title: Vec<u16> = "SC Overlay\0".encode_utf16().collect();

            let hwnd = CreateWindowExW(
                ex_style,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                hinstance,
                None,
            )?;

            // Exclude overlay from capture by default so Desktop Duplication never reads our own brackets/panel
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            let mut current_excluded = true;
            let mut visible = false;

            let mut dib_w = 0i32;
            let mut dib_h = 0i32;
            let mut mem_dc = HDC::default();
            let mut dib_bmp = HBITMAP::default();
            let mut dib_bits: *mut c_void = std::ptr::null_mut();

            let _ = ready_tx.send(Ok(()));

            let mut msg = MSG::default();
            loop {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                // Coalesce queued Present commands so we always render the freshest frame
                let Ok(mut cmd) = rx.recv() else {
                    break;
                };
                while let Ok(newer) = rx.try_recv() {
                    if matches!(newer, OverlayCmd::Shutdown) {
                        cmd = OverlayCmd::Shutdown;
                        break;
                    }
                    cmd = newer;
                }

                match cmd {
                    OverlayCmd::Shutdown => break,
                    OverlayCmd::Hide => {
                        if visible {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                            visible = false;
                        }
                    }
                    OverlayCmd::Present {
                        bounds,
                        pixels,
                        visible_in_capture,
                    } => {
                        if bounds.width <= 0 || bounds.height <= 0 {
                            if visible {
                                let _ = ShowWindow(hwnd, SW_HIDE);
                                visible = false;
                            }
                            continue;
                        }
                        let expected_len = (bounds.width as usize) * (bounds.height as usize) * 4;
                        if pixels.len() < expected_len {
                            continue;
                        }

                        let want_excluded = !visible_in_capture;
                        if want_excluded != current_excluded {
                            let affinity: WINDOW_DISPLAY_AFFINITY = if want_excluded {
                                WDA_EXCLUDEFROMCAPTURE
                            } else {
                                WDA_NONE
                            };
                            let _ = SetWindowDisplayAffinity(hwnd, affinity);
                            current_excluded = want_excluded;
                        }

                        let screen_dc = GetDC(None);
                        if mem_dc.0.is_null() {
                            mem_dc = CreateCompatibleDC(screen_dc);
                        }

                        if dib_w != bounds.width || dib_h != bounds.height || dib_bits.is_null() {
                            if !dib_bmp.0.is_null() {
                                let _ = DeleteObject(dib_bmp);
                                dib_bmp = HBITMAP::default();
                                dib_bits = std::ptr::null_mut();
                            }
                            let bmi = BITMAPINFO {
                                bmiHeader: BITMAPINFOHEADER {
                                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                                    biWidth: bounds.width,
                                    biHeight: -bounds.height, // top-down BGRA8
                                    biPlanes: 1,
                                    biBitCount: 32,
                                    biCompression: BI_RGB.0,
                                    ..Default::default()
                                },
                                ..Default::default()
                            };
                            if let Ok(bmp) = CreateDIBSection(
                                mem_dc,
                                &bmi,
                                DIB_RGB_COLORS,
                                &mut dib_bits,
                                None,
                                0,
                            ) {
                                dib_bmp = bmp;
                                dib_w = bounds.width;
                                dib_h = bounds.height;
                                let _ = SelectObject(mem_dc, dib_bmp);
                            }
                        }

                        if !dib_bits.is_null() {
                            std::ptr::copy_nonoverlapping(
                                pixels.as_ptr(),
                                dib_bits as *mut u8,
                                expected_len,
                            );

                            let dst_pt = POINT {
                                x: bounds.x,
                                y: bounds.y,
                            };
                            let size = SIZE {
                                cx: bounds.width,
                                cy: bounds.height,
                            };
                            let src_pt = POINT { x: 0, y: 0 };
                            let blend = BLENDFUNCTION {
                                BlendOp: AC_SRC_OVER as u8,
                                BlendFlags: 0,
                                SourceConstantAlpha: 255,
                                AlphaFormat: AC_SRC_ALPHA as u8,
                            };

                            let _ = UpdateLayeredWindow(
                                hwnd,
                                screen_dc,
                                Some(&dst_pt),
                                Some(&size),
                                mem_dc,
                                Some(&src_pt),
                                COLORREF(0),
                                Some(&blend),
                                ULW_ALPHA,
                            );

                            if !visible {
                                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                                visible = true;
                            }
                            let _ = SetWindowPos(
                                hwnd,
                                HWND_TOPMOST,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                            );
                        }

                        ReleaseDC(None, screen_dc);
                    }
                }
            }

            if !dib_bmp.0.is_null() {
                let _ = DeleteObject(dib_bmp);
            }
            if !mem_dc.0.is_null() {
                let _ = DeleteDC(mem_dc);
            }
            let _ = DestroyWindow(hwnd);
        }
        Ok(())
    }
}

impl OverlayWindowBackend for Win32OverlayWindow {
    fn present(
        &mut self,
        bounds: IntRect,
        bgra_premul: &[u8],
        visible_in_capture: bool,
    ) -> anyhow::Result<()> {
        let _ = self.tx.try_send(OverlayCmd::Present {
            bounds,
            pixels: Arc::new(bgra_premul.to_vec()),
            visible_in_capture,
        });
        Ok(())
    }

    fn hide(&mut self) {
        let _ = self.tx.send(OverlayCmd::Hide);
    }
}

impl Drop for Win32OverlayWindow {
    fn drop(&mut self) {
        let _ = self.tx.send(OverlayCmd::Shutdown);
    }
}
