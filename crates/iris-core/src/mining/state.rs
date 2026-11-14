use crate::detection::image_ops::BgrImage;
use crate::mining::reader::{MiningSettings, MiningSignalReader, SignalRead, SignalState};
use crate::mining::table::MiningTable;
use crate::models::{Bgr, IntRect, PixelRect};
use std::collections::VecDeque;

pub struct SignalConfidence;

impl SignalConfidence {
    pub fn min_margin(read: &SignalRead) -> f64 {
        let Some(scores) = read.digit_scores.as_ref() else {
            return 0.0;
        };
        if scores.is_empty() {
            return 0.0;
        }
        let mut min_val = f64::INFINITY;
        for digit in scores {
            let mut best = f64::NEG_INFINITY;
            let mut second = f64::NEG_INFINITY;
            for &s in digit {
                if s > best {
                    second = best;
                    best = s;
                } else if s > second {
                    second = s;
                }
            }
            min_val = min_val.min(best - second);
        }
        min_val
    }

    pub fn is_confident(read: &SignalRead, margin: f64) -> bool {
        read.state == SignalState::Signal && Self::min_margin(read) >= margin
    }
}

#[derive(Debug, Clone)]
pub struct DigitEvidence {
    max_reads: usize,
    reads: Vec<Vec<[f64; 10]>>,
    layout: Option<String>,
}

impl DigitEvidence {
    pub fn new(max_reads: usize) -> Self {
        Self {
            max_reads: max_reads.max(1),
            reads: Vec::new(),
            layout: None,
        }
    }

    pub fn count(&self) -> usize {
        self.reads.len()
    }

    pub fn reset(&mut self) {
        self.reads.clear();
        self.layout = None;
    }

    pub fn add(&mut self, read: &SignalRead) {
        if !read.found() {
            return;
        }
        let Some(scores) = read.digit_scores.as_ref() else {
            return;
        };
        if scores.is_empty() {
            return;
        }
        let layout: String = read
            .text
            .chars()
            .map(|c| if c.is_ascii_digit() { '#' } else { c })
            .collect();
        if layout.chars().filter(|&c| c == '#').count() != scores.len() {
            return;
        }
        if self.layout.as_deref() != Some(layout.as_str()) {
            self.reads.clear();
            self.layout = Some(layout);
        }
        self.reads.push(scores.clone());
        if self.reads.len() > self.max_reads {
            self.reads.remove(0);
        }
    }

    pub fn fused(&self, current: &SignalRead, min_digit_score: f64) -> Option<SignalRead> {
        if self.reads.len() < 2 {
            return None;
        }
        let layout = self.layout.as_deref()?;
        let places = self.reads[0].len();
        let mut averaged = vec![[0.0f64; 10]; places];
        let mut text = String::new();
        let mut place = 0usize;
        let mut min_score = f64::INFINITY;
        let count_f = self.reads.len() as f64;

        for ch in layout.chars() {
            if ch != '#' {
                text.push(ch);
                continue;
            }
            let mut sum = [0.0f64; 10];
            for r in &self.reads {
                for d in 0..10 {
                    sum[d] += r[place][d] / count_f;
                }
            }
            let mut best = 0usize;
            for d in 1..10 {
                if sum[d] > sum[best] {
                    best = d;
                }
            }
            averaged[place] = sum;
            min_score = min_score.min(sum[best]);
            text.push((b'0' + best as u8) as char);
            place += 1;
        }

        if !MiningSignalReader::is_number(&text) || min_score < min_digit_score {
            return None;
        }
        let value = text.replace(',', "").parse::<i32>().ok()?;
        let mut out = current.clone();
        out.state = SignalState::Signal;
        out.text = text;
        out.value = Some(value);
        out.min_score = min_score;
        out.digit_scores = Some(averaged);
        Some(out)
    }
}

#[derive(Debug, Default)]
pub struct FrameAverage {
    max_frames: usize,
    frames: VecDeque<BgrImage>,
    region: IntRect,
}

impl FrameAverage {
    pub fn new(max_frames: usize) -> Self {
        Self {
            max_frames: max_frames.max(2),
            frames: VecDeque::new(),
            region: IntRect::default(),
        }
    }

    pub fn add(&mut self, image: &BgrImage, region: IntRect) {
        if image.is_empty() {
            return;
        }
        if region != self.region
            || self
                .frames
                .front()
                .is_some_and(|f| f.width != image.width || f.height != image.height)
        {
            self.clear();
            self.region = region;
        }
        self.frames.push_back(image.clone());
        while self.frames.len() > self.max_frames {
            self.frames.pop_front();
        }
    }

    pub fn average(&self) -> Option<BgrImage> {
        if self.frames.len() < 2 {
            return None;
        }
        let first = self.frames.front()?;
        let len = first.data.len();
        let mut sum = vec![0u32; len];
        for f in &self.frames {
            for i in 0..len {
                sum[i] += f.data[i] as u32;
            }
        }
        let n = self.frames.len() as u32;
        let data = sum
            .into_iter()
            .map(|s| ((s + n / 2) / n).min(255) as u8)
            .collect();
        Some(BgrImage {
            width: first.width,
            height: first.height,
            data,
        })
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }
}

#[derive(Debug, Clone)]
pub struct LabelColours {
    settings: MiningSettings,
    tb: f64,
    tg: f64,
    tr: f64,
    bb: f64,
    bg: f64,
    br: f64,
    published_ms: f64,
    pub text: Bgr,
    pub back: Bgr,
}

impl LabelColours {
    pub fn new(settings: MiningSettings, text: Bgr, back: Bgr) -> Self {
        Self {
            settings,
            tb: text.b as f64,
            tg: text.g as f64,
            tr: text.r as f64,
            bb: back.b as f64,
            bg: back.g as f64,
            br: back.r as f64,
            published_ms: f64::NEG_INFINITY,
            text,
            back,
        }
    }

    pub fn add(&mut self, text: Bgr, back: Bgr, now_ms: f64) -> bool {
        let a = self.settings.colour_blend;
        self.tb += a * (text.b as f64 - self.tb);
        self.tg += a * (text.g as f64 - self.tg);
        self.tr += a * (text.r as f64 - self.tr);
        self.bb += a * (back.b as f64 - self.bb);
        self.bg += a * (back.g as f64 - self.bg);
        self.br += a * (back.r as f64 - self.br);

        if now_ms - self.published_ms < self.settings.colour_update_ms {
            return false;
        }
        let to_u8 = |v: f64| v.round().clamp(0.0, 255.0) as u8;
        let new_text = Bgr::new(to_u8(self.tb), to_u8(self.tg), to_u8(self.tr));
        let new_back = Self::readable(
            new_text,
            Bgr::new(to_u8(self.bb), to_u8(self.bg), to_u8(self.br)),
            self.settings.min_contrast,
        );
        if Self::distance(new_text, self.text) < self.settings.colour_step
            && Self::distance(new_back, self.back) < self.settings.colour_step
        {
            return false;
        }
        self.text = new_text;
        self.back = new_back;
        self.published_ms = now_ms;
        true
    }

    pub fn readable(text: Bgr, mut back: Bgr, min_ratio: f64) -> Bgr {
        let lt = Self::luminance(text);
        for _ in 0..20 {
            let lb = Self::luminance(back);
            if (lt.max(lb) + 0.05) / (lt.min(lb) + 0.05) >= min_ratio {
                return back;
            }
            back = Bgr::new(
                (back.b as f64 * 0.8) as u8,
                (back.g as f64 * 0.8) as u8,
                (back.r as f64 * 0.8) as u8,
            );
        }
        back
    }

    fn luminance(c: Bgr) -> f64 {
        let lin = |v: u8| -> f64 {
            let s = v as f64 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    fn distance(a: Bgr, b: Bgr) -> i32 {
        (a.b as i32 - b.b as i32)
            .abs()
            .max((a.g as i32 - b.g as i32).abs())
            .max((a.r as i32 - b.r as i32).abs())
    }
}

pub struct TableSnap;

impl TableSnap {
    pub fn apply(read: SignalRead, table: &MiningTable, settings: &MiningSettings) -> SignalRead {
        if !read.found() || !MiningSignalReader::is_number(&read.text) {
            return read;
        }
        let Some(scores) = read.digit_scores.as_ref() else {
            return read;
        };
        if scores.is_empty()
            || read.text.chars().filter(|c| c.is_ascii_digit()).count() != scores.len()
        {
            return read;
        }

        let mut best = vec![0usize; scores.len()];
        let mut uncertain: Option<usize> = None;

        for (place, s) in scores.iter().enumerate() {
            let mut b = 0usize;
            let mut second: Option<usize> = None;
            for d in 1..10 {
                if s[d] > s[b] {
                    second = Some(b);
                    b = d;
                } else if second.map(|sec| s[d] > s[sec]).unwrap_or(true) {
                    second = Some(d);
                }
            }
            best[place] = b;
            if s[b] < settings.min_digit_score {
                return read;
            }
            let sec = second.unwrap_or(0);
            if s[b] - s[sec] < settings.confident_margin {
                if uncertain.is_some() {
                    return read;
                }
                uncertain = Some(place);
            }
        }

        let Some(uncertain_place) = uncertain else {
            return read;
        };

        let choices = &scores[uncertain_place];
        let mut found: Option<(i32, String, usize)> = None;

        for d in 0..10 {
            if choices[d] < choices[best[uncertain_place]] - settings.snap_choice_margin
                || choices[d] < settings.min_digit_score
            {
                continue;
            }
            let candidate_text = Self::with_digit(&read.text, uncertain_place, d);
            let Ok(val) = candidate_text.replace(',', "").parse::<i32>() else {
                continue;
            };
            if !table.contains(val) {
                continue;
            }
            if found.is_some() {
                return read;
            }
            found = Some((val, candidate_text, d));
        }

        let Some((snapped, found_text, found_digit)) = found else {
            return read;
        };
        if read.state == SignalState::Signal && read.value == Some(snapped) {
            return read;
        }
        let mut min_score = choices[found_digit];
        for (place, s) in scores.iter().enumerate() {
            if place != uncertain_place {
                min_score = min_score.min(s[best[place]]);
            }
        }
        let mut out = read;
        out.state = SignalState::Signal;
        out.text = found_text;
        out.value = Some(snapped);
        out.min_score = min_score;
        out
    }

    fn with_digit(text: &str, place: usize, digit: usize) -> String {
        let mut seen = 0usize;
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            if ch.is_ascii_digit() {
                if seen == place {
                    out.push((b'0' + digit as u8) as char);
                } else {
                    out.push(ch);
                }
                seen += 1;
            } else {
                out.push(ch);
            }
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct MiningState {
    pub locked_ms: f64,
    pub row_top: f64,
    pub signal_box: PixelRect,
    pub digit_height: f64,
    pub text_colour: Bgr,
    pub back_colour: Bgr,
    pub value: Option<i32>,
    pub label: Option<String>,
}

impl MiningState {
    pub fn from_lock(read: &SignalRead, frame_ms: f64, table: &MiningTable) -> Self {
        let label = read.value.and_then(|v| table.lookup(v)).map(str::to_string);
        let back = LabelColours::readable(
            read.text_colour,
            read.back_colour,
            MiningSettings::default().min_contrast,
        );
        Self {
            locked_ms: frame_ms,
            row_top: read.row_top,
            signal_box: read.box_rect,
            digit_height: read.digit_height,
            text_colour: read.text_colour,
            back_colour: back,
            value: if label.is_some() { read.value } else { None },
            label,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LockGeometry {
    settings: MiningSettings,
    reads: Vec<(PixelRect, f64, f64)>,
}

impl LockGeometry {
    pub fn new(settings: MiningSettings) -> Self {
        Self {
            settings,
            reads: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.reads.clear();
    }

    pub fn add(
        &mut self,
        box_rect: PixelRect,
        row_top: f64,
        digit_height: f64,
        locked: &MiningState,
    ) -> Option<(PixelRect, f64, f64)> {
        self.reads.push((box_rect, row_top, digit_height));
        if self.reads.len() > self.settings.geometry_reads {
            self.reads.remove(0);
        }
        if self.reads.len() < self.settings.geometry_min_reads {
            return None;
        }
        let median_rect = PixelRect::new(
            self.median(|r| r.0.x),
            self.median(|r| r.0.y),
            self.median(|r| r.0.width),
            self.median(|r| r.0.height),
        );
        let row = self.median(|r| r.1);
        let mut height = self.median(|r| r.2);
        if locked.digit_height > 0.0
            && (height - locked.digit_height).abs()
                < self
                    .settings
                    .label_size_change_px
                    .max(self.settings.label_size_change_share * locked.digit_height)
        {
            height = locked.digit_height;
        }
        let mv = self.settings.geometry_change_px;
        let differs = (median_rect.x - locked.signal_box.x).abs() >= mv
            || (median_rect.y - locked.signal_box.y).abs() >= mv
            || (median_rect.right() - locked.signal_box.right()).abs() >= mv
            || (median_rect.bottom() - locked.signal_box.bottom()).abs() >= mv
            || (row - locked.row_top).abs() >= mv
            || (height - locked.digit_height).abs() >= mv;
        if differs {
            Some((median_rect, row, height))
        } else {
            None
        }
    }

    fn median(&self, f: impl Fn(&(PixelRect, f64, f64)) -> f64) -> f64 {
        let mut vals: Vec<f64> = self.reads.iter().map(f).collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = vals.len() / 2;
        if vals.len() % 2 == 1 {
            vals[mid]
        } else {
            (vals[mid - 1] + vals[mid]) / 2.0
        }
    }
}

#[derive(Debug, Clone)]
pub struct MiningLabelState {
    settings: MiningSettings,
    last_seen_ms: f64,
    last_value_ms: f64,
    pending: Option<i32>,
    pending_count: usize,
    pub value: Option<i32>,
    pub label: Option<String>,
    pub ended: bool,
}

impl MiningLabelState {
    pub fn new(
        settings: MiningSettings,
        locked_ms: f64,
        value: Option<i32>,
        label: Option<String>,
    ) -> Self {
        Self {
            settings,
            last_seen_ms: locked_ms,
            last_value_ms: locked_ms,
            pending: None,
            pending_count: 0,
            value,
            label,
            ended: false,
        }
    }

    pub fn apply(&mut self, read: &SignalRead, frame_ms: f64, table: &MiningTable) -> bool {
        if self.ended {
            return false;
        }
        let mut changed = false;
        match read.state {
            SignalState::Signal if read.value.is_some() => {
                let val = read.value.unwrap();
                self.last_seen_ms = self.last_seen_ms.max(frame_ms);
                if Some(val) == self.value {
                    self.last_value_ms = self.last_value_ms.max(frame_ms);
                    self.pending = None;
                } else {
                    let lbl = table.lookup(val).map(str::to_string);
                    let needed = if let Some(cur) = self.value {
                        if Self::is_clearly_different(read, cur, self.settings.clear_digit_margin) {
                            1
                        } else {
                            self.settings.change_reads
                        }
                    } else if lbl.is_some() {
                        1
                    } else {
                        self.settings.off_table_reads
                    };

                    if self.pending == Some(val) {
                        self.pending_count += 1;
                    } else {
                        self.pending = Some(val);
                        self.pending_count = 1;
                    }

                    if self.pending_count >= needed {
                        self.pending = None;
                        self.last_value_ms = frame_ms;
                        let final_lbl =
                            lbl.or_else(|| table.above_limit_label(val).map(str::to_string));
                        changed = self.show(Some(val), final_lbl);
                    }
                }
            }
            SignalState::Unreadable => {
                self.last_seen_ms = self.last_seen_ms.max(frame_ms);
            }
            SignalState::Disqualified => {
                self.last_seen_ms = self.last_seen_ms.max(frame_ms);
                self.pending = None;
                changed = self.show(None, None);
            }
            _ => {
                self.pending = None;
                if frame_ms - self.last_seen_ms >= self.settings.hold_ms {
                    self.ended = true;
                    self.value = None;
                    self.label = None;
                    return true;
                }
            }
        }

        if self.value.is_some() && frame_ms - self.last_value_ms >= self.settings.hold_ms {
            changed |= self.show(None, None);
        }
        changed
    }

    pub fn is_clearly_different(read: &SignalRead, shown: i32, margin: f64) -> bool {
        let (Some(val), Some(scores)) = (read.value, read.digit_scores.as_ref()) else {
            return false;
        };
        let rd = val.to_string();
        let sd = shown.to_string();
        if rd.len() != sd.len() {
            return true;
        }
        if scores.len() != rd.len() {
            return false;
        }
        for ((rc, sc), digit_scores) in rd.bytes().zip(sd.bytes()).zip(scores.iter()) {
            if rc == sc {
                continue;
            }
            if digit_scores[(rc - b'0') as usize] - digit_scores[(sc - b'0') as usize] < margin {
                return false;
            }
        }
        true
    }

    fn show(&mut self, value: Option<i32>, label: Option<String>) -> bool {
        let changed = value != self.value || label != self.label;
        self.value = value;
        self.label = label;
        changed
    }
}
