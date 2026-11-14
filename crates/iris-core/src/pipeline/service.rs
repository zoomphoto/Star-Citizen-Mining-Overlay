use crate::config::{
    AppSettings, ColorResolver, DataPaths, DossierNotes, ExclusionList, SessionExclusions,
    SocialConfig,
};
use crate::detection::{
    BgrImage, LayerCheck, NameShape, NameplateDetector, NameplateDetectorSettings, NameplateReader,
    PlateColors,
};
use crate::lookup::{CacheDatabase, NotFoundKind, RsiClient, RsiResult};
use crate::mining::{
    LabelColours, LockGeometry, MiningLabelState, MiningSignalReader, MiningState,
    MiningTableSource, SignalState, TableSnap,
};
use crate::models::{
    BracketItem, MarkerIdentity, MiningLabelItem, OrgMembership, OverlayModel, PanelEntry,
    PanelOrgs, PlateColor, RsiState,
};
use crate::overlay::TrackingBoundary;
use crate::pipeline::tracker::{
    InteractiveRoster, LineSubPixel, MarkerTracker, NameClass, Observation, PlateNameJudge,
    PlateStatus, RosterEntry,
};
use crate::pipeline::watches::{
    AberrationState, AberrationWatch, BlankPictureWatch, LiveSettings, PipelineSettings,
    WhiteDuplicateRule,
};
use chrono::Utc;
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub bgr: BgrImage,
    pub captured_ms: f64,
}

impl CapturedFrame {
    pub fn from_bgr8(width: usize, height: usize, data: Vec<u8>, captured_ms: f64) -> Self {
        Self {
            bgr: BgrImage {
                width,
                height,
                data,
            },
            captured_ms,
        }
    }

    /// Converts a BGRA8 capture buffer (with optional row pitch `stride_bytes`) into a contiguous `BgrImage`.
    pub fn from_bgra8(
        width: usize,
        height: usize,
        stride_bytes: usize,
        bgra: &[u8],
        captured_ms: f64,
    ) -> Self {
        let mut data = vec![0u8; width * height * 3];
        let row_bytes = width * 4;
        let stride = stride_bytes.max(row_bytes);
        for y in 0..height {
            let src_start = y * stride;
            if src_start + row_bytes > bgra.len() {
                break;
            }
            let src_row = &bgra[src_start..src_start + row_bytes];
            let dst_row = &mut data[y * width * 3..(y + 1) * width * 3];
            for x in 0..width {
                let s = x * 4;
                let d = x * 3;
                dst_row[d] = src_row[s];
                dst_row[d + 1] = src_row[s + 1];
                dst_row[d + 2] = src_row[s + 2];
            }
        }
        Self::from_bgr8(width, height, data, captured_ms)
    }

    /// Samples maximum pixel brightness across a coarse grid for `BlankPictureWatch`.
    pub fn sample_brightest(&self) -> f64 {
        if self.bgr.is_empty() {
            return 0.0;
        }
        let step_y = (self.bgr.height / 24).max(1);
        let step_x = (self.bgr.width / 32).max(1);
        let mut max_val = 0u8;
        for y in (0..self.bgr.height).step_by(step_y) {
            let row = &self.bgr.data[y * self.bgr.width * 3..];
            for x in (0..self.bgr.width).step_by(step_x) {
                let idx = x * 3;
                let v = row[idx].max(row[idx + 1]).max(row[idx + 2]);
                if v > max_val {
                    max_val = v;
                    if max_val > LiveSettings::BLANK_FRAME_MAX_VALUE as u8 {
                        return max_val as f64;
                    }
                }
            }
        }
        max_val as f64
    }
}

#[derive(Debug, Clone, Default)]
pub struct PipelineSnapshot {
    pub model: OverlayModel,
    pub roster: Vec<RosterEntry>,
    pub blank_picture: bool,
    pub aberration: Option<AberrationState>,
}

struct LookupWorkerRequest {
    handle: String,
    need_profile: bool,
}

struct LookupWorkerResult {
    handle: String,
    rsi_state: RsiState,
    orgs: Vec<OrgMembership>,
}

struct ActiveMiningLock {
    state: MiningState,
    label_state: MiningLabelState,
    colours: LabelColours,
    geometry: LockGeometry,
}

pub struct LiveDetectionPipeline {
    paths: DataPaths,
    cache: Arc<CacheDatabase>,
    notes: Arc<RwLock<DossierNotes>>,
    exclusions: ExclusionList,
    session_exclusions: SessionExclusions,
    detector: NameplateDetector,
    tracker: MarkerTracker,
    white_dupes: WhiteDuplicateRule,
    aberration_watch: AberrationWatch,
    aberration_state: AberrationState,
    aberration_note_until_ms: f64,
    blank_watch: BlankPictureWatch,
    mining_reader: MiningSignalReader,
    mining_table_source: MiningTableSource,
    mining_lock: Option<ActiveMiningLock>,
    last_mining_ms: f64,
    pipeline_settings: PipelineSettings,
    live_settings: LiveSettings,
    tick_counter: i64,
    in_flight_lookups: HashMap<String, f64>,
    lookup_tx: Sender<LookupWorkerRequest>,
    lookup_rx: Receiver<LookupWorkerResult>,
}

impl LiveDetectionPipeline {
    pub fn new(
        paths: DataPaths,
        cache: Arc<CacheDatabase>,
        notes: Arc<RwLock<DossierNotes>>,
        rsi_client: Arc<RsiClient>,
    ) -> Self {
        let exclusions = ExclusionList::load_or_embedded(&paths.exclusions_file());
        let mining_table_source = MiningTableSource::new(paths.mining_file());
        let pipeline_settings = PipelineSettings::default();
        let white_dupes = WhiteDuplicateRule::new(pipeline_settings.duplicate_window_ticks);

        let (req_tx, req_rx) = bounded::<LookupWorkerRequest>(128);
        let (res_tx, res_rx) = bounded::<LookupWorkerResult>(256);

        {
            let worker_cache = Arc::clone(&cache);
            let worker_notes = Arc::clone(&notes);
            let worker_paths = paths.clone();
            let worker_client = Arc::clone(&rsi_client);

            thread::Builder::new()
                .name("sc-overlay-rsi-lookup".to_string())
                .spawn(move || {
                    Self::run_lookup_worker(
                        req_rx,
                        res_tx,
                        worker_cache,
                        worker_notes,
                        worker_paths,
                        worker_client,
                    );
                })
                .expect("failed to spawn RSI lookup worker thread");
        }

        Self {
            paths,
            cache,
            notes,
            exclusions,
            session_exclusions: SessionExclusions::new(),
            detector: NameplateDetector::new(
                NameplateReader::default(),
                NameplateDetectorSettings::default(),
            ),
            tracker: MarkerTracker::new(),
            white_dupes,
            aberration_watch: AberrationWatch::default(),
            aberration_state: AberrationState::Unknown,
            aberration_note_until_ms: f64::NEG_INFINITY,
            blank_watch: BlankPictureWatch::default(),
            mining_reader: MiningSignalReader::default(),
            mining_table_source,
            mining_lock: None,
            last_mining_ms: f64::NEG_INFINITY,
            pipeline_settings,
            live_settings: LiveSettings::default(),
            tick_counter: 0,
            in_flight_lookups: HashMap::new(),
            lookup_tx: req_tx,
            lookup_rx: res_rx,
        }
    }

    pub fn reload_exclusions(&mut self) {
        self.exclusions = ExclusionList::load_or_embedded(&self.paths.exclusions_file());
    }

    pub fn reset_tracking(&mut self) {
        self.tracker.markers.clear();
        self.white_dupes.clear();
        self.mining_lock = None;
        self.blank_watch.reset();
    }

    /// Requests a profile/orgs lookup for an arbitrary handle (e.g. from Interactive Overlay search).
    pub fn request_lookup(&mut self, handle: &str, need_profile: bool, now_ms: f64) {
        let key = handle.to_ascii_lowercase();
        if let Some(&sent_at) = self.in_flight_lookups.get(&key) {
            if now_ms - sent_at < self.live_settings.lookup_retry_ms {
                return;
            }
        }
        if self
            .lookup_tx
            .try_send(LookupWorkerRequest {
                handle: handle.to_string(),
                need_profile,
            })
            .is_ok()
        {
            self.in_flight_lookups.insert(key, now_ms);
        }
    }

    pub fn process_frame(
        &mut self,
        frame: &CapturedFrame,
        settings: &AppSettings,
        social: &SocialConfig,
    ) -> PipelineSnapshot {
        self.tick_counter += 1;
        let now_ms = frame.captured_ms;
        let win_w = frame.bgr.width as i32;
        let win_h = frame.bgr.height as i32;

        // 1. Drain completed async RSI lookups
        self.drain_lookup_results(social);

        // 2. Blank picture watch
        if self.blank_watch.due(now_ms) {
            let brightest = frame.sample_brightest();
            self.blank_watch.update(now_ms, brightest);
        }

        if self.blank_watch.active {
            let mut model = OverlayModel {
                window_width: win_w,
                window_height: win_h,
                note: Some(BlankPictureWatch::NOTE_TEXT.to_string()),
                ..Default::default()
            };
            model.panel_entries = self.build_panel_entries(now_ms, social);
            return PipelineSnapshot {
                model,
                roster: InteractiveRoster::build(&self.tracker, now_ms),
                blank_picture: true,
                aberration: Some(self.aberration_state),
            };
        }

        // 3. Configure detector scan region from TrackingBoundary
        let boundary = TrackingBoundary::from_settings(settings);
        self.detector.settings_mut().region_left = boundary.left;
        self.detector.settings_mut().region_top = boundary.top;
        self.detector.settings_mut().region_right = boundary.right;
        self.detector.settings_mut().region_bottom = boundary.bottom;

        // 4. Detect & read candidate nameplates
        let observations = self.scan_and_read_plates(&frame.bgr, now_ms, social);

        // 5. Update Hungarian + sub-pixel velocity tracker
        let forgotten = self.tracker.update(
            &observations,
            now_ms,
            settings.tracking_loss_display_seconds as f64,
            win_w,
            win_h,
            boundary,
            &self.pipeline_settings,
        );
        for id in forgotten {
            self.white_dupes.forget(id);
        }

        // 6. Attach RSI identities & queue lookups for confirmed handles
        self.resolve_tracked_identities(now_ms, social);

        // 7. Build bracket items with WhiteDuplicateRule + Green party duplicate rule
        let brackets = self.build_brackets(now_ms, social);

        // 8. Build right-side panel entries (live + stale within panel_stale_ms)
        let panel_entries = self.build_panel_entries(now_ms, social);

        // 9. Mining HUD signal scanner
        let mining_labels = if !settings.mining_disabled {
            self.update_mining(&frame.bgr, now_ms, win_w, win_h)
        } else {
            self.mining_lock = None;
            Vec::new()
        };

        let note = if now_ms < self.aberration_note_until_ms {
            Some(AberrationWatch::NOTE_TEXT.to_string())
        } else {
            None
        };

        let model = OverlayModel {
            window_width: win_w,
            window_height: win_h,
            brackets,
            panel_entries,
            mining_labels,
            toast: None,
            note,
            boundary_rect: None,
            dossier_placeholder: None,
        };

        let roster = InteractiveRoster::build(&self.tracker, now_ms);

        PipelineSnapshot {
            model,
            roster,
            blank_picture: false,
            aberration: Some(self.aberration_state),
        }
    }

    fn drain_lookup_results(&mut self, social: &SocialConfig) {
        while let Ok(res) = self.lookup_rx.try_recv() {
            let key = res.handle.to_ascii_lowercase();
            self.in_flight_lookups.remove(&key);
            if res.rsi_state == RsiState::NotFound {
                self.session_exclusions.add(&res.handle);
            }
            for marker in &mut self.tracker.markers {
                if let Some(h) = marker.confirmed_handle.as_deref() {
                    if h.eq_ignore_ascii_case(&res.handle) {
                        let cm = match res.rsi_state {
                            RsiState::Verified => {
                                ColorResolver::resolve(social, &res.handle, &res.orgs)
                            }
                            _ => ColorResolver::resolve_unknown(social, &res.handle),
                        };
                        marker.identity = Some(MarkerIdentity {
                            handle: res.handle.clone(),
                            rsi: res.rsi_state,
                            orgs: res.orgs.clone(),
                            color_match: cm,
                        });
                    }
                }
            }
        }
    }

    fn scan_and_read_plates(
        &mut self,
        bgr: &BgrImage,
        now_ms: f64,
        social: &SocialConfig,
    ) -> Vec<Observation> {
        let candidates = self.detector.find_candidates(bgr);
        if candidates.boxes.is_empty() {
            return Vec::new();
        }

        let mut pair_list: Vec<(usize, usize)> = Vec::new();
        pair_list.extend_from_slice(&candidates.pairs);
        pair_list.extend_from_slice(&candidates.loose_pairs);
        for cp in &candidates.crowd_pairs {
            pair_list.push((cp.top, cp.bottom));
        }

        let rule = AberrationWatch::rule_for(self.aberration_state);
        let check_active = self.aberration_state != AberrationState::Off;
        let mut verdicts = Vec::new();
        let mut observations = Vec::new();
        let mut reads_done = 0usize;
        let mut claimed_tops: HashSet<usize> = HashSet::new();

        for (top_idx, bot_idx) in pair_list {
            if reads_done >= self.pipeline_settings.max_reads_per_tick {
                break;
            }
            if claimed_tops.contains(&top_idx) {
                continue;
            }
            let top_rect = candidates.boxes[top_idx];
            let bot_rect = candidates.boxes[bot_idx];

            // Verify distance row on bottom
            let dist_reading = {
                let mut push_verdict = |shifted: bool| verdicts.push(shifted);
                let mut dist_check = LayerCheck {
                    active: check_active,
                    rule,
                    on_note: if check_active {
                        Some(&mut push_verdict)
                    } else {
                        None
                    },
                };
                self.detector
                    .read_distance_line(bgr, bot_rect, Some(&mut dist_check))
            };
            if dist_reading.is_none() {
                continue;
            }
            reads_done += 1;

            let name_rows = candidates
                .stack_of(top_rect, bot_rect)
                .map(|s| s.rows.clone())
                .unwrap_or_else(|| vec![top_rect]);

            let mut combined_text = String::new();
            let mut combined_o0 = Vec::new();
            let mut row_masks = Vec::new();
            let mut valid = true;
            let mut union_rect = name_rows[0];

            for r in &name_rows {
                union_rect = union_rect.union(r);
                let row_read = {
                    let mut push_verdict = |shifted: bool| verdicts.push(shifted);
                    let mut name_check = LayerCheck {
                        active: check_active,
                        rule,
                        on_note: if check_active {
                            Some(&mut push_verdict)
                        } else {
                            None
                        },
                    };
                    self.detector
                        .read_name_row(bgr, *r, Some(&mut name_check))
                };
                if !row_read.usable() {
                    valid = false;
                    break;
                }
                let offset = combined_text.chars().count();
                for mut g in row_read.o0 {
                    g.index += offset;
                    combined_o0.push(g);
                }
                combined_text.push_str(&row_read.text);
                if let Some(m) = row_read.mask {
                    row_masks.push(m);
                }
            }

            if !valid || !NameShape::has_valid_edges(&combined_text) {
                continue;
            }

            let cached_orgs = self.cache.get_player(&combined_text).map(|p| p.orgs);
            let is_listed =
                PlateNameJudge::is_listed(&combined_text, cached_orgs.as_deref(), social);
            if PlateNameJudge::classify(
                &combined_text,
                social,
                &self.exclusions,
                &self.session_exclusions,
                is_listed,
            ) != NameClass::Valid
            {
                continue;
            }

            let plate_color = PlateColors::classify(
                row_masks.iter(),
                &self.detector.settings().plate_color_rules,
            )
            .map(|r| r.color)
            .or_else(|| self.detector.ink_colour(bgr, top_rect));

            let fix = LineSubPixel::measure(bgr, union_rect);
            let refined_box = if fix.refined {
                crate::models::PixelRect::new(
                    fix.centre_x - union_rect.width as f64 / 2.0,
                    fix.top,
                    union_rect.width as f64,
                    union_rect.height as f64,
                )
            } else {
                union_rect.to_pixel_rect()
            };

            claimed_tops.insert(top_idx);
            observations.push(Observation {
                name_box: refined_box,
                distance_box: Some(bot_rect.to_pixel_rect()),
                raw_text: Some(combined_text),
                o0: combined_o0,
                plate_color,
            });
        }

        if !verdicts.is_empty() {
            if let Some(next_state) = self.aberration_watch.add(self.aberration_state, &verdicts) {
                self.aberration_state = next_state;
                if next_state == AberrationState::On {
                    self.aberration_note_until_ms = now_ms + LiveSettings::ABERRATION_NOTE_MS;
                } else {
                    self.aberration_note_until_ms = f64::NEG_INFINITY;
                }
            }
        }

        observations
    }

    fn resolve_tracked_identities(&mut self, now_ms: f64, social: &SocialConfig) {
        let mut to_request: Vec<(String, bool)> = Vec::new();
        let now_utc = Utc::now();

        for marker in &mut self.tracker.markers {
            if marker.status != PlateStatus::Confirmed {
                continue;
            }
            let Some(handle) = marker.confirmed_handle.clone() else {
                continue;
            };

            if let Some(cached) = self.cache.get_player(&handle) {
                self.cache.note_seen_on_plate(&cached.handle, now_utc);
                let cm = ColorResolver::resolve(social, &cached.handle, &cached.orgs);
                marker.confirmed_handle = Some(cached.handle.clone());
                marker.identity = Some(MarkerIdentity {
                    handle: cached.handle.clone(),
                    rsi: RsiState::Verified,
                    orgs: cached.orgs,
                    color_match: cm,
                });

                let needs_profile = self
                    .cache
                    .get_player_extra(&cached.handle)
                    .and_then(|e| e.profile_refreshed)
                    .is_none();
                if needs_profile {
                    to_request.push((cached.handle, true));
                }
                continue;
            }

            if let Some(nf) = self.cache.get_not_found(NotFoundKind::Player, &handle) {
                if nf.is_fresh(now_utc) {
                    self.session_exclusions.add(&handle);
                    let cm = ColorResolver::resolve_unknown(social, &handle);
                    marker.identity = Some(MarkerIdentity {
                        handle: handle.clone(),
                        rsi: RsiState::NotFound,
                        orgs: Vec::new(),
                        color_match: cm,
                    });
                    continue;
                }
            }

            let cm = ColorResolver::resolve_unknown(social, &handle);
            marker.identity = Some(MarkerIdentity {
                handle: handle.clone(),
                rsi: RsiState::Unknown,
                orgs: Vec::new(),
                color_match: cm,
            });
            to_request.push((handle, true));
        }

        for (handle, need_profile) in to_request {
            self.request_lookup(&handle, need_profile, now_ms);
        }
    }

    fn build_brackets(&mut self, now_ms: f64, social: &SocialConfig) -> Vec<BracketItem> {
        let mut handle_counts: HashMap<String, (usize, usize)> = HashMap::new();
        for m in &self.tracker.markers {
            if m.status != PlateStatus::Confirmed {
                continue;
            }
            if let Some(id) = &m.identity {
                if id.rsi == RsiState::NotFound {
                    continue;
                }
                let entry = handle_counts
                    .entry(id.handle.to_ascii_lowercase())
                    .or_insert((0, 0));
                entry.0 += 1;
                if m.plate_color == Some(PlateColor::Green) {
                    entry.1 += 1;
                }
            }
        }

        let mut brackets = Vec::new();
        for m in &self.tracker.markers {
            if m.status != PlateStatus::Confirmed {
                continue;
            }
            let Some(id) = &m.identity else {
                continue;
            };
            if id.rsi == RsiState::NotFound {
                continue;
            }

            let key = id.handle.to_ascii_lowercase();
            if let Some(&(total, green_count)) = handle_counts.get(&key) {
                if total > 1 && (green_count != 1 || m.plate_color != Some(PlateColor::Green)) {
                    continue;
                }
            }

            let is_white = m.plate_color.unwrap_or(PlateColor::White) == PlateColor::White;
            let (pred_box, _) = m.predict_box(now_ms, &self.pipeline_settings);
            if self
                .white_dupes
                .note(m.id, &id.handle, self.tick_counter, is_white, pred_box)
                .is_some()
            {
                continue;
            }

            let cm = match id.rsi {
                RsiState::Verified => ColorResolver::resolve(social, &id.handle, &id.orgs),
                _ => ColorResolver::resolve_unknown(social, &id.handle),
            };

            brackets.push(BracketItem {
                marker_id: m.id,
                handle: id.handle.clone(),
                name_box: pred_box,
                color: cm.color,
                kind: cm.kind,
            });
        }
        brackets
    }

    fn build_panel_entries(&self, now_ms: f64, social: &SocialConfig) -> Vec<PanelEntry> {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        let notes_guard = self.notes.read();

        let make_entry = |id: &MarkerIdentity, live: bool| -> Option<PanelEntry> {
            if id.rsi == RsiState::NotFound {
                return None;
            }
            let cm = match id.rsi {
                RsiState::Verified => ColorResolver::resolve(social, &id.handle, &id.orgs),
                _ => ColorResolver::resolve_unknown(social, &id.handle),
            };
            let orgs = PanelOrgs {
                looked_up: id.rsi == RsiState::Verified,
                orgs: id.orgs.clone(),
                deciding_sid: cm.deciding_org_sid.clone(),
                match_kind: Some(cm.kind),
                attribute_match: cm.deciding_attribute.clone(),
            };
            let note = notes_guard.panel_text(&id.handle);
            Some(PanelEntry {
                handle: id.handle.clone(),
                color: cm.color,
                kind: cm.kind,
                orgs,
                note,
                live,
            })
        };

        for m in &self.tracker.markers {
            if m.status != PlateStatus::Confirmed {
                continue;
            }
            if let Some(id) = &m.identity {
                let key = id.handle.to_ascii_lowercase();
                if seen.insert(key) {
                    if let Some(entry) = make_entry(id, true) {
                        entries.push(entry);
                    }
                }
            }
        }

        for ended in self.tracker.recently_ended.iter().rev() {
            if now_ms - ended.ended_ms > self.pipeline_settings.panel_stale_ms {
                continue;
            }
            let key = ended.identity.handle.to_ascii_lowercase();
            if seen.insert(key) {
                if let Some(entry) = make_entry(&ended.identity, false) {
                    entries.push(entry);
                }
            }
        }

        entries.sort_by_key(|e| (if e.live { 0 } else { 1 }, e.kind));
        entries
    }

    fn update_mining(
        &mut self,
        bgr: &BgrImage,
        now_ms: f64,
        win_w: i32,
        win_h: i32,
    ) -> Vec<MiningLabelItem> {
        let min_period_ms = 1000.0 / self.mining_reader.settings().lock_hz.max(1.0);
        if now_ms - self.last_mining_ms >= min_period_ms {
            self.last_mining_ms = now_ms;
            let table = self.mining_table_source.current(now_ms);
            let ms_cfg = self.mining_reader.settings().clone();

            if let Some(mut active) = self.mining_lock.take() {
                let lock_rect = MiningSignalReader::lock_area(
                    win_w,
                    win_h,
                    active.state.row_top,
                    &ms_cfg,
                )
                .to_int_rect()
                .clamp_to(win_w, win_h);

                if !lock_rect.is_empty() {
                    let crop = bgr.crop(lock_rect);
                    let raw_read = self.mining_reader.read(&crop, lock_rect, win_w, win_h);
                    let snapped = TableSnap::apply(raw_read, &table, &ms_cfg);
                    if snapped.found() {
                        active
                            .colours
                            .add(snapped.text_colour, snapped.back_colour, now_ms);
                        if let Some((med_box, med_row, med_h)) = active.geometry.add(
                            snapped.box_rect,
                            snapped.row_top,
                            snapped.digit_height,
                            &active.state,
                        ) {
                            active.state.signal_box = med_box;
                            active.state.row_top = med_row;
                            active.state.digit_height = med_h;
                        }
                        active.state.text_colour = active.colours.text;
                        active.state.back_colour = active.colours.back;
                    }
                    active.label_state.apply(&snapped, now_ms, &table);
                    if !active.label_state.ended {
                        self.mining_lock = Some(active);
                    }
                }
            } else {
                let search_rect = MiningSignalReader::search_area(win_w, win_h, &ms_cfg)
                    .to_int_rect()
                    .clamp_to(win_w, win_h);
                if !search_rect.is_empty() {
                    let crop = bgr.crop(search_rect);
                    let raw_read = self.mining_reader.search(&crop, search_rect, win_w, win_h);
                    let snapped = TableSnap::apply(raw_read, &table, &ms_cfg);
                    if snapped.state == SignalState::Signal && snapped.value.is_some() {
                        let state = MiningState::from_lock(&snapped, now_ms, &table);
                        let label_state = MiningLabelState::new(
                            ms_cfg.clone(),
                            now_ms,
                            state.value,
                            state.label.clone(),
                        );
                        let colours = LabelColours::new(
                            ms_cfg.clone(),
                            state.text_colour,
                            state.back_colour,
                        );
                        let geometry = LockGeometry::new(ms_cfg);
                        self.mining_lock = Some(ActiveMiningLock {
                            state,
                            label_state,
                            colours,
                            geometry,
                        });
                    }
                }
            }
        }

        if let Some(active) = &self.mining_lock {
            if let Some(lbl) = &active.label_state.label {
                return vec![MiningLabelItem {
                    text: lbl.clone(),
                    signal_box: active.state.signal_box,
                    digit_height: active.state.digit_height,
                    text_colour: active.state.text_colour,
                    back_colour: active.state.back_colour,
                }];
            }
        }
        Vec::new()
    }

    fn run_lookup_worker(
        rx: Receiver<LookupWorkerRequest>,
        tx: Sender<LookupWorkerResult>,
        cache: Arc<CacheDatabase>,
        notes: Arc<RwLock<DossierNotes>>,
        paths: DataPaths,
        client: Arc<RsiClient>,
    ) {
        while let Ok(req) = rx.recv() {
            let now = Utc::now();

            if let Some(cached) = cache.get_player(&req.handle) {
                if req.need_profile {
                    let has_profile = cache
                        .get_player_extra(&cached.handle)
                        .and_then(|e| e.profile_refreshed)
                        .is_some();
                    if !has_profile {
                        Self::fetch_and_link_profile(
                            &cached.handle,
                            &cache,
                            &notes,
                            &paths,
                            &client,
                        );
                    }
                }
                let _ = tx.send(LookupWorkerResult {
                    handle: cached.handle,
                    rsi_state: RsiState::Verified,
                    orgs: cached.orgs,
                });
                continue;
            }

            match client.fetch_player_orgs(&req.handle) {
                RsiResult::Found(mut player_orgs) => {
                    for org in &mut player_orgs.orgs {
                        if let Some(sid) = org.sid.clone() {
                            if org.archetype.is_none() || org.activities.is_empty() {
                                if let Some(cached_org) = cache.get_org(&sid) {
                                    org.archetype = cached_org.info.archetype;
                                    org.activities = cached_org.info.activities;
                                    if org.name.is_none() {
                                        org.name = Some(cached_org.info.long_name);
                                    }
                                } else if let RsiResult::Found(org_info) = client.fetch_org(&sid) {
                                    let _ = cache.save_org(&org_info, "live", Utc::now());
                                    org.archetype = org_info.archetype;
                                    org.activities = org_info.activities;
                                    if org.name.is_none() {
                                        org.name = Some(org_info.long_name);
                                    }
                                }
                            }
                        }
                    }

                    let _ = cache.save_player(&player_orgs, "live", now);
                    cache.note_seen_on_plate(&player_orgs.handle, now);

                    if req.need_profile {
                        Self::fetch_and_link_profile(
                            &player_orgs.handle,
                            &cache,
                            &notes,
                            &paths,
                            &client,
                        );
                    }

                    let _ = tx.send(LookupWorkerResult {
                        handle: player_orgs.handle,
                        rsi_state: RsiState::Verified,
                        orgs: player_orgs.orgs,
                    });
                }
                RsiResult::NotFound => {
                    let _ = cache.record_not_found(NotFoundKind::Player, &req.handle, "live", now);
                    let _ = tx.send(LookupWorkerResult {
                        handle: req.handle,
                        rsi_state: RsiState::NotFound,
                        orgs: Vec::new(),
                    });
                }
                RsiResult::Failed(_) => {
                    let _ = tx.send(LookupWorkerResult {
                        handle: req.handle,
                        rsi_state: RsiState::Offline,
                        orgs: Vec::new(),
                    });
                }
            }
        }
    }

    fn fetch_and_link_profile(
        handle: &str,
        cache: &CacheDatabase,
        notes: &RwLock<DossierNotes>,
        paths: &DataPaths,
        client: &RsiClient,
    ) {
        if let RsiResult::Found(profile) = client.fetch_player_profile(handle) {
            cache.save_profile(
                &profile.handle,
                profile.moniker.as_deref(),
                profile.citizen_record.as_deref(),
                Utc::now(),
            );
            let guard = notes.read();
            let (_, changed) = guard.link(&profile.handle, profile.citizen_record.as_deref());
            if changed {
                let _ = guard.save_to(&paths.dossier_notes_file());
            }
        }
    }
}
