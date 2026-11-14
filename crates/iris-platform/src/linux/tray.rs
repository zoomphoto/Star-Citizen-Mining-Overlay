use crate::{TrayAction, TrayBackend, TRAY_OFF_ICO, TRAY_ON_ICO};
use crossbeam_channel::{unbounded, Receiver, Sender};
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{StandardItem, SubMenu};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct ScOverlayKsniTray {
    overlay_enabled: Arc<AtomicBool>,
    tx: Sender<TrayAction>,
    icon_on: Vec<ksni::Icon>,
    icon_off: Vec<ksni::Icon>,
}

impl ksni::Tray for ScOverlayKsniTray {
    fn id(&self) -> String {
        "sc-overlay".to_string()
    }

    fn title(&self) -> String {
        if self.overlay_enabled.load(Ordering::Relaxed) {
            "SC Overlay (Active)".to_string()
        } else {
            "SC Overlay (Disabled)".to_string()
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.overlay_enabled.load(Ordering::Relaxed) {
            self.icon_on.clone()
        } else {
            self.icon_off.clone()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayAction::OpenSettings);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let enabled = self.overlay_enabled.load(Ordering::Relaxed);
        let toggle_label = if enabled {
            "Disable Overlay"
        } else {
            "Enable Overlay"
        };

        vec![
            StandardItem {
                label: "Settings…".to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayAction::OpenSettings);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: toggle_label.to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayAction::ToggleOverlay);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Open log folder".to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayAction::OpenLogs);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open config location".to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayAction::OpenConfigDir);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct LinuxTrayBackend {
    overlay_enabled: Arc<AtomicBool>,
    handle: Handle<ScOverlayKsniTray>,
    rx: Receiver<TrayAction>,
}

impl LinuxTrayBackend {
    pub fn new(overlay_enabled: bool) -> anyhow::Result<Self> {
        let flag = Arc::new(AtomicBool::new(overlay_enabled));
        let (tx, rx) = unbounded();
        let tray = ScOverlayKsniTray {
            overlay_enabled: Arc::clone(&flag),
            tx,
            icon_on: decode_ico_to_ksni(TRAY_ON_ICO),
            icon_off: decode_ico_to_ksni(TRAY_OFF_ICO),
        };
        let handle = tray
            .assume_sni_available(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn StatusNotifierItem tray: {}", e))?;

        let _ = std::marker::PhantomData::<SubMenu<ScOverlayKsniTray>>;
        Ok(Self {
            overlay_enabled: flag,
            handle,
            rx,
        })
    }
}

impl TrayBackend for LinuxTrayBackend {
    fn set_overlay_enabled(&mut self, enabled: bool) {
        self.overlay_enabled.store(enabled, Ordering::Relaxed);
        let _ = self.handle.update(|_| {});
    }

    fn try_recv(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }
}

fn decode_ico_to_ksni(ico_bytes: &[u8]) -> Vec<ksni::Icon> {
    let Ok(img) = image::load_from_memory(ico_bytes) else {
        return Vec::new();
    };
    let rgba = img.to_rgba8();
    let width = rgba.width() as i32;
    let height = rgba.height() as i32;
    // StatusNotifierItem expects ARGB32 in network byte order (A, R, G, B)
    let mut argb = Vec::with_capacity((width * height * 4) as usize);
    for px in rgba.pixels() {
        argb.push(px[3]);
        argb.push(px[0]);
        argb.push(px[1]);
        argb.push(px[2]);
    }
    vec![ksni::Icon {
        width,
        height,
        data: argb,
    }]
}
