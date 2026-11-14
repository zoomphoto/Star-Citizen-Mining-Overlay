use crate::config::{ColorResolver, ExclusionList, SessionExclusions, SocialConfig};
use crate::detection::{
    BgrImage, ContactCode, LookAlikes, NameFilter, O0EvidenceTable, O0Glyph,
};
use crate::lookup::RsiNames;
use crate::models::{
    IntRect, MarkerIdentity, MatchKind, OrgMembership, PixelRect, PlateColor, RsiState,
};
use crate::overlay::TrackingBoundary;
use crate::pipeline::hungarian::Hungarian;
use crate::pipeline::watches::PipelineSettings;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineFix {
    pub centre_x: f64,
    pub top: f64,
    pub refined: bool,
}

pub struct LineSubPixel;

impl LineSubPixel {
    const PAD_X: i32 = 2;
    const PAD_Y: i32 = 1;

    pub fn measure(bgr: &BgrImage, box_rect: IntRect) -> LineFix {
        let fallback = LineFix {
            centre_x: box_rect.x as f64 + box_rect.width as f64 / 2.0,
            top: box_rect.y as f64,
            refined: false,
        };
        if bgr.is_empty() || box_rect.width < 2 || box_rect.height < 2 {
            return fallback;
        }
        let img_w = bgr.width as i32;
        let img_h = bgr.height as i32;
        if box_rect.x < 0
            || box_rect.y < 0
            || box_rect.right() > img_w
            || box_rect.bottom() > img_h
        {
            return fallback;
        }

        let x0 = (box_rect.x - Self::PAD_X).max(0);
        let y0 = (box_rect.y - Self::PAD_Y).max(0);
        let x1 = (box_rect.right() + Self::PAD_X).min(img_w);
        let y1 = (box_rect.bottom() + Self::PAD_Y).min(img_h);
        let width = (x1 - x0) as usize;
        let height = (y1 - y0) as usize;
        let box_x = (box_rect.x - x0) as usize;
        let box_y = (box_rect.y - y0) as usize;
        let box_w = box_rect.width as usize;
        let box_h = box_rect.height as usize;

        if box_x + box_w > width || box_y + box_h > height {
            return fallback;
        }

        let mut columns = vec![0i32; width];
        let mut rows = vec![0i32; height];

        for (x, col_max) in columns.iter_mut().enumerate() {
            let mut m = 0i32;
            let gx = x0 as usize + x;
            for y in box_y..(box_y + box_h) {
                let gy = y0 as usize + y;
                let idx = (gy * bgr.width + gx) * 3;
                let b = bgr.data[idx] as i32;
                let g = bgr.data[idx + 1] as i32;
                let r = bgr.data[idx + 2] as i32;
                m = m.max(b.max(g.max(r)));
            }
            *col_max = m;
        }

        for (y, row_max) in rows.iter_mut().enumerate() {
            let mut m = 0i32;
            let gy = y0 as usize + y;
            for x in box_x..(box_x + box_w) {
                let gx = x0 as usize + x;
                let idx = (gy * bgr.width + gx) * 3;
                let b = bgr.data[idx] as i32;
                let g = bgr.data[idx + 1] as i32;
                let r = bgr.data[idx + 2] as i32;
                m = m.max(b.max(g.max(r)));
            }
            *row_max = m;
        }

        let (Some(left), Some(right_gap), Some(top)) = (
            Self::leading(&columns, false),
            Self::leading(&columns, true),
            Self::leading(&rows, false),
        ) else {
            return fallback;
        };

        let right = width as f64 - right_gap;
        let local_centre = (left + right) / 2.0;
        if right - left < PipelineSettings::SUBPIXEL_MIN_WIDTH_FRACTION * box_w as f64 {
            return fallback;
        }
        let local_fallback_cx = box_x as f64 + box_w as f64 / 2.0;
        let max_cx_shift = PipelineSettings::SUBPIXEL_MAX_SHIFT_PX
            .max(PipelineSettings::SUBPIXEL_MAX_SHIFT_LINES * box_h as f64);
        if (local_centre - local_fallback_cx).abs() > max_cx_shift
            || (top - box_y as f64).abs() > PipelineSettings::SUBPIXEL_MAX_SHIFT_PX
        {
            return fallback;
        }

        LineFix {
            centre_x: local_centre + x0 as f64,
            top: top + y0 as f64,
            refined: true,
        }
    }

    fn leading(profile: &[i32], reverse: bool) -> Option<f64> {
        let mut min = i32::MAX;
        let mut max = i32::MIN;
        for &v in profile {
            min = min.min(v);
            max = max.max(v);
        }
        let range = (max - min) as f64;
        if range < PipelineSettings::SUBPIXEL_MIN_CONTRAST {
            return None;
        }
        let mut missing = 0.0;
        for n in 0..profile.len() {
            let value = if reverse {
                profile[profile.len() - 1 - n]
            } else {
                profile[n]
            };
            let coverage = ((value - min) as f64 / range).clamp(0.0, 1.0);
            if coverage >= PipelineSettings::SUBPIXEL_FULL_COVERAGE {
                return if n > 0 { Some(missing) } else { None };
            }
            missing += 1.0 - coverage;
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameClass {
    Valid,
    Contact,
    Excluded,
}

pub struct PlateNameJudge;

impl PlateNameJudge {
    pub fn classify(
        name: &str,
        config: &SocialConfig,
        exclusions: &ExclusionList,
        session: &SessionExclusions,
        is_listed: bool,
    ) -> NameClass {
        if !RsiNames::is_valid_handle(name) {
            return NameClass::Excluded;
        }
        if !NameFilter::is_on_player_list(name, config) {
            if ContactCode::is_contact_code(name) {
                return NameClass::Contact;
            }
            if exclusions.contains(name) {
                return NameClass::Excluded;
            }
        }
        if session.contains(name) && !is_listed {
            return NameClass::Excluded;
        }
        NameClass::Valid
    }

    pub fn is_listed(
        handle: &str,
        orgs: Option<&[OrgMembership]>,
        config: &SocialConfig,
    ) -> bool {
        if NameFilter::is_on_player_list(handle, config) {
            return true;
        }
        if let Some(orgs) = orgs {
            return matches!(
                ColorResolver::resolve(config, handle, orgs).kind,
                MatchKind::PlayerList | MatchKind::OrgList
            );
        }
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateStatus {
    Unconfirmed,
    Confirmed,
}

#[derive(Debug, Clone, Copy)]
pub struct TrackSample {
    pub ms: f64,
    pub cx: f64,
    pub top: f64,
}

#[derive(Debug, Clone)]
pub struct Observation {
    pub name_box: PixelRect,
    pub distance_box: Option<PixelRect>,
    pub raw_text: Option<String>,
    pub o0: Vec<O0Glyph>,
    pub plate_color: Option<PlateColor>,
}

#[derive(Debug, Clone)]
pub struct TrackedMarker {
    pub id: u64,
    pub born_ms: f64,
    pub status: PlateStatus,
    pub name_box: PixelRect,
    pub distance_box: Option<PixelRect>,
    pub last_found_ms: f64,
    pub line_ms: f64,
    pub samples: VecDeque<TrackSample>,
    pub missed_frames: usize,
    pub prediction_error_px: f64,
    pub read_history: VecDeque<(f64, String)>,
    pub o0_evidence: O0EvidenceTable,
    pub confirmed_handle: Option<String>,
    pub plate_color: Option<PlateColor>,
    pub identity: Option<MarkerIdentity>,
}

impl TrackedMarker {
    pub const MAX_SAMPLES: usize = 6;

    pub fn new(id: u64, obs: &Observation, now_ms: f64) -> Self {
        let mut samples = VecDeque::with_capacity(Self::MAX_SAMPLES);
        samples.push_back(TrackSample {
            ms: now_ms,
            cx: obs.name_box.center_x(),
            top: obs.name_box.y,
        });
        let mut m = Self {
            id,
            born_ms: now_ms,
            status: PlateStatus::Unconfirmed,
            name_box: obs.name_box,
            distance_box: obs.distance_box,
            last_found_ms: now_ms,
            line_ms: now_ms,
            samples,
            missed_frames: 0,
            prediction_error_px: 0.0,
            read_history: VecDeque::new(),
            o0_evidence: O0EvidenceTable::default(),
            confirmed_handle: None,
            plate_color: obs.plate_color,
            identity: None,
        };
        m.record_observation(obs, now_ms, &PipelineSettings::default());
        m
    }

    pub fn velocity(&self, window_ms: f64, max_px_per_ms: f64) -> (f64, f64) {
        if self.samples.len() < 2 {
            return (0.0, 0.0);
        }
        let newest = self
            .samples
            .back()
            .map(|s| s.ms)
            .unwrap_or(f64::NEG_INFINITY);

        let mut n = 0.0;
        let mut st = 0.0;
        let mut sx = 0.0;
        let mut sy = 0.0;
        for s in &self.samples {
            if window_ms > 0.0 && s.ms < newest - window_ms {
                continue;
            }
            n += 1.0;
            st += s.ms;
            sx += s.cx;
            sy += s.top;
        }

        let (vx, vy) = if n >= 2.0 {
            let mt = st / n;
            let mx = sx / n;
            let my = sy / n;
            let mut stt = 0.0;
            let mut stx = 0.0;
            let mut sty = 0.0;
            for s in &self.samples {
                if window_ms > 0.0 && s.ms < newest - window_ms {
                    continue;
                }
                let dt = s.ms - mt;
                stt += dt * dt;
                stx += dt * (s.cx - mx);
                sty += dt * (s.top - my);
            }
            if stt > 0.0 {
                (stx / stt, sty / stt)
            } else {
                (0.0, 0.0)
            }
        } else {
            let a = &self.samples[self.samples.len() - 2];
            let b = &self.samples[self.samples.len() - 1];
            let dt = b.ms - a.ms;
            if dt > 0.0 {
                ((b.cx - a.cx) / dt, (b.top - a.top) / dt)
            } else {
                (0.0, 0.0)
            }
        };

        let speed = (vx * vx + vy * vy).sqrt();
        if speed > max_px_per_ms && speed > 0.0 {
            (vx * max_px_per_ms / speed, vy * max_px_per_ms / speed)
        } else {
            (vx, vy)
        }
    }

    pub fn predict_box(&self, now_ms: f64, settings: &PipelineSettings) -> (PixelRect, f64) {
        let dt_ms = (now_ms - self.line_ms).max(0.0);
        let max_speed = settings.max_speed_lines_per_ms * self.name_box.height.max(10.0);
        let (vx, vy) = self.velocity(settings.velocity_window_ms, max_speed);
        let look_ms = dt_ms.min(settings.max_look_ahead_ms);
        let dx = vx * look_ms;
        let dy = vy * look_ms;
        let disp = (dx * dx + dy * dy).sqrt();
        let dt_sec = dt_ms / 1000.0;
        let is_lost = self.status == PlateStatus::Confirmed && dt_ms > settings.lost_after_ms;
        let grow = if is_lost {
            settings
                .search_grow_lines_per_sec
                .max(settings.lost_search_grow_lines_per_sec)
        } else {
            settings.search_grow_lines_per_sec
        };
        let max_lines = if is_lost {
            settings.lost_search_max_lines
        } else {
            settings.search_max_lines
        };
        let h = self.name_box.height.max(10.0);
        let margin = (h * (settings.search_base_lines + grow * dt_sec)
            + settings.prediction_error_gain * self.prediction_error_px)
            .min(h * max_lines)
            + settings.velocity_gain * disp;

        let predicted = PixelRect::new(
            self.name_box.x + dx,
            self.name_box.y + dy,
            self.name_box.width,
            self.name_box.height,
        );
        (predicted, margin.max(12.0))
    }

    pub fn record_observation(
        &mut self,
        obs: &Observation,
        now_ms: f64,
        settings: &PipelineSettings,
    ) {
        let (pred, _) = self.predict_box(now_ms, settings);
        let err = ((pred.center_x() - obs.name_box.center_x()).powi(2)
            + (pred.y - obs.name_box.y).powi(2))
        .sqrt();
        self.prediction_error_px = settings.prediction_error_smoothing * err
            + (1.0 - settings.prediction_error_smoothing) * self.prediction_error_px;

        self.name_box = obs.name_box;
        if obs.distance_box.is_some() {
            self.distance_box = obs.distance_box;
        }
        if let Some(color) = obs.plate_color {
            self.plate_color = Some(color);
        }
        self.line_ms = now_ms;
        self.last_found_ms = now_ms;
        self.missed_frames = 0;

        if self.samples.len() >= Self::MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(TrackSample {
            ms: now_ms,
            cx: obs.name_box.center_x(),
            top: obs.name_box.y,
        });

        if let Some(raw) = obs.raw_text.as_deref() {
            let resolved = if !obs.o0.is_empty() {
                self.o0_evidence.resolve(raw, &obs.o0)
            } else {
                raw.to_string()
            };
            self.read_history.push_back((now_ms, resolved));
            while self.read_history.len() > settings.max_plate_read_history {
                self.read_history.pop_front();
            }
            self.evaluate_consensus(settings);
        }
    }

    fn evaluate_consensus(&mut self, settings: &PipelineSettings) {
        if self.read_history.is_empty() {
            return;
        }
        // Group reads by look-alike canonical key and pick the majority spelling
        let mut counts: Vec<(String, usize, f64, f64)> = Vec::new();
        for (ms, text) in &self.read_history {
            if let Some(entry) = counts.iter_mut().find(|(existing, _, _, _)| {
                LookAlikes::fold_o0(existing).eq_ignore_ascii_case(&LookAlikes::fold_o0(text))
            }) {
                entry.1 += 1;
                entry.2 = entry.2.min(*ms);
                entry.3 = entry.3.max(*ms);
                // Prefer most recent spelling when O/0 evidence updated it
                entry.0 = text.clone();
            } else {
                counts.push((text.clone(), 1, *ms, *ms));
            }
        }

        counts.sort_by(|a, b| b.1.cmp(&a.1));
        if let Some((best_name, count, first_ms, last_ms)) = counts.first() {
            let needed = settings.confirm_reads.max(1);
            let span_ok = needed == 1 || (*last_ms - *first_ms) >= settings.confirm_span_ms * 0.5;
            if *count >= needed && span_ok {
                self.status = PlateStatus::Confirmed;
                if self
                    .confirmed_handle
                    .as_deref()
                    .map(|cur| !cur.eq_ignore_ascii_case(best_name))
                    .unwrap_or(true)
                {
                    self.confirmed_handle = Some(best_name.clone());
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct EndedMarker {
    pub handle: String,
    pub identity: MarkerIdentity,
    pub last_found_ms: f64,
    pub ended_ms: f64,
}

pub struct MarkerTracker {
    next_id: u64,
    pub markers: Vec<TrackedMarker>,
    pub recently_ended: VecDeque<EndedMarker>,
    pub session_memory: VecDeque<EndedMarker>,
}

impl Default for MarkerTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkerTracker {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            markers: Vec::new(),
            recently_ended: VecDeque::new(),
            session_memory: VecDeque::new(),
        }
    }

    pub fn update(
        &mut self,
        observations: &[Observation],
        now_ms: f64,
        retention_seconds: f64,
        window_width: i32,
        window_height: i32,
        boundary: TrackingBoundary,
        settings: &PipelineSettings,
    ) -> Vec<u64> {
        let mut forgotten_ids = Vec::new();
        let n_tracks = self.markers.len();
        let n_obs = observations.len();

        let mut matched_obs = vec![false; n_obs];

        if n_tracks > 0 && n_obs > 0 {
            let cols = n_obs + n_tracks;
            let mut cost = vec![settings.miss_cost; n_tracks * cols];

            for (ti, track) in self.markers.iter().enumerate() {
                let (pred, margin) = track.predict_box(now_ms, settings);
                let h = track.name_box.height.max(10.0);

                for (oi, obs) in observations.iter().enumerate() {
                    let dx = (pred.center_x() - obs.name_box.center_x()).abs();
                    let dy = (pred.y - obs.name_box.y).abs();
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist > margin * 1.5 {
                        cost[ti * cols + oi] = settings.miss_cost + 100.0;
                        continue;
                    }
                    let pos_cost = dist / margin.max(1.0);
                    let width_diff = (track.name_box.width - obs.name_box.width).abs() / h;
                    let color_penalty = match (track.plate_color, obs.plate_color) {
                        (Some(a), Some(b)) if a != b => settings.colour_weight,
                        _ => 0.0,
                    };
                    let name_bonus = match (&track.confirmed_handle, &obs.raw_text) {
                        (Some(th), Some(oh)) if LookAlikes::names_equal(th, oh) => -0.35,
                        _ => 0.0,
                    };
                    let pair_cost = (pos_cost
                        + settings.width_weight * width_diff.min(2.0)
                        + color_penalty
                        + name_bonus)
                        .max(0.0);
                    cost[ti * cols + oi] = pair_cost;
                }
                // Dummy column `n_obs + ti` allows track `ti` to remain unassigned at `miss_cost`
                cost[ti * cols + (n_obs + ti)] = settings.miss_cost;
            }

            if let Ok(assignment) = Hungarian::solve(&cost, n_tracks, cols) {
                for (ti, &col) in assignment.iter().enumerate() {
                    if col < n_obs && cost[ti * cols + col] < settings.miss_cost {
                        matched_obs[col] = true;
                        self.markers[ti].record_observation(&observations[col], now_ms, settings);
                    } else {
                        self.markers[ti].missed_frames += 1;
                    }
                }
            }
        } else {
            for track in &mut self.markers {
                track.missed_frames += 1;
            }
        }

        // Spawn new tracks for unmatched observations that have a readable text candidate
        for (oi, obs) in observations.iter().enumerate() {
            if !matched_obs[oi] && obs.raw_text.is_some() && self.markers.len() < settings.max_tracked_markers {
                let id = self.next_id;
                self.next_id += 1;
                let mut marker = TrackedMarker::new(id, obs, now_ms);
                // If single-frame confirmation is configured
                if settings.confirm_reads <= 1 {
                    marker.status = PlateStatus::Confirmed;
                    marker.confirmed_handle = obs.raw_text.clone();
                }
                self.markers.push(marker);
            }
        }

        // Expire lost or out-of-boundary tracks
        let boundary_px = boundary.to_pixels(window_width as f64, window_height as f64);
        let retain_ms = (retention_seconds.max(0.0) * 1000.0).max(250.0);

        let mut kept = Vec::with_capacity(self.markers.len());
        for marker in self.markers.drain(..) {
            let age_since_hit = now_ms - marker.last_found_ms;
            let (pred, _) = marker.predict_box(now_ms, settings);
            let outside_boundary = pred.center_x() < boundary_px.x
                || pred.center_x() > boundary_px.right()
                || pred.center_y() < boundary_px.y
                || pred.center_y() > boundary_px.bottom();

            let drop = match marker.status {
                PlateStatus::Unconfirmed => {
                    marker.missed_frames >= settings.unconfirmed_check_fails || outside_boundary
                }
                PlateStatus::Confirmed => age_since_hit > retain_ms || outside_boundary,
            };

            if drop {
                forgotten_ids.push(marker.id);
                if marker.status == PlateStatus::Confirmed {
                    if let (Some(handle), Some(identity)) =
                        (marker.confirmed_handle.clone(), marker.identity.clone())
                    {
                        let ended = EndedMarker {
                            handle,
                            identity,
                            last_found_ms: marker.last_found_ms,
                            ended_ms: now_ms,
                        };
                        self.recently_ended.push_back(ended.clone());
                        self.session_memory.push_back(ended);
                        while self.session_memory.len() > settings.max_session_memory {
                            self.session_memory.pop_front();
                        }
                    }
                }
            } else {
                kept.push(marker);
            }
        }
        self.markers = kept;

        while let Some(front) = self.recently_ended.front() {
            if now_ms - front.ended_ms > settings.panel_keep_max_ms {
                self.recently_ended.pop_front();
            } else {
                break;
            }
        }

        forgotten_ids
    }
}

#[derive(Debug, Clone)]
pub struct RosterEntry {
    pub handle: String,
    pub identity: MarkerIdentity,
    pub last_seen_ms: f64,
    pub recent: bool,
}

pub struct InteractiveRoster;

impl InteractiveRoster {
    pub const WINDOW_MS: f64 = 5.0 * 60.0 * 1000.0;

    pub fn build(tracker: &MarkerTracker, now_ms: f64) -> Vec<RosterEntry> {
        Self::build_with_window(tracker, now_ms, Self::WINDOW_MS)
    }

    pub fn build_with_window(
        tracker: &MarkerTracker,
        now_ms: f64,
        window_ms: f64,
    ) -> Vec<RosterEntry> {
        let mut newest: HashMap<String, RosterEntry> = HashMap::new();

        let mut offer = |identity: Option<&MarkerIdentity>, seen_ms: f64| {
            let Some(id) = identity else {
                return;
            };
            if id.rsi != RsiState::Verified || id.handle.is_empty() {
                return;
            }
            let key = id.handle.to_ascii_lowercase();
            let replace = newest
                .get(&key)
                .map(|known| known.last_seen_ms < seen_ms)
                .unwrap_or(true);
            if replace {
                newest.insert(
                    key,
                    RosterEntry {
                        handle: id.handle.clone(),
                        identity: id.clone(),
                        last_seen_ms: seen_ms,
                        recent: now_ms - seen_ms <= window_ms,
                    },
                );
            }
        };

        for marker in &tracker.markers {
            if marker.status == PlateStatus::Confirmed {
                offer(marker.identity.as_ref(), marker.last_found_ms);
            }
        }
        for ended in &tracker.recently_ended {
            offer(Some(&ended.identity), ended.last_found_ms);
        }
        for remembered in &tracker.session_memory {
            offer(Some(&remembered.identity), remembered.ended_ms);
        }

        let mut list: Vec<RosterEntry> = newest.into_values().collect();
        list.sort_by(|a, b| {
            b.last_seen_ms
                .partial_cmp(&a.last_seen_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        list
    }
}
