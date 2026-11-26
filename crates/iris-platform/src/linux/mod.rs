pub mod capture;
pub mod clipboard;
pub mod game_window;
pub mod hotkeys;
pub mod ocr;
pub mod overlay;
pub mod tray;

pub use capture::LinuxFrameCapturer;
pub use clipboard::LinuxClipboardWatcher;
pub use game_window::LinuxGameDetector;
pub use hotkeys::LinuxHotkeyListener;
pub use ocr::LinuxDisplayInfoOcr;
pub use overlay::LinuxOverlayWindow;
pub use tray::LinuxTrayBackend;

