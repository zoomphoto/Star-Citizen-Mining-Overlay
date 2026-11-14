use crate::{GameWindowDetector, GameWindowInfo};
use iris_core::models::IntRect;
use iris_core::overlay::{StarCitizenDisplaySettings, WindowMode};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};
use x11rb::rust_connection::RustConnection;

pub struct LinuxGameDetector {
    x11: Option<(RustConnection, usize)>,
    last_proc_scan: Instant,
    cached_sc_exe: Option<(u32, Option<PathBuf>)>,
    cached_xml_mode: Option<(PathBuf, Instant, Option<WindowMode>)>,
}

impl LinuxGameDetector {
    pub fn new() -> Self {
        let x11 = x11rb::connect(None).ok();
        Self {
            x11,
            last_proc_scan: Instant::now() - Duration::from_secs(10),
            cached_sc_exe: None,
            cached_xml_mode: None,
        }
    }

    /// Scans `/proc` on Bazzite/Linux for `StarCitizen.exe` running under Wine / Proton / LUG.
    fn find_star_citizen_process(&mut self) -> Option<(u32, Option<PathBuf>)> {
        let now = Instant::now();
        if now.duration_since(self.last_proc_scan) < Duration::from_millis(1500) {
            return self.cached_sc_exe.clone();
        }
        self.last_proc_scan = now;

        let mut found: Option<(u32, Option<PathBuf>)> = None;
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Ok(pid_str) = entry.file_name().into_string() else {
                    continue;
                };
                let Ok(pid) = pid_str.parse::<u32>() else {
                    continue;
                };
                let cmdline_path = entry.path().join("cmdline");
                let Ok(raw_cmd) = fs::read(&cmdline_path) else {
                    continue;
                };
                if raw_cmd.is_empty() {
                    continue;
                }
                let cmd_text = String::from_utf8_lossy(&raw_cmd);
                let lower = cmd_text.to_ascii_lowercase();
                if !lower.contains("starcitizen.exe") {
                    continue;
                }

                // Attempt to resolve the real host path to StarCitizen.exe inside WINEPREFIX
                let exe_host_path = Self::resolve_wine_exe_path(&entry.path(), &cmd_text);
                found = Some((pid, exe_host_path));
                break;
            }
        }

        self.cached_sc_exe = found.clone();
        found
    }

    fn resolve_wine_exe_path(proc_dir: &Path, cmd_text: &str) -> Option<PathBuf> {
        let args: Vec<&str> = cmd_text.split('\0').filter(|s| !s.is_empty()).collect();
        let sc_arg = args
            .iter()
            .copied()
            .find(|a| a.to_ascii_lowercase().ends_with("starcitizen.exe"))?;

        // Direct Linux path check
        let normalized_slashes = sc_arg.replace('\\', "/");
        let direct = PathBuf::from(&normalized_slashes);
        if direct.is_file() {
            return Some(direct);
        }

        // Resolve via WINEPREFIX or cwd (`/proc/<pid>/cwd` in Wine is often `<Install>/Bin64` or `<Install>`)
        if let Ok(cwd) = fs::read_link(proc_dir.join("cwd")) {
            let candidate_in_cwd = cwd.join("StarCitizen.exe");
            if candidate_in_cwd.is_file() {
                return Some(candidate_in_cwd);
            }
            let candidate_bin64 = cwd.join("Bin64").join("StarCitizen.exe");
            if candidate_bin64.is_file() {
                return Some(candidate_bin64);
            }
        }

        // Parse WINEPREFIX from `/proc/<pid>/environ`
        if let Ok(env_bytes) = fs::read(proc_dir.join("environ")) {
            for var in env_bytes.split(|&b| b == 0) {
                if let Ok(s) = std::str::from_utf8(var) {
                    if let Some(prefix) = s.strip_prefix("WINEPREFIX=") {
                        if let Some(rest) = normalized_slashes
                            .strip_prefix("C:/")
                            .or_else(|| normalized_slashes.strip_prefix("c:/"))
                        {
                            let host = Path::new(prefix).join("drive_c").join(rest);
                            if host.is_file() {
                                return Some(host);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn read_xml_window_mode(&mut self, exe_path: &Path) -> Option<WindowMode> {
        let now = Instant::now();
        if let Some((cached_exe, ts, mode)) = &self.cached_xml_mode {
            if cached_exe == exe_path && now.duration_since(*ts) < Duration::from_secs(2) {
                return *mode;
            }
        }
        let mode = StarCitizenDisplaySettings::attributes_path_for_exe(exe_path)
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|xml| StarCitizenDisplaySettings::parse_window_mode(&xml));
        self.cached_xml_mode = Some((exe_path.to_path_buf(), now, mode));
        mode
    }

    fn poll_x11(&mut self, sc_exe: Option<&Path>) -> Option<GameWindowInfo> {
        if self.x11.is_none() {
            self.x11 = x11rb::connect(None).ok();
        }
        let (conn, screen_num) = self.x11.as_ref()?;
        let screen = conn.setup().roots.get(*screen_num)?;
        let root = screen.root;
        let mon_bounds = IntRect::new(
            0,
            0,
            (screen.width_in_pixels as i32).max(1280),
            (screen.height_in_pixels as i32).max(720),
        );

        let atom_active = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let atom_client_list = conn
            .intern_atom(false, b"_NET_CLIENT_LIST")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let atom_net_wm_name = conn
            .intern_atom(false, b"_NET_WM_NAME")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let atom_utf8 = conn
            .intern_atom(false, b"UTF8_STRING")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let atom_wm_state = conn
            .intern_atom(false, b"_NET_WM_STATE")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let atom_hidden = conn
            .intern_atom(false, b"_NET_WM_STATE_HIDDEN")
            .ok()?
            .reply()
            .ok()?
            .atom;

        let active_win: Window = conn
            .get_property(false, root, atom_active, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut it| it.next()))
            .unwrap_or(0);

        let clients: Vec<Window> = conn
            .get_property(false, root, atom_client_list, AtomEnum::WINDOW, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|it| it.collect()))
            .unwrap_or_default();

        let window_title = |w: Window| -> String {
            if let Ok(cookie) = conn.get_property(false, w, atom_net_wm_name, atom_utf8, 0, 256) {
                if let Ok(rep) = cookie.reply() {
                    if !rep.value.is_empty() {
                        return String::from_utf8_lossy(&rep.value).to_string();
                    }
                }
            }
            if let Ok(cookie) =
                conn.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 256)
            {
                if let Ok(rep) = cookie.reply() {
                    if !rep.value.is_empty() {
                        return String::from_utf8_lossy(&rep.value).to_string();
                    }
                }
            }
            String::new()
        };

        let mut sc_win: Option<Window> = None;
        if active_win != 0 && window_title(active_win).contains("Star Citizen") {
            sc_win = Some(active_win);
        } else {
            for &w in &clients {
                let t = window_title(w);
                if t == "Star Citizen" || t.starts_with("Star Citizen ") {
                    sc_win = Some(w);
                    break;
                }
            }
        }

        let w = sc_win?;
        let geom = conn.get_geometry(w).ok()?.reply().ok()?;
        let trans = conn
            .translate_coordinates(w, root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        let bounds = IntRect::new(
            trans.dst_x as i32,
            trans.dst_y as i32,
            geom.width as i32,
            geom.height as i32,
        );

        let minimized = conn
            .get_property(false, w, atom_wm_state, AtomEnum::ATOM, 0, 32)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|mut it| it.any(|a| a == atom_hidden)))
            .unwrap_or(false);

        // On Wayland compositors, XWayland `active_win` can be 0 when no other X11 window has focus;
        // if Star Citizen is not minimized and either `active_win == w` or `active_win == 0`, treat as focused.
        let is_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let focused = !minimized && (active_win == w || (is_wayland && active_win == 0));

        let xml_mode = sc_exe.and_then(|p| self.read_xml_window_mode(p));
        let covers_screen = (bounds.width - mon_bounds.width).abs() <= 4
            && (bounds.height - mon_bounds.height).abs() <= 4;
        let mode = xml_mode.unwrap_or(if covers_screen {
            WindowMode::Borderless
        } else {
            WindowMode::Windowed
        });

        Some(GameWindowInfo {
            present: true,
            focused,
            minimized,
            bounds,
            monitor_bounds: mon_bounds,
            window_mode: Some(mode),
            native_handle: w as u64,
            process_dir: sc_exe.and_then(|p| p.parent().map(|d| d.to_path_buf())),
        })
    }
}

impl Default for LinuxGameDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GameWindowDetector for LinuxGameDetector {
    fn poll(&mut self) -> GameWindowInfo {
        let proc_info = self.find_star_citizen_process();
        let sc_exe = proc_info.as_ref().and_then(|(_, p)| p.clone());

        if let Some(info) = self.poll_x11(sc_exe.as_deref()) {
            return info;
        }

        // Pure Wayland Wine driver fallback when StarCitizen.exe is running without XWayland
        if proc_info.is_some() {
            let xml_mode = sc_exe
                .as_deref()
                .and_then(|p| self.read_xml_window_mode(p))
                .or(Some(WindowMode::Borderless));
            let default_bounds = IntRect::new(0, 0, 1920, 1080);
            return GameWindowInfo {
                present: true,
                focused: true,
                minimized: false,
                bounds: default_bounds,
                monitor_bounds: default_bounds,
                window_mode: xml_mode,
                native_handle: 0,
                process_dir: sc_exe.and_then(|p| p.parent().map(|d| d.to_path_buf())),
            };
        }

        GameWindowInfo::default()
    }
}

