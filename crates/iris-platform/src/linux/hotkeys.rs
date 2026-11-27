use crate::{HotkeyAction, HotkeyListener};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use crossbeam_channel::{unbounded, Receiver, Sender};
use evdev::{Device, InputEventKind, Key};
use futures_util_shim::StreamNonBlocking;
use iris_core::config::{AppSettings, HotkeyChord, HotkeyText};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Clone, Default)]
struct ActiveBindings {
    toggle: Option<HotkeyChord>,
    toggle_players_orgs: Option<HotkeyChord>,
    timed: Option<HotkeyChord>,
    interactive: Option<HotkeyChord>,
    save_waypoint: Option<HotkeyChord>,
    radar_zoom_in: Option<HotkeyChord>,
    radar_zoom_out: Option<HotkeyChord>,
}

pub struct LinuxHotkeyListener {
    bindings: Arc<RwLock<ActiveBindings>>,
    rx: Receiver<HotkeyAction>,
    stop: Arc<AtomicBool>,
}

impl LinuxHotkeyListener {
    pub fn new(settings: &AppSettings) -> Self {
        let bindings = Arc::new(RwLock::new(Self::parse_bindings(settings)));
        let (tx, rx) = unbounded();
        let stop = Arc::new(AtomicBool::new(false));

        // 1. Non-consuming `/dev/input/event*` listener when accessible (ideal for `Tab` peek on Wayland!)
        Self::spawn_evdev_listener(
            Arc::clone(&bindings),
            tx.clone(),
            Arc::clone(&stop),
        );

        // 2. XDG GlobalShortcuts portal listener for Wayland (KDE Plasma 6 / GNOME 47+)
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Self::spawn_portal_shortcuts(
                settings.clone(),
                tx.clone(),
                Arc::clone(&stop),
            );
        }

        Self { bindings, rx, stop }
    }

    fn parse_bindings(settings: &AppSettings) -> ActiveBindings {
        ActiveBindings {
            toggle: HotkeyText::parse(&settings.hotkey_toggle_overlay).ok(),
            toggle_players_orgs: HotkeyText::parse(&settings.hotkey_toggle_players_orgs).ok(),
            timed: HotkeyText::parse(&settings.hotkey_timed_overlay).ok(),
            interactive: HotkeyText::parse(&settings.hotkey_interactive_overlay).ok(),
            save_waypoint: HotkeyText::parse(&settings.hotkey_save_waypoint).ok(),
            radar_zoom_in: HotkeyText::parse(&settings.hotkey_radar_zoom_in).ok(),
            radar_zoom_out: HotkeyText::parse(&settings.hotkey_radar_zoom_out).ok(),
        }
    }

    fn spawn_evdev_listener(
        bindings: Arc<RwLock<ActiveBindings>>,
        tx: Sender<HotkeyAction>,
        stop: Arc<AtomicBool>,
    ) {
        let Ok(entries) = fs::read_dir("/dev/input") else {
            return;
        };
        let mut keyboards: Vec<Device> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("event") {
                continue;
            }
            if let Ok(dev) = Device::open(&path) {
                if dev
                    .supported_keys()
                    .is_some_and(|keys| keys.contains(Key::KEY_TAB) && keys.contains(Key::KEY_A))
                {
                    // Set non-blocking on fd
                    unsafe {
                        use std::os::fd::AsRawFd;
                        let fd = dev.as_raw_fd();
                        let flags = libc::fcntl(fd, libc::F_GETFL);
                        if flags >= 0 {
                            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                        }
                    }
                    keyboards.push(dev);
                }
            }
        }

        if keyboards.is_empty() {
            return;
        }

        tracing::info!(
            "Bound {} evdev keyboard device(s) for non-consuming hotkeys",
            keyboards.len()
        );

        thread::Builder::new()
            .name("sc-overlay-evdev-hotkeys".to_string())
            .spawn(move || {
                let mut held: HashSet<Key> = HashSet::new();
                while !stop.load(Ordering::Relaxed) {
                    let mut any_event = false;
                    for dev in &mut keyboards {
                        if let Ok(events) = dev.fetch_events() {
                            for ev in events {
                                any_event = true;
                                if let InputEventKind::Key(key) = ev.kind() {
                                    match ev.value() {
                                        0 => {
                                            held.remove(&key);
                                        }
                                        1 => {
                                            held.insert(key);
                                            if let Some(key_name) = evdev_key_name(key) {
                                                let ctrl = held.contains(&Key::KEY_LEFTCTRL)
                                                    || held.contains(&Key::KEY_RIGHTCTRL);
                                                let alt = held.contains(&Key::KEY_LEFTALT)
                                                    || held.contains(&Key::KEY_RIGHTALT);
                                                let shift = held.contains(&Key::KEY_LEFTSHIFT)
                                                    || held.contains(&Key::KEY_RIGHTSHIFT);

                                                let matches = |opt: &Option<HotkeyChord>| -> bool {
                                                    let Some(c) = opt else {
                                                        return false;
                                                    };
                                                    c.ctrl == ctrl
                                                        && c.alt == alt
                                                        && c.shift == shift
                                                        && c.key.eq_ignore_ascii_case(&key_name)
                                                };

                                                let guard = bindings.read();
                                                if matches(&guard.toggle) {
                                                    let _ = tx.send(HotkeyAction::ToggleOverlay);
                                                } else if matches(&guard.toggle_players_orgs) {
                                                    let _ =
                                                        tx.send(HotkeyAction::TogglePlayersOrgs);
                                                } else if matches(&guard.interactive) {
                                                    let _ =
                                                        tx.send(HotkeyAction::InteractiveOverlay);
                                                } else if matches(&guard.save_waypoint) {
                                                    let _ = tx.send(HotkeyAction::SaveWaypoint);
                                                } else if matches(&guard.radar_zoom_in) {
                                                    let _ = tx.send(HotkeyAction::RadarZoomIn);
                                                } else if matches(&guard.radar_zoom_out) {
                                                    let _ = tx.send(HotkeyAction::RadarZoomOut);
                                                } else if matches(&guard.timed) {
                                                    let _ = tx.send(HotkeyAction::TimedOverlay);
                                                }
                                            }
                                        }
                                        _ => {} // value == 2 is repeat; ignore
                                    }
                                }
                            }
                        }
                    }
                    if !any_event {
                        thread::sleep(Duration::from_millis(12));
                    }
                }
            })
            .ok();
    }

    fn spawn_portal_shortcuts(
        settings: AppSettings,
        tx: Sender<HotkeyAction>,
        stop: Arc<AtomicBool>,
    ) {
        thread::Builder::new()
            .name("sc-overlay-portal-shortcuts".to_string())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                rt.block_on(async move {
                    let Ok(portal) = GlobalShortcuts::new().await else {
                        return;
                    };
                    let Ok(session) = portal.create_session().await else {
                        return;
                    };
                    let mut shortcuts = vec![
                        NewShortcut::new("toggle-overlay", "Toggle SC Overlay")
                            .preferred_trigger(Some(
                                to_xdg_trigger(&settings.hotkey_toggle_overlay).as_str(),
                            )),
                        NewShortcut::new(
                            "toggle-players-orgs",
                            "Toggle Players & Orgs Overlay",
                        )
                        .preferred_trigger(Some(
                            to_xdg_trigger(&settings.hotkey_toggle_players_orgs).as_str(),
                        )),
                        NewShortcut::new("interactive-overlay", "Open SC Interactive Dossier")
                            .preferred_trigger(Some(
                                to_xdg_trigger(&settings.hotkey_interactive_overlay).as_str(),
                            )),
                        NewShortcut::new("save-waypoint", "Save Current POI Waypoint")
                            .preferred_trigger(Some(
                                to_xdg_trigger(&settings.hotkey_save_waypoint).as_str(),
                            )),
                        NewShortcut::new("radar-zoom-in", "Breadcrumb Radar Zoom In")
                            .preferred_trigger(Some(
                                to_xdg_trigger(&settings.hotkey_radar_zoom_in).as_str(),
                            )),
                        NewShortcut::new("radar-zoom-out", "Breadcrumb Radar Zoom Out")
                            .preferred_trigger(Some(
                                to_xdg_trigger(&settings.hotkey_radar_zoom_out).as_str(),
                            )),
                    ];
                    // Only bind timed-overlay in XDG portal if it uses a modifier (so bare `Tab` isn't eaten!)
                    if let Ok(chord) = HotkeyText::parse(&settings.hotkey_timed_overlay) {
                        if chord.ctrl || chord.alt || chord.shift {
                            shortcuts.push(
                                NewShortcut::new("timed-overlay", "Timed Peek SC Overlay")
                                    .preferred_trigger(Some(
                                        to_xdg_trigger(&settings.hotkey_timed_overlay).as_str(),
                                    )),
                            );
                        }
                    }
                    let _ = portal.bind_shortcuts(&session, &shortcuts, None).await;
                    let Ok(mut activated) = portal.receive_activated().await else {
                        return;
                    };
                    while !stop.load(Ordering::Relaxed) {
                        if let Some(act) = activated.next_async().await {
                            match act.shortcut_id() {
                                "toggle-overlay" => {
                                    let _ = tx.send(HotkeyAction::ToggleOverlay);
                                }
                                "toggle-players-orgs" => {
                                    let _ = tx.send(HotkeyAction::TogglePlayersOrgs);
                                }
                                "timed-overlay" => {
                                    let _ = tx.send(HotkeyAction::TimedOverlay);
                                }
                                "interactive-overlay" => {
                                    let _ = tx.send(HotkeyAction::InteractiveOverlay);
                                }
                                "save-waypoint" => {
                                    let _ = tx.send(HotkeyAction::SaveWaypoint);
                                }
                                "radar-zoom-in" => {
                                    let _ = tx.send(HotkeyAction::RadarZoomIn);
                                }
                                "radar-zoom-out" => {
                                    let _ = tx.send(HotkeyAction::RadarZoomOut);
                                }
                                _ => {}
                            }
                        } else {
                            break;
                        }
                    }
                });
            })
            .ok();
    }
}

impl HotkeyListener for LinuxHotkeyListener {
    fn update_bindings(&mut self, settings: &AppSettings) {
        *self.bindings.write() = Self::parse_bindings(settings);
    }

    fn try_recv(&self) -> Option<HotkeyAction> {
        self.rx.try_recv().ok()
    }
}

impl Drop for LinuxHotkeyListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

mod futures_util_shim {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    pub trait StreamNonBlocking: Unpin {
        type Item;
        fn poll_next_unpin(&mut self, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>;

        fn next_async(&mut self) -> NextFut<'_, Self> {
            NextFut { stream: self }
        }
    }

    pub struct NextFut<'a, S: ?Sized> {
        stream: &'a mut S,
    }

    impl<S: StreamNonBlocking + ?Sized> Future for NextFut<'_, S> {
        type Output = Option<S::Item>;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.stream.poll_next_unpin(cx)
        }
    }

    impl<T> StreamNonBlocking for T
    where
        T: ashpd::zbus::export::futures_core::Stream + Unpin,
    {
        type Item = <T as ashpd::zbus::export::futures_core::Stream>::Item;
        fn poll_next_unpin(&mut self, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Pin::new(self).poll_next(cx)
        }
    }
}

fn to_xdg_trigger(hotkey: &str) -> String {
    let Ok(chord) = HotkeyText::parse(hotkey) else {
        return String::new();
    };
    let mut s = String::new();
    if chord.ctrl {
        s.push_str("<Control>");
    }
    if chord.alt {
        s.push_str("<Alt>");
    }
    if chord.shift {
        s.push_str("<Shift>");
    }
    s.push_str(&chord.key);
    s
}

fn evdev_key_name(key: Key) -> Option<String> {
    let s = match key {
        Key::KEY_TAB => "Tab",
        Key::KEY_BACKSPACE => "Backspace",
        Key::KEY_ENTER => "Enter",
        Key::KEY_ESC => "Escape",
        Key::KEY_SPACE => "Space",
        Key::KEY_PAGEUP => "PageUp",
        Key::KEY_PAGEDOWN => "PageDown",
        Key::KEY_END => "End",
        Key::KEY_HOME => "Home",
        Key::KEY_LEFT => "Left",
        Key::KEY_UP => "Up",
        Key::KEY_RIGHT => "Right",
        Key::KEY_DOWN => "Down",
        Key::KEY_INSERT => "Insert",
        Key::KEY_DELETE => "Delete",
        Key::KEY_0 => "0",
        Key::KEY_1 => "1",
        Key::KEY_2 => "2",
        Key::KEY_3 => "3",
        Key::KEY_4 => "4",
        Key::KEY_5 => "5",
        Key::KEY_6 => "6",
        Key::KEY_7 => "7",
        Key::KEY_8 => "8",
        Key::KEY_9 => "9",
        Key::KEY_A => "A",
        Key::KEY_B => "B",
        Key::KEY_C => "C",
        Key::KEY_D => "D",
        Key::KEY_E => "E",
        Key::KEY_F => "F",
        Key::KEY_G => "G",
        Key::KEY_H => "H",
        Key::KEY_I => "I",
        Key::KEY_J => "J",
        Key::KEY_K => "K",
        Key::KEY_L => "L",
        Key::KEY_M => "M",
        Key::KEY_N => "N",
        Key::KEY_O => "O",
        Key::KEY_P => "P",
        Key::KEY_Q => "Q",
        Key::KEY_R => "R",
        Key::KEY_S => "S",
        Key::KEY_T => "T",
        Key::KEY_U => "U",
        Key::KEY_V => "V",
        Key::KEY_W => "W",
        Key::KEY_X => "X",
        Key::KEY_Y => "Y",
        Key::KEY_Z => "Z",
        Key::KEY_F1 => "F1",
        Key::KEY_F2 => "F2",
        Key::KEY_F3 => "F3",
        Key::KEY_F4 => "F4",
        Key::KEY_F5 => "F5",
        Key::KEY_F6 => "F6",
        Key::KEY_F7 => "F7",
        Key::KEY_F8 => "F8",
        Key::KEY_F9 => "F9",
        Key::KEY_F10 => "F10",
        Key::KEY_F11 => "F11",
        Key::KEY_F12 => "F12",
        Key::KEY_MINUS => "-",
        Key::KEY_EQUAL => "=",
        Key::KEY_LEFTBRACE => "[",
        Key::KEY_RIGHTBRACE => "]",
        Key::KEY_SEMICOLON => ";",
        Key::KEY_APOSTROPHE => "'",
        Key::KEY_GRAVE => "`",
        Key::KEY_BACKSLASH => "\\",
        Key::KEY_COMMA => ",",
        Key::KEY_DOT => ".",
        Key::KEY_SLASH => "/",
        _ => return None,
    };
    Some(s.to_string())
}

