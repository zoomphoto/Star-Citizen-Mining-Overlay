use crate::detection::image_ops::{
    connected_components_with_stats, match_template_ccoeff_normed, morph_open_rect,
    morph_tophat_rect, otsu_threshold, resize_area_f32, resize_linear_f32, BgrImage,
    ComponentStats, GrayImage,
};
use crate::mining::templates::DigitTemplates;
use crate::models::{Bgr, IntRect, PixelRect};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct MiningSettings {
    pub lock_hz: f64,
    pub hold_ms: f64,
    pub off_table_reads: usize,
    pub change_reads: usize,
    pub clear_digit_margin: f64,
    pub confident_margin: f64,
    pub snap_choice_margin: f64,
    pub average_frames: usize,
    pub fuse_reads: usize,
    pub geometry_reads: usize,
    pub geometry_min_reads: usize,
    pub geometry_change_px: f64,
    pub label_size_change_px: f64,
    pub label_size_change_share: f64,
    pub line_height_at_1440: f64,
    pub row_centre_y: f64,
    pub search_above_lines: f64,
    pub search_below_lines: f64,
    pub row_search_half_lines: f64,
    pub pin_height_min: i32,
    pub pin_height_max: i32,
    pub pin_min_score: f64,
    pub pin_digit_ratio: f64,
    pub small_row_lines: f64,
    pub lock_above_lines: f64,
    pub lock_below_lines: f64,
    pub left_lines: f64,
    pub right_lines: f64,
    pub centre_offset_lines: f64,
    pub centre_tolerance_lines: f64,
    pub min_digit_score: f64,
    pub tie_margin: f64,
    pub split_width_lines: f64,
    pub split_whole_max_lines: f64,
    pub condensed_min_red: f64,
    pub condensed_green_share: f64,
    pub condensed_blue_share: f64,
    pub condensed_narrow_share: f64,
    pub condensed_one_share: f64,
    pub condensed_pair_share: f64,
    pub condensed_pitch_share: f64,
    pub condensed_cell_lines: f64,
    pub condensed_min_ink_pixels: usize,
    pub condensed_shape_min_share: f64,
    pub condensed_shape_max_share: f64,
    pub condensed_max_shape_lines: f64,
    pub condensed_cut_reach: f64,
    pub min_ink_threshold: f64,
    pub label_gap_lines: f64,
    pub label_pad_x_lines: f64,
    pub label_pad_y_lines: f64,
    pub label_back_alpha: u8,
    pub colour_blend: f64,
    pub colour_step: i32,
    pub colour_update_ms: f64,
    pub min_contrast: f64,
}

impl Default for MiningSettings {
    fn default() -> Self {
        Self {
            lock_hz: 12.0,
            hold_ms: 1000.0,
            off_table_reads: 2,
            change_reads: 2,
            clear_digit_margin: 0.08,
            confident_margin: 0.08,
            snap_choice_margin: 0.08,
            average_frames: 4,
            fuse_reads: 5,
            geometry_reads: 5,
            geometry_min_reads: 3,
            geometry_change_px: 1.0,
            label_size_change_px: 2.0,
            label_size_change_share: 0.15,
            line_height_at_1440: 12.0,
            row_centre_y: 472.0,
            search_above_lines: 4.0,
            search_below_lines: 11.0,
            row_search_half_lines: 4.0,
            pin_height_min: 13,
            pin_height_max: 26,
            pin_min_score: 0.7,
            pin_digit_ratio: 0.5,
            small_row_lines: 0.75,
            lock_above_lines: 1.5,
            lock_below_lines: 2.5,
            left_lines: 6.0,
            right_lines: 11.0,
            centre_offset_lines: 0.4,
            centre_tolerance_lines: 1.0,
            min_digit_score: 0.55,
            tie_margin: 0.08,
            split_width_lines: 0.8,
            split_whole_max_lines: 1.05,
            condensed_min_red: 150.0,
            condensed_green_share: 0.7,
            condensed_blue_share: 0.4,
            condensed_narrow_share: 0.52,
            condensed_one_share: 0.5,
            condensed_pair_share: 0.6,
            condensed_pitch_share: 0.55,
            condensed_cell_lines: 0.45,
            condensed_min_ink_pixels: 4,
            condensed_shape_min_share: 0.35,
            condensed_shape_max_share: 0.8,
            condensed_max_shape_lines: 2.6,
            condensed_cut_reach: 0.2,
            min_ink_threshold: 12.0,
            label_gap_lines: 0.6,
            label_pad_x_lines: 0.5,
            label_pad_y_lines: 0.25,
            label_back_alpha: 0x80,
            colour_blend: 0.3,
            colour_step: 12,
            colour_update_ms: 250.0,
            min_contrast: 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalState {
    None,
    Disqualified,
    Unreadable,
    Signal,
}

#[derive(Debug, Clone)]
pub struct SignalRead {
    pub state: SignalState,
    pub text: String,
    pub value: Option<i32>,
    pub box_rect: PixelRect,
    pub row_top: f64,
    pub digit_height: f64,
    pub text_colour: Bgr,
    pub back_colour: Bgr,
    pub offset_px: f64,
    pub min_score: f64,
    pub digit_scores: Option<Vec<[f64; 10]>>,
}

impl SignalRead {
    pub fn nothing() -> Self {
        Self {
            state: SignalState::None,
            text: String::new(),
            value: None,
            box_rect: PixelRect::default(),
            row_top: f64::NAN,
            digit_height: 0.0,
            text_colour: Bgr::default(),
            back_colour: Bgr::default(),
            offset_px: f64::NAN,
            min_score: f64::NAN,
            digit_scores: None,
        }
    }

    pub fn found(&self) -> bool {
        matches!(self.state, SignalState::Signal | SignalState::Unreadable)
    }
}

#[derive(Debug, Clone, Copy)]
struct Cell {
    digit: bool,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    shape: i32,
}

#[derive(Debug, Clone, Copy)]
struct SymbolBox {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone)]
struct Group {
    cells: Vec<Cell>,
    strip: Vec<u8>,
    strip_height: usize,
    threshold: f64,
    row_height: i32,
    small: bool,
    condensed: bool,
    left: i32,
    right: i32,
    top: i32,
    height: i32,
    symbol: Option<SymbolBox>,
    scores: Vec<f64>,
    all_scores: Vec<[f64; 10]>,
}

impl Group {
    fn centre(&self) -> f64 {
        match self.symbol {
            Some(s) => (s.x + self.right) as f64 / 2.0,
            None => (self.left + self.right) as f64 / 2.0,
        }
    }
}

pub struct MiningSignalReader {
    settings: MiningSettings,
}

impl Default for MiningSignalReader {
    fn default() -> Self {
        Self::new(MiningSettings::default())
    }
}

impl MiningSignalReader {
    const MAX_ROWS: usize = 4;

    pub fn new(settings: MiningSettings) -> Self {
        Self { settings }
    }

    pub fn settings(&self) -> &MiningSettings {
        &self.settings
    }

    pub fn line_height(window_height: i32, settings: &MiningSettings) -> f64 {
        settings.line_height_at_1440 * window_height as f64 / 1440.0
    }

    pub fn search_area(window_width: i32, window_height: i32, settings: &MiningSettings) -> PixelRect {
        let lh = Self::line_height(window_height, settings);
        let cy = settings.row_centre_y * window_height as f64 / 1440.0;
        Self::area(
            window_width,
            window_height,
            settings,
            cy - settings.search_above_lines * lh,
            cy + settings.search_below_lines * lh,
        )
    }

    pub fn row_search_area(
        window_width: i32,
        window_height: i32,
        settings: &MiningSettings,
    ) -> PixelRect {
        let lh = Self::line_height(window_height, settings);
        let cy = settings.row_centre_y * window_height as f64 / 1440.0;
        Self::area(
            window_width,
            window_height,
            settings,
            cy - settings.row_search_half_lines * lh,
            cy + settings.row_search_half_lines * lh,
        )
    }

    pub fn lock_area(
        window_width: i32,
        window_height: i32,
        row_top: f64,
        settings: &MiningSettings,
    ) -> PixelRect {
        let lh = Self::line_height(window_height, settings);
        Self::area(
            window_width,
            window_height,
            settings,
            row_top - settings.lock_above_lines * lh,
            row_top + settings.lock_below_lines * lh,
        )
    }

    fn area(
        window_width: i32,
        window_height: i32,
        settings: &MiningSettings,
        top: f64,
        bottom: f64,
    ) -> PixelRect {
        let lh = Self::line_height(window_height, settings);
        let cx = window_width as f64 / 2.0;
        let x0 = (cx - settings.left_lines * lh)
            .round()
            .clamp(0.0, window_width as f64);
        let x1 = (cx + settings.right_lines * lh)
            .round()
            .clamp(0.0, window_width as f64);
        let y0 = top.round().clamp(0.0, window_height as f64);
        let y1 = bottom.round().clamp(0.0, window_height as f64);
        PixelRect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    pub fn search(
        &self,
        image: &BgrImage,
        region: IntRect,
        window_width: i32,
        window_height: i32,
    ) -> SignalRead {
        let rows = Self::row_search_area(window_width, window_height, &self.settings);
        let x0 = (rows.x.round() as i32).max(region.x);
        let y0 = (rows.y.round() as i32).max(region.y);
        let x1 = ((rows.x + rows.width).round() as i32)
            .min(region.right().min(region.x + image.width as i32));
        let y1 = ((rows.y + rows.height).round() as i32)
            .min(region.bottom().min(region.y + image.height as i32));
        if x1 - x0 >= 4 && y1 - y0 >= 4 && !image.is_empty() {
            let band = IntRect::new(x0, y0, x1 - x0, y1 - y0);
            let part = image.crop(IntRect::new(
                band.x - region.x,
                band.y - region.y,
                band.width,
                band.height,
            ));
            let read = self.read_rows(&part, band, window_width, window_height);
            if read.state != SignalState::None {
                return read;
            }
        }
        self.read_from_pin(image, region, window_width, window_height)
    }

    pub fn read(
        &self,
        image: &BgrImage,
        region: IntRect,
        window_width: i32,
        window_height: i32,
    ) -> SignalRead {
        let read = self.read_rows(image, region, window_width, window_height);
        if read.state != SignalState::None {
            read
        } else {
            self.read_from_pin(image, region, window_width, window_height)
        }
    }

    fn compute_ink(&self, bgr: &BgrImage, window_height: i32) -> Option<GrayImage> {
        if bgr.is_empty() || bgr.width < 4 || bgr.height < 4 || window_height <= 0 {
            return None;
        }
        let scale = window_height as f64 / 1440.0;
        let (b, g, r) = bgr.split_channels();
        let n = bgr.width * bgr.height;
        let mut low = GrayImage::new(bgr.width, bgr.height);
        let mut high = GrayImage::new(bgr.width, bgr.height);
        for i in 0..n {
            low.data[i] = b.data[i].min(g.data[i]).min(r.data[i]);
            high.data[i] = b.data[i].max(g.data[i]).max(r.data[i]);
        }

        let k = ((15.0 * scale).round() as usize) | 1;
        let length = ((2.5 * self.settings.line_height_at_1440 * scale).round() as usize).max(3);

        let tophat_no_lines = |ch: &GrayImage| -> GrayImage {
            let hat = morph_tophat_rect(ch, k, k);
            let open_h = morph_open_rect(&hat, length, 1);
            let open_v = morph_open_rect(&hat, 1, length);
            let mut out = GrayImage::new(ch.width, ch.height);
            for i in 0..out.data.len() {
                let lines = open_h.data[i].max(open_v.data[i]);
                out.data[i] = hat.data[i].saturating_sub(lines);
            }
            out
        };

        let mut ink = tophat_no_lines(&low);
        let work = tophat_no_lines(&high);
        for i in 0..ink.data.len() {
            ink.data[i] = ink.data[i].max(work.data[i]);
        }
        Some(ink)
    }

    fn read_rows(
        &self,
        image: &BgrImage,
        region: IntRect,
        window_width: i32,
        window_height: i32,
    ) -> SignalRead {
        let lh = self.settings.line_height_at_1440 * window_height as f64 / 1440.0;
        let h = lh.round() as i32;
        if h < 4 {
            return SignalRead::nothing();
        }
        let Some(ink) = self.compute_ink(image, window_height) else {
            return SignalRead::nothing();
        };

        let mut groups = Vec::new();
        for row_top in Self::row_tops(&ink.data, ink.width, ink.height, h as usize) {
            groups.extend(self.read_row(image, &ink, row_top as i32, h, lh, false));
        }
        self.decide(groups, lh, image, &ink, region, window_width)
    }

    fn read_from_pin(
        &self,
        image: &BgrImage,
        region: IntRect,
        window_width: i32,
        window_height: i32,
    ) -> SignalRead {
        let Some(ink) = self.compute_ink(image, window_height) else {
            return SignalRead::nothing();
        };
        let scale = window_height as f64 / 1440.0;
        let ink_f: Vec<f32> = ink.data.iter().map(|&v| v as f32).collect();
        let pin = DigitTemplates::pin_template();

        let mut best_score = f64::NEG_INFINITY;
        let mut best_height = 0usize;
        let mut best_y = 0usize;

        for pin_h in self.settings.pin_height_min..=self.settings.pin_height_max {
            let th = (pin_h as f64 * scale).round() as usize;
            let tw = ((th as f64 * DigitTemplates::PIN_WIDTH as f64
                / DigitTemplates::PIN_HEIGHT as f64)
                .round() as usize)
                .max(3);
            if th < 4 || th > ink.height || tw > ink.width {
                continue;
            }
            let tpl = resize_area_f32(
                pin,
                DigitTemplates::PIN_WIDTH,
                DigitTemplates::PIN_HEIGHT,
                tw,
                th,
            );
            let (sc, _bx, by) =
                match_template_ccoeff_normed(&ink_f, ink.width, ink.height, &tpl, tw, th);
            if !sc.is_nan() && sc > best_score {
                best_score = sc;
                best_height = th;
                best_y = by;
            }
        }

        if best_score < self.settings.pin_min_score {
            return SignalRead::nothing();
        }
        let lh = best_height as f64 * self.settings.pin_digit_ratio;
        let h = lh.round() as i32;
        if h < 4 {
            return SignalRead::nothing();
        }
        let row_top = (best_y as f64 + best_height as f64 / 2.0 - lh / 2.0).round() as i32;
        let small =
            (h as f64) < self.settings.small_row_lines * self.settings.line_height_at_1440 * scale;
        let groups = self.read_row(image, &ink, row_top, h, lh, small);
        self.decide(groups, lh, image, &ink, region, window_width)
    }

    fn decide(
        &self,
        groups: Vec<Group>,
        lh: f64,
        bgr: &BgrImage,
        ink: &GrayImage,
        region: IntRect,
        window_width: i32,
    ) -> SignalRead {
        let mut unique: Vec<Group> = Vec::new();
        for g in groups {
            if !unique.iter().any(|u| {
                ((g.left - u.left).abs() as f64) <= 0.5 * lh
                    && ((g.top - u.top).abs() as f64) <= 0.5 * lh
            }) {
                unique.push(g);
            }
        }

        let expected =
            window_width as f64 / 2.0 + self.settings.centre_offset_lines * lh - region.x as f64;
        let mut main_idx: Option<usize> = None;
        for (idx, g) in unique.iter().enumerate() {
            if g.symbol.is_some()
                && main_idx
                    .map(|mi| (g.centre() - expected).abs() < (unique[mi].centre() - expected).abs())
                    .unwrap_or(true)
            {
                main_idx = Some(idx);
            }
        }
        let Some(mi) = main_idx else {
            return SignalRead::nothing();
        };

        let chosen_top = unique[mi].top;
        let offset = unique[mi].centre() - expected;
        let others = unique
            .iter()
            .enumerate()
            .filter(|&(idx, g)| idx != mi && ((g.top - chosen_top).abs() as f64) <= 0.5 * lh)
            .count();

        let mut main = unique.swap_remove(mi);
        let text = self.classify(&mut main, ink.width);
        if offset.abs() > self.settings.centre_tolerance_lines * lh || others > 0 {
            let mut r = SignalRead::nothing();
            r.state = SignalState::Disqualified;
            r.text = text;
            r.offset_px = offset;
            return r;
        }

        let sym = main.symbol.unwrap();
        let top = sym.y.min(main.top);
        let bottom = (sym.y + sym.height).max(main.top + main.height);
        let box_rect = PixelRect::new(
            (region.x + sym.x) as f64,
            (region.y + top) as f64,
            (main.right - sym.x) as f64,
            (bottom - top) as f64,
        );
        let (text_colour, back_colour) = Self::colours(&main, bgr, ink);
        let min_score = main
            .scores
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let min_score = if min_score.is_infinite() {
            0.0
        } else {
            min_score
        };

        let readable = Self::is_number(&text)
            && main
                .scores
                .iter()
                .all(|&s| s >= self.settings.min_digit_score);
        let value = if readable {
            text.replace(',', "").parse::<i32>().ok()
        } else {
            None
        };

        SignalRead {
            state: if value.is_some() {
                SignalState::Signal
            } else {
                SignalState::Unreadable
            },
            text,
            value,
            box_rect,
            row_top: (region.y + main.top) as f64,
            digit_height: main.height as f64,
            text_colour,
            back_colour,
            offset_px: offset,
            min_score,
            digit_scores: Some(main.all_scores),
        }
    }

    pub fn is_number(text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        if (4..=6).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit()) {
            return true;
        }
        let parts: Vec<&str> = text.split(',').collect();
        if !(1..=3).contains(&parts[0].len()) {
            return false;
        }
        for (i, part) in parts.iter().enumerate() {
            if i > 0 && part.len() != 3 {
                return false;
            }
            if !part.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
        }
        true
    }

    fn row_tops(ink: &[u8], width: usize, height: usize, h: usize) -> Vec<usize> {
        let mut tops = Vec::new();
        if height < h || width == 0 {
            return tops;
        }
        let mut profile = vec![0.0f64; height];
        for y in 0..height {
            let sum: u64 = ink[y * width..(y + 1) * width]
                .iter()
                .map(|&v| v as u64)
                .sum();
            profile[y] = sum as f64 / width as f64;
        }
        let len = height - h + 1;
        let mut sums: Vec<(usize, f64)> = (0..len)
            .map(|y| (y, profile[y..y + h].iter().sum::<f64>()))
            .collect();
        sums.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        for (y, _) in sums {
            if tops
                .iter()
                .all(|&t: &usize| (y as isize - t as isize).abs() as f64 >= 0.8 * h as f64)
            {
                tops.push(y);
            }
            if tops.len() == Self::MAX_ROWS {
                break;
            }
        }
        tops
    }

    fn read_row(
        &self,
        bgr: &BgrImage,
        ink: &GrayImage,
        row_top: i32,
        h: i32,
        lh: f64,
        small: bool,
    ) -> Vec<Group> {
        let mut groups = Vec::new();
        let width = ink.width;
        let height = ink.height;
        let ya = ((row_top as f64 - 0.3 * h as f64) as i32).max(0) as usize;
        let yb = ((row_top as f64 + h as f64 + 0.3 * h as f64) as i32 + 1)
            .clamp(0, height as i32) as usize;
        if yb.saturating_sub(ya) < 2 {
            return groups;
        }
        let strip_height = yb - ya;
        let strip = ink.data[ya * width..yb * width].to_vec();

        let threshold = otsu_threshold(&strip).max(self.settings.min_ink_threshold);
        let thr_u8 = threshold.floor() as u8;
        let binary = GrayImage::from_vec(
            width,
            strip_height,
            strip
                .iter()
                .map(|&v| if v > thr_u8 { 1 } else { 0 })
                .collect(),
        );
        let (labels, stats) = connected_components_with_stats(&binary);
        let mut cells = Vec::new();

        for (i, st) in stats.iter().enumerate().skip(1) {
            if st.area < 2 {
                continue;
            }
            let wide = (st.w as f64) > self.settings.split_width_lines * h as f64
                && (st.h as f64) <= 1.15 * h as f64;
            if (st.h as f64) >= 0.7 * h as f64
                && (st.w as f64) <= 1.05 * h as f64
                && (st.h as f64) <= 1.4 * h as f64
                && !wide
            {
                cells.push(Cell {
                    digit: true,
                    x: st.x,
                    y: st.y,
                    w: st.w,
                    h: st.h,
                    shape: i as i32,
                });
            } else if (st.h as f64) >= 0.7 * h as f64 && wide && (st.w as f64) <= 1.6 * h as f64 {
                let whole = Cell {
                    digit: true,
                    x: st.x,
                    y: st.y,
                    w: st.w,
                    h: st.h,
                    shape: i as i32,
                };
                cells.extend(self.split_or_whole(
                    &strip,
                    strip_height,
                    width,
                    h,
                    small,
                    &labels,
                    whole,
                ));
            } else if (st.h as f64) >= 0.25 * h as f64
                && (st.h as f64) <= 0.6 * h as f64
                && (st.w as f64) <= 0.45 * h as f64
                && (st.y as f64) >= 0.55 * h as f64
            {
                cells.push(Cell {
                    digit: false,
                    x: st.x,
                    y: st.y,
                    w: st.w,
                    h: st.h,
                    shape: i as i32,
                });
            }
        }

        cells.sort_by_key(|c| c.x);
        let mut runs: Vec<Vec<Cell>> = Vec::new();
        let mut current: Vec<Cell> = Vec::new();
        for c in cells {
            if let Some(last) = current.last() {
                if (c.x - (last.x + last.w)) as f64 > 0.75 * h as f64 {
                    runs.push(std::mem::take(&mut current));
                }
            }
            current.push(c);
        }
        if !current.is_empty() {
            runs.push(current);
        }

        for run in runs {
            let mut g: Vec<Cell> = run.into_iter().skip_while(|c| !c.digit).collect();
            while g.last().is_some_and(|c| !c.digit) {
                g.pop();
            }
            let distinct_shapes: HashSet<i32> =
                g.iter().filter(|c| c.digit).map(|c| c.shape).collect();
            if distinct_shapes.len() < 2 {
                continue;
            }
            let left = g[0].x;
            let right = g.last().unwrap().x + g.last().unwrap().w;
            let g_top = g.iter().filter(|c| c.digit).map(|c| c.y).min().unwrap();
            let g_bottom = g
                .iter()
                .filter(|c| c.digit)
                .map(|c| c.y + c.h)
                .max()
                .unwrap();
            let gh = g_bottom - g_top;
            let symbol = Self::find_symbol(
                &ink.data,
                width,
                height,
                left,
                ya as i32 + g_top,
                ya as i32 + g_bottom,
                gh,
                lh,
                threshold,
            );
            let condensed =
                symbol.is_some_and(|sb| self.is_condensed(bgr, ink, sb, threshold, &g));
            if condensed {
                g = self.recut(
                    &strip,
                    strip_height,
                    width,
                    h,
                    small,
                    &labels,
                    &stats,
                    left,
                    right,
                );
            }
            groups.push(Group {
                cells: g,
                strip: strip.clone(),
                strip_height,
                threshold,
                row_height: h,
                small,
                condensed,
                left,
                right,
                top: ya as i32 + g_top,
                height: gh,
                symbol,
                scores: Vec::new(),
                all_scores: Vec::new(),
            });
        }
        groups
    }

    #[allow(clippy::too_many_arguments)]
    fn split_or_whole(
        &self,
        strip: &[u8],
        strip_height: usize,
        width: usize,
        h: i32,
        small: bool,
        labels: &[i32],
        whole: Cell,
    ) -> Vec<Cell> {
        let parts = Self::split_cell(labels, width, whole);
        if (whole.w as f64) > self.settings.split_whole_max_lines * h as f64 {
            return parts;
        }
        if parts.len() != 2 {
            return vec![whole];
        }
        let one = DigitTemplates::best_score(
            &self.patch(strip, strip_height, h, whole, width, false),
            small,
        );
        let two = parts
            .iter()
            .map(|&p| {
                DigitTemplates::best_score(
                    &self.patch(strip, strip_height, h, p, width, false),
                    small,
                )
            })
            .fold(f64::INFINITY, f64::min);
        if two > one {
            parts
        } else {
            vec![whole]
        }
    }

    fn split_cell(labels: &[i32], width: usize, whole: Cell) -> Vec<Cell> {
        let w = whole.w as usize;
        let hh = whole.h as usize;
        let mut cols = vec![0usize; w];
        for r in 0..hh {
            for c in 0..w {
                if labels[(whole.y as usize + r) * width + whole.x as usize + c] == whole.shape {
                    cols[c] += 1;
                }
            }
        }
        let lo = (0.3 * w as f64) as usize;
        let hi = ((0.7 * w as f64).round() as usize).max(lo + 1).min(w);
        let mut cut = lo.min(w.saturating_sub(1));
        for c in lo..hi {
            if cols[c] < cols[cut] {
                cut = c;
            }
        }
        let mut out = Vec::new();
        for (from, to) in [(0usize, cut), (cut + 1, w)] {
            if from >= to {
                continue;
            }
            let mut min_x = i32::MAX;
            let mut max_x = -1i32;
            let mut min_y = i32::MAX;
            let mut max_y = -1i32;
            for r in 0..hh {
                for c in from..to {
                    if labels[(whole.y as usize + r) * width + whole.x as usize + c] == whole.shape
                    {
                        min_x = min_x.min(c as i32);
                        max_x = max_x.max(c as i32);
                        min_y = min_y.min(r as i32);
                        max_y = max_y.max(r as i32);
                    }
                }
            }
            if max_x >= 0 {
                out.push(Cell {
                    digit: true,
                    x: whole.x + min_x,
                    y: whole.y + min_y,
                    w: max_x - min_x + 1,
                    h: max_y - min_y + 1,
                    shape: whole.shape,
                });
            }
        }
        out
    }

    fn is_condensed(
        &self,
        bgr: &BgrImage,
        ink: &GrayImage,
        symbol: SymbolBox,
        threshold: f64,
        cells: &[Cell],
    ) -> bool {
        let mut b_sum = 0u64;
        let mut g_sum = 0u64;
        let mut r_sum = 0u64;
        let mut n = 0usize;
        let w = ink.width;
        for y in symbol.y.max(0)..(symbol.y + symbol.height).min(ink.height as i32) {
            for x in symbol.x.max(0)..(symbol.x + symbol.width).min(w as i32) {
                let idx = y as usize * w + x as usize;
                if (ink.data[idx] as f64) <= threshold {
                    continue;
                }
                b_sum += bgr.data[idx * 3] as u64;
                g_sum += bgr.data[idx * 3 + 1] as u64;
                r_sum += bgr.data[idx * 3 + 2] as u64;
                n += 1;
            }
        }
        if n < self.settings.condensed_min_ink_pixels {
            return false;
        }
        let mb = b_sum as f64 / n as f64;
        let mg = g_sum as f64 / n as f64;
        let mr = r_sum as f64 / n as f64;
        if mr < self.settings.condensed_min_red
            || mg >= self.settings.condensed_green_share * mr
            || mb >= self.settings.condensed_blue_share * mr
        {
            return false;
        }
        let mut shares: Vec<f64> = cells
            .iter()
            .filter(|c| c.digit)
            .map(|c| c.w as f64 / c.h as f64)
            .filter(|&s| {
                s > self.settings.condensed_shape_min_share
                    && s < self.settings.condensed_shape_max_share
            })
            .collect();
        shares.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        !shares.is_empty() && shares[(shares.len() - 1) / 4] < self.settings.condensed_narrow_share
    }

    #[allow(clippy::too_many_arguments)]
    fn recut(
        &self,
        strip: &[u8],
        strip_height: usize,
        width: usize,
        h: i32,
        small: bool,
        labels: &[i32],
        stats: &[ComponentStats],
        left: i32,
        right: i32,
    ) -> Vec<Cell> {
        let mut cells = Vec::new();
        for (i, st) in stats.iter().enumerate().skip(1) {
            if st.area < 2 || st.x < left - 1 || st.x + st.w > right + 1 {
                continue;
            }
            if (st.h as f64) >= 0.7 * h as f64
                && (st.h as f64) <= 1.4 * h as f64
                && (st.w as f64) <= self.settings.condensed_max_shape_lines * h as f64
            {
                let whole = Cell {
                    digit: true,
                    x: st.x,
                    y: st.y,
                    w: st.w,
                    h: st.h,
                    shape: i as i32,
                };
                cells.extend(self.cut_condensed(
                    strip,
                    strip_height,
                    width,
                    h,
                    small,
                    labels,
                    whole,
                ));
            } else if (st.h as f64) >= 0.25 * h as f64
                && (st.h as f64) <= 0.6 * h as f64
                && (st.w as f64) <= 0.45 * h as f64
                && (st.y as f64) >= 0.55 * h as f64
            {
                cells.push(Cell {
                    digit: false,
                    x: st.x,
                    y: st.y,
                    w: st.w,
                    h: st.h,
                    shape: i as i32,
                });
            }
        }
        cells.sort_by_key(|c| c.x);
        cells
    }

    #[allow(clippy::too_many_arguments)]
    fn cut_condensed(
        &self,
        strip: &[u8],
        strip_height: usize,
        width: usize,
        h: i32,
        small: bool,
        labels: &[i32],
        whole: Cell,
    ) -> Vec<Cell> {
        let share = whole.w as f64 / whole.h as f64;
        if share <= self.settings.condensed_one_share {
            return vec![whole];
        }
        if share <= self.settings.condensed_pair_share {
            let parts = Self::cut_into(labels, width, whole, 2, self.settings.condensed_cut_reach);
            if parts.len() != 2 {
                return vec![whole];
            }
            let one = DigitTemplates::best_score(
                &self.patch(strip, strip_height, h, whole, width, true),
                small,
            );
            let two = parts
                .iter()
                .map(|&p| {
                    DigitTemplates::best_score(
                        &self.patch(strip, strip_height, h, p, width, true),
                        small,
                    )
                })
                .fold(f64::INFINITY, f64::min);
            return if two > one { parts } else { vec![whole] };
        }
        let n = ((whole.w as f64 + 1.0) / (self.settings.condensed_pitch_share * whole.h as f64))
            .round()
            .clamp(2.0, 5.0) as usize;
        Self::cut_into(labels, width, whole, n, self.settings.condensed_cut_reach)
    }

    fn cut_into(labels: &[i32], width: usize, whole: Cell, n: usize, reach: f64) -> Vec<Cell> {
        let w = whole.w as usize;
        let hh = whole.h as usize;
        let mut cols = vec![0usize; w];
        for r in 0..hh {
            for c in 0..w {
                if labels[(whole.y as usize + r) * width + whole.x as usize + c] == whole.shape {
                    cols[c] += 1;
                }
            }
        }
        let mut cuts: Vec<usize> = Vec::new();
        for i in 1..n {
            let at = i as f64 * w as f64 / n as f64;
            let mut lo = ((at - reach * w as f64 / n as f64) as isize).max(1) as usize;
            if let Some(&last) = cuts.last() {
                lo = lo.max(last + 2);
            }
            if lo >= w {
                break;
            }
            let hi = (((at + reach * w as f64 / n as f64).round() as usize) + 1)
                .min(w)
                .max(lo + 1);
            let mut cut = lo;
            for c in lo..hi.min(w) {
                if cols[c] < cols[cut] {
                    cut = c;
                }
            }
            cuts.push(cut);
        }

        let mut edges = vec![-1isize];
        edges.extend(cuts.into_iter().map(|c| c as isize));
        edges.push(w as isize);

        let mut parts = Vec::new();
        for win in edges.windows(2) {
            let from = (win[0] + 1) as usize;
            let to = win[1] as usize;
            let mut min_x = i32::MAX;
            let mut max_x = -1i32;
            let mut min_y = i32::MAX;
            let mut max_y = -1i32;
            for r in 0..hh {
                for c in from..to.min(w) {
                    if labels[(whole.y as usize + r) * width + whole.x as usize + c] == whole.shape
                    {
                        min_x = min_x.min(c as i32);
                        max_x = max_x.max(c as i32);
                        min_y = min_y.min(r as i32);
                        max_y = max_y.max(r as i32);
                    }
                }
            }
            if max_x >= 0 {
                parts.push(Cell {
                    digit: true,
                    x: whole.x + min_x,
                    y: whole.y + min_y,
                    w: max_x - min_x + 1,
                    h: max_y - min_y + 1,
                    shape: whole.shape,
                });
            }
        }
        parts
    }

    #[allow(clippy::too_many_arguments)]
    fn find_symbol(
        ink: &[u8],
        width: usize,
        height: usize,
        left: i32,
        top: i32,
        bottom: i32,
        gh: i32,
        lh: f64,
        threshold: f64,
    ) -> Option<SymbolBox> {
        let sx0 = ((left as f64 - 2.6 * lh) as i32).max(0);
        let sx1 = (left as f64 - 0.3 * lh) as i32;
        let sy0 = ((top as f64 - 0.7 * gh as f64) as i32).max(0);
        let sy1 = ((bottom as f64 + 0.7 * gh as f64) as i32).min(height as i32);
        if sx1 <= sx0 || sy1 <= sy0 {
            return None;
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        let mut count = 0usize;
        for y in sy0..sy1 {
            for x in sx0..sx1.min(width as i32) {
                if (ink[y as usize * width + x as usize] as f64) <= threshold {
                    continue;
                }
                count += 1;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
        if (count as f64) < 0.4 * (gh as f64) * (gh as f64) {
            return None;
        }
        let sw = max_x - min_x + 1;
        let sh = max_y - min_y + 1;
        if sh >= gh && (sw as f64) >= 0.5 * gh as f64 && (sw as f64) <= 2.4 * gh as f64 {
            Some(SymbolBox {
                x: min_x,
                y: min_y,
                width: sw,
                height: sh,
            })
        } else {
            None
        }
    }

    fn classify(&self, group: &mut Group, width: usize) -> String {
        let mut text = String::new();
        group.scores.clear();
        group.all_scores.clear();
        for &cell in &group.cells {
            if !cell.digit {
                text.push(',');
                continue;
            }
            let p = self.patch(
                &group.strip,
                group.strip_height,
                group.row_height,
                cell,
                width,
                group.condensed,
            );
            let (digit, sc, all) =
                DigitTemplates::match_patch(&p, self.settings.tie_margin, group.small);
            group.all_scores.push(all);
            text.push(digit);
            group.scores.push(sc);
        }
        text
    }

    fn patch(
        &self,
        strip: &[u8],
        strip_height: usize,
        h: i32,
        cell: Cell,
        width: usize,
        condensed: bool,
    ) -> Vec<f32> {
        let cw = if condensed {
            (self.settings.condensed_cell_lines * h as f64).round() as i32 + 2
        } else if (cell.w as f64) > self.settings.split_width_lines * h as f64
            && (cell.h as f64) <= 1.15 * h as f64
        {
            cell.w
        } else {
            (0.62 * h as f64).round() as i32 + 2
        }
        .max(1) as usize;

        let x0 = (cell.x as f64 + cell.w as f64 / 2.0 - cw as f64 / 2.0).round() as i32;
        let y0 = cell.y - 1;
        let rows = (cell.h + 2).max(1) as usize;
        let mut sub = vec![0.0f32; rows * cw];

        for r in 0..rows {
            let y = y0 + r as i32;
            if y < 0 || y >= strip_height as i32 {
                continue;
            }
            for c in 0..cw {
                let x = x0 + c as i32;
                if x >= 0 && x < width as i32 {
                    sub[r * cw + c] = strip[y as usize * width + x as usize] as f32;
                }
            }
        }

        let mut sorted = sub.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pos = 0.95 * (sorted.len().saturating_sub(1)) as f64;
        let lo = pos.floor() as usize;
        let hi = (lo + 1).min(sorted.len().saturating_sub(1));
        let p95 = sorted[lo] as f64 + (sorted[hi] as f64 - sorted[lo] as f64) * (pos - lo as f64);
        let denom = (p95 + 1e-3) as f32;
        for v in &mut sub {
            *v = (*v / denom).clamp(0.0, 1.0);
        }

        resize_linear_f32(&sub, cw, rows, DigitTemplates::WIDTH, DigitTemplates::HEIGHT)
    }

    fn colours(group: &Group, bgr: &BgrImage, ink: &GrayImage) -> (Bgr, Bgr) {
        let mut tb = 0u64;
        let mut tg = 0u64;
        let mut tr = 0u64;
        let mut tn = 0u64;
        let mut back_b = Vec::new();
        let mut back_g = Vec::new();
        let mut back_r = Vec::new();

        let top = (group.top - group.height / 3).max(0) as usize;
        let bottom = (group.top + group.height + group.height / 3)
            .clamp(0, bgr.height as i32) as usize;
        let left = group.left.max(0) as usize;
        let right = group.right.clamp(0, bgr.width as i32) as usize;

        for y in top..bottom {
            for x in left..right {
                let idx = y * bgr.width + x;
                let iv = ink.data[idx] as f64;
                if iv > group.threshold {
                    tb += bgr.data[idx * 3] as u64;
                    tg += bgr.data[idx * 3 + 1] as u64;
                    tr += bgr.data[idx * 3 + 2] as u64;
                    tn += 1;
                } else if iv < group.threshold * 0.5 {
                    back_b.push(bgr.data[idx * 3]);
                    back_g.push(bgr.data[idx * 3 + 1]);
                    back_r.push(bgr.data[idx * 3 + 2]);
                }
            }
        }

        let text = if tn == 0 {
            Bgr::new(255, 255, 255)
        } else {
            Bgr::new((tb / tn) as u8, (tg / tn) as u8, (tr / tn) as u8)
        };
        if back_b.is_empty() {
            return (text, Bgr::new(0, 0, 0));
        }
        back_b.sort_unstable();
        back_g.sort_unstable();
        back_r.sort_unstable();
        let mid = back_b.len() / 2;
        (text, Bgr::new(back_b[mid], back_g[mid], back_r[mid]))
    }
}
