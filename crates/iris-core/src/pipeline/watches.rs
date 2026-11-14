use crate::config::AppSettings;
use crate::detection::LayerRule;
use crate::models::PixelRect;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone)]
pub struct LiveSettings {
    pub min_speed_px_per_ms: f64,
    pub max_shift_box_heights: f64,
    pub org_cache_ms: f64,
    pub org_cache_entries: usize,
    pub cache_remembered: usize,
    pub lookup_retry_ms: f64,
    pub idle_release_ms: f64,
    pub summary_every_ms: f64,
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self {
            min_speed_px_per_ms: 0.01,
            max_shift_box_heights: 8.0,
            org_cache_ms: 2000.0,
            org_cache_entries: 256,
            cache_remembered: 512,
            lookup_retry_ms: 60_000.0,
            idle_release_ms: 5000.0,
            summary_every_ms: 60_000.0,
        }
    }
}

impl LiveSettings {
    pub const BLANK_FRAME_MAX_VALUE: f64 = 16.0;
    pub const BLANK_CHECK_EVERY_MS: f64 = 1000.0;
    pub const BLANK_NOTE_AFTER_MS: f64 = 3000.0;

    pub const ABERRATION_ON_CROPS: usize = 12;
    pub const ABERRATION_ON_SHIFTED: usize = 8;
    pub const ABERRATION_OFF_CROPS: usize = 40;
    pub const ABERRATION_OFF_UNSHIFTED: usize = 30;
    pub const ABERRATION_NOTE_MS: f64 = 30_000.0;
}

#[derive(Debug, Clone)]
pub struct PipelineSettings {
    pub scan_hz: f64,
    pub plate_hz: f64,
    pub max_reads_per_tick: usize,
    pub confirm_reads: usize,
    pub confirm_span_ms: f64,
    pub stable_width_lines: f64,
    pub stable_width_min_px: f64,
    pub stop_reads: usize,
    pub unconfirmed_check_fails: usize,
    pub confirmed_check_fails: usize,
    pub search_base_lines: f64,
    pub search_grow_lines_per_sec: f64,
    pub search_max_lines: f64,
    pub lost_after_ms: f64,
    pub lost_search_grow_lines_per_sec: f64,
    pub lost_search_max_lines: f64,
    pub velocity_gain: f64,
    pub max_speed_lines_per_ms: f64,
    pub max_look_ahead_ms: f64,
    pub velocity_window_ms: f64,
    pub prediction_error_gain: f64,
    pub prediction_error_smoothing: f64,
    pub max_tracked_markers: usize,
    pub max_plate_read_history: usize,
    pub duplicate_window_ticks: i64,
    pub panel_stale_ms: f64,
    pub panel_keep_max_ms: f64,
    pub max_session_memory: usize,
    pub miss_cost: f64,
    pub width_weight: f64,
    pub colour_weight: f64,
}

impl Default for PipelineSettings {
    fn default() -> Self {
        Self {
            scan_hz: 4.0,
            plate_hz: 12.0,
            max_reads_per_tick: 10,
            confirm_reads: 3,
            confirm_span_ms: 150.0,
            stable_width_lines: 0.3,
            stable_width_min_px: 2.0,
            stop_reads: 3,
            unconfirmed_check_fails: 2,
            confirmed_check_fails: 4,
            search_base_lines: 2.5,
            search_grow_lines_per_sec: 6.0,
            search_max_lines: 12.0,
            lost_after_ms: 100.0,
            lost_search_grow_lines_per_sec: 30.0,
            lost_search_max_lines: 10.0,
            velocity_gain: 0.3,
            max_speed_lines_per_ms: 0.33,
            max_look_ahead_ms: 400.0,
            velocity_window_ms: 85.0,
            prediction_error_gain: 2.0,
            prediction_error_smoothing: 0.5,
            max_tracked_markers: 48,
            max_plate_read_history: 8,
            duplicate_window_ticks: 3,
            panel_stale_ms: 5000.0,
            panel_keep_max_ms: 30_000.0,
            max_session_memory: 500,
            miss_cost: 3.0,
            width_weight: 0.5,
            colour_weight: 0.2,
        }
    }
}

impl PipelineSettings {
    pub const SUBPIXEL_MIN_CONTRAST: f64 = 40.0;
    pub const SUBPIXEL_FULL_COVERAGE: f64 = 0.9;
    pub const SUBPIXEL_MIN_WIDTH_FRACTION: f64 = 0.5;
    pub const SUBPIXEL_MAX_SHIFT_PX: f64 = 2.0;
    pub const SUBPIXEL_MAX_SHIFT_LINES: f64 = 0.3;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AberrationState {
    Unknown,
    On,
    Off,
}

#[derive(Debug, Clone, Default)]
pub struct AberrationWatch {
    recent: VecDeque<bool>,
}

impl AberrationWatch {
    pub const NOTE_TEXT: &'static str =
        "SC-Overlay: Chromatic Aberration is on in Star Citizen's graphics settings. Names read better with it off.";

    pub fn rule_for(state: AberrationState) -> LayerRule {
        match state {
            AberrationState::On => LayerRule::Agree,
            _ => LayerRule::Any,
        }
    }

    pub fn add(&mut self, state: AberrationState, verdicts: &[bool]) -> Option<AberrationState> {
        if state == AberrationState::Off {
            self.recent.clear();
            return None;
        }
        let max_keep = LiveSettings::ABERRATION_ON_CROPS.max(LiveSettings::ABERRATION_OFF_CROPS);
        for &shifted in verdicts {
            self.recent.push_back(shifted);
            while self.recent.len() > max_keep {
                self.recent.pop_front();
            }
            if state == AberrationState::Unknown
                && self.count(LiveSettings::ABERRATION_ON_CROPS, true)
                    >= LiveSettings::ABERRATION_ON_SHIFTED
            {
                self.recent.clear();
                return Some(AberrationState::On);
            }
            if state == AberrationState::On
                && self.count(LiveSettings::ABERRATION_OFF_CROPS, false)
                    >= LiveSettings::ABERRATION_OFF_UNSHIFTED
            {
                self.recent.clear();
                return Some(AberrationState::Off);
            }
        }
        None
    }

    fn count(&self, n: usize, value: bool) -> usize {
        let skip = self.recent.len().saturating_sub(n);
        self.recent
            .iter()
            .skip(skip)
            .filter(|&&v| v == value)
            .count()
    }
}

#[derive(Debug, Clone)]
pub struct BlankPictureWatch {
    last_check_ms: f64,
    black_since_ms: f64,
    pub active: bool,
}

impl Default for BlankPictureWatch {
    fn default() -> Self {
        Self {
            last_check_ms: f64::NEG_INFINITY,
            black_since_ms: f64::NAN,
            active: false,
        }
    }
}

impl BlankPictureWatch {
    pub const NOTE_TEXT: &'static str =
        "SC-Overlay: Video captured from Star Citizen is black. No OCR possible.";

    pub fn is_black(brightest_value: f64) -> bool {
        brightest_value <= LiveSettings::BLANK_FRAME_MAX_VALUE
    }

    pub fn due(&self, now_ms: f64) -> bool {
        now_ms - self.last_check_ms >= LiveSettings::BLANK_CHECK_EVERY_MS
    }

    pub fn update(&mut self, now_ms: f64, brightest_value: f64) -> bool {
        self.last_check_ms = now_ms;
        if !Self::is_black(brightest_value) {
            self.black_since_ms = f64::NAN;
        } else if self.black_since_ms.is_nan() {
            self.black_since_ms = now_ms;
        }
        let active = !self.black_since_ms.is_nan()
            && now_ms - self.black_since_ms >= LiveSettings::BLANK_NOTE_AFTER_MS;
        let changed = active != self.active;
        self.active = active;
        changed
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Default)]
struct WhiteSighting {
    names: HashMap<i64, String>,
    box_rect: PixelRect,
}

#[derive(Debug, Clone)]
pub struct WhiteDuplicateRule {
    plates: HashMap<u64, WhiteSighting>,
    window: i64,
}

impl WhiteDuplicateRule {
    pub fn new(window_ticks: i64) -> Self {
        Self {
            plates: HashMap::new(),
            window: window_ticks.max(1),
        }
    }

    pub fn forget(&mut self, plate_id: u64) {
        self.plates.remove(&plate_id);
    }

    pub fn clear(&mut self) {
        self.plates.clear();
    }

    pub fn note(
        &mut self,
        plate_id: u64,
        name: &str,
        tick: i64,
        white: bool,
        box_rect: PixelRect,
    ) -> Option<u64> {
        if !white {
            return None;
        }
        let window = self.window;
        {
            let mine = self.plates.entry(plate_id).or_default();
            mine.names.insert(tick, name.to_string());
            mine.box_rect = box_rect;
            mine.names.retain(|&k, _| k > tick - window);
        }

        let mine_names = self.plates.get(&plate_id)?.names.clone();
        for (&other_id, other) in &self.plates {
            if other_id == plate_id {
                continue;
            }
            let mut shared = false;
            for n in (tick - window + 1)..=tick {
                if let (Some(other_name), Some(my_name)) = (other.names.get(&n), mine_names.get(&n))
                {
                    if other_name.eq_ignore_ascii_case(name) && my_name.eq_ignore_ascii_case(name) {
                        shared = true;
                        break;
                    }
                }
            }
            if !shared || Self::overlaps(other.box_rect, box_rect) {
                continue;
            }
            return Some(other_id);
        }
        None
    }

    fn overlaps(a: PixelRect, b: PixelRect) -> bool {
        a.right().min(b.right()) - a.x.max(b.x) > 0.0
            && a.bottom().min(b.bottom()) - a.y.max(b.y) > 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimedOverlayPress {
    Ignored,
    Started,
    Restarted,
}

#[derive(Debug, Clone, Default)]
pub struct TimedOverlayRun {
    end_ms: i64,
    pub is_active: bool,
}

impl TimedOverlayRun {
    pub fn press(&mut self, overlay_disabled: bool, seconds: i32, now_ms: i64) -> TimedOverlayPress {
        if !overlay_disabled && !self.is_active {
            return TimedOverlayPress::Ignored;
        }
        let restarted = self.is_active;
        self.is_active = true;
        let clamped = seconds.clamp(
            AppSettings::TIMED_SECONDS_MIN,
            AppSettings::TIMED_SECONDS_MAX,
        ) as i64;
        self.end_ms = now_ms + clamped * 1000;
        if restarted {
            TimedOverlayPress::Restarted
        } else {
            TimedOverlayPress::Started
        }
    }

    pub fn seconds_left(&self, now_ms: i64) -> i32 {
        if !self.is_active {
            return 0;
        }
        let left = self.end_ms - now_ms;
        if left <= 0 {
            0
        } else {
            ((left + 999) / 1000) as i32
        }
    }

    pub fn expire(&mut self, now_ms: i64) -> bool {
        if !self.is_active || now_ms < self.end_ms {
            return false;
        }
        self.is_active = false;
        true
    }

    pub fn stop(&mut self) {
        self.is_active = false;
    }

    pub fn countdown_text(seconds: i32) -> String {
        format!("Overlay ON ({} sec)", seconds)
    }
}
