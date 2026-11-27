pub mod ipc;

#[cfg(windows)]
pub mod win32;

#[cfg(target_os = "linux")]
pub mod linux;

pub use ipc::{IpcCommand, SingleInstanceIpc};

use iris_core::config::AppSettings;
use iris_core::models::IntRect;
use iris_core::overlay::WindowMode;
use iris_core::pipeline::CapturedFrame;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GameWindowInfo {
    pub present: bool,
    pub focused: bool,
    pub minimized: bool,
    pub bounds: IntRect,
    pub monitor_bounds: IntRect,
    pub window_mode: Option<WindowMode>,
    pub native_handle: u64,
    pub process_dir: Option<PathBuf>,
}

impl Default for GameWindowInfo {
    fn default() -> Self {
        Self {
            present: false,
            focused: false,
            minimized: false,
            bounds: IntRect::new(0, 0, 0, 0),
            monitor_bounds: IntRect::new(0, 0, 1920, 1080),
            window_mode: None,
            native_handle: 0,
            process_dir: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ToggleOverlay,
    TogglePlayersOrgs,
    TimedOverlay,
    InteractiveOverlay,
    SaveWaypoint,
    RadarZoomIn,
    RadarZoomOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSettings,
    ToggleOverlay,
    TogglePlayersOrgs,
    ToggleDebugOverlay,
    SaveWaypoint,
    CycleRadarScale,
    ClearBreadcrumbs,
    OpenLogs,
    OpenConfigDir,
    Quit,
}

pub trait ClipboardWatcher: Send {
    /// Returns `Some(text)` when the OS clipboard text has changed since the last poll.
    fn poll_text(&mut self) -> Option<String>;
}

pub trait DisplayInfoOcrReader: Send {
    /// Submits a top-right BGR24 crop (`width * height * 3` bytes) for asynchronous OCR.
    fn submit_top_right_bgr(&self, bgr: Vec<u8>, width: usize, height: usize);
    /// Returns the most recent recognized OCR text if a background job completed.
    fn try_recv_text(&self) -> Option<String>;
}

pub trait GameWindowDetector: Send {
    fn poll(&mut self) -> GameWindowInfo;
}

pub trait FrameCapturer: Send {
    /// Captures the latest game frame. Returns `Ok(Some(frame))` when a new frame is available,
    /// `Ok(None)` on benign timeout (no screen update yet), or `Err(...)` on device loss/error.
    /// May update `settings.linux_portal_restore_token` on Linux when XDG ScreenCast grants a token,
    /// returning `true` in the boolean tuple slot if `settings` was mutated and should be saved.
    fn capture_frame(
        &mut self,
        game: &GameWindowInfo,
        settings: &mut AppSettings,
        now_ms: f64,
    ) -> anyhow::Result<(Option<CapturedFrame>, bool)>;

    fn reset(&mut self);

    /// Returns a processed frame's backing buffer to the capturer so subsequent frames reuse warm pages.
    fn recycle_frame(&mut self, _frame: CapturedFrame) {}
}

pub trait OverlayWindowBackend: Send {
    /// Updates and shows the click-through transparent overlay window with premultiplied BGRA8 pixels.
    fn present(
        &mut self,
        bounds: IntRect,
        bgra_premul: &[u8],
        visible_in_capture: bool,
    ) -> anyhow::Result<()>;

    /// Hides the click-through overlay window without destroying resources if possible.
    fn hide(&mut self);
}

pub trait HotkeyListener: Send {
    fn update_bindings(&mut self, settings: &AppSettings);
    fn try_recv(&self) -> Option<HotkeyAction>;
}

pub trait TrayBackend: Send {
    fn set_overlay_enabled(&mut self, enabled: bool);
    fn set_players_orgs_enabled(&mut self, enabled: bool);
    fn set_debug_enabled(&mut self, enabled: bool);
    fn set_radar_range_km(&mut self, range_km: i32);
    fn try_recv(&self) -> Option<TrayAction>;
}

pub fn create_game_detector() -> Box<dyn GameWindowDetector> {
    #[cfg(windows)]
    {
        Box::new(win32::Win32GameDetector::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxGameDetector::new())
    }
}

pub fn create_frame_capturer() -> Box<dyn FrameCapturer> {
    #[cfg(windows)]
    {
        Box::new(win32::Win32FrameCapturer::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxFrameCapturer::new())
    }
}

pub fn create_overlay_window() -> anyhow::Result<Box<dyn OverlayWindowBackend>> {
    #[cfg(windows)]
    {
        Ok(Box::new(win32::Win32OverlayWindow::new()?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxOverlayWindow::new()?))
    }
}

pub fn create_hotkey_listener(settings: &AppSettings) -> Box<dyn HotkeyListener> {
    #[cfg(windows)]
    {
        Box::new(win32::Win32HotkeyListener::new(settings))
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxHotkeyListener::new(settings))
    }
}

pub fn create_clipboard_watcher() -> Box<dyn ClipboardWatcher> {
    #[cfg(windows)]
    {
        Box::new(win32::Win32ClipboardWatcher::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxClipboardWatcher::new())
    }
}

pub fn create_display_info_ocr() -> Box<dyn DisplayInfoOcrReader> {
    #[cfg(windows)]
    {
        Box::new(win32::Win32DisplayInfoOcr::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxDisplayInfoOcr::new())
    }
}

/// Converts a top-right BGR24 crop into an upscaled (up to 3x, capped at 3600px for WinRT
/// `OcrEngine::MaxImageDimension()`), dark-on-light grayscale image (`Gray8`) optimized for OCR
/// of Star Citizen's small `r_DisplayInfo` text — using fast local adaptive background subtraction
/// so bright daytime planets, clouds, or cockpit glare never wash out the telemetry text.
pub(crate) fn prepare_display_info_gray_3x(
    bgr: &[u8],
    width: usize,
    height: usize,
) -> (Vec<u8>, usize, usize) {
    if width < 8 || height < 8 || bgr.len() < width * height * 3 {
        return (Vec::new(), 0, 0);
    }
    let count = width * height;
    let mut lum = vec![0u8; count];
    for (i, px) in bgr.chunks_exact(3).take(count).enumerate() {
        let luma = ((px[2] as u32 * 77 + px[1] as u32 * 150 + px[0] as u32 * 29) >> 8) as u8;
        let max_ch = px[0].max(px[1]).max(px[2]);
        lum[i] = ((luma as u16 + max_ch as u16) / 2) as u8;
    }

    // Compute a fast 2D integral image to get each pixel's local 19x19 background luminance.
    // Bright r_DisplayInfo glyph strokes stand out sharply above their immediate neighborhood
    // both against black space AND against bright planetary clouds/terrain.
    let stride = width + 1;
    let mut sat = vec![0u32; stride * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0u32;
        let src_row = y * width;
        let dst_row = (y + 1) * stride;
        let prev_row = y * stride;
        for x in 0..width {
            row_sum += lum[src_row + x] as u32;
            sat[dst_row + x + 1] = sat[prev_row + x + 1] + row_sum;
        }
    }

    let rad: isize = 9;
    let mut enhanced = vec![255u8; count];
    for y in 0..height {
        let y0 = (y as isize - rad).max(0) as usize;
        let y1 = (y as isize + rad + 1).min(height as isize) as usize;
        let r0 = y0 * stride;
        let r1 = y1 * stride;
        let row_out = y * width;
        for x in 0..width {
            let x0 = (x as isize - rad).max(0) as usize;
            let x1 = (x as isize + rad + 1).min(width as isize) as usize;
            let area = ((y1 - y0) * (x1 - x0)) as u32;
            let sum = sat[r1 + x1] + sat[r0 + x0] - sat[r1 + x0] - sat[r0 + x1];
            let local_bg = (sum / area.max(1)) as f32;
            let v = lum[row_out + x] as f32;

            // Measure how much brighter this pixel is than its local 19x19 background.
            // Over bright sky (bg ~210), white text (250) still has diff ~40 -> crisp ink.
            // Smooth bright clouds (bg ~220, v ~220) have diff ~0 -> clean white paper.
            let diff = (v - local_bg).max(0.0);
            let span = (255.0 - local_bg).clamp(28.0, 95.0);
            let ink = ((diff - 6.0) / span).clamp(0.0, 1.0).powf(0.8);
            enhanced[row_out + x] = ((1.0 - ink) * 255.0).round() as u8;
        }
    }

    // Choose integer scale up to 3x while staying strictly below WinRT OcrEngine's 4096px limit
    let max_dim = width.max(height);
    let scale = if max_dim * 3 <= 3600 {
        3usize
    } else if max_dim * 2 <= 3600 {
        2usize
    } else {
        1usize
    };
    let w3 = width * scale;
    let h3 = height * scale;
    if scale == 1 {
        return (enhanced, w3, h3);
    }
    let inv_s = 1.0 / scale as f32;
    let mut out = vec![0u8; w3 * h3];
    for y3 in 0..h3 {
        let sy = (y3 as f32 + 0.5) * inv_s - 0.5;
        let y0 = (sy.floor().max(0.0) as usize).min(height - 1);
        let y1 = (y0 + 1).min(height - 1);
        let fy = (sy - y0 as f32).clamp(0.0, 1.0);
        let row0 = y0 * width;
        let row1 = y1 * width;
        let dst_row = y3 * w3;
        for x3 in 0..w3 {
            let sx = (x3 as f32 + 0.5) * inv_s - 0.5;
            let x0 = (sx.floor().max(0.0) as usize).min(width - 1);
            let x1 = (x0 + 1).min(width - 1);
            let fx = (sx - x0 as f32).clamp(0.0, 1.0);
            let a = enhanced[row0 + x0] as f32;
            let b = enhanced[row0 + x1] as f32;
            let c = enhanced[row1 + x0] as f32;
            let d = enhanced[row1 + x1] as f32;
            let top = a + (b - a) * fx;
            let bot = c + (d - c) * fx;
            out[dst_row + x3] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
        }
    }
    (out, w3, h3)
}

pub fn create_tray_backend(
    overlay_enabled: bool,
    players_orgs_enabled: bool,
    debug_enabled: bool,
    radar_range_km: i32,
) -> anyhow::Result<Box<dyn TrayBackend>> {
    #[cfg(windows)]
    {
        Ok(Box::new(win32::Win32TrayBackend::new(
            overlay_enabled,
            players_orgs_enabled,
            debug_enabled,
            radar_range_km,
        )?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxTrayBackend::new(
            overlay_enabled,
            players_orgs_enabled,
            debug_enabled,
            radar_range_km,
        )?))
    }
}

/// Safely opens an `https://` URL in the user's default browser.
pub fn open_https_url(url: &str) -> anyhow::Result<()> {
    let trimmed = url.trim();
    if !trimmed.starts_with("https://") {
        anyhow::bail!("Refusing to open non-HTTPS URL: {}", trimmed);
    }
    open::that_detached(trimmed)?;
    Ok(())
}

/// Opens a local directory or log file in Explorer (Windows) or Dolphin/Nautilus (`xdg-open` on Bazzite).
pub fn open_folder_or_file(path: &Path) -> anyhow::Result<()> {
    open::that_detached(path)?;
    Ok(())
}

pub(crate) const TRAY_ON_ICO: &[u8] = include_bytes!("../../../assets/tray-on.ico");
pub(crate) const TRAY_OFF_ICO: &[u8] = include_bytes!("../../../assets/tray-off.ico");
