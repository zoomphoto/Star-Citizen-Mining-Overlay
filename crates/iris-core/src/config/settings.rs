use crate::config::{AtomicFile, HostedConfigUrl, HotkeyText};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMethod {
    /// Windows DXGI Desktop Duplication / Linux PipeWire Desktop Portal or X11 monitor capture
    DesktopDuplication,
    /// Windows Graphics Capture (WGC) / Linux Portal Window capture
    WindowCapture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportMode {
    Merge,
    MergeImportedWins,
    BackupExisting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgDisplay {
    FullName,
    Sid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DarkMode {
    System,
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    pub cache_ttl_days: i32,
    pub session_log_enabled: bool,
    pub hosted_config_url: String,
    pub hosted_config_last_hash: String,
    pub hosted_config_auto_check: bool,
    pub hosted_config_last_check: Option<DateTime<Utc>>,
    pub import_mode: ImportMode,
    pub panel_width_percent: i32,
    pub panel_text_size: i32,
    pub panel_right_gap_percent: i32,
    pub panel_top_gap_percent: i32,
    pub panel_org_display: OrgDisplay,
    pub panel_main_org_only: bool,
    pub dossier_width_percent: i32,
    pub boundary_top: i32,
    pub boundary_bottom: i32,
    pub boundary_left: i32,
    pub boundary_right: i32,
    pub nameplate_tracking_fps: i32,
    pub tracking_loss_display_seconds: i32,
    pub capture_method: CaptureMethod,
    pub overlay_visible_in_capture: bool,
    pub hide_capture_border: bool,
    pub check_for_updates: bool,
    pub settings_window_scale: i32,
    pub settings_dark_mode: DarkMode,
    pub overlay_disabled: bool,
    pub hotkey_toggle_overlay: String,
    pub hotkey_timed_overlay: String,
    pub hotkey_interactive_overlay: String,
    pub timed_overlay_seconds: i32,
    pub mining_disabled: bool,
    pub mining_label_zoom_percent: i32,
    pub breadcrumbs_enabled: bool,
    pub breadcrumbs_persist: bool,
    pub breadcrumbs_visited_radius_km: i32,
    pub breadcrumbs_screen_cloud: bool,
    pub breadcrumbs_auto_displayinfo: bool,
    pub breadcrumbs_auto_interval_secs: i32,
    pub debug_overlay_enabled: bool,
    pub debug_show_candidates: bool,
    /// Linux XDG ScreenCast portal restore token so Bazzite Wayland only prompts once
    pub linux_portal_restore_token: String,
}

impl AppSettings {
    pub const FORMAT_VERSION: i64 = 1;
    pub const PANEL_WIDTH_MIN: i32 = 8;
    pub const PANEL_WIDTH_MAX: i32 = 25;
    pub const PANEL_TEXT_MIN: i32 = 8;
    pub const PANEL_TEXT_MAX: i32 = 25;
    pub const PANEL_GAP_MIN: i32 = 0;
    pub const PANEL_GAP_MAX: i32 = 15;
    pub const DOSSIER_WIDTH_MIN: i32 = 25;
    pub const DOSSIER_WIDTH_MAX: i32 = 45;
    pub const BOUNDARY_MIN: i32 = 0;
    pub const BOUNDARY_MAX: i32 = 30;
    pub const TRACKING_FPS_MIN: i32 = 30;
    pub const TRACKING_FPS_HIGHEST: i32 = 90;
    pub const TRACKING_FPS_STEP: i32 = 5;
    pub const TRACKING_FPS_MAX: i32 = 95;
    pub const LOSS_SECONDS_MIN: i32 = 5;
    pub const LOSS_SECONDS_MAX: i32 = 30;
    pub const LOSS_SECONDS_STEP: i32 = 5;
    pub const TIMED_SECONDS_MIN: i32 = 5;
    pub const TIMED_SECONDS_MAX: i32 = 30;
    pub const MINING_ZOOM_MIN: i32 = 100;
    pub const MINING_ZOOM_MAX: i32 = 200;
    pub const MINING_ZOOM_STEP: i32 = 10;
    pub const BREADCRUMB_RADIUS_MIN_KM: i32 = 1;
    pub const BREADCRUMB_RADIUS_MAX_KM: i32 = 25;
    pub const BREADCRUMB_INTERVAL_MIN_SECS: i32 = 5;
    pub const BREADCRUMB_INTERVAL_MAX_SECS: i32 = 120;
    pub const WINDOW_SCALES: [i32; 5] = [100, 110, 125, 150, 175];

    pub fn overlay_shown_in_capture(&self) -> bool {
        self.overlay_visible_in_capture && self.capture_method == CaptureMethod::WindowCapture
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            cache_ttl_days: 30,
            session_log_enabled: true,
            hosted_config_url: String::new(),
            hosted_config_last_hash: String::new(),
            hosted_config_auto_check: false,
            hosted_config_last_check: None,
            import_mode: ImportMode::Merge,
            panel_width_percent: 15,
            panel_text_size: 15,
            panel_right_gap_percent: 5,
            panel_top_gap_percent: 5,
            panel_org_display: OrgDisplay::FullName,
            panel_main_org_only: false,
            dossier_width_percent: 35,
            boundary_top: 15,
            boundary_bottom: 15,
            boundary_left: 8,
            boundary_right: 8,
            nameplate_tracking_fps: Self::TRACKING_FPS_MAX,
            tracking_loss_display_seconds: 5,
            capture_method: CaptureMethod::DesktopDuplication,
            overlay_visible_in_capture: true,
            hide_capture_border: true,
            check_for_updates: true,
            settings_window_scale: 100,
            settings_dark_mode: DarkMode::System,
            overlay_disabled: false,
            hotkey_toggle_overlay: "Shift+F12".to_string(),
            hotkey_timed_overlay: "Tab".to_string(),
            hotkey_interactive_overlay: "Shift+0".to_string(),
            timed_overlay_seconds: 10,
            mining_disabled: false,
            mining_label_zoom_percent: 150,
            breadcrumbs_enabled: true,
            breadcrumbs_persist: true,
            breadcrumbs_visited_radius_km: 5,
            breadcrumbs_screen_cloud: true,
            breadcrumbs_auto_displayinfo: true,
            breadcrumbs_auto_interval_secs: 15,
            debug_overlay_enabled: false,
            debug_show_candidates: false,
            linux_portal_restore_token: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppSettingsLoadResult {
    pub settings: AppSettings,
    pub is_new: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl AppSettingsLoadResult {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

pub struct AppSettingsToml;

impl AppSettingsToml {
    pub fn write(settings: &AppSettings) -> String {
        let ttl = if settings.cache_ttl_days == 7 { 7 } else { 30 };
        let mut out = String::new();
        out.push_str("# IRIS settings for this PC (TOML format). Not part of the shareable list file; never exported.\n");
        out.push_str(&format!("format_version = {}\n", AppSettings::FORMAT_VERSION));
        out.push_str(&format!("cache_ttl_days = {}\n", ttl));
        out.push_str(&format!(
            "session_log_enabled = {}\n",
            bool_str(settings.session_log_enabled)
        ));
        out.push_str(&format!(
            "hosted_config_url = {}\n",
            quote(&settings.hosted_config_url)
        ));
        out.push_str(&format!(
            "hosted_config_last_hash = {}\n",
            quote(&settings.hosted_config_last_hash)
        ));
        out.push_str(&format!(
            "hosted_config_auto_check = {}\n",
            bool_str(settings.hosted_config_auto_check)
        ));
        out.push_str(&format!(
            "import_mode = {}\n",
            quote(import_mode_text(settings.import_mode))
        ));
        if let Some(ts) = settings.hosted_config_last_check {
            out.push_str(&format!(
                "hosted_config_last_check = {}\n",
                quote(&ts.to_rfc3339_opts(SecondsFormat::Secs, true))
            ));
        }
        out.push_str(&format!(
            "panel_width_percent = {}\n",
            settings
                .panel_width_percent
                .clamp(AppSettings::PANEL_WIDTH_MIN, AppSettings::PANEL_WIDTH_MAX)
        ));
        out.push_str(&format!(
            "panel_text_size = {}\n",
            settings
                .panel_text_size
                .clamp(AppSettings::PANEL_TEXT_MIN, AppSettings::PANEL_TEXT_MAX)
        ));
        out.push_str(&format!(
            "panel_right_gap_percent = {}\n",
            settings
                .panel_right_gap_percent
                .clamp(AppSettings::PANEL_GAP_MIN, AppSettings::PANEL_GAP_MAX)
        ));
        out.push_str(&format!(
            "panel_top_gap_percent = {}\n",
            settings
                .panel_top_gap_percent
                .clamp(AppSettings::PANEL_GAP_MIN, AppSettings::PANEL_GAP_MAX)
        ));
        out.push_str(&format!(
            "panel_org_display = {}\n",
            quote(org_display_text(settings.panel_org_display))
        ));
        out.push_str(&format!(
            "panel_main_org_only = {}\n",
            bool_str(settings.panel_main_org_only)
        ));
        out.push_str(&format!(
            "dossier_width_percent = {}\n",
            settings
                .dossier_width_percent
                .clamp(AppSettings::DOSSIER_WIDTH_MIN, AppSettings::DOSSIER_WIDTH_MAX)
        ));
        out.push_str(&format!(
            "boundary_top = {}\n",
            settings
                .boundary_top
                .clamp(AppSettings::BOUNDARY_MIN, AppSettings::BOUNDARY_MAX)
        ));
        out.push_str(&format!(
            "boundary_bottom = {}\n",
            settings
                .boundary_bottom
                .clamp(AppSettings::BOUNDARY_MIN, AppSettings::BOUNDARY_MAX)
        ));
        out.push_str(&format!(
            "boundary_left = {}\n",
            settings
                .boundary_left
                .clamp(AppSettings::BOUNDARY_MIN, AppSettings::BOUNDARY_MAX)
        ));
        out.push_str(&format!(
            "boundary_right = {}\n",
            settings
                .boundary_right
                .clamp(AppSettings::BOUNDARY_MIN, AppSettings::BOUNDARY_MAX)
        ));
        out.push_str(&format!(
            "nameplate_tracking_fps = {}\n",
            write_tracking_fps(settings.nameplate_tracking_fps)
        ));
        out.push_str(&format!(
            "tracking_loss_display_seconds = {}\n",
            snap_step(
                settings.tracking_loss_display_seconds,
                AppSettings::LOSS_SECONDS_STEP,
                AppSettings::LOSS_SECONDS_MIN,
                AppSettings::LOSS_SECONDS_MAX
            )
        ));
        out.push_str(&format!(
            "capture_method = {}\n",
            quote(capture_method_text(settings.capture_method))
        ));
        out.push_str(&format!(
            "overlay_visible_in_capture = {}\n",
            bool_str(settings.overlay_visible_in_capture)
        ));
        out.push_str(&format!(
            "hide_capture_border = {}\n",
            bool_str(settings.hide_capture_border)
        ));
        out.push_str(&format!(
            "check_for_updates = {}\n",
            bool_str(settings.check_for_updates)
        ));
        let scale = AppSettings::WINDOW_SCALES
            .iter()
            .copied()
            .min_by_key(|v| (v - settings.settings_window_scale).abs())
            .unwrap_or(100);
        out.push_str(&format!("settings_window_scale = {}\n", scale));
        out.push_str(&format!(
            "settings_dark_mode = {}\n",
            quote(dark_mode_text(settings.settings_dark_mode))
        ));
        out.push_str(&format!(
            "overlay_disabled = {}\n",
            bool_str(settings.overlay_disabled)
        ));
        out.push_str(&format!(
            "hotkey_toggle_overlay = {}\n",
            quote(&HotkeyText::normalize(&settings.hotkey_toggle_overlay))
        ));
        out.push_str(&format!(
            "hotkey_timed_overlay = {}\n",
            quote(&HotkeyText::normalize(&settings.hotkey_timed_overlay))
        ));
        out.push_str(&format!(
            "hotkey_interactive_overlay = {}\n",
            quote(&HotkeyText::normalize(&settings.hotkey_interactive_overlay))
        ));
        out.push_str(&format!(
            "timed_overlay_seconds = {}\n",
            settings
                .timed_overlay_seconds
                .clamp(AppSettings::TIMED_SECONDS_MIN, AppSettings::TIMED_SECONDS_MAX)
        ));
        out.push_str(&format!(
            "mining_disabled = {}\n",
            bool_str(settings.mining_disabled)
        ));
        out.push_str(&format!(
            "mining_label_zoom_percent = {}\n",
            snap_step(
                settings.mining_label_zoom_percent,
                AppSettings::MINING_ZOOM_STEP,
                AppSettings::MINING_ZOOM_MIN,
                AppSettings::MINING_ZOOM_MAX
            )
        ));
        out.push_str(&format!(
            "breadcrumbs_enabled = {}\n",
            bool_str(settings.breadcrumbs_enabled)
        ));
        out.push_str(&format!(
            "breadcrumbs_persist = {}\n",
            bool_str(settings.breadcrumbs_persist)
        ));
        out.push_str(&format!(
            "breadcrumbs_visited_radius_km = {}\n",
            settings.breadcrumbs_visited_radius_km.clamp(
                AppSettings::BREADCRUMB_RADIUS_MIN_KM,
                AppSettings::BREADCRUMB_RADIUS_MAX_KM
            )
        ));
        out.push_str(&format!(
            "breadcrumbs_screen_cloud = {}\n",
            bool_str(settings.breadcrumbs_screen_cloud)
        ));
        out.push_str(&format!(
            "breadcrumbs_auto_displayinfo = {}\n",
            bool_str(settings.breadcrumbs_auto_displayinfo)
        ));
        out.push_str(&format!(
            "breadcrumbs_auto_interval_secs = {}\n",
            settings.breadcrumbs_auto_interval_secs.clamp(
                AppSettings::BREADCRUMB_INTERVAL_MIN_SECS,
                AppSettings::BREADCRUMB_INTERVAL_MAX_SECS
            )
        ));
        out.push_str(&format!(
            "debug_overlay_enabled = {}\n",
            bool_str(settings.debug_overlay_enabled)
        ));
        out.push_str(&format!(
            "debug_show_candidates = {}\n",
            bool_str(settings.debug_show_candidates)
        ));
        if !settings.linux_portal_restore_token.is_empty() {
            out.push_str(&format!(
                "linux_portal_restore_token = {}\n",
                quote(&settings.linux_portal_restore_token)
            ));
        }
        out
    }

    pub fn read(text: &str) -> AppSettingsLoadResult {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let defaults = AppSettings::default();
        let root: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(err) => {
                return AppSettingsLoadResult {
                    settings: defaults,
                    is_new: false,
                    errors: vec![format!("TOML syntax: {}", err)],
                    warnings: Vec::new(),
                };
            }
        };

        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        if let Some(v) = root.get("format_version") {
            match v.as_integer() {
                Some(ver) if (1..=AppSettings::FORMAT_VERSION).contains(&ver) => {}
                Some(ver) => errors.push(format!(
                    "format_version: {} is not supported (this app reads version {}).",
                    ver,
                    AppSettings::FORMAT_VERSION
                )),
                None => errors.push("format_version: must be a whole number.".to_string()),
            }
        }

        if !errors.is_empty() {
            return AppSettingsLoadResult {
                settings: defaults,
                is_new: false,
                errors,
                warnings,
            };
        }

        let mut s = defaults.clone();
        if let Some(v) = root.get("cache_ttl_days") {
            if let Some(days) = v.as_integer() {
                if days == 7 || days == 30 {
                    s.cache_ttl_days = days as i32;
                } else {
                    warnings.push(format!(
                        "cache_ttl_days: must be 30 or 7; using {}.",
                        defaults.cache_ttl_days
                    ));
                }
            }
        }

        s.session_log_enabled =
            read_bool(&root, "session_log_enabled", defaults.session_log_enabled, &mut warnings);

        if let Some(v) = root.get("hosted_config_url") {
            if let Some(url_str) = v.as_str() {
                let trimmed = url_str.trim();
                if !trimmed.is_empty() {
                    if let Err(problem) = HostedConfigUrl::check(trimmed) {
                        warnings.push(format!(
                            "hosted_config_url: {} The address is ignored.",
                            problem
                        ));
                    } else {
                        s.hosted_config_url = trimmed.to_string();
                    }
                }
            }
        }

        if let Some(v) = root.get("hosted_config_last_hash") {
            if let Some(h) = v.as_str() {
                let cand = h.trim().to_ascii_lowercase();
                if cand.is_empty() {
                    s.hosted_config_last_hash.clear();
                } else if cand.len() == 64 && cand.chars().all(|c| c.is_ascii_hexdigit()) {
                    s.hosted_config_last_hash = cand;
                } else {
                    warnings.push("hosted_config_last_hash: must be 64 hex characters.".to_string());
                }
            }
        }

        s.hosted_config_auto_check = read_bool(
            &root,
            "hosted_config_auto_check",
            defaults.hosted_config_auto_check,
            &mut warnings,
        );
        if s.hosted_config_auto_check && s.hosted_config_url.is_empty() {
            s.hosted_config_auto_check = false;
        }

        if let Some(v) = root.get("hosted_config_last_check") {
            if let Some(ts_str) = v.as_str() {
                if !ts_str.trim().is_empty() {
                    if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str.trim()) {
                        s.hosted_config_last_check = Some(dt.with_timezone(&Utc));
                    }
                }
            }
        }

        s.panel_width_percent = read_int(
            &root,
            "panel_width_percent",
            defaults.panel_width_percent,
            AppSettings::PANEL_WIDTH_MIN,
            AppSettings::PANEL_WIDTH_MAX,
            &mut warnings,
        );
        s.dossier_width_percent = read_int(
            &root,
            "dossier_width_percent",
            defaults.dossier_width_percent,
            AppSettings::DOSSIER_WIDTH_MIN,
            AppSettings::DOSSIER_WIDTH_MAX,
            &mut warnings,
        );
        s.panel_text_size = read_int(
            &root,
            "panel_text_size",
            defaults.panel_text_size,
            AppSettings::PANEL_TEXT_MIN,
            AppSettings::PANEL_TEXT_MAX,
            &mut warnings,
        );
        s.panel_right_gap_percent = read_int(
            &root,
            "panel_right_gap_percent",
            defaults.panel_right_gap_percent,
            AppSettings::PANEL_GAP_MIN,
            AppSettings::PANEL_GAP_MAX,
            &mut warnings,
        );
        s.panel_top_gap_percent = read_int(
            &root,
            "panel_top_gap_percent",
            defaults.panel_top_gap_percent,
            AppSettings::PANEL_GAP_MIN,
            AppSettings::PANEL_GAP_MAX,
            &mut warnings,
        );

        s.boundary_top = read_int(
            &root,
            "boundary_top",
            defaults.boundary_top,
            AppSettings::BOUNDARY_MIN,
            AppSettings::BOUNDARY_MAX,
            &mut warnings,
        );
        s.boundary_bottom = read_int(
            &root,
            "boundary_bottom",
            defaults.boundary_bottom,
            AppSettings::BOUNDARY_MIN,
            AppSettings::BOUNDARY_MAX,
            &mut warnings,
        );
        s.boundary_left = read_int(
            &root,
            "boundary_left",
            defaults.boundary_left,
            AppSettings::BOUNDARY_MIN,
            AppSettings::BOUNDARY_MAX,
            &mut warnings,
        );
        s.boundary_right = read_int(
            &root,
            "boundary_right",
            defaults.boundary_right,
            AppSettings::BOUNDARY_MIN,
            AppSettings::BOUNDARY_MAX,
            &mut warnings,
        );

        if let Some(v) = root.get("nameplate_tracking_fps") {
            if let Some(s_val) = v.as_str() {
                if s_val.trim().eq_ignore_ascii_case("max") {
                    s.nameplate_tracking_fps = AppSettings::TRACKING_FPS_MAX;
                }
            } else if let Some(n) = v.as_integer() {
                if n > AppSettings::TRACKING_FPS_HIGHEST as i64 {
                    s.nameplate_tracking_fps = AppSettings::TRACKING_FPS_MAX;
                } else if n < AppSettings::TRACKING_FPS_MIN as i64 {
                    s.nameplate_tracking_fps = AppSettings::TRACKING_FPS_MIN;
                } else {
                    s.nameplate_tracking_fps = snap_step(
                        n as i32,
                        AppSettings::TRACKING_FPS_STEP,
                        AppSettings::TRACKING_FPS_MIN,
                        AppSettings::TRACKING_FPS_HIGHEST,
                    );
                }
            }
        }

        s.tracking_loss_display_seconds = snap_step(
            read_int(
                &root,
                "tracking_loss_display_seconds",
                defaults.tracking_loss_display_seconds,
                AppSettings::LOSS_SECONDS_MIN,
                AppSettings::LOSS_SECONDS_MAX,
                &mut warnings,
            ),
            AppSettings::LOSS_SECONDS_STEP,
            AppSettings::LOSS_SECONDS_MIN,
            AppSettings::LOSS_SECONDS_MAX,
        );

        if let Some(v) = root.get("capture_method").and_then(|v| v.as_str()) {
            match v.trim().to_ascii_lowercase().as_str() {
                "desktop_duplication" | "monitor" | "x11" => {
                    s.capture_method = CaptureMethod::DesktopDuplication;
                }
                "wgc" | "window" | "portal" => {
                    s.capture_method = CaptureMethod::WindowCapture;
                }
                _ => warnings.push("capture_method: unknown value.".to_string()),
            }
        }

        s.overlay_visible_in_capture = read_bool(
            &root,
            "overlay_visible_in_capture",
            defaults.overlay_visible_in_capture,
            &mut warnings,
        );
        s.hide_capture_border = read_bool(
            &root,
            "hide_capture_border",
            defaults.hide_capture_border,
            &mut warnings,
        );
        s.check_for_updates = read_bool(
            &root,
            "check_for_updates",
            defaults.check_for_updates,
            &mut warnings,
        );

        if let Some(n) = root.get("settings_window_scale").and_then(|v| v.as_integer()) {
            if AppSettings::WINDOW_SCALES.contains(&(n as i32)) {
                s.settings_window_scale = n as i32;
            }
        }

        if let Some(dm) = root.get("settings_dark_mode").and_then(|v| v.as_str()) {
            s.settings_dark_mode = match dm.trim().to_ascii_lowercase().as_str() {
                "on" => DarkMode::On,
                "off" => DarkMode::Off,
                _ => DarkMode::System,
            };
        }

        s.overlay_disabled =
            read_bool(&root, "overlay_disabled", defaults.overlay_disabled, &mut warnings);
        s.hotkey_toggle_overlay = read_hotkey(
            &root,
            "hotkey_toggle_overlay",
            &defaults.hotkey_toggle_overlay,
            &mut warnings,
        );
        s.hotkey_timed_overlay = read_hotkey(
            &root,
            "hotkey_timed_overlay",
            &defaults.hotkey_timed_overlay,
            &mut warnings,
        );
        s.hotkey_interactive_overlay = read_hotkey(
            &root,
            "hotkey_interactive_overlay",
            &defaults.hotkey_interactive_overlay,
            &mut warnings,
        );
        s.timed_overlay_seconds = read_int(
            &root,
            "timed_overlay_seconds",
            defaults.timed_overlay_seconds,
            AppSettings::TIMED_SECONDS_MIN,
            AppSettings::TIMED_SECONDS_MAX,
            &mut warnings,
        );
        s.mining_disabled =
            read_bool(&root, "mining_disabled", defaults.mining_disabled, &mut warnings);
        s.mining_label_zoom_percent = snap_step(
            read_int(
                &root,
                "mining_label_zoom_percent",
                defaults.mining_label_zoom_percent,
                AppSettings::MINING_ZOOM_MIN,
                AppSettings::MINING_ZOOM_MAX,
                &mut warnings,
            ),
            AppSettings::MINING_ZOOM_STEP,
            AppSettings::MINING_ZOOM_MIN,
            AppSettings::MINING_ZOOM_MAX,
        );
        s.breadcrumbs_enabled = read_bool(
            &root,
            "breadcrumbs_enabled",
            defaults.breadcrumbs_enabled,
            &mut warnings,
        );
        s.breadcrumbs_persist = read_bool(
            &root,
            "breadcrumbs_persist",
            defaults.breadcrumbs_persist,
            &mut warnings,
        );
        s.breadcrumbs_visited_radius_km = read_int(
            &root,
            "breadcrumbs_visited_radius_km",
            defaults.breadcrumbs_visited_radius_km,
            AppSettings::BREADCRUMB_RADIUS_MIN_KM,
            AppSettings::BREADCRUMB_RADIUS_MAX_KM,
            &mut warnings,
        );
        s.breadcrumbs_screen_cloud = read_bool(
            &root,
            "breadcrumbs_screen_cloud",
            defaults.breadcrumbs_screen_cloud,
            &mut warnings,
        );
        s.breadcrumbs_auto_displayinfo = read_bool(
            &root,
            "breadcrumbs_auto_displayinfo",
            defaults.breadcrumbs_auto_displayinfo,
            &mut warnings,
        );
        s.breadcrumbs_auto_interval_secs = read_int(
            &root,
            "breadcrumbs_auto_interval_secs",
            defaults.breadcrumbs_auto_interval_secs,
            AppSettings::BREADCRUMB_INTERVAL_MIN_SECS,
            AppSettings::BREADCRUMB_INTERVAL_MAX_SECS,
            &mut warnings,
        );
        s.debug_overlay_enabled = read_bool(
            &root,
            "debug_overlay_enabled",
            defaults.debug_overlay_enabled,
            &mut warnings,
        );
        s.debug_show_candidates = read_bool(
            &root,
            "debug_show_candidates",
            defaults.debug_show_candidates,
            &mut warnings,
        );

        if let Some(od) = root.get("panel_org_display").and_then(|v| v.as_str()) {
            s.panel_org_display = if od.trim().eq_ignore_ascii_case("sid") {
                OrgDisplay::Sid
            } else {
                OrgDisplay::FullName
            };
        }
        s.panel_main_org_only = read_bool(
            &root,
            "panel_main_org_only",
            defaults.panel_main_org_only,
            &mut warnings,
        );
        if let Some(im) = root.get("import_mode").and_then(|v| v.as_str()) {
            s.import_mode = match im.trim().to_ascii_lowercase().as_str() {
                "merge_imported" => ImportMode::MergeImportedWins,
                "backup" => ImportMode::BackupExisting,
                _ => ImportMode::Merge,
            };
        }
        if let Some(tok) = root
            .get("linux_portal_restore_token")
            .and_then(|v| v.as_str())
        {
            s.linux_portal_restore_token = tok.trim().to_string();
        }

        AppSettingsLoadResult {
            settings: s,
            is_new: false,
            errors,
            warnings,
        }
    }

    pub fn load(path: &Path) -> AppSettingsLoadResult {
        if !path.exists() {
            return AppSettingsLoadResult {
                settings: AppSettings::default(),
                is_new: true,
                errors: Vec::new(),
                warnings: Vec::new(),
            };
        }
        match fs::read_to_string(path) {
            Ok(text) => Self::read(&text),
            Err(err) => AppSettingsLoadResult {
                settings: AppSettings::default(),
                is_new: false,
                errors: vec![format!("The file could not be read: {}", err)],
                warnings: Vec::new(),
            },
        }
    }

    pub fn save(path: &Path, settings: &AppSettings) -> Result<(), String> {
        let text = Self::write(settings);
        let check = Self::read(&text);
        if !check.is_valid() {
            return Err(format!(
                "The settings are not valid and were not saved: {}",
                check.errors.join(" ")
            ));
        }
        AtomicFile::write_text(path, &text, true)
            .map_err(|e| format!("The settings could not be saved: {}", e))
    }
}

fn bool_str(v: bool) -> &'static str {
    if v {
        "true"
    } else {
        "false"
    }
}

fn capture_method_text(m: CaptureMethod) -> &'static str {
    match m {
        CaptureMethod::DesktopDuplication => "desktop_duplication",
        CaptureMethod::WindowCapture => "wgc",
    }
}

fn import_mode_text(m: ImportMode) -> &'static str {
    match m {
        ImportMode::Merge => "merge_local",
        ImportMode::MergeImportedWins => "merge_imported",
        ImportMode::BackupExisting => "backup",
    }
}

fn org_display_text(d: OrgDisplay) -> &'static str {
    match d {
        OrgDisplay::FullName => "name",
        OrgDisplay::Sid => "sid",
    }
}

fn dark_mode_text(m: DarkMode) -> &'static str {
    match m {
        DarkMode::On => "on",
        DarkMode::Off => "off",
        DarkMode::System => "system",
    }
}

fn write_tracking_fps(fps: i32) -> String {
    if fps > AppSettings::TRACKING_FPS_HIGHEST {
        "\"max\"".to_string()
    } else {
        snap_step(
            fps,
            AppSettings::TRACKING_FPS_STEP,
            AppSettings::TRACKING_FPS_MIN,
            AppSettings::TRACKING_FPS_HIGHEST,
        )
        .to_string()
    }
}

fn snap_step(val: i32, step: i32, min: i32, max: i32) -> i32 {
    let q = (val as f64 / step as f64).round() as i32 * step;
    q.clamp(min, max)
}

fn read_bool(
    root: &toml::Table,
    key: &str,
    fallback: bool,
    warnings: &mut Vec<String>,
) -> bool {
    match root.get(key) {
        None => fallback,
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => {
                warnings.push(format!("{}: must be true or false.", key));
                fallback
            }
        },
    }
}

fn read_int(
    root: &toml::Table,
    key: &str,
    fallback: i32,
    min: i32,
    max: i32,
    warnings: &mut Vec<String>,
) -> i32 {
    match root.get(key) {
        None => fallback,
        Some(v) => match v.as_integer() {
            Some(n) => {
                if n >= min as i64 && n <= max as i64 {
                    n as i32
                } else {
                    let clamped = (n as i32).clamp(min, max);
                    warnings.push(format!(
                        "{}: {} is outside {} to {}; using {}.",
                        key, n, min, max, clamped
                    ));
                    clamped
                }
            }
            None => {
                warnings.push(format!("{}: must be a whole number.", key));
                fallback
            }
        },
    }
}

fn read_hotkey(
    root: &toml::Table,
    key: &str,
    fallback: &str,
    warnings: &mut Vec<String>,
) -> String {
    match root.get(key) {
        None => fallback.to_string(),
        Some(v) => match v.as_str() {
            Some(s) => {
                if s.trim().is_empty() {
                    return String::new();
                }
                match HotkeyText::parse(s) {
                    Ok(chord) => {
                        if chord.key.eq_ignore_ascii_case("Escape") {
                            warnings.push(format!("{}: Escape is reserved.", key));
                            fallback.to_string()
                        } else {
                            chord.format()
                        }
                    }
                    Err(e) => {
                        warnings.push(format!("{}: {}", key, e));
                        fallback.to_string()
                    }
                }
            }
            None => {
                warnings.push(format!("{}: must be text.", key));
                fallback.to_string()
            }
        },
    }
}

pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

