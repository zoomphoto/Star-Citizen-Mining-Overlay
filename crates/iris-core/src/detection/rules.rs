use crate::detection::detector::NameplateDetectorSettings;
use crate::detection::features::{CapBand, LineMask, NameplateFeatures};
use crate::detection::image_ops::{resize_cubic_f32, BgrImage};
use crate::detection::reader::{PieceRead, ReadPiece};
use crate::models::{IntRect, PlateColor};
use std::collections::{HashMap, VecDeque};

pub struct LookAlikes;

impl LookAlikes {
    pub const EFFICIENCY_GROUPS: [&'static str; 6] = ["0OQD", "1I7", "5S", "8B", "6G", "2Z"];
    pub const EFFICIENCY_LETTERS: &'static str = "OQDISBGZ";

    pub fn names_equal(a: &str, b: &str) -> bool {
        a.eq_ignore_ascii_case(b)
    }

    pub fn fold_o0(name: &str) -> String {
        name.replace('0', "O")
    }

    pub fn efficiency_digit(c: char) -> Option<char> {
        if c.is_ascii_digit() {
            return Some(c);
        }
        for group in Self::EFFICIENCY_GROUPS {
            if let Some(pos) = group.find(c) {
                if pos > 0 {
                    return group.chars().next();
                }
            }
        }
        None
    }
}

pub struct ContactCode;

impl ContactCode {
    /// Detects Star Citizen anonymized contact codes (`XX-XXXX-XX` len 10 with dashes at 2 and 7).
    pub fn is_contact_code(name: &str) -> bool {
        let b = name.as_bytes();
        if b.len() != 10 || b[2] != b'-' || b[7] != b'-' {
            return false;
        }
        for (i, &ch) in b.iter().enumerate() {
            if i == 2 || i == 7 {
                continue;
            }
            if !ch.is_ascii_alphanumeric() {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct PlateColorSettings {
    pub white_saturation: f64,
    pub green_saturation: f64,
    pub green_hue_from: f64,
    pub green_hue_to: f64,
    pub red_hue_from: f64,
    pub red_hue_to: f64,
    pub blue_hue_from: f64,
    pub blue_hue_to: f64,
    pub blue_saturation: f64,
}

impl Default for PlateColorSettings {
    fn default() -> Self {
        Self {
            white_saturation: 0.15,
            green_saturation: 0.45,
            green_hue_from: 40.0,
            green_hue_to: 165.0,
            red_hue_from: 330.0,
            red_hue_to: 40.0,
            blue_hue_from: 195.0,
            blue_hue_to: 270.0,
            blue_saturation: 0.25,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlateColorStats {
    pub hue: f64,
    pub saturation: f64,
    pub value: f64,
    pub pixels: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PlateColorReading {
    pub color: PlateColor,
    pub hue: f64,
    pub saturation: f64,
}

pub struct PlateColors;

impl PlateColors {
    pub fn measure<'a>(rows: impl IntoIterator<Item = &'a LineMask>) -> Option<PlateColorStats> {
        let (bm, gm, rm, total) = Self::median_ink(rows)?;
        let mx = rm.max(gm.max(bm));
        let mn = rm.min(gm.min(bm));
        let saturation = if mx == 0.0 { 0.0 } else { (mx - mn) / mx };
        let hue = if (mx - mn).abs() < 1e-12 {
            0.0
        } else if mx == rm {
            (60.0 * ((gm - bm) / (mx - mn))).rem_euclid(360.0)
        } else if mx == gm {
            60.0 * ((bm - rm) / (mx - mn)) + 120.0
        } else {
            60.0 * ((rm - gm) / (mx - mn)) + 240.0
        };
        Some(PlateColorStats {
            hue,
            saturation,
            value: mx / 255.0,
            pixels: total,
        })
    }

    pub fn classify<'a>(
        rows: impl IntoIterator<Item = &'a LineMask>,
        settings: &PlateColorSettings,
    ) -> Option<PlateColorReading> {
        let st = Self::measure(rows)?;
        let color = Self::classify_hs(st.hue, st.saturation, settings);
        Some(PlateColorReading {
            color,
            hue: st.hue,
            saturation: st.saturation,
        })
    }

    pub fn classify_hs(hue: f64, saturation: f64, p: &PlateColorSettings) -> PlateColor {
        if saturation < p.white_saturation {
            return PlateColor::White;
        }
        if saturation >= p.green_saturation {
            if p.green_hue_from <= hue && hue < p.green_hue_to {
                return PlateColor::Green;
            }
            if hue >= p.red_hue_from || hue < p.red_hue_to {
                return PlateColor::Red;
            }
        }
        if p.blue_hue_from <= hue && hue < p.blue_hue_to && saturation >= p.blue_saturation {
            return PlateColor::Blue;
        }
        PlateColor::White
    }

    fn median_ink<'a>(
        rows: impl IntoIterator<Item = &'a LineMask>,
    ) -> Option<(f64, f64, f64, usize)> {
        let mut slices: Vec<&[u8]> = Vec::new();
        let mut total = 0usize;
        for row in rows {
            if let Some(cols) = row.ink_colors.as_deref() {
                slices.push(cols);
                total += cols.len() / 3;
            }
        }
        if total < NameplateDetectorSettings::PLATE_COLOR_MIN_PIXELS {
            return None;
        }

        let mut brightness = [0usize; 256];
        for cols in &slices {
            for px in cols.chunks_exact(3) {
                let v = px[0].max(px[1].max(px[2])) as usize;
                brightness[v] += 1;
            }
        }
        let threshold = hist_at(&brightness, total / 2) as u8;

        let mut b_hist = [0usize; 256];
        let mut g_hist = [0usize; 256];
        let mut r_hist = [0usize; 256];
        let mut core = 0usize;
        for cols in &slices {
            for px in cols.chunks_exact(3) {
                let (b, g, r) = (px[0], px[1], px[2]);
                if b.max(g.max(r)) < threshold {
                    continue;
                }
                b_hist[b as usize] += 1;
                g_hist[g as usize] += 1;
                r_hist[r as usize] += 1;
                core += 1;
            }
        }
        if core == 0 {
            return None;
        }
        Some((
            hist_median(&b_hist, core),
            hist_median(&g_hist, core),
            hist_median(&r_hist, core),
            total,
        ))
    }
}

fn hist_at(hist: &[usize; 256], idx: usize) -> usize {
    let mut seen = 0usize;
    for (v, &c) in hist.iter().enumerate() {
        seen += c;
        if idx < seen {
            return v;
        }
    }
    255
}

fn hist_median(hist: &[usize; 256], count: usize) -> f64 {
    if count % 2 == 1 {
        hist_at(hist, count / 2) as f64
    } else {
        (hist_at(hist, count / 2 - 1) + hist_at(hist, count / 2)) as f64 / 2.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DistanceReading {
    pub text: String,
    pub value: f64,
    pub kilometers: bool,
    pub has_speed_group: bool,
}

pub struct DistanceLineRule;

impl DistanceLineRule {
    pub fn alphabet() -> String {
        format!("0123456789.km{}", LookAlikes::EFFICIENCY_LETTERS)
    }

    pub fn evaluate<F>(
        read: &PieceRead,
        settings: &NameplateDetectorSettings,
        mut is_open_bracket: F,
    ) -> Option<DistanceReading>
    where
        F: FnMut(&ReadPiece) -> bool,
    {
        if read.pieces.is_empty() {
            return None;
        }
        let cap = read.cap_height as f64;
        let mut first = 0usize;
        let mut last = read.pieces.len() - 1;
        while last > first && read.pieces[last].score < settings.distance_end_score {
            last -= 1;
        }
        while first < last && read.pieces[first].score < settings.distance_end_score {
            first += 1;
        }
        let mut pieces = &read.pieces[first..=last];
        let mut speed_group = false;

        for i in 0..pieces.len().saturating_sub(1) {
            if (pieces[i + 1].start.saturating_sub(pieces[i].end)) as f64 <= settings.word_gap * cap
            {
                continue;
            }
            if !is_open_bracket(&pieces[i + 1]) {
                return None;
            }
            pieces = &pieces[..=i];
            speed_group = true;
            break;
        }

        let mut text = String::new();
        let mut chars = 0usize;
        let mut total = 0.0f64;
        for p in pieces {
            for ch in p.text.chars() {
                text.push(LookAlikes::efficiency_digit(ch).unwrap_or(ch));
            }
            chars += p.text.len();
            total += p.score;
        }
        let mean = total / chars.max(1) as f64;

        let count = pieces.len().min(6);
        let mut gap_sum = 0.0f64;
        for p in &pieces[..count] {
            gap_sum += (p.free_score - p.score) / p.text.len().max(1) as f64;
        }
        let gap = gap_sum / count.max(1) as f64;

        if mean < settings.min_distance_score || gap > settings.max_distance_gap {
            return None;
        }
        let mut parsed = Self::parse(&text, settings.max_distance_value)?;
        parsed.has_speed_group = speed_group;
        Some(parsed)
    }

    pub fn parse(text: &str, max_value: i32) -> Option<DistanceReading> {
        let bytes = text.as_bytes();
        let mut at = 0usize;
        while at < bytes.len() && at < 4 && bytes[at].is_ascii_digit() {
            at += 1;
        }
        if at == 0 || at >= bytes.len() || bytes[at] != b'.' {
            return None;
        }
        let whole: i32 = text[..at].parse().ok()?;
        if whole > max_value {
            return None;
        }
        let dot = at;
        if dot + 1 >= bytes.len() || !bytes[dot + 1].is_ascii_digit() {
            return None;
        }
        let tenth = (bytes[dot + 1] - b'0') as f64;
        let unit = &text[dot + 2..];
        let kilometers = match unit {
            "km" => true,
            "m" => false,
            _ => return None,
        };
        Some(DistanceReading {
            text: text.to_string(),
            value: whole as f64 + tenth / 10.0,
            kilometers,
            has_speed_group: false,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct O0Measurement {
    pub outer_width: f64,
    pub outer_height: f64,
    pub hole_width: f64,
    pub hole_height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct O0Glyph {
    pub index: usize,
    pub probability_o: f64,
}

pub struct O0Rule;

impl O0Rule {
    pub const WEIGHT_A: f64 = 0.5667556378738614;
    pub const WEIGHT_B: f64 = 0.8238859429181884;
    pub const MEAN_O: f64 = 0.7533676849758579;
    pub const SD_O: f64 = 0.025796386202400184;
    pub const MEAN_0: f64 = 0.6808192767983506;
    pub const SD_0: f64 = 0.01720362317305907;
    pub const UPSCALE: usize = 8;
    pub const PAD: i32 = 3;

    pub fn measure(crop: &BgrImage) -> Option<O0Measurement> {
        if crop.is_empty() {
            return None;
        }
        let n = crop.width * crop.height;
        let mut gray = vec![0.0f32; n];
        for i in 0..n {
            let b = crop.data[i * 3];
            let g = crop.data[i * 3 + 1];
            let r = crop.data[i * 3 + 2];
            gray[i] = b.max(g.max(r)) as f32;
        }
        let w = crop.width * Self::UPSCALE;
        let h = crop.height * Self::UPSCALE;
        let big = resize_cubic_f32(&gray, crop.width, crop.height, w, h);

        let mut sorted = big.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let thr = ((percentile_f32(&sorted, 10.0) + percentile_f32(&sorted, 99.0)) * 0.5) as f32;
        let mask: Vec<u8> = big.iter().map(|&v| if v > thr { 1 } else { 0 }).collect();
        Self::measure_mask(&mask, w, h)
    }

    pub fn measure_mask(mask: &[u8], width: usize, height: usize) -> Option<O0Measurement> {
        if mask.len() != width * height || width == 0 || height == 0 {
            return None;
        }
        let mut cols = vec![0usize; width];
        for y in 0..height {
            for x in 0..width {
                cols[x] += mask[y * width + x] as usize;
            }
        }
        let mut best_start: Option<usize> = None;
        let mut best_end = 0usize;
        let mut best_ink = 0usize;
        let mut at = 0usize;
        while at < width {
            if cols[at] > 0 {
                let start = at;
                let mut ink = 0usize;
                while at < width && cols[at] > 0 {
                    ink += cols[at];
                    at += 1;
                }
                if best_start.is_none() || ink > best_ink {
                    best_ink = ink;
                    best_start = Some(start);
                    best_end = at;
                }
            } else {
                at += 1;
            }
        }
        let best_start = best_start?;
        let glyph_w = best_end - best_start;

        let mut first_row: Option<usize> = None;
        let mut last_row = 0usize;
        for y in 0..height {
            for x in best_start..best_end {
                if mask[y * width + x] != 0 {
                    if first_row.is_none() {
                        first_row = Some(y);
                    }
                    last_row = y;
                    break;
                }
            }
        }
        let first_row = first_row?;
        let glyph_h = last_row - first_row + 1;
        if glyph_w as f64 > 1.6 * glyph_h as f64 {
            return None;
        }

        let mut label = vec![0i32; glyph_w * glyph_h];
        let mut queue = Vec::with_capacity(glyph_w * glyph_h);
        let mut best_area = 0usize;
        let mut hole_w = 0usize;
        let mut hole_h = 0usize;
        let mut next = 0i32;

        for start in 0..label.len() {
            let gx = start % glyph_w;
            let gy = start / glyph_w;
            if label[start] != 0 || mask[(first_row + gy) * width + best_start + gx] != 0 {
                continue;
            }
            next += 1;
            label[start] = next;
            queue.clear();
            queue.push((gx, gy));
            let mut head = 0usize;
            let mut area = 0usize;
            let mut min_x = glyph_w;
            let mut min_y = glyph_h;
            let mut max_x = 0usize;
            let mut max_y = 0usize;

            while head < queue.len() {
                let (cx, cy) = queue[head];
                head += 1;
                area += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                    let nx = cx as isize + dx;
                    let ny = cy as isize + dy;
                    if nx < 0 || ny < 0 || nx >= glyph_w as isize || ny >= glyph_h as isize {
                        continue;
                    }
                    let (nxu, nyu) = (nx as usize, ny as usize);
                    let nb = nyu * glyph_w + nxu;
                    if label[nb] != 0 || mask[(first_row + nyu) * width + best_start + nxu] != 0 {
                        continue;
                    }
                    label[nb] = next;
                    queue.push((nxu, nyu));
                }
            }

            let interior = min_x > 0 && min_y > 0 && max_x + 1 < glyph_w && max_y + 1 < glyph_h;
            if interior && area > best_area {
                best_area = area;
                hole_w = max_x - min_x + 1;
                hole_h = max_y - min_y + 1;
            }
        }

        if best_area == 0 {
            return None;
        }
        Some(O0Measurement {
            outer_width: glyph_w as f64 / Self::UPSCALE as f64,
            outer_height: glyph_h as f64 / Self::UPSCALE as f64,
            hole_width: hole_w as f64 / Self::UPSCALE as f64,
            hole_height: hole_h as f64 / Self::UPSCALE as f64,
        })
    }

    pub fn probability_o(m: O0Measurement) -> f64 {
        let mut s = (m.outer_width / m.outer_height) * Self::WEIGHT_A
            + (m.hole_width / m.outer_height) * Self::WEIGHT_B;
        s = s.clamp(Self::MEAN_0.min(Self::MEAN_O), Self::MEAN_0.max(Self::MEAN_O));
        let l_o = -0.5 * ((s - Self::MEAN_O) / Self::SD_O).powi(2) - Self::SD_O.ln();
        let l_0 = -0.5 * ((s - Self::MEAN_0) / Self::SD_0).powi(2) - Self::SD_0.ln();
        let d = l_0 - l_o;
        if d > 700.0 {
            0.0
        } else {
            1.0 / (1.0 + d.exp())
        }
    }

    pub fn verdict(prob_o: f64) -> Option<char> {
        if prob_o >= NameplateDetectorSettings::O0_DECIDE {
            Some('O')
        } else if prob_o <= 1.0 - NameplateDetectorSettings::O0_DECIDE {
            Some('0')
        } else {
            None
        }
    }

    pub fn glyph_box(
        mask: &LineMask,
        start: usize,
        end: usize,
        pad: i32,
    ) -> Option<(i32, i32, i32, i32)> {
        let mut first_row: Option<usize> = None;
        let mut last_row = 0usize;
        for y in 0..mask.height {
            for x in start..end.min(mask.width) {
                if mask.data[y * mask.width + x] != 0 {
                    if first_row.is_none() {
                        first_row = Some(y);
                    }
                    last_row = y;
                    break;
                }
            }
        }
        let first_row = first_row?;
        let scale = NameplateFeatures::SCALE as f64;
        let x0 = mask.origin_x + ((mask.column_offset + start) as f64 / scale).floor() as i32 - pad;
        let x1 = mask.origin_x + ((mask.column_offset + end) as f64 / scale).ceil() as i32 + pad;
        let y0 = mask.origin_y + ((mask.row_offset + first_row) as f64 / scale).floor() as i32 - pad;
        let y1 =
            mask.origin_y + ((mask.row_offset + last_row + 1) as f64 / scale).ceil() as i32 + pad;
        Some((x0, y0, x1, y1))
    }

    pub fn correct(
        bgr: &BgrImage,
        read: &PieceRead,
        text: &str,
        mut evidence: Option<&mut Vec<O0Glyph>>,
    ) -> String {
        let Some(mask) = read.mask.as_ref() else {
            return text.to_string();
        };
        if read.character_spans.len() != text.len()
            || (!text.contains('O') && !text.contains('0'))
        {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        for (k, ch) in chars.iter_mut().enumerate() {
            if *ch != 'O' && *ch != '0' {
                continue;
            }
            let (start, end) = read.character_spans[k];
            let Some((bx0, by0, bx1, by1)) = Self::glyph_box(mask, start, end, Self::PAD) else {
                continue;
            };
            let x0 = bx0.max(0);
            let y0 = by0.max(0);
            let x1 = bx1.min(bgr.width as i32);
            let y1 = by1.min(bgr.height as i32);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let crop = bgr.crop(IntRect::new(x0, y0, x1 - x0, y1 - y0));
            let Some(m) = Self::measure(&crop) else {
                continue;
            };
            let prob = Self::probability_o(m);
            if let Some(ev) = evidence.as_deref_mut() {
                ev.push(O0Glyph {
                    index: k,
                    probability_o: prob,
                });
            }
            if let Some(v) = Self::verdict(prob) {
                *ch = v;
            }
        }
        chars.into_iter().collect()
    }
}

fn percentile_f32(sorted: &[f32], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = (sorted.len() - 1) as f64 * q / 100.0;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    let frac = pos - lo as f64;
    sorted[lo] as f64 + (sorted[hi] as f64 - sorted[lo] as f64) * frac
}

/// Multi-frame log-likelihood ratio accumulator for O vs 0 across reads of a marker.
#[derive(Debug, Default, Clone)]
pub struct O0EvidenceTable {
    totals: HashMap<String, Vec<f64>>,
    order: VecDeque<String>,
}

impl O0EvidenceTable {
    pub fn llr(prob_o: f64) -> f64 {
        if prob_o.is_nan() {
            return 0.0;
        }
        if prob_o <= 0.0 {
            return -NameplateDetectorSettings::EVIDENCE_LLR_CLIP;
        }
        if prob_o >= 1.0 {
            return NameplateDetectorSettings::EVIDENCE_LLR_CLIP;
        }
        (prob_o / (1.0 - prob_o)).ln().clamp(
            -NameplateDetectorSettings::EVIDENCE_LLR_CLIP,
            NameplateDetectorSettings::EVIDENCE_LLR_CLIP,
        )
    }

    pub fn resolve(&mut self, name: &str, glyphs: &[O0Glyph]) -> String {
        let key = LookAlikes::fold_o0(name);
        if !self.totals.contains_key(&key) {
            if self.totals.len() >= NameplateDetectorSettings::EVIDENCE_MAX_KEYS {
                if let Some(oldest) = self.order.pop_front() {
                    self.totals.remove(&oldest);
                }
            }
            self.totals.insert(key.clone(), vec![0.0; name.len()]);
            self.order.push_back(key.clone());
        }
        let totals = self.totals.get_mut(&key).unwrap();
        if totals.len() != name.len() {
            totals.resize(name.len(), 0.0);
        }

        let mut chars: Vec<char> = name.chars().collect();
        for g in glyphs {
            if g.index < chars.len() && (chars[g.index] == 'O' || chars[g.index] == '0') {
                totals[g.index] += Self::llr(g.probability_o);
            }
        }

        for (i, ch) in chars.iter_mut().enumerate() {
            if *ch != 'O' && *ch != '0' {
                continue;
            }
            if totals[i].abs() >= NameplateDetectorSettings::EVIDENCE_DECIDE_LLR {
                *ch = if totals[i] > 0.0 { 'O' } else { '0' };
            }
        }
        chars.into_iter().collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BarMeasurement {
    pub frac: f64,
    pub thick: f64,
    pub length: f64,
}

pub struct UnderscoreHyphenRule;

impl UnderscoreHyphenRule {
    pub fn measure(m: &LineMask, start: usize, end: usize, band: CapBand) -> Option<BarMeasurement> {
        let end = end.min(m.width);
        if end <= start {
            return None;
        }
        let mut row_sums = vec![0usize; m.height];
        let mut max_sum = 0usize;
        for y in 0..m.height {
            let s: usize = m.data[y * m.width + start..y * m.width + end]
                .iter()
                .map(|&v| v as usize)
                .sum();
            row_sums[y] = s;
            max_sum = max_sum.max(s);
        }
        if max_sum == 0 {
            return None;
        }
        let limit = NameplateDetectorSettings::BAR_ROW_FRACTION * max_sum as f64;
        let mut r0: Option<usize> = None;
        let mut r1 = 0usize;
        for (y, &s) in row_sums.iter().enumerate() {
            if (s as f64) >= limit {
                if r0.is_none() {
                    r0 = Some(y);
                }
                r1 = y;
            }
        }
        let r0 = r0?;

        let mut c0: Option<usize> = None;
        let mut c1 = 0usize;
        for x in start..end {
            let mut ink = false;
            for y in r0..=r1 {
                if m.data[y * m.width + x] != 0 {
                    ink = true;
                    break;
                }
            }
            if ink {
                if c0.is_none() {
                    c0 = Some(x);
                }
                c1 = x;
            }
        }

        let cap = band.height().max(1) as f64;
        let frac = ((r0 + r1 + 1) as f64 * 0.5 - band.top as f64) / cap;
        let thick = (r1 - r0 + 1) as f64 / cap;
        let length = c0.map(|c| (c1 - c + 1) as f64 / cap).unwrap_or(0.0);
        Some(BarMeasurement {
            frac,
            thick,
            length,
        })
    }

    pub fn verdict(bar: BarMeasurement) -> Option<char> {
        if bar.thick > NameplateDetectorSettings::BAR_MAX_THICK {
            return None;
        }
        if bar.frac >= NameplateDetectorSettings::BAR_UNDERSCORE_MIN {
            return Some('_');
        }
        if bar.frac <= NameplateDetectorSettings::BAR_HYPHEN_MAX {
            return Some('-');
        }
        None
    }

    pub fn correct(read: &PieceRead, text: &str) -> String {
        let Some(mask) = read.mask.as_ref() else {
            return text.to_string();
        };
        if read.character_spans.len() != text.len()
            || (!text.contains('_') && !text.contains('-'))
        {
            return text.to_string();
        }
        let band = NameplateFeatures::find_cap_band(mask);
        let mut chars: Vec<char> = text.chars().collect();
        for (k, ch) in chars.iter_mut().enumerate() {
            if *ch != '_' && *ch != '-' {
                continue;
            }
            let (start, end) = read.character_spans[k];
            if let Some(bar) = Self::measure(mask, start, end, band) {
                if let Some(v) = Self::verdict(bar) {
                    *ch = v;
                }
            }
        }
        chars.into_iter().collect()
    }
}
