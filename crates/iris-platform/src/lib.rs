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
    TimedOverlay,
    InteractiveOverlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSettings,
    ToggleOverlay,
    ToggleDebugOverlay,
    OpenLogs,
    OpenConfigDir,
    Quit,
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
    fn set_debug_enabled(&mut self, enabled: bool);
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

pub fn create_tray_backend(
    overlay_enabled: bool,
    debug_enabled: bool,
) -> anyhow::Result<Box<dyn TrayBackend>> {
    #[cfg(windows)]
    {
        Ok(Box::new(win32::Win32TrayBackend::new(
            overlay_enabled,
            debug_enabled,
        )?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxTrayBackend::new(
            overlay_enabled,
            debug_enabled,
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
