pub mod capture;
pub mod clipboard;
pub mod game_window;
pub mod hotkeys;
pub mod ocr;
pub mod overlay;
pub mod tray;

pub use capture::Win32FrameCapturer;
pub use clipboard::Win32ClipboardWatcher;
pub use game_window::Win32GameDetector;
pub use hotkeys::Win32HotkeyListener;
pub use ocr::Win32DisplayInfoOcr;
pub use overlay::Win32OverlayWindow;
pub use tray::Win32TrayBackend;

