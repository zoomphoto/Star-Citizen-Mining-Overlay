use crate::state::{AppSharedState, CoordinatorCommand};
use chrono::{Duration as ChronoDuration, Utc};
use crossbeam_channel::Receiver;
use iris_core::config::AppSettings;
use iris_core::models::{IntRect, OverlayModel};
use iris_core::overlay::{
    BorderlessNoticeTracker, GameWindowRules, GameWindowState, SoftwareOverlayPainter,
    TestOverlayBuilder, TestOverlayPart, TrackingBoundary,
};
use iris_core::pipeline::{LiveDetectionPipeline, TimedOverlayPress, TimedOverlayRun};
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
    let mut tray = match create_tray_backend(!initial_settings.overlay_disabled) {
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
                    }
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

        if win_state != GameWindowState::Ready && !show_borderless_note {
            if !game.present {
                pipeline.reset_tracking();
            }
            if overlay_currently_shown {
                if let Some(o) = overlay.as_mut() {
                    o.hide();
                }
                overlay_currently_shown = false;
                last_drawn_model = None;
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
            thread::sleep(Duration::from_millis(80));
            continue;
        }

        let target_bounds = game.bounds;
        if target_bounds.width < 64 || target_bounds.height < 64 {
            thread::sleep(Duration::from_millis(100));
            continue;
        }

        // 8. Capture & Process Live Frame (or render toast-only model if disabled / not borderless)
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
                        let snap = {
                            let social_guard = shared.social.read();
                            pipeline.process_frame(&frame, &settings, &social_guard)
                        };
                        capturer.recycle_frame(frame);
                        if !snap.roster.is_empty() || last_roster_len != 0 {
                            last_roster_len = snap.roster.len();
                            *shared.live_roster.write() = snap.roster;
                        }
                        model_to_draw = snap.model;
                    } else if let Some(prev) = &last_drawn_model {
                        // Frame timeout / cursor-only update: keep existing HUD markers steady
                        model_to_draw = prev.clone();
                    }
                }
                Err(err) => {
                    tracing::debug!("Frame capture transient error: {}", err);
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
