use crate::state::{AppSharedState, CoordinatorCommand, LiveDiagnosticsSnapshot};
use chrono::{Duration as ChronoDuration, Utc};
use crossbeam_channel::Receiver;
use iris_core::config::{AppSettings, CaptureMethod};
use iris_core::models::{DebugOverlayInfo, DebugStatusLevel, IntRect, OverlayModel};
use iris_core::overlay::{
    BorderlessNoticeTracker, GameWindowRules, GameWindowState, SoftwareOverlayPainter,
    TestOverlayBuilder, TestOverlayPart, TrackingBoundary,
};
use iris_core::pipeline::{
    LiveDetectionPipeline, PipelineStats, TimedOverlayPress, TimedOverlayRun,
};
use iris_platform::{
    create_frame_capturer, create_game_detector, create_hotkey_listener, create_overlay_window,
    create_tray_backend, open_folder_or_file, HotkeyAction, IpcCommand, OverlayWindowBackend,
    SingleInstanceIpc, TrayAction,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const TEST_OVERLAY_DURATION_MS: f64 = 15_000.0;
const TOGGLE_TOAST_DURATION_MS: f64 = 2_400.0;
const BORDERLESS_TOAST_DURATION_MS: f64 = 8_000.0;

pub fn spawn_coordinator(
    shared: Arc<AppSharedState>,
    ipc: SingleInstanceIpc,
    cmd_rx: Receiver<CoordinatorCommand>,
    initial_action: Option<IpcCommand>,
) {
    thread::Builder::new()
        .name("sc-overlay-coordinator".to_string())
        .spawn(move || {
            run_coordinator_loop(shared, ipc, cmd_rx, initial_action);
        })
        .expect("failed to spawn sc-overlay-coordinator thread");
}

fn run_coordinator_loop(
    shared: Arc<AppSharedState>,
    ipc: SingleInstanceIpc,
    cmd_rx: Receiver<CoordinatorCommand>,
    initial_action: Option<IpcCommand>,
) {
    let epoch = Instant::now();
    let now_ms_fn = || epoch.elapsed().as_secs_f64() * 1000.0;

    let initial_settings = shared.settings.read().clone();
    let mut detector = create_game_detector();
    let mut capturer = create_frame_capturer();
    let mut hotkeys = create_hotkey_listener(&initial_settings);
    let mut tray = match create_tray_backend(
        !initial_settings.overlay_disabled,
        initial_settings.debug_overlay_enabled,
    ) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!("System tray unavailable: {}", e);
            None
        }
    };
    let mut overlay: Option<Box<dyn OverlayWindowBackend>> = match create_overlay_window() {
        Ok(o) => Some(o),
        Err(e) => {
            tracing::warn!("Overlay window init deferred ({})", e);
            None
        }
    };

    let mut pipeline = LiveDetectionPipeline::new(
        shared.paths.clone(),
        Arc::clone(&shared.cache),
        Arc::clone(&shared.notes),
        Arc::clone(&shared.rsi_client),
    );
    let mut painter = SoftwareOverlayPainter::new();
    let mut timed_run = TimedOverlayRun::default();
    let mut borderless_tracker = BorderlessNoticeTracker::default();
    let mut borderless_until_ms = f64::NEG_INFINITY;
    let mut transient_toast: Option<(String, f64)> = None;
    let mut test_overlay: Option<(TestOverlayPart, f64)> = None;
    let mut overlay_currently_shown = false;
    let mut last_drawn_model: Option<OverlayModel> = None;
    let mut last_drawn_bounds = IntRect::default();
    let mut last_drawn_vis_cap = false;
    let mut last_roster_len = 0usize;

    // Live pipeline & capture diagnostics
    let mut coordinator_ticks: u64 = 0;
    let mut rate_window_start_ms: f64 = 0.0;
    let mut ticks_in_window: u32 = 0;
    let mut captures_in_window: u32 = 0;
    let mut loop_hz: f64 = 0.0;
    let mut capture_fps: f64 = 0.0;
    let mut frames_captured: u64 = 0;
    let mut capture_timeouts: u64 = 0;
    let mut capture_errors: u64 = 0;
    let mut last_capture_ms: f64 = f64::NEG_INFINITY;
    let mut last_capture_error: Option<(String, f64)> = None;
    let mut last_stats = PipelineStats::default();
    let mut last_blank_picture = false;
    let mut heartbeat_step: u64 = 0;
    let mut last_debug_refresh_ms: f64 = f64::NEG_INFINITY;
    let mut cached_debug_info: Option<DebugOverlayInfo> = None;
    let mut last_diag_publish_ms: f64 = f64::NEG_INFINITY;

    // Startup background checks (Hosted Config daily sync + optional release update check)
    {
        let s = shared.settings.read().clone();
        if s.hosted_config_auto_check && !s.hosted_config_url.trim().is_empty() {
            let due = s
                .hosted_config_last_check
                .map(|last| Utc::now() - last >= ChronoDuration::hours(24))
                .unwrap_or(true);
            if due {
                shared.spawn_hosted_config_sync(false);
            }
        }
        shared.spawn_update_check();
    }

    let apply_toggle = |shared: &Arc<AppSharedState>,
                        tray: &mut Option<Box<dyn iris_platform::TrayBackend>>,
                        timed_run: &mut TimedOverlayRun,
                        transient_toast: &mut Option<(String, f64)>,
                        now_ms: f64| {
        let new_disabled = {
            let mut s = shared.settings.write();
            s.overlay_disabled = !s.overlay_disabled;
            s.overlay_disabled
        };
        timed_run.stop();
        if let Some(t) = tray.as_mut() {
            t.set_overlay_enabled(!new_disabled);
        }
        let msg = if new_disabled {
            "Overlay DISABLED"
        } else {
            "Overlay ENABLED"
        };
        *transient_toast = Some((msg.to_string(), now_ms + TOGGLE_TOAST_DURATION_MS));
        shared.save_settings(false);
    };

    let apply_debug_toggle = |shared: &Arc<AppSharedState>,
                              tray: &mut Option<Box<dyn iris_platform::TrayBackend>>,
                              transient_toast: &mut Option<(String, f64)>,
                              now_ms: f64| {
        let new_debug = {
            let mut s = shared.settings.write();
            s.debug_overlay_enabled = !s.debug_overlay_enabled;
            s.debug_overlay_enabled
        };
        if let Some(t) = tray.as_mut() {
            t.set_debug_enabled(new_debug);
        }
        let msg = if new_debug {
            "Debug HUD ON"
        } else {
            "Debug HUD OFF"
        };
        *transient_toast = Some((msg.to_string(), now_ms + TOGGLE_TOAST_DURATION_MS));
        shared.save_settings(false);
    };

    let apply_timed = |shared: &Arc<AppSharedState>,
                       timed_run: &mut TimedOverlayRun,
                       now_ms: f64| {
        let (disabled, secs) = {
            let s = shared.settings.read();
            (s.overlay_disabled, s.timed_overlay_seconds)
        };
        let res = timed_run.press(disabled, secs, now_ms.round() as i64);
        if matches!(res, TimedOverlayPress::Started | TimedOverlayPress::Restarted) {
            tracing::debug!("Timed overlay peek activated for {}s", secs);
        }
    };

    let apply_interactive = |shared: &Arc<AppSharedState>| {
        let prev = shared.interactive_open.load(Ordering::Relaxed);
        shared.interactive_open.store(!prev, Ordering::Relaxed);
        shared.request_repaint();
    };

    if let Some(cmd) = initial_action {
        let now_ms = now_ms_fn();
        match cmd {
            IpcCommand::OpenSettings => {
                shared.settings_open.store(true, Ordering::Relaxed);
                shared.request_repaint();
            }
            IpcCommand::ToggleOverlay => {
                apply_toggle(
                    &shared,
                    &mut tray,
                    &mut timed_run,
                    &mut transient_toast,
                    now_ms,
                );
            }
            IpcCommand::ToggleDebugOverlay => {
                apply_debug_toggle(&shared, &mut tray, &mut transient_toast, now_ms);
            }
            IpcCommand::TimedOverlay => apply_timed(&shared, &mut timed_run, now_ms),
            IpcCommand::InteractiveOverlay => apply_interactive(&shared),
            IpcCommand::Quit => {
                shared.quit_requested.store(true, Ordering::Relaxed);
                shared.request_repaint();
                return;
            }
        }
    }

    while !shared.quit_requested.load(Ordering::Relaxed) {
        let tick_start = Instant::now();
        let now_ms = now_ms_fn();
        coordinator_ticks = coordinator_ticks.wrapping_add(1);
        ticks_in_window += 1;
        if now_ms - rate_window_start_ms >= 500.0 {
            let dt_s = ((now_ms - rate_window_start_ms) / 1000.0).max(0.001);
            loop_hz = ticks_in_window as f64 / dt_s;
            capture_fps = captures_in_window as f64 / dt_s;
            rate_window_start_ms = now_ms;
            ticks_in_window = 0;
            captures_in_window = 0;
        }

        // 1. Drain IPC commands from secondary CLI invocations
        while let Some(ipc_cmd) = ipc.try_recv() {
            match ipc_cmd {
                IpcCommand::OpenSettings => {
                    shared.settings_open.store(true, Ordering::Relaxed);
                    shared.request_repaint();
                }
                IpcCommand::ToggleOverlay => {
                    apply_toggle(
                        &shared,
                        &mut tray,
                        &mut timed_run,
                        &mut transient_toast,
                        now_ms,
                    );
                }
                IpcCommand::ToggleDebugOverlay => {
                    apply_debug_toggle(&shared, &mut tray, &mut transient_toast, now_ms);
                }
                IpcCommand::TimedOverlay => apply_timed(&shared, &mut timed_run, now_ms),
                IpcCommand::InteractiveOverlay => apply_interactive(&shared),
                IpcCommand::Quit => {
                    shared.quit_requested.store(true, Ordering::Relaxed);
                    shared.request_repaint();
                }
            }
        }

        // 2. Drain System Tray actions
        let mut pending_tray_actions = Vec::new();
        if let Some(t) = tray.as_ref() {
            while let Some(act) = t.try_recv() {
                pending_tray_actions.push(act);
            }
        }
        for act in pending_tray_actions {
            match act {
                TrayAction::OpenSettings => {
                    shared.settings_open.store(true, Ordering::Relaxed);
                    shared.request_repaint();
                }
                TrayAction::ToggleOverlay => {
                    apply_toggle(
                        &shared,
                        &mut tray,
                        &mut timed_run,
                        &mut transient_toast,
                        now_ms,
                    );
                }
                TrayAction::ToggleDebugOverlay => {
                    apply_debug_toggle(&shared, &mut tray, &mut transient_toast, now_ms);
                }
                TrayAction::OpenLogs => {
                    let _ = open_folder_or_file(&shared.paths.logs_dir);
                }
                TrayAction::OpenConfigDir => {
                    let _ = open_folder_or_file(&shared.paths.config_dir);
                }
                TrayAction::Quit => {
                    shared.quit_requested.store(true, Ordering::Relaxed);
                    shared.request_repaint();
                }
            }
        }

        // 3. Drain Global Hotkey actions
        while let Some(hk) = hotkeys.try_recv() {
            match hk {
                HotkeyAction::ToggleOverlay => {
                    apply_toggle(
                        &shared,
                        &mut tray,
                        &mut timed_run,
                        &mut transient_toast,
                        now_ms,
                    );
                }
                HotkeyAction::TimedOverlay => apply_timed(&shared, &mut timed_run, now_ms),
                HotkeyAction::InteractiveOverlay => apply_interactive(&shared),
            }
        }

        // 4. Drain UI CoordinatorCommands
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                CoordinatorCommand::SettingsUpdated { reset_capture } => {
                    let s = shared.settings.read().clone();
                    hotkeys.update_bindings(&s);
                    if let Some(t) = tray.as_mut() {
                        t.set_overlay_enabled(!s.overlay_disabled);
                        t.set_debug_enabled(s.debug_overlay_enabled);
                    }
                    cached_debug_info = None;
                    if reset_capture {
                        capturer.reset();
                    }
                }
                CoordinatorCommand::ShowTestOverlay(part) => {
                    test_overlay = Some((part, now_ms + TEST_OVERLAY_DURATION_MS));
                }
                CoordinatorCommand::ReloadExclusions => {
                    pipeline.reload_exclusions();
                }
                CoordinatorCommand::LookupPlayer(handle) => {
                    pipeline.request_lookup(&handle, true, now_ms);
                }
                CoordinatorCommand::ToggleOverlay => {
                    apply_toggle(
                        &shared,
                        &mut tray,
                        &mut timed_run,
                        &mut transient_toast,
                        now_ms,
                    );
                }
                CoordinatorCommand::ToggleDebugOverlay => {
                    apply_debug_toggle(&shared, &mut tray, &mut transient_toast, now_ms);
                }
                CoordinatorCommand::TimedOverlay => {
                    apply_timed(&shared, &mut timed_run, now_ms);
                }
                CoordinatorCommand::OpenInteractive => {
                    apply_interactive(&shared);
                }
            }
        }

        if shared.quit_requested.load(Ordering::Relaxed) {
            break;
        }

        // Expire timers
        timed_run.expire(now_ms.round() as i64);
        if let Some((_, until)) = &transient_toast {
            if now_ms >= *until {
                transient_toast = None;
            }
        }
        if let Some((_, until)) = &test_overlay {
            if now_ms >= *until {
                test_overlay = None;
            }
        }

        // 5. Poll Star Citizen window state
        let game = detector.poll();
        *shared.last_game_window.write() = game.clone();

        let mut settings = shared.settings.read().clone();
        let interactive_open = shared.interactive_open.load(Ordering::Relaxed);

        // Retry creating overlay window lazily if display server wasn't ready earlier
        if overlay.is_none() && (game.present || test_overlay.is_some()) {
            if let Ok(o) = create_overlay_window() {
                overlay = Some(o);
            }
        }

        // Hide click-through overlay while interactive viewport is open on top
        if interactive_open {
            if overlay_currently_shown {
                if let Some(o) = overlay.as_mut() {
                    o.hide();
                }
                overlay_currently_shown = false;
                last_drawn_model = None;
            }
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        // 6. Test Overlay Preview Mode (works on desktop even if Star Citizen is closed!)
        if let Some((part, until_ms)) = test_overlay {
            let target_bounds = if game.present && !game.bounds.is_empty() {
                game.bounds
            } else if !game.monitor_bounds.is_empty() {
                game.monitor_bounds
            } else {
                IntRect::new(0, 0, 1920, 1080)
            };

            let boundary = TrackingBoundary::from_settings(&settings);
            let mut model = {
                let social_guard = shared.social.read();
                TestOverlayBuilder::build(
                    &social_guard,
                    target_bounds.width as f64,
                    target_bounds.height as f64,
                    Some(boundary),
                    part,
                    Some(&settings),
                )
            };
            let secs_left = (((until_ms - now_ms) / 1000.0).ceil() as i32).max(1);
            model.toast = Some(format!("Test Preview ({}s)", secs_left));
            if let Some(dbg) = model.debug_info.as_mut() {
                let step = (now_ms / 250.0) as u64;
                dbg.heartbeat_phase = (step % 4) as u8;
                dbg.headline = format!("PREVIEW - SCANNING (IDLE)  #{:04}", step % 10_000);
            }

            if let Some(o) = overlay.as_mut() {
                let vis_cap = settings.overlay_shown_in_capture();
                let unchanged = overlay_currently_shown
                    && last_drawn_bounds == target_bounds
                    && last_drawn_vis_cap == vis_cap
                    && last_drawn_model.as_ref() == Some(&model);
                if !unchanged {
                    let bgra = painter.paint_slice(
                        &model,
                        &settings,
                        target_bounds.width as u32,
                        target_bounds.height as u32,
                    );
                    if o.present(target_bounds, bgra, vis_cap).is_ok() {
                        overlay_currently_shown = true;
                        last_drawn_bounds = target_bounds;
                        last_drawn_vis_cap = vis_cap;
                        last_drawn_model = Some(model);
                    }
                }
            }
            if now_ms - last_diag_publish_ms >= 250.0 {
                last_diag_publish_ms = now_ms;
                publish_diagnostics(
                    &shared,
                    coordinator_ticks,
                    loop_hz,
                    capture_fps,
                    frames_captured,
                    capture_timeouts,
                    capture_errors,
                    &last_capture_error,
                    last_blank_picture,
                    &last_stats,
                    &settings,
                    overlay_currently_shown,
                    "Test Preview Active",
                    now_ms,
                );
            }
            thread::sleep(Duration::from_millis(80));
            continue;
        }

        // 7. Evaluate Star Citizen window readiness & borderless notice
        let win_state = GameWindowRules::decide(game.focused && !game.minimized, game.window_mode);
        if borderless_tracker.should_show(
            win_state,
            settings.overlay_disabled,
            shared.settings_open.load(Ordering::Relaxed) || interactive_open,
            game.window_mode,
        ) {
            borderless_tracker.mark_shown();
            borderless_until_ms = now_ms + BORDERLESS_TOAST_DURATION_MS;
        }

        let show_borderless_note = now_ms < borderless_until_ms && game.focused && !game.minimized;
        let effective_overlay_active = !settings.overlay_disabled || timed_run.is_active;
        let show_toggle_toast = transient_toast.is_some() && game.focused && !game.minimized;
        let show_debug_hud =
            settings.debug_overlay_enabled && effective_overlay_active && game.focused && !game.minimized;

        if win_state != GameWindowState::Ready && !show_borderless_note && !show_debug_hud {
            if !game.present {
                pipeline.reset_tracking();
                capture_fps = 0.0;
                last_blank_picture = false;
            }
            if overlay_currently_shown {
                if let Some(o) = overlay.as_mut() {
                    o.hide();
                }
                overlay_currently_shown = false;
                last_drawn_model = None;
            }
            if now_ms - last_diag_publish_ms >= 250.0 {
                last_diag_publish_ms = now_ms;
                let idle_status = if !game.present {
                    "Waiting for Star Citizen"
                } else if game.minimized {
                    "Paused (Star Citizen Minimized)"
                } else if win_state == GameWindowState::NotBorderless {
                    "Star Citizen Not Borderless"
                } else {
                    "Paused (Star Citizen Unfocused)"
                };
                publish_diagnostics(
                    &shared,
                    coordinator_ticks,
                    loop_hz,
                    capture_fps,
                    frames_captured,
                    capture_timeouts,
                    capture_errors,
                    &last_capture_error,
                    last_blank_picture,
                    &last_stats,
                    &settings,
                    overlay_currently_shown,
                    idle_status,
                    now_ms,
                );
            }
            thread::sleep(Duration::from_millis(120));
            continue;
        }

        if !effective_overlay_active && !show_toggle_toast && !show_borderless_note {
            if overlay_currently_shown {
                if let Some(o) = overlay.as_mut() {
                    o.hide();
                }
                overlay_currently_shown = false;
                last_drawn_model = None;
            }
            if now_ms - last_diag_publish_ms >= 250.0 {
                last_diag_publish_ms = now_ms;
                publish_diagnostics(
                    &shared,
                    coordinator_ticks,
                    loop_hz,
                    0.0,
                    frames_captured,
                    capture_timeouts,
                    capture_errors,
                    &last_capture_error,
                    last_blank_picture,
                    &last_stats,
                    &settings,
                    overlay_currently_shown,
                    "Overlay Disabled",
                    now_ms,
                );
            }
            thread::sleep(Duration::from_millis(80));
            continue;
        }

        let target_bounds = game.bounds;
        if target_bounds.width < 64 || target_bounds.height < 64 {
            thread::sleep(Duration::from_millis(100));
            continue;
        }

        // 8. Capture & Process Live Frame (or render toast/debug model if disabled / not borderless)
        let mut model_to_draw = OverlayModel {
            window_width: target_bounds.width,
            window_height: target_bounds.height,
            ..Default::default()
        };

        if win_state == GameWindowState::Ready && effective_overlay_active {
            match capturer.capture_frame(&game, &mut settings, now_ms) {
                Ok((maybe_frame, mutated_settings)) => {
                    if mutated_settings {
                        shared.settings.write().linux_portal_restore_token =
                            settings.linux_portal_restore_token.clone();
                        shared.save_settings(false);
                    }
                    if let Some(frame) = maybe_frame {
                        frames_captured = frames_captured.wrapping_add(1);
                        captures_in_window += 1;
                        last_capture_ms = now_ms;
                        let snap = {
                            let social_guard = shared.social.read();
                            pipeline.process_frame(&frame, &settings, &social_guard)
                        };
                        capturer.recycle_frame(frame);
                        if !snap.roster.is_empty() || last_roster_len != 0 {
                            last_roster_len = snap.roster.len();
                            *shared.live_roster.write() = snap.roster;
                        }
                        last_stats = snap.stats;
                        last_blank_picture = snap.blank_picture;
                        model_to_draw = snap.model;
                    } else {
                        capture_timeouts = capture_timeouts.wrapping_add(1);
                        if let Some(prev) = &last_drawn_model {
                            // Frame timeout / cursor-only update: keep existing HUD markers steady
                            model_to_draw.brackets = prev.brackets.clone();
                            model_to_draw.panel_entries = prev.panel_entries.clone();
                            model_to_draw.mining_labels = prev.mining_labels.clone();
                            model_to_draw.note = prev.note.clone();
                        }
                    }
                }
                Err(err) => {
                    tracing::debug!("Frame capture transient error: {}", err);
                    capture_errors = capture_errors.wrapping_add(1);
                    last_capture_error = Some((sanitize_ascii(&err.to_string(), 42), now_ms));
                    capturer.reset();
                }
            }
        }

        if show_borderless_note {
            model_to_draw.note = Some(BorderlessNoticeTracker::TEXT.to_string());
        }
        if timed_run.is_active {
            let secs = timed_run.seconds_left(now_ms.round() as i64);
            if secs > 0 {
                model_to_draw.toast = Some(TimedOverlayRun::countdown_text(secs));
            }
        } else if let Some((ref msg, _)) = transient_toast {
            model_to_draw.toast = Some(msg.clone());
        }

        let active_status = compute_active_status_text(
            win_state,
            &last_capture_error,
            last_capture_ms,
            last_blank_picture,
            &model_to_draw,
            now_ms,
        );

        if show_debug_hud {
            let refresh_interval = if settings.debug_show_candidates {
                125.0
            } else {
                250.0
            };
            if cached_debug_info.is_none() || now_ms - last_debug_refresh_ms >= refresh_interval {
                heartbeat_step = heartbeat_step.wrapping_add(1);
                last_debug_refresh_ms = now_ms;
                cached_debug_info = Some(build_debug_overlay_info(
                    win_state,
                    &settings,
                    target_bounds,
                    loop_hz,
                    capture_fps,
                    frames_captured,
                    capture_errors,
                    &last_capture_error,
                    last_capture_ms,
                    last_blank_picture,
                    &last_stats,
                    &model_to_draw,
                    heartbeat_step,
                    now_ms,
                ));
            }
            model_to_draw.debug_info = cached_debug_info.clone();
        } else {
            cached_debug_info = None;
            model_to_draw.debug_info = None;
        }

        if model_to_draw.is_empty() {
            if overlay_currently_shown {
                if let Some(o) = overlay.as_mut() {
                    o.hide();
                }
                overlay_currently_shown = false;
                last_drawn_model = None;
            }
        } else if let Some(o) = overlay.as_mut() {
            let vis_cap = settings.overlay_shown_in_capture();
            let unchanged = overlay_currently_shown
                && last_drawn_bounds == target_bounds
                && last_drawn_vis_cap == vis_cap
                && last_drawn_model.as_ref() == Some(&model_to_draw);
            if !unchanged {
                let bgra = painter.paint_slice(
                    &model_to_draw,
                    &settings,
                    target_bounds.width as u32,
                    target_bounds.height as u32,
                );
                if o.present(target_bounds, bgra, vis_cap).is_ok() {
                    overlay_currently_shown = true;
                    last_drawn_bounds = target_bounds;
                    last_drawn_vis_cap = vis_cap;
                    last_drawn_model = Some(model_to_draw);
                }
            }
        }

        if now_ms - last_diag_publish_ms >= 250.0 {
            last_diag_publish_ms = now_ms;
            publish_diagnostics(
                &shared,
                coordinator_ticks,
                loop_hz,
                capture_fps,
                frames_captured,
                capture_timeouts,
                capture_errors,
                &last_capture_error,
                last_blank_picture,
                &last_stats,
                &settings,
                overlay_currently_shown,
                &active_status,
                now_ms,
            );
        }

        // 9. Frame rate pacing
        let target_fps = if settings.nameplate_tracking_fps > AppSettings::TRACKING_FPS_HIGHEST {
            120.0
        } else {
            settings.nameplate_tracking_fps.max(AppSettings::TRACKING_FPS_MIN) as f64
        };
        let frame_budget = Duration::from_secs_f64(1.0 / target_fps);
        let elapsed = tick_start.elapsed();
        if elapsed < frame_budget {
            thread::sleep(frame_budget - elapsed);
        }
    }

    if let Some(mut o) = overlay {
        o.hide();
    }
}

fn sanitize_ascii(raw: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_chars));
    for c in raw.chars() {
        if out.chars().count() >= max_chars {
            break;
        }
        if (' '..='~').contains(&c) {
            out.push(c);
        } else if c.is_whitespace() {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

fn capture_backend_short_name(method: CaptureMethod) -> &'static str {
    match method {
        CaptureMethod::DesktopDuplication => {
            if cfg!(target_os = "linux") {
                "PipeWire/X11"
            } else {
                "DXGI"
            }
        }
        CaptureMethod::WindowCapture => {
            if cfg!(target_os = "linux") {
                "XDG Portal"
            } else {
                "WGC"
            }
        }
    }
}

fn compute_active_status_text(
    win_state: GameWindowState,
    last_capture_error: &Option<(String, f64)>,
    last_capture_ms: f64,
    last_blank_picture: bool,
    model_to_draw: &OverlayModel,
    now_ms: f64,
) -> String {
    if win_state == GameWindowState::NotBorderless {
        return "Not Borderless (Windowed/Fullscreen)".to_string();
    }
    if let Some((_, err_ms)) = last_capture_error {
        if now_ms - *err_ms < 3_000.0 && last_capture_ms < *err_ms {
            return "Capture Error".to_string();
        }
    }
    if last_blank_picture {
        return "Blank Black Frame".to_string();
    }
    if now_ms - last_capture_ms > 1_200.0 {
        return "Scanning (Static Screen / Idle)".to_string();
    }
    let active_targets = model_to_draw.brackets.len() + model_to_draw.mining_labels.len();
    if active_targets > 0 {
        format!("Tracking ({} active)", active_targets)
    } else {
        "Scanning (Idle - 0 targets)".to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn build_debug_overlay_info(
    win_state: GameWindowState,
    settings: &AppSettings,
    target_bounds: IntRect,
    loop_hz: f64,
    capture_fps: f64,
    frames_captured: u64,
    capture_errors: u64,
    last_capture_error: &Option<(String, f64)>,
    last_capture_ms: f64,
    last_blank_picture: bool,
    stats: &PipelineStats,
    model_to_draw: &OverlayModel,
    heartbeat_step: u64,
    now_ms: f64,
) -> DebugOverlayInfo {
    let recent_err = last_capture_error
        .as_ref()
        .filter(|(_, err_ms)| now_ms - *err_ms < 3_000.0 && last_capture_ms < *err_ms);

    let (level, status_tag) = if win_state == GameWindowState::NotBorderless {
        (
            DebugStatusLevel::Warning,
            "WARN - NOT BORDERLESS".to_string(),
        )
    } else if recent_err.is_some() {
        (DebugStatusLevel::Error, "ERR - CAPTURE FAILED".to_string())
    } else if last_blank_picture {
        (
            DebugStatusLevel::Warning,
            "WARN - BLANK BLACK FRAME".to_string(),
        )
    } else if now_ms - last_capture_ms > 1_200.0 {
        (
            DebugStatusLevel::Idle,
            "LIVE - STATIC SCREEN (IDLE)".to_string(),
        )
    } else {
        let active_targets = model_to_draw.brackets.len() + model_to_draw.mining_labels.len();
        if active_targets > 0 {
            (
                DebugStatusLevel::Active,
                format!("LIVE - TRACKING ({} ACTIVE)", active_targets),
            )
        } else {
            (
                DebugStatusLevel::Active,
                "LIVE - SCANNING (0 TARGETS)".to_string(),
            )
        }
    };

    let headline = format!("{}  #{:04}", status_tag, heartbeat_step % 10_000);
    let backend = capture_backend_short_name(settings.capture_method);

    let line1 = format!(
        "Loop: {:.0} Hz  |  Cap: {:.0} FPS ({} {}x{})  |  Scan: {:.1} ms",
        loop_hz,
        capture_fps,
        backend,
        target_bounds.width,
        target_bounds.height,
        stats.process_ms
    );

    let line2 = format!(
        "Plates: {} box, {} pair, {} OCR  |  Tracks: {} conf, {} tent  |  RSI: {}",
        stats.candidate_boxes,
        stats.candidate_pairs,
        stats.ocr_reads,
        stats.confirmed_tracks,
        stats.tentative_tracks,
        stats.in_flight_lookups
    );

    let mining_txt = if settings.mining_disabled {
        "Off".to_string()
    } else if let Some(lbl) = &stats.mining_locked {
        format!("Locked ({})", sanitize_ascii(lbl, 18))
    } else {
        "Searching".to_string()
    };

    let ocr_txt = match &stats.last_read_sample {
        Some((sample, verdict, seen_ms)) if now_ms - *seen_ms < 20_000.0 => {
            let age_s = ((now_ms - *seen_ms) / 1000.0).max(0.0);
            format!(
                "\"{}\" ({}, {:.0}s)",
                sanitize_ascii(sample, 18),
                verdict,
                age_s
            )
        }
        _ => "none".to_string(),
    };

    let line3 = if win_state == GameWindowState::NotBorderless {
        "Set Options > Graphics > Window Mode = Borderless in Star Citizen".to_string()
    } else if let Some((err_txt, err_ms)) = last_capture_error {
        if now_ms - *err_ms < 8_000.0 {
            format!(
                "Cap Err: {}  |  Frames: {} ({} err)",
                err_txt, frames_captured, capture_errors
            )
        } else {
            format!(
                "Mining: {}  |  Last OCR: {}  |  Frames: {}",
                mining_txt, ocr_txt, frames_captured
            )
        }
    } else {
        format!(
            "Mining: {}  |  Last OCR: {}  |  Frames: {}",
            mining_txt, ocr_txt, frames_captured
        )
    };

    let (candidate_boxes, scan_boundary) =
        if settings.debug_show_candidates && win_state == GameWindowState::Ready {
            let boundary = TrackingBoundary::from_settings(settings);
            (
                stats.debug_boxes.clone(),
                Some(boundary.to_pixels(target_bounds.width as f64, target_bounds.height as f64)),
            )
        } else {
            (Vec::new(), None)
        };

    DebugOverlayInfo {
        level,
        headline,
        lines: vec![line1, line2, line3],
        heartbeat_phase: (heartbeat_step % 4) as u8,
        candidate_boxes,
        scan_boundary,
    }
}

#[allow(clippy::too_many_arguments)]
fn publish_diagnostics(
    shared: &Arc<AppSharedState>,
    coordinator_ticks: u64,
    loop_hz: f64,
    capture_fps: f64,
    frames_captured: u64,
    capture_timeouts: u64,
    capture_errors: u64,
    last_capture_error: &Option<(String, f64)>,
    blank_frame: bool,
    stats: &PipelineStats,
    settings: &AppSettings,
    overlay_visible: bool,
    active_status: &str,
    now_ms: f64,
) {
    let mining_status = if settings.mining_disabled {
        "Disabled".to_string()
    } else if let Some(lbl) = &stats.mining_locked {
        format!("Locked: {}", lbl)
    } else {
        "Searching".to_string()
    };

    let last_ocr_sample = stats.last_read_sample.as_ref().map(|(txt, verdict, seen_ms)| {
        let age_s = ((now_ms - *seen_ms) / 1000.0).max(0.0);
        format!("\"{}\" ({}, {:.0}s ago)", txt, verdict, age_s)
    });

    let snap = LiveDiagnosticsSnapshot {
        coordinator_ticks,
        loop_hz,
        capture_fps,
        last_scan_ms: stats.process_ms,
        frames_captured,
        capture_timeouts,
        capture_errors,
        last_capture_error: last_capture_error.as_ref().map(|(e, _)| e.clone()),
        blank_frame,
        candidate_boxes: stats.candidate_boxes,
        candidate_pairs: stats.candidate_pairs,
        ocr_reads: stats.ocr_reads,
        confirmed_tracks: stats.confirmed_tracks,
        tentative_tracks: stats.tentative_tracks,
        in_flight_lookups: stats.in_flight_lookups,
        mining_status,
        last_ocr_sample,
        overlay_visible,
        active_status: active_status.to_string(),
    };
    *shared.live_diagnostics.write() = snap;
}
