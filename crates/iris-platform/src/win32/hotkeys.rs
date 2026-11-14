use crate::{HotkeyAction, HotkeyListener};
use crossbeam_channel::{unbounded, Receiver, Sender};
use iris_core::config::{AppSettings, HotkeyChord, HotkeyText};
use parking_lot::RwLock;
use std::sync::{Arc, OnceLock};
use std::thread;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

#[derive(Clone, Default)]
struct ActiveBindings {
    toggle: Option<HotkeyChord>,
    timed: Option<HotkeyChord>,
    interactive: Option<HotkeyChord>,
}

struct HookGlobalState {
    bindings: Arc<RwLock<ActiveBindings>>,
    tx: Sender<HotkeyAction>,
    pressed_vks: RwLock<[bool; 256]>,
}

static HOOK_STATE: OnceLock<HookGlobalState> = OnceLock::new();

pub struct Win32HotkeyListener {
    bindings: Arc<RwLock<ActiveBindings>>,
    rx: Receiver<HotkeyAction>,
    thread_id: u32,
}

impl Win32HotkeyListener {
    pub fn new(settings: &AppSettings) -> Self {
        let bindings = Arc::new(RwLock::new(Self::parse_bindings(settings)));
        let (tx, rx) = unbounded();

        let _ = HOOK_STATE.set(HookGlobalState {
            bindings: Arc::clone(&bindings),
            tx,
            pressed_vks: RwLock::new([false; 256]),
        });

        let (tid_tx, tid_rx) = crossbeam_channel::bounded::<u32>(1);
        thread::Builder::new()
            .name("sc-overlay-win32-hotkeys".to_string())
            .spawn(move || unsafe {
                let tid = GetCurrentThreadId();
                let _ = tid_tx.send(tid);
                let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_kbd_proc), None, 0)
                else {
                    tracing::error!("Failed to install WH_KEYBOARD_LL hook");
                    return;
                };
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.message == WM_QUIT {
                        break;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let _ = UnhookWindowsHookEx(hook);
            })
            .ok();

        let thread_id = tid_rx.recv().unwrap_or(0);
        Self {
            bindings,
            rx,
            thread_id,
        }
    }

    fn parse_bindings(settings: &AppSettings) -> ActiveBindings {
        ActiveBindings {
            toggle: HotkeyText::parse(&settings.hotkey_toggle_overlay).ok(),
            timed: HotkeyText::parse(&settings.hotkey_timed_overlay).ok(),
            interactive: HotkeyText::parse(&settings.hotkey_interactive_overlay).ok(),
        }
    }
}

impl HotkeyListener for Win32HotkeyListener {
    fn update_bindings(&mut self, settings: &AppSettings) {
        let parsed = Self::parse_bindings(settings);
        *self.bindings.write() = parsed.clone();
        if let Some(global) = HOOK_STATE.get() {
            *global.bindings.write() = parsed;
        }
    }

    fn try_recv(&self) -> Option<HotkeyAction> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Win32HotkeyListener {
    fn drop(&mut self) {
        if self.thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

unsafe extern "system" fn low_level_kbd_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        if let Some(state) = HOOK_STATE.get() {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk = (kb.vkCode & 0xFF) as usize;
            let msg = wparam.0 as u32;

            if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                state.pressed_vks.write()[vk] = false;
            } else if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                let mut down = state.pressed_vks.write();
                let was_down = down[vk];
                down[vk] = true;
                drop(down);

                // Ignore auto-repeat so holding `Tab` doesn't spam restarts
                if !was_down {
                    if let Some(key_name) = vk_to_hotkey_name(kb.vkCode) {
                        let ctrl = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                        let alt = (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;
                        let shift = (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;

                        let matches = |opt: &Option<HotkeyChord>| -> bool {
                            let Some(c) = opt else {
                                return false;
                            };
                            c.ctrl == ctrl
                                && c.alt == alt
                                && c.shift == shift
                                && c.key.eq_ignore_ascii_case(&key_name)
                        };

                        let guard = state.bindings.read();
                        if matches(&guard.toggle) {
                            let _ = state.tx.send(HotkeyAction::ToggleOverlay);
                        } else if matches(&guard.interactive) {
                            let _ = state.tx.send(HotkeyAction::InteractiveOverlay);
                        } else if matches(&guard.timed) {
                            let _ = state.tx.send(HotkeyAction::TimedOverlay);
                        }
                    }
                }
            }
        }
    }
    // Always pass input through so Star Citizen receives Tab and all game keys!
    CallNextHookEx(None, code, wparam, lparam)
}

fn vk_to_hotkey_name(vk: u32) -> Option<String> {
    match vk {
        0x08 => Some("Backspace".to_string()),
        0x09 => Some("Tab".to_string()),
        0x0D => Some("Enter".to_string()),
        0x1B => Some("Escape".to_string()),
        0x20 => Some("Space".to_string()),
        0x21 => Some("PageUp".to_string()),
        0x22 => Some("PageDown".to_string()),
        0x23 => Some("End".to_string()),
        0x24 => Some("Home".to_string()),
        0x25 => Some("Left".to_string()),
        0x26 => Some("Up".to_string()),
        0x27 => Some("Right".to_string()),
        0x28 => Some("Down".to_string()),
        0x2D => Some("Insert".to_string()),
        0x2E => Some("Delete".to_string()),
        0x30..=0x39 => Some(char::from_u32(vk)?.to_string()),
        0x41..=0x5A => Some(char::from_u32(vk)?.to_string()),
        0x60..=0x69 => Some(format!("NumPad{}", vk - 0x60)),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        0xBA => Some(";".to_string()),
        0xBB => Some("=".to_string()),
        0xBC => Some(",".to_string()),
        0xBD => Some("-".to_string()),
        0xBE => Some(".".to_string()),
        0xBF => Some("/".to_string()),
        0xC0 => Some("`".to_string()),
        0xDB => Some("[".to_string()),
        0xDC => Some("\\".to_string()),
        0xDD => Some("]".to_string()),
        0xDE => Some("'".to_string()),
        _ => None,
    }
}
