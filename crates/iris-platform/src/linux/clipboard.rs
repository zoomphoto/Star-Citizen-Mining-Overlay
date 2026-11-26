use crate::ClipboardWatcher;
use std::process::Command;
use std::time::{Duration, Instant};

/// Lightweight Linux clipboard watcher supporting Wayland (`wl-paste`) and X11 (`xclip` / `xsel`).
pub struct LinuxClipboardWatcher {
    last_poll: Instant,
    last_text: String,
    initialized: bool,
}

impl LinuxClipboardWatcher {
    pub fn new() -> Self {
        Self {
            last_poll: Instant::now() - Duration::from_secs(1),
            last_text: String::new(),
            initialized: false,
        }
    }

    fn read_clipboard_text() -> Option<String> {
        let candidates: &[(&str, &[&str])] = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            &[
                ("wl-paste", &["--no-newline", "--type", "text/plain"]),
                ("xclip", &["-o", "-selection", "clipboard"]),
                ("xsel", &["-ob"]),
            ]
        } else {
            &[
                ("xclip", &["-o", "-selection", "clipboard"]),
                ("xsel", &["-ob"]),
                ("wl-paste", &["--no-newline", "--type", "text/plain"]),
            ]
        };

        for (bin, args) in candidates {
            if let Ok(out) = Command::new(bin).args(*args).output() {
                if out.status.success() {
                    if let Ok(s) = String::from_utf8(out.stdout) {
                        return Some(s);
                    }
                }
            }
        }
        None
    }
}

impl Default for LinuxClipboardWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardWatcher for LinuxClipboardWatcher {
    fn poll_text(&mut self) -> Option<String> {
        if self.last_poll.elapsed() < Duration::from_millis(450) {
            return None;
        }
        self.last_poll = Instant::now();
        let current = Self::read_clipboard_text()?;
        if !self.initialized {
            self.initialized = true;
            self.last_text = current;
            return None;
        }
        if current == self.last_text || current.is_empty() {
            return None;
        }
        self.last_text = current.clone();
        Some(current)
    }
}

