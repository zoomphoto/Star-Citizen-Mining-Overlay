#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod coordinator;
mod interactive_ui;
mod settings_ui;
mod state;
mod theme;

use chrono::Local;
use coordinator::spawn_coordinator;
use crossbeam_channel::unbounded;
use eframe::egui::{self, Margin, ViewportBuilder, ViewportCommand};
use interactive_ui::InteractiveOverlayState;
use iris_core::config::{
    AppSettingsToml, DataPaths, DossierNotes, SocialConfig, SocialConfigToml,
};
use iris_core::lookup::{CacheDatabase, RsiClient};
use iris_core::waypoints::WaypointStore;
use iris_platform::{IpcCommand, SingleInstanceIpc};
use parking_lot::{Mutex, RwLock};
use settings_ui::SettingsWindowState;
use state::AppSharedState;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const APP_ICO_BYTES: &[u8] = include_bytes!("../../../assets/iris-on.ico");

#[derive(Default)]
struct CliOptions {
    force_portable: bool,
    start_minimized: bool,
    ipc_cmd: Option<IpcCommand>,
    print_help: bool,
    print_version: bool,
}

impl CliOptions {
    fn parse() -> Self {
        let mut opts = Self::default();
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--portable" => opts.force_portable = true,
                "--minimized" | "--tray" | "--silent" => opts.start_minimized = true,
                "--settings" => opts.ipc_cmd = Some(IpcCommand::OpenSettings),
                "--toggle" => opts.ipc_cmd = Some(IpcCommand::ToggleOverlay),
                "--toggle-players" | "--players" => {
                    opts.ipc_cmd = Some(IpcCommand::TogglePlayersOrgs)
                }
                "--debug" => opts.ipc_cmd = Some(IpcCommand::ToggleDebugOverlay),
                "--timed" => opts.ipc_cmd = Some(IpcCommand::TimedOverlay),
                "--interactive" => opts.ipc_cmd = Some(IpcCommand::InteractiveOverlay),
                "--save-poi" => opts.ipc_cmd = Some(IpcCommand::SaveWaypoint),
                "--clear-trail" => opts.ipc_cmd = Some(IpcCommand::ClearBreadcrumbs),
                "--radar-zoom-in" => opts.ipc_cmd = Some(IpcCommand::RadarZoomIn),
                "--radar-zoom-out" => opts.ipc_cmd = Some(IpcCommand::RadarZoomOut),
                "--radar-scale-cycle" => opts.ipc_cmd = Some(IpcCommand::CycleRadarScale),
                "--quit" => opts.ipc_cmd = Some(IpcCommand::Quit),
                "-h" | "--help" => opts.print_help = true,
                "-V" | "--version" => opts.print_version = true,
                _ => {}
            }
        }
        opts
    }
}

fn attach_parent_console_on_windows() {
    #[cfg(windows)]
    unsafe {
        use windows_AttachConsole::{AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle == 0 || handle == -1isize {
            let _ = AttachConsole(u32::MAX);
        }
    }
}

#[cfg(windows)]
#[allow(non_snake_case)]
mod windows_AttachConsole {
    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetStdHandle(nStdHandle: u32) -> isize;
        pub fn AttachConsole(dwProcessId: u32) -> i32;
    }
}

fn main() -> anyhow::Result<()> {
    let cli = CliOptions::parse();
    if cli.print_version {
        attach_parent_console_on_windows();
        println!("sc-overlay {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cli.print_help {
        attach_parent_console_on_windows();
        println!(
            "SC Overlay v{} — Cross-platform Star Citizen overlay for Windows 11 & Bazzite Linux\n\n\
             USAGE:\n  sc-overlay [OPTIONS]\n\n\
             OPTIONS:\n  \
               --settings      Open or focus the Settings window\n  \
               --minimized     Start hidden in the system tray\n  \
               --toggle          Toggle the overlay on/off (signals running instance)\n  \
               --toggle-players  Toggle Players & Orgs nameplate scanning on/off (CPU saver)\n  \
               --debug           Toggle the live Debug / Heartbeat HUD on/off (signals running instance)\n  \
               --timed         Trigger timed overlay peek (signals running instance)\n  \
               --interactive   Open/close Interactive Citizen Dossier (signals running instance)\n  \
               --save-poi          Save current position as a POI beacon (signals running instance)\n  \
               --clear-trail       Clear recorded /showlocation mining breadcrumbs (signals running instance)\n  \
               --radar-zoom-in     Zoom breadcrumb radar in one step (e.g. 100 km -> 50 km)\n  \
               --radar-zoom-out    Zoom breadcrumb radar out one step (e.g. 10 km -> 25 km)\n  \
               --radar-scale-cycle Cycle breadcrumb radar scale presets (1 km .. 1000 km / Auto)\n  \
               --quit              Stop a running SC Overlay instance\n  \
               --portable      Store Config/, Data/, and Logs/ next to the executable\n  \
               -h, --help      Print help\n  \
               -V, --version   Print version",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    let paths = DataPaths::resolve(cli.force_portable);
    paths.seed_defaults_if_missing();

    let cmd_for_existing = cli.ipc_cmd.unwrap_or(IpcCommand::OpenSettings);
    let ipc = match SingleInstanceIpc::acquire_or_send(&paths.data_dir, cmd_for_existing) {
        Ok(guard) => {
            if cli.ipc_cmd == Some(IpcCommand::Quit) {
                return Ok(());
            }
            guard
        }
        Err(sent_to_existing) => {
            if sent_to_existing || cli.ipc_cmd == Some(IpcCommand::Quit) {
                return Ok(());
            }
            anyhow::bail!("Could not bind local single-instance IPC socket");
        }
    };

    let settings_loaded = AppSettingsToml::load(&paths.settings_file());
    let settings = settings_loaded.settings;

    init_logging(&paths, settings.session_log_enabled);
    tracing::info!(
        "Starting SC Overlay v{} (config_dir={}, data_dir={})",
        env!("CARGO_PKG_VERSION"),
        paths.config_dir.display(),
        paths.data_dir.display()
    );

    let social_loaded = SocialConfigToml::load(&paths.social_config_file());
    let mut social = social_loaded.config.unwrap_or_else(SocialConfig::default);
    social.ensure_normalized();

    let notes = Arc::new(RwLock::new(DossierNotes::load(&paths.dossier_notes_file())));
    let cache = Arc::new(
        CacheDatabase::open_file(&paths.cache_db_file())
            .or_else(|e| {
                tracing::warn!("Opening cache.db failed ({}), falling back to in-memory cache", e);
                CacheDatabase::open_in_memory()
            })?,
    );
    let rsi_client = Arc::new(RsiClient::new()?);
    let waypoints = WaypointStore::load(&paths.waypoints_file());

    // Determine whether the Settings window starts visible:
    // - Hidden if `--minimized` or if launched specifically to run an overlay action (`--toggle`, `--timed`, `--interactive`, `--save-poi`)
    // - Visible on normal launch or `--settings`
    let action_only = matches!(
        cli.ipc_cmd,
        Some(
            IpcCommand::ToggleOverlay
                | IpcCommand::TogglePlayersOrgs
                | IpcCommand::ToggleDebugOverlay
                | IpcCommand::TimedOverlay
                | IpcCommand::InteractiveOverlay
                | IpcCommand::SaveWaypoint
                | IpcCommand::ClearBreadcrumbs
                | IpcCommand::RadarZoomIn
                | IpcCommand::RadarZoomOut
                | IpcCommand::CycleRadarScale
        )
    );
    let start_settings_open = !cli.start_minimized && !action_only;

    let (coord_tx, coord_rx) = unbounded();
    let shared = AppSharedState::new(
        paths,
        settings,
        social,
        notes,
        cache,
        rsi_client,
        waypoints,
        start_settings_open,
        coord_tx,
    );

    if !social_loaded.errors.is_empty() {
        shared.set_status(
            format!(
                "Warning in social-config.txt: {}",
                social_loaded.errors.join("; ")
            ),
            true,
        );
    } else if !settings_loaded.warnings.is_empty() {
        shared.set_status(settings_loaded.warnings.join(" "), false);
    }

    spawn_coordinator(Arc::clone(&shared), ipc, coord_rx, cli.ipc_cmd);

    let mut viewport = ViewportBuilder::default()
        .with_title("SC Overlay")
        .with_app_id("sc-overlay")
        .with_inner_size([920.0, 620.0])
        .with_min_inner_size([780.0, 500.0])
        .with_visible(start_settings_open);

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app_shared = Arc::clone(&shared);
    eframe::run_native(
        "SC Overlay",
        native_options,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            app_shared.set_ui_ctx(cc.egui_ctx.clone());
            Ok(Box::new(ScOverlayEframeApp::new(
                app_shared,
                start_settings_open,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("UI event loop error: {}", e))?;

    Ok(())
}

struct ScOverlayEframeApp {
    shared: Arc<AppSharedState>,
    settings_ui: SettingsWindowState,
    interactive_ui: InteractiveOverlayState,
    root_visible: bool,
}

impl ScOverlayEframeApp {
    fn new(shared: Arc<AppSharedState>, initially_visible: bool) -> Self {
        Self {
            shared,
            settings_ui: SettingsWindowState::new(),
            interactive_ui: InteractiveOverlayState::new(),
            root_visible: initially_visible,
        }
    }
}

impl eframe::App for ScOverlayEframeApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.shared.quit_requested.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        // Intercept root window [X] so SC Overlay hides to the system tray instead of exiting
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.shared.settings_open.store(false, Ordering::Relaxed);
            self.root_visible = false;
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        let want_visible = self.shared.settings_open.load(Ordering::Relaxed);
        if want_visible != self.root_visible {
            self.root_visible = want_visible;
            ctx.send_viewport_cmd(ViewportCommand::Visible(want_visible));
            if want_visible {
                ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
        }

        let interactive_open = self.shared.interactive_open.load(Ordering::Relaxed);
        if self.root_visible || interactive_open {
            let (dark_mode, scale) = {
                let s = self.shared.settings.read();
                (s.settings_dark_mode, s.settings_window_scale)
            };
            let palette = theme::apply_theme(ctx, dark_mode, scale);

            if self.root_visible {
                let frame = egui::Frame::none()
                    .fill(palette.bg)
                    .inner_margin(Margin::symmetric(16.0, 12.0));
                egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
                    self.settings_ui.render(ui, &palette, &self.shared);
                });
            }

            // Secondary Interactive Overlay viewport (Shift+0) works even when Settings is hidden in tray
            self.interactive_ui.show_if_open(ctx, &self.shared);
            ctx.request_repaint_after(Duration::from_millis(200));
        } else {
            // Ensure interactive viewport tears down if it was just closed, then sleep deeply while in tray
            self.interactive_ui.show_if_open(ctx, &self.shared);
            ctx.request_repaint_after(Duration::from_millis(2000));
        }
    }
}

fn load_app_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(APP_ICO_BYTES).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

struct DualLogWriter {
    file: Option<Arc<Mutex<fs::File>>>,
}

impl Write for DualLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(f) = &self.file {
            let mut guard = f.lock();
            let _ = guard.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Some(f) = &self.file {
            let mut guard = f.lock();
            let _ = guard.flush();
        }
        Ok(())
    }
}

fn init_logging(paths: &DataPaths, session_log_enabled: bool) {
    let file_arc = if session_log_enabled {
        let _ = fs::create_dir_all(&paths.logs_dir);
        let date_str = Local::now().format("%Y-%m-%d").to_string();
        let log_path = paths.logs_dir.join(format!("sc-overlay-{}.log", date_str));
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .ok()
            .map(|f| Arc::new(Mutex::new(f)))
    } else {
        None
    };

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || DualLogWriter {
            file: file_arc.clone(),
        })
        .try_init();
}
