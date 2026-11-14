use chrono::Utc;
use crossbeam_channel::Sender;
use eframe::egui;
use iris_core::config::{
    AppSettings, AppSettingsToml, DataPaths, DossierNotes, HostedConfigClient, SocialConfig,
    SocialConfigMerger, SocialConfigToml,
};
use iris_core::lookup::{CacheDatabase, NotFoundKind, RsiClient, RsiNames, RsiResult};
use iris_core::overlay::TestOverlayPart;
use iris_core::pipeline::RosterEntry;
use iris_platform::GameWindowInfo;
use parking_lot::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum CoordinatorCommand {
    SettingsUpdated { reset_capture: bool },
    ShowTestOverlay(TestOverlayPart),
    ReloadExclusions,
    LookupPlayer(String),
    ToggleOverlay,
    ToggleDebugOverlay,
    TimedOverlay,
    OpenInteractive,
}

#[derive(Debug, Clone, Default)]
pub struct LiveDiagnosticsSnapshot {
    pub coordinator_ticks: u64,
    pub loop_hz: f64,
    pub capture_fps: f64,
    pub last_scan_ms: f64,
    pub frames_captured: u64,
    pub capture_timeouts: u64,
    pub capture_errors: u64,
    pub last_capture_error: Option<String>,
    pub blank_frame: bool,
    pub candidate_boxes: usize,
    pub candidate_pairs: usize,
    pub ocr_reads: usize,
    pub confirmed_tracks: usize,
    pub tentative_tracks: usize,
    pub in_flight_lookups: usize,
    pub mining_status: String,
    pub last_ocr_sample: Option<String>,
    pub overlay_visible: bool,
    pub active_status: String,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub is_error: bool,
}

pub struct AppSharedState {
    pub paths: DataPaths,
    pub settings: RwLock<AppSettings>,
    pub social: RwLock<SocialConfig>,
    pub notes: Arc<RwLock<DossierNotes>>,
    pub cache: Arc<CacheDatabase>,
    pub rsi_client: Arc<RsiClient>,
    pub ui_ctx: Mutex<Option<egui::Context>>,
    pub settings_open: AtomicBool,
    pub interactive_open: AtomicBool,
    pub quit_requested: AtomicBool,
    pub sync_in_progress: AtomicBool,
    pub refresh_in_progress: AtomicBool,
    pub last_game_window: RwLock<GameWindowInfo>,
    pub live_roster: RwLock<Vec<RosterEntry>>,
    pub live_diagnostics: RwLock<LiveDiagnosticsSnapshot>,
    pub status_banner: RwLock<Option<StatusMessage>>,
    pub update_available: RwLock<Option<(String, String)>>,
    pub coordinator_tx: Sender<CoordinatorCommand>,
}

impl AppSharedState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        paths: DataPaths,
        settings: AppSettings,
        social: SocialConfig,
        notes: Arc<RwLock<DossierNotes>>,
        cache: Arc<CacheDatabase>,
        rsi_client: Arc<RsiClient>,
        start_settings_open: bool,
        coordinator_tx: Sender<CoordinatorCommand>,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths,
            settings: RwLock::new(settings),
            social: RwLock::new(social),
            notes,
            cache,
            rsi_client,
            ui_ctx: Mutex::new(None),
            settings_open: AtomicBool::new(start_settings_open),
            interactive_open: AtomicBool::new(false),
            quit_requested: AtomicBool::new(false),
            sync_in_progress: AtomicBool::new(false),
            refresh_in_progress: AtomicBool::new(false),
            last_game_window: RwLock::new(GameWindowInfo::default()),
            live_roster: RwLock::new(Vec::new()),
            live_diagnostics: RwLock::new(LiveDiagnosticsSnapshot::default()),
            status_banner: RwLock::new(None),
            update_available: RwLock::new(None),
            coordinator_tx,
        })
    }

    pub fn set_ui_ctx(&self, ctx: egui::Context) {
        *self.ui_ctx.lock() = Some(ctx);
    }

    pub fn request_repaint(&self) {
        if let Some(ctx) = self.ui_ctx.lock().as_ref() {
            ctx.request_repaint();
        }
    }

    pub fn set_status(&self, text: impl Into<String>, is_error: bool) {
        *self.status_banner.write() = Some(StatusMessage {
            text: text.into(),
            is_error,
        });
        self.request_repaint();
    }

    pub fn clear_status(&self) {
        *self.status_banner.write() = None;
        self.request_repaint();
    }

    pub fn save_settings(&self, reset_capture: bool) {
        let snap = self.settings.read().clone();
        if let Err(err) = AppSettingsToml::save(&self.paths.settings_file(), &snap) {
            self.set_status(err, true);
        } else {
            let _ = self
                .coordinator_tx
                .send(CoordinatorCommand::SettingsUpdated { reset_capture });
        }
        self.request_repaint();
    }

    pub fn save_social(&self) {
        let snap = {
            let mut guard = self.social.write();
            guard.ensure_normalized();
            guard.clone()
        };
        if let Err(err) = SocialConfigToml::save(&self.paths.social_config_file(), &snap) {
            self.set_status(err, true);
        }
        self.request_repaint();
    }

    pub fn save_notes(&self) {
        let guard = self.notes.read();
        if let Err(err) = guard.save_to(&self.paths.dossier_notes_file()) {
            self.set_status(format!("Failed saving dossier-notes.txt: {}", err), true);
        }
    }

    /// Background RSI lookup + verification when adding a player or inspecting a handle in Dossier.
    pub fn spawn_verify_and_cache_player(self: &Arc<Self>, raw_handle: String, announce: bool) {
        let trimmed = raw_handle.trim().to_string();
        if !RsiNames::is_valid_handle(&trimmed) {
            if announce {
                self.set_status(
                    format!("\"{}\" is not a valid Star Citizen handle.", trimmed),
                    true,
                );
            }
            return;
        }
        let shared = Arc::clone(self);
        thread::Builder::new()
            .name("sc-overlay-verify-player".to_string())
            .spawn(move || {
                let now = Utc::now();
                let mut canonical = trimmed.clone();
                let mut found_any = false;

                match shared.rsi_client.fetch_player_profile(&trimmed) {
                    RsiResult::Found(prof) => {
                        found_any = true;
                        canonical = prof.handle.clone();
                        shared.cache.save_profile(
                            &prof.handle,
                            prof.moniker.as_deref(),
                            prof.citizen_record.as_deref(),
                            now,
                        );
                        if let Some(rec) = prof.citizen_record.as_deref() {
                            let (_, changed) = shared.notes.read().link(&prof.handle, Some(rec));
                            if changed {
                                shared.save_notes();
                            }
                        }
                    }
                    RsiResult::NotFound => {
                        let _ = shared.cache.record_not_found(
                            NotFoundKind::Player,
                            &trimmed,
                            "ui",
                            now,
                        );
                    }
                    RsiResult::Failed(_) => {}
                }

                match shared.rsi_client.fetch_player_orgs(&canonical) {
                    RsiResult::Found(mut p_orgs) => {
                        found_any = true;
                        // Enrich affiliate orgs if already cached or fetch org pages
                        for m in &mut p_orgs.orgs {
                            if let Some(sid) = m.sid.clone() {
                                if let Some(cached_org) = shared.cache.get_org(&sid) {
                                    if m.archetype.is_none() {
                                        m.archetype = cached_org.info.archetype;
                                    }
                                    if m.activities.is_empty() {
                                        m.activities = cached_org.info.activities;
                                    }
                                    if m.name.is_none() {
                                        m.name = Some(cached_org.info.long_name);
                                    }
                                } else if m.archetype.is_none() || m.activities.is_empty() {
                                    if let RsiResult::Found(org_info) =
                                        shared.rsi_client.fetch_org(&sid)
                                    {
                                        let _ = shared.cache.save_org(&org_info, "ui", Utc::now());
                                        if m.archetype.is_none() {
                                            m.archetype = org_info.archetype;
                                        }
                                        if m.activities.is_empty() {
                                            m.activities = org_info.activities;
                                        }
                                        if m.name.is_none() {
                                            m.name = Some(org_info.long_name);
                                        }
                                    }
                                }
                            }
                        }
                        canonical = p_orgs.handle.clone();
                        let _ = shared.cache.save_player(&p_orgs, "ui", Utc::now());
                    }
                    RsiResult::NotFound => {
                        if !found_any {
                            let _ = shared.cache.record_not_found(
                                NotFoundKind::Player,
                                &canonical,
                                "ui",
                                Utc::now(),
                            );
                        }
                    }
                    RsiResult::Failed(_) => {}
                }

                // Canonicalize casing in social-config if listed
                {
                    let mut social = shared.social.write();
                    let mut updated_case = false;
                    for p in &mut social.players {
                        if p.handle.eq_ignore_ascii_case(&canonical) && p.handle != canonical {
                            p.handle = canonical.clone();
                            updated_case = true;
                        }
                    }
                    if updated_case {
                        drop(social);
                        shared.save_social();
                    }
                }

                let _ = shared
                    .coordinator_tx
                    .send(CoordinatorCommand::LookupPlayer(canonical.clone()));

                if announce {
                    if found_any {
                        shared.set_status(
                            format!("Verified citizen \"{}\" on RSI.", canonical),
                            false,
                        );
                    } else {
                        shared.set_status(
                            format!(
                                "Added \"{}\", but citizen was not found on RSI (or RSI is unreachable).",
                                trimmed
                            ),
                            true,
                        );
                    }
                } else {
                    shared.request_repaint();
                }
            })
            .ok();
    }

    /// Background RSI lookup when adding an Organization SID in Settings.
    pub fn spawn_verify_and_cache_org(self: &Arc<Self>, raw_sid: String) {
        let sid = raw_sid.trim().to_ascii_uppercase();
        if !RsiNames::is_valid_sid(&sid) {
            self.set_status(format!("\"{}\" is not a valid Org SID.", raw_sid), true);
            return;
        }
        let shared = Arc::clone(self);
        thread::Builder::new()
            .name("sc-overlay-verify-org".to_string())
            .spawn(move || match shared.rsi_client.fetch_org(&sid) {
                RsiResult::Found(info) => {
                    let count_txt = info
                        .member_count
                        .map(|n| format!(" ({} members)", n))
                        .unwrap_or_default();
                    let msg = format!(
                        "Verified org [{}] — {}{}.",
                        info.sid, info.long_name, count_txt
                    );
                    let _ = shared.cache.save_org(&info, "ui", Utc::now());
                    shared.set_status(msg, false);
                }
                RsiResult::NotFound => {
                    let _ =
                        shared
                            .cache
                            .record_not_found(NotFoundKind::Org, &sid, "ui", Utc::now());
                    shared.set_status(
                        format!("Org [{}] was not found on RSI.", sid),
                        true,
                    );
                }
                RsiResult::Failed(err) => {
                    shared.set_status(
                        format!("Added [{}], RSI check failed: {}", sid, err),
                        true,
                    );
                }
            })
            .ok();
    }

    /// Syncs `social-config.txt` from `settings.hosted_config_url`.
    pub fn spawn_hosted_config_sync(self: &Arc<Self>, manual: bool) {
        if self
            .sync_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let (url, last_hash, mode) = {
            let s = self.settings.read();
            (
                s.hosted_config_url.trim().to_string(),
                s.hosted_config_last_hash.clone(),
                s.import_mode,
            )
        };
        if url.is_empty() {
            self.sync_in_progress.store(false, Ordering::SeqCst);
            if manual {
                self.set_status("Enter an https:// Hosted Config URL first.", true);
            }
            return;
        }

        let shared = Arc::clone(self);
        thread::Builder::new()
            .name("sc-overlay-hosted-sync".to_string())
            .spawn(move || {
                let res = HostedConfigClient::fetch(&url);
                shared.sync_in_progress.store(false, Ordering::SeqCst);

                match res {
                    Err(err) => {
                        if manual {
                            shared.set_status(format!("Hosted sync failed: {}", err), true);
                        } else {
                            tracing::warn!("Auto hosted config sync failed: {}", err);
                        }
                    }
                    Ok((_raw_txt, hash, remote_cfg)) => {
                        let now = Utc::now();
                        {
                            let mut s = shared.settings.write();
                            s.hosted_config_last_check = Some(now);
                        }
                        if !manual && !last_hash.is_empty() && last_hash.eq_ignore_ascii_case(&hash)
                        {
                            shared.save_settings(false);
                            return;
                        }

                        if mode == iris_core::config::ImportMode::BackupExisting {
                            let _ =
                                SocialConfigMerger::backup_existing(&shared.paths.social_config_file());
                        }
                        let merged = {
                            let local = shared.social.read();
                            SocialConfigMerger::apply(&local, &remote_cfg, mode)
                        };
                        *shared.social.write() = merged;
                        shared.save_social();

                        {
                            let mut s = shared.settings.write();
                            s.hosted_config_last_hash = hash;
                            s.hosted_config_last_check = Some(now);
                        }
                        shared.save_settings(false);

                        let label = remote_cfg
                            .name
                            .as_deref()
                            .map(|n| format!(" \"{}\"", n))
                            .unwrap_or_default();
                        shared.set_status(
                            format!(
                                "Synced hosted config{} ({} players, {} orgs).",
                                label,
                                remote_cfg.players.len(),
                                remote_cfg.orgs.len()
                            ),
                            false,
                        );
                    }
                }
            })
            .ok();
    }

    /// Background refresh of listed orgs & cached players on RSI.
    pub fn spawn_cache_refresh(self: &Arc<Self>) {
        if self
            .refresh_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let shared = Arc::clone(self);
        thread::Builder::new()
            .name("sc-overlay-cache-refresh".to_string())
            .spawn(move || {
                let sids: Vec<String> = shared
                    .social
                    .read()
                    .orgs
                    .iter()
                    .map(|o| o.sid.clone())
                    .collect();
                let mut handles: Vec<String> = shared
                    .social
                    .read()
                    .players
                    .iter()
                    .map(|p| p.handle.clone())
                    .collect();
                for h in shared.cache.all_cached_handles().into_iter().take(60) {
                    if !handles.iter().any(|x| x.eq_ignore_ascii_case(&h)) {
                        handles.push(h);
                    }
                }

                let mut refreshed_orgs = 0usize;
                for sid in &sids {
                    if shared.quit_requested.load(Ordering::Relaxed) {
                        break;
                    }
                    if let RsiResult::Found(info) = shared.rsi_client.fetch_org(sid) {
                        let _ = shared.cache.save_org(&info, "refresh", Utc::now());
                        refreshed_orgs += 1;
                    }
                }

                let mut refreshed_players = 0usize;
                for handle in handles.iter().take(50) {
                    if shared.quit_requested.load(Ordering::Relaxed) {
                        break;
                    }
                    if let RsiResult::Found(p_orgs) = shared.rsi_client.fetch_player_orgs(handle) {
                        let _ = shared.cache.save_player(&p_orgs, "refresh", Utc::now());
                        refreshed_players += 1;
                    }
                }

                shared.refresh_in_progress.store(false, Ordering::SeqCst);
                shared.set_status(
                    format!(
                        "Cache refresh complete: {} orgs and {} players updated.",
                        refreshed_orgs, refreshed_players
                    ),
                    false,
                );
            })
            .ok();
    }

    /// Optional non-intrusive GitHub release check (`settings.check_for_updates`).
    pub fn spawn_update_check(self: &Arc<Self>) {
        if !self.settings.read().check_for_updates {
            return;
        }
        let shared = Arc::clone(self);
        thread::Builder::new()
            .name("sc-overlay-update-check".to_string())
            .spawn(move || {
                let Ok(client) = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(8))
                    .user_agent(format!("sc-overlay/{}", env!("CARGO_PKG_VERSION")))
                    .build()
                else {
                    return;
                };
                let url = "https://api.github.com/repos/joesp/sc_overlay_rust/releases/latest";
                let Ok(resp) = client.get(url).send() else {
                    return;
                };
                if !resp.status().is_success() {
                    return;
                }
                let Ok(json) = resp.json::<serde_json::Value>() else {
                    return;
                };
                let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(html_url) = json.get("html_url").and_then(|v| v.as_str()) else {
                    return;
                };
                let clean_tag = tag.trim_start_matches('v').trim();
                if clean_tag != env!("CARGO_PKG_VERSION") && !clean_tag.is_empty() {
                    *shared.update_available.write() =
                        Some((tag.to_string(), html_url.to_string()));
                    shared.request_repaint();
                }
            })
            .ok();
    }
}

