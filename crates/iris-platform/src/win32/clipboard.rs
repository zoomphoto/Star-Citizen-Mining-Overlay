use crate::ClipboardWatcher;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;

/// Zero-overhead Win32 clipboard watcher using `GetClipboardSequenceNumber()`.
/// Only opens the clipboard when the kernel sequence counter actually advances.
pub struct Win32ClipboardWatcher {
    last_seq: u32,
}

impl Win32ClipboardWatcher {
    pub fn new() -> Self {
        let initial_seq = unsafe { GetClipboardSequenceNumber() };
        Self {
            last_seq: initial_seq,
        }
    }
}

impl Default for Win32ClipboardWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardWatcher for Win32ClipboardWatcher {
    fn poll_text(&mut self) -> Option<String> {
        let seq = unsafe { GetClipboardSequenceNumber() };
        if seq == 0 || seq == self.last_seq {
            return None;
        }
        // Only advance `last_seq` once we successfully open the clipboard so a transient
        // lock by Star Citizen doesn't cause us to miss a `/showlocation` copy.
        unsafe {
            if OpenClipboard(None).is_err() {
                return None;
            }
            self.last_seq = seq;
            let text = read_unicode_clipboard();
            let _ = CloseClipboard();
            text
        }
    }
}

unsafe fn read_unicode_clipboard() -> Option<String> {
    let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
    if handle.0.is_null() {
        return None;
    }
    let hglobal = HGLOBAL(handle.0);
    let byte_len = GlobalSize(hglobal);
    if byte_len < 2 {
        return None;
    }
    let ptr = GlobalLock(hglobal) as *const u16;
    if ptr.is_null() {
        return None;
    }
    let max_u16 = (byte_len / 2).min(65_536);
    let slice = std::slice::from_raw_parts(ptr, max_u16);
    let nul_pos = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    let text = String::from_utf16_lossy(&slice[..nul_pos]);
    let _ = GlobalUnlock(hglobal);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

