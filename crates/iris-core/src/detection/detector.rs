use crate::config::social::SocialConfig;
use crate::detection::features::{LayerCheck, LineMask, NameplateFeatures};
use crate::detection::image_ops::{
    bounding_rect_nonzero, box_blur, connected_components_with_stats, dilate_rect,
    morph_tophat_rect, resize_area_u8, resize_linear_u8, BgrImage, ComponentStats, GrayImage,
};
use crate::detection::reader::{NameplateReader, PieceRead};
use crate::detection::rules::{
    DistanceLineRule, DistanceReading, LookAlikes, O0Glyph, O0Rule, PlateColorSettings,
    PlateColors, UnderscoreHyphenRule,
};
use crate::models::{IntRect, PlateColor};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

pub struct NameShape;

impl NameShape {
    pub fn has_valid_edges(name: &str) -> bool {
        !name.is_empty() && !name.starts_with('-') && !name.ends_with('-')
    }
}

pub struct NameFilter;

impl NameFilter {
    pub fn is_on_player_list(handle: &str, config: &SocialConfig) -> bool {
        config
            .players
            .iter()
            .any(|p| LookAlikes::names_equal(&p.handle, handle))
    }
}

#[derive(Debug, Clone, Default)]
pub struct NameRowRead {
    pub text: String,
    pub score: f64,
    pub one_word: bool,
    pub valid_chars: bool,
    pub mask: Option<LineMask>,
    pub o0: Vec<O0Glyph>,
}

impl NameRowRead {
    pub fn usable(&self) -> bool {
        self.mask.is_some() && !self.text.is_empty() && self.one_word && self.valid_chars
    }
}

#[derive(Debug, Clone)]
pub struct PlateStack {
    pub rows: Vec<IntRect>,
    pub blockers: Vec<IntRect>,
}

#[derive(Debug, Clone)]
pub struct CrowdPair {
    pub top: usize,
    pub bottom: usize,
    pub neighbours: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct NameplateCandidates {
    pub boxes: Vec<IntRect>,
    pub pairs: Vec<(usize, usize)>,
    pub loose_pairs: Vec<(usize, usize)>,
    pub crowd_pairs: Vec<CrowdPair>,
    pub stacks: HashMap<(IntRect, IntRect), PlateStack>,
}

impl NameplateCandidates {
    pub fn stack_of(&self, top: IntRect, bottom: IntRect) -> Option<&PlateStack> {
        self.stacks.get(&(top, bottom))
    }
}

#[derive(Debug, Clone)]
pub struct NameplateDetectorSettings {
    pub region_left: f64,
    pub region_right: f64,
    pub region_top: f64,
    pub region_bottom: f64,
    pub reference_height: i32,
    pub scale_height: i32,
    pub min_name_length: usize,
    pub max_name_length: usize,
    pub min_name_score: f64,
    pub min_distance_score: f64,
    pub max_distance_gap: f64,
    pub distance_end_score: f64,
    pub max_distance_value: i32,
    pub distance_min_ink: f64,
    pub word_gap: f64,
    pub clear_lines: f64,
    pub crowd_exception: bool,
    pub speck_area: f64,
    pub center_tolerance: f64,
    pub left_tolerance: f64,
    pub left_width_difference: f64,
    pub max_rows: usize,
    pub row_gap_max: f64,
    pub row_pitch_min: f64,
    pub row_pitch_max: f64,
    pub row_min_caps: f64,
    pub stack_tint_tolerance: f64,
    pub narrow_min_width: f64,
    pub narrow_max_lines: f64,
    pub sub_pixel_o0_rule: bool,
    pub underscore_hyphen_rule: bool,
    pub plate_color_rules: PlateColorSettings,
    pub speed_join_gap: f64,
    pub speed_join_max_lines: f64,
    pub bracket_min_score: f64,

    pub read_crop_min_height: f64,
    pub read_crop_min_width: f64,
    pub near_crop_min_height: f64,
    pub near_crop_min_width: f64,
    pub near_height_tolerance: f64,
    pub near_height_min_px: f64,
    pub near_slack_px: f64,
    pub near_max_tries: usize,
    pub anchor_max_offset_px: f64,
    pub anchor_max_row_offset_lines: f64,
    pub top_gap_max_lines: f64,
    pub top_height_diff_min_px: f64,
    pub top_height_diff_lines: f64,
    pub top_centre_tolerance_px: f64,
    pub top_left_tolerance_px: f64,
    pub top_left_width_difference_px: f64,

    pub top_hat_kernel_px: f64,
    pub top_hat_min: f64,
    pub green_top_hat_min: f64,
    pub cyan_blue_min: f64,
    pub cyan_green_min: f64,
    pub white_any_tint: bool,
    pub white_value_min: f64,
    pub red_min: f64,
    pub red_over_green_min: f64,
    pub red_over_blue_min: f64,
    pub green_min: f64,
    pub green_over_red_min: f64,
    pub green_over_blue_min: f64,

    pub contrast_mask: bool,
    pub bg_dark_max: f64,
    pub bg_full_contrast: f64,
    pub contrast_hat_max: f64,
    pub background_shrink_px: f64,
    pub background_blur: usize,

    pub line_max_height: f64,
    pub line_min_height: f64,
    pub line_min_width: f64,
    pub line_aspect_min: f64,
    pub join_width_px: f64,
    pub fill_min: f64,
    pub fill_max: f64,

    pub height_tolerance_px: f64,
    pub height_tolerance_lines: f64,
    pub row_tolerance_px: f64,
    pub row_tolerance_lines: f64,
    pub row_join_top_px: f64,
    pub row_join_height_px: f64,
    pub row_join_gap_px: f64,
}

impl Default for NameplateDetectorSettings {
    fn default() -> Self {
        Self {
            region_left: 0.15,
            region_right: 0.85,
            region_top: 0.10,
            region_bottom: 0.70,
            reference_height: 1440,
            scale_height: 0,
            min_name_length: 4,
            max_name_length: 64,
            min_name_score: -0.20,
            min_distance_score: -0.35,
            max_distance_gap: 4.0,
            distance_end_score: -1.0,
            max_distance_value: 1000,
            distance_min_ink: 0.003,
            word_gap: 0.33,
            clear_lines: 3.0,
            crowd_exception: true,
            speck_area: 4.0,
            center_tolerance: 5.0,
            left_tolerance: 2.0,
            left_width_difference: 8.0,
            max_rows: 3,
            row_gap_max: 0.8,
            row_pitch_min: 1.05,
            row_pitch_max: 1.8,
            row_min_caps: 11.0,
            stack_tint_tolerance: 0.10,
            narrow_min_width: 6.0,
            narrow_max_lines: 2.2,
            sub_pixel_o0_rule: true,
            underscore_hyphen_rule: true,
            plate_color_rules: PlateColorSettings::default(),
            speed_join_gap: 1.5,
            speed_join_max_lines: 8.0,
            bracket_min_score: -0.7,

            read_crop_min_height: 30.0,
            read_crop_min_width: 60.0,
            near_crop_min_height: 12.0,
            near_crop_min_width: 30.0,
            near_height_tolerance: 0.35,
            near_height_min_px: 2.0,
            near_slack_px: 3.0,
            near_max_tries: 3,
            anchor_max_offset_px: 8.0,
            anchor_max_row_offset_lines: 0.8,
            top_gap_max_lines: 1.4,
            top_height_diff_min_px: 4.0,
            top_height_diff_lines: 0.45,
            top_centre_tolerance_px: 5.0,
            top_left_tolerance_px: 2.0,
            top_left_width_difference_px: 8.0,

            top_hat_kernel_px: 11.0,
            top_hat_min: 28.0,
            green_top_hat_min: 20.0,
            cyan_blue_min: 150.0,
            cyan_green_min: 140.0,
            white_any_tint: true,
            white_value_min: 150.0,
            red_min: 140.0,
            red_over_green_min: 80.0,
            red_over_blue_min: 60.0,
            green_min: 90.0,
            green_over_red_min: 35.0,
            green_over_blue_min: 30.0,

            contrast_mask: true,
            bg_dark_max: 100.0,
            bg_full_contrast: 180.0,
            contrast_hat_max: 45.0,
            background_shrink_px: 4.0,
            background_blur: 15,

            line_max_height: 22.0,
            line_min_height: 10.0,
            line_min_width: 24.0,
            line_aspect_min: 1.5,
            join_width_px: 7.0,
            fill_min: 0.12,
            fill_max: 0.75,

            height_tolerance_px: 4.0,
            height_tolerance_lines: 0.45,
            row_tolerance_px: 3.0,
            row_tolerance_lines: 0.25,
            row_join_top_px: 2.0,
            row_join_height_px: 3.0,
            row_join_gap_px: 4.0,
        }
    }
}

impl NameplateDetectorSettings {
    pub const MAX_LINE_WIDTH: f64 = 832.0;
    pub const PAIR_GAP_MAX_LINES: f64 = 1.4;
    pub const MAX_PIECE_CAPS: f64 = 8.0;
    pub const MAX_SPLIT_SPANS: usize = 1500;
    pub const NAME_MIN_INK: f64 = 0.02;
    pub const O0_DECIDE: f64 = 0.95;
    pub const BAR_UNDERSCORE_MIN: f64 = 0.87;
    pub const BAR_HYPHEN_MAX: f64 = 0.71;
    pub const BAR_MAX_THICK: f64 = 0.45;
    pub const BAR_ROW_FRACTION: f64 = 0.25;
    pub const EVIDENCE_MAX_KEYS: usize = 16;
    pub const EVIDENCE_LLR_CLIP: f64 = 4.0;
    pub const EVIDENCE_DECIDE_ODDS: f64 = 19.0;
    pub const EVIDENCE_DECIDE_LLR: f64 = 2.9444389791664403;
    pub const PLATE_COLOR_MIN_PIXELS: usize = 30;
    pub const PLATE_MASK_MIN_PIXELS: usize = 8;

    pub fn effective_scale_height(&self, image_height: i32) -> i32 {
        if self.scale_height > 0 {
            self.scale_height
        } else {
            image_height
        }
    }

    pub fn scale(&self, frame_height: i32, value: f64) -> i32 {
        ((value * frame_height as f64 / self.reference_height as f64 + 0.5).floor() as i32).max(1)
    }

    pub fn context_padding(&self, scale_height: i32) -> i32 {
        3 * self.scale(scale_height, self.line_max_height)
            + self.scale(scale_height, self.height_tolerance_px)
            + 2
    }

    pub fn vertical_context_padding(&self, scale_height: i32) -> i32 {
        self.scale(scale_height, self.top_hat_kernel_px) + 2
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if !(0.0 <= self.region_left
            && self.region_left < self.region_right
            && self.region_right <= 1.0)
        {
            return Err("The scan region needs 0 <= left < right <= 1.");
        }
        if !(0.0 <= self.region_top
            && self.region_top < self.region_bottom
            && self.region_bottom <= 1.0)
        {
            return Err("The scan region needs 0 <= top < bottom <= 1.");
        }
        if self.reference_height <= 0 {
            return Err("ReferenceHeight must be positive.");
        }
        if self.scale_height < 0 {
            return Err("ScaleHeight must not be negative.");
        }
        if self.min_name_length < 1 || self.max_name_length < self.min_name_length {
            return Err("The name length limits are not usable.");
        }
        Ok(())
    }
}

pub struct NameplateDetector {
    reader: NameplateReader,
    settings: NameplateDetectorSettings,
    distance_alphabet: Vec<bool>,
    name_alphabet: Vec<bool>,
    hat_limits: [u8; 256],

    mask: GrayImage,
    kept: GrayImage,
    region_buf: BgrImage,
    ch_b: GrayImage,
    ch_g: GrayImage,
    ch_r: GrayImage,
    ch_val: GrayImage,
    cw_mask: GrayImage,
    r_mask: GrayImage,
    g_mask: GrayImage,
    mask_labels: Vec<i32>,
    mask_stats: Vec<ComponentStats>,
    solid: Vec<bool>,
    tall: Vec<bool>,
    pub last_mask_ms: f64,
}

impl NameplateDetector {
    pub const NAME_ALPHABET: &'static str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

    pub fn new(reader: NameplateReader, settings: NameplateDetectorSettings) -> Self {
        settings.validate().expect("Invalid NameplateDetectorSettings");
        let distance_alphabet = reader.create_alphabet(&DistanceLineRule::alphabet());
        let name_alphabet = reader.create_alphabet(Self::NAME_ALPHABET);
        let hat_limits = Self::build_hat_limits(&settings);
        Self {
            reader,
            settings,
            distance_alphabet,
            name_alphabet,
            hat_limits,
            mask: GrayImage::default(),
            kept: GrayImage::default(),
            region_buf: BgrImage::default(),
            ch_b: GrayImage::default(),
            ch_g: GrayImage::default(),
            ch_r: GrayImage::default(),
            ch_val: GrayImage::default(),
            cw_mask: GrayImage::default(),
            r_mask: GrayImage::default(),
            g_mask: GrayImage::default(),
            mask_labels: Vec::new(),
            mask_stats: Vec::new(),
            solid: Vec::new(),
            tall: Vec::new(),
            last_mask_ms: 0.0,
        }
    }

    pub fn settings(&self) -> &NameplateDetectorSettings {
        &self.settings
    }

    pub fn settings_mut(&mut self) -> &mut NameplateDetectorSettings {
        &mut self.settings
    }

    pub fn update_settings(&mut self, settings: NameplateDetectorSettings) {
        if settings.validate().is_ok() {
            self.hat_limits = Self::build_hat_limits(&settings);
            self.settings = settings;
        }
    }

    pub fn reader(&self) -> &NameplateReader {
        &self.reader
    }

    pub fn mask(&self) -> &GrayImage {
        &self.mask
    }

    fn scaled(&self, frame_height: i32, value: f64) -> i32 {
        self.settings.scale(frame_height, value)
    }

    fn scaled_odd(&self, frame_height: i32, value: f64) -> usize {
        let k = self.scaled(frame_height, value) as usize;
        if k % 2 == 1 {
            k
        } else {
            k + 1
        }
    }

    fn build_hat_limits(settings: &NameplateDetectorSettings) -> [u8; 256] {
        let mut limits = [0u8; 256];
        for (bg, out) in limits.iter_mut().enumerate() {
            let t = settings.top_hat_min
                + (bg as f64 - settings.bg_dark_max)
                    / (settings.bg_full_contrast - settings.bg_dark_max)
                    * (settings.contrast_hat_max - settings.top_hat_min);
            let clamped = t.clamp(settings.top_hat_min, settings.contrast_hat_max);
            *out = clamped.floor().clamp(0.0, 255.0) as u8;
        }
        limits
    }

    pub fn find_candidates(&mut self, bgr: &BgrImage) -> NameplateCandidates {
        if bgr.is_empty() {
            return NameplateCandidates::default();
        }
        let started = Instant::now();
        self.last_mask_ms = 0.0;

        let width = bgr.width as i32;
        let height = bgr.height as i32;
        let scale_height = self.settings.effective_scale_height(height);
        let x0 = (self.settings.region_left * width as f64) as i32;
        let x1 = ((self.settings.region_right * width as f64) as i32).min(width);
        let y0 = (self.settings.region_top * height as f64) as i32;
        let y1 = ((self.settings.region_bottom * height as f64) as i32).min(height);
        if x1 <= x0 || y1 <= y0 {
            return NameplateCandidates::default();
        }

        let pad = self.settings.context_padding(scale_height);
        let px0 = (x0 - pad).max(0);
        let px1 = (x1 + pad).min(width);

        let mut region = std::mem::take(&mut self.region_buf);
        bgr.crop_into(IntRect::new(px0, y0, px1 - px0, y1 - y0), &mut region);
        self.build_color_mask(&region, scale_height);
        self.region_buf = region;
        self.last_mask_ms = started.elapsed().as_secs_f64() * 1000.0;

        let mut narrow_shapes = Vec::new();
        let lines = self.find_text_lines(scale_height, Some(&mut narrow_shapes));
        if lines.is_empty() {
            return NameplateCandidates::default();
        }

        let plain: Vec<IntRect> = lines
            .into_iter()
            .filter(|line| x0 <= line.x + px0 && line.x + line.width + px0 <= x1)
            .collect();
        if plain.is_empty() {
            return NameplateCandidates::default();
        }

        let plain_count = plain.len();
        let unions = self.speed_unions(&plain, scale_height);

        let narrow_inside: Vec<IntRect> = narrow_shapes
            .into_iter()
            .filter(|s| x0 <= s.x + px0 && s.x + s.width + px0 <= x1)
            .collect();

        let mut row_pool_cache: Option<Vec<IntRect>> = None;
        let promoted = if !narrow_inside.is_empty() {
            let pool = row_pool_cache
                .get_or_insert_with(|| self.row_candidates(&plain, plain_count, scale_height))
                .clone();
            self.promote_narrow(&narrow_inside, &plain, &unions, scale_height, &pool)
        } else {
            Vec::new()
        };

        let mut local = plain.clone();
        for (narrow_row, _) in &promoted {
            local.push(*narrow_row);
        }
        let real_count = plain_count + promoted.len();
        local.extend_from_slice(&unions);

        let mut all_pairs = self.pair_lines(&local, scale_height, real_count);
        if !promoted.is_empty() {
            let mut sandwich = HashSet::new();
            for (narrow_row, bottoms) in &promoted {
                for b in bottoms {
                    sandwich.insert((*narrow_row, *b));
                }
            }
            all_pairs.retain(|&(top, bottom)| {
                !(bottom >= plain_count && bottom < real_count)
                    && !(top >= plain_count
                        && top < real_count
                        && !sandwich.contains(&(local[top], local[bottom])))
            });
        }

        let pool = row_pool_cache
            .get_or_insert_with(|| self.row_candidates(&plain, plain_count, scale_height));
        let stacks_local = self.find_stacks(&local, &all_pairs, pool, scale_height);

        let mut clear_known = vec![0i8; local.len()];
        let (pairs, loose_pairs, crowd_pairs) =
            self.sort_pairs(&local, &mut clear_known, &all_pairs, real_count);

        let shift = |r: IntRect| IntRect::new(r.x + px0, r.y + y0, r.width, r.height);
        let boxes: Vec<IntRect> = local.iter().copied().map(shift).collect();

        let mut stacks = HashMap::new();
        for ((t, b), (rows, blockers)) in stacks_local {
            stacks.insert(
                (shift(t), shift(b)),
                PlateStack {
                    rows: rows.into_iter().map(shift).collect(),
                    blockers: blockers.into_iter().map(shift).collect(),
                },
            );
        }

        NameplateCandidates {
            boxes,
            pairs,
            loose_pairs,
            crowd_pairs,
            stacks,
        }
    }

    pub fn find_lines(
        &mut self,
        bgr: &BgrImage,
        with_speed_unions: bool,
        with_narrow_rows: bool,
    ) -> (Vec<IntRect>, usize, usize) {
        if bgr.is_empty() {
            return (Vec::new(), 0, 0);
        }
        let scale_height = self.settings.effective_scale_height(bgr.height as i32);
        let started = Instant::now();
        self.build_color_mask(bgr, scale_height);
        self.last_mask_ms = started.elapsed().as_secs_f64() * 1000.0;

        let mut narrow = Vec::new();
        let mut lines = self.find_text_lines(
            scale_height,
            if with_narrow_rows {
                Some(&mut narrow)
            } else {
                None
            },
        );
        let real_count = lines.len();
        if with_speed_unions && lines.len() > 1 {
            let u = self.speed_unions(&lines, scale_height);
            lines.extend(u);
        }
        let mut narrow_count = 0usize;
        if with_narrow_rows && !narrow.is_empty() {
            let ht = self.scaled(scale_height, self.settings.height_tolerance_px);
            let ct = self.scaled(scale_height, self.settings.center_tolerance);
            let lt = self.scaled(scale_height, self.settings.left_tolerance);
            let lwd = self.scaled(scale_height, self.settings.left_width_difference);
            let by_top = LinesByTop::new(&lines, None);
            let mut near = Vec::new();
            for shape in narrow {
                by_top.below(shape, &mut near);
                let found = near
                    .iter()
                    .any(|&i| self.pair_ok(shape, lines[i], ht, ct, lt, lwd));
                if found && self.narrow_fill_ok(shape) {
                    lines.push(shape);
                    narrow_count += 1;
                }
            }
        }
        (lines, real_count, narrow_count)
    }

    pub fn is_narrow_line(&self, width: f64, height: f64, frame_height: i32) -> bool {
        !(width >= self.scaled(frame_height, self.settings.line_min_width) as f64
            && width > height * self.settings.line_aspect_min)
    }

    pub fn column_ink(&self, box_rect: IntRect) -> Vec<i32> {
        let w = box_rect.width.max(0) as usize;
        let mut columns = vec![0i32; w];
        if self.mask.is_empty() || w == 0 {
            return columns;
        }
        let top = box_rect.y.max(0) as usize;
        let bottom = box_rect.bottom().min(self.mask.height as i32).max(0) as usize;
        let from = box_rect.x.max(0) as usize;
        let to = box_rect.right().min(self.mask.width as i32).max(0) as usize;
        if to <= from || bottom <= top {
            return columns;
        }
        for y in top..bottom {
            let row = &self.mask.data[y * self.mask.width..];
            for x in from..to {
                if row[x] != 0 {
                    columns[x - box_rect.x as usize] += 1;
                }
            }
        }
        columns
    }

    pub fn ink_colour(&self, bgr: &BgrImage, box_rect: IntRect) -> Option<PlateColor> {
        if self.mask.is_empty()
            || bgr.is_empty()
            || bgr.width != self.mask.width
            || bgr.height != self.mask.height
        {
            return None;
        }
        let r = box_rect.clamp_to(self.mask.width as i32, self.mask.height as i32);
        if r.is_empty() {
            return None;
        }
        let mut colours = Vec::new();
        for y in (r.y as usize)..(r.bottom() as usize) {
            let mrow = &self.mask.data[y * self.mask.width..];
            let crow = &bgr.data[y * bgr.width * 3..];
            for x in (r.x as usize)..(r.right() as usize) {
                if mrow[x] != 0 {
                    colours.push(crow[x * 3]);
                    colours.push(crow[x * 3 + 1]);
                    colours.push(crow[x * 3 + 2]);
                }
            }
        }
        let count = colours.len() / 3;
        if count == 0 {
            return None;
        }
        let ink = LineMask {
            height: 1,
            width: count,
            data: vec![0u8; count],
            origin_x: 0,
            origin_y: 0,
            column_offset: 0,
            row_offset: 0,
            ink_colors: Some(colours),
        };
        PlateColors::classify(std::iter::once(&ink), &self.settings.plate_color_rules)
            .map(|r| r.color)
    }

    pub fn read_distance_line(
        &self,
        bgr: &BgrImage,
        box_rect: IntRect,
        layers: Option<&mut LayerCheck<'_>>,
    ) -> Option<DistanceReading> {
        let mask = NameplateFeatures::create_mask(bgr, box_rect, None, 2, false, layers)?;
        self.read_distance_from_mask(mask)
    }

    pub fn read_distance_from_mask(&self, mask: LineMask) -> Option<DistanceReading> {
        let read = self.reader.read_pieces_from_mask(
            mask,
            Some(&self.distance_alphabet),
            self.settings.distance_min_ink,
        );
        let mask_ref = read.mask.as_ref()?;
        DistanceLineRule::evaluate(&read, &self.settings, |piece| {
            self.reader.is_open_bracket(
                mask_ref,
                piece.start,
                piece.end,
                self.settings.bracket_min_score,
            )
        })
    }

    pub fn read_name_row(
        &self,
        bgr: &BgrImage,
        box_rect: IntRect,
        layers: Option<&mut LayerCheck<'_>>,
    ) -> NameRowRead {
        let mask = NameplateFeatures::create_mask(bgr, box_rect, None, 2, true, layers);
        self.read_name_row_from_mask(bgr, mask)
    }

    pub fn reads_as_name_not_distance(
        &self,
        bgr: &BgrImage,
        box_rect: IntRect,
        layers: Option<&mut LayerCheck<'_>>,
    ) -> bool {
        let Some(mask) = NameplateFeatures::create_mask(bgr, box_rect, None, 2, true, layers)
        else {
            return false;
        };
        let mask_clone = mask.clone();
        self.read_name_row_from_mask(bgr, Some(mask)).usable()
            && self.read_distance_from_mask(mask_clone).is_none()
    }

    pub fn read_name_row_from_mask(&self, bgr: &BgrImage, mask: Option<LineMask>) -> NameRowRead {
        let Some(mask) = mask else {
            return NameRowRead::default();
        };
        let read = self.reader.read_pieces_from_mask(
            mask,
            Some(&self.name_alphabet),
            NameplateDetectorSettings::NAME_MIN_INK,
        );
        if read.pieces.is_empty() {
            return NameRowRead::default();
        }
        let mut text: String = read.pieces.iter().map(|p| p.text.as_str()).collect();
        if text.is_empty() {
            return NameRowRead::default();
        }
        let one_word = self.is_one_word(&read);
        let valid_chars = text
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_' || ch == '-');
        let score: f64 = read.pieces.iter().map(|p| p.score).sum();
        if !one_word || !valid_chars {
            return NameRowRead {
                text,
                score,
                one_word,
                valid_chars,
                mask: read.mask,
                o0: Vec::new(),
            };
        }

        let mut o0 = Vec::new();
        if self.settings.sub_pixel_o0_rule {
            text = O0Rule::correct(bgr, &read, &text, Some(&mut o0));
        }
        if self.settings.underscore_hyphen_rule {
            text = UnderscoreHyphenRule::correct(&read, &text);
        }
        NameRowRead {
            text,
            score,
            one_word: true,
            valid_chars: true,
            mask: read.mask,
            o0,
        }
    }

    fn is_one_word(&self, read: &PieceRead) -> bool {
        for pair in read.pieces.windows(2) {
            if (pair[1].start.saturating_sub(pair[0].end)) as f64
                > self.settings.word_gap * read.cap_height as f64
            {
                return false;
            }
        }
        true
    }

    // --- Mask building (`NameplateDetector.Mask.cs`) ---

    fn build_color_mask(&mut self, region: &BgrImage, scale_height: i32) {
        let tophat = self.scaled_odd(scale_height, self.settings.top_hat_kernel_px);
        let mut blue = std::mem::take(&mut self.ch_b);
        let mut green = std::mem::take(&mut self.ch_g);
        let mut red = std::mem::take(&mut self.ch_r);
        region.split_channels_into(&mut blue, &mut green, &mut red);
        let w = region.width;
        let h = region.height;
        let n = w * h;

        let mut value = std::mem::take(&mut self.ch_val);
        value.ensure_size(w, h);
        if self.settings.white_any_tint || self.settings.contrast_mask {
            for i in 0..n {
                value.data[i] = blue.data[i].max(green.data[i]).max(red.data[i]);
            }
        } else {
            value.data[..n].fill(0);
        }

        let bright_info = if self.settings.contrast_mask {
            self.bright_area(&value, tophat as i32, scale_height)
        } else {
            None
        };
        let bright_rect = bright_info.as_ref().map(|(r, _, _)| *r);

        let mut cyan_white = std::mem::take(&mut self.cw_mask);
        cyan_white.ensure_size(w, h);
        let mut value_hat_cache: Option<(IntRect, GrayImage)> = None;

        if self.settings.white_any_tint {
            let thr = self.settings.white_value_min as u8;
            for i in 0..n {
                cyan_white.data[i] = if value.data[i] > thr { 255 } else { 0 };
            }
            value_hat_cache =
                self.apply_value_tophat(&value, &mut cyan_white, tophat, bright_rect);
        } else {
            let cb_min = self.settings.cyan_blue_min as u8;
            let cg_min = self.settings.cyan_green_min as u8;
            for i in 0..n {
                cyan_white.data[i] = if blue.data[i] > cb_min
                    && green.data[i] > cg_min
                    && red.data[i].saturating_sub(blue.data[i]) == 0
                {
                    255
                } else {
                    0
                };
            }
            self.apply_tophat(
                &blue,
                &mut cyan_white,
                tophat,
                self.settings.top_hat_min as u8,
            );
        }

        let mut red_mask = std::mem::take(&mut self.r_mask);
        red_mask.ensure_size(w, h);
        let r_min = self.settings.red_min as u8;
        let r_og = self.settings.red_over_green_min as u8;
        let r_ob = self.settings.red_over_blue_min as u8;
        for i in 0..n {
            red_mask.data[i] = if red.data[i] > r_min
                && red.data[i].saturating_sub(green.data[i]) > r_og
                && red.data[i].saturating_sub(blue.data[i]) > r_ob
            {
                255
            } else {
                0
            };
        }
        self.apply_tophat(
            &red,
            &mut red_mask,
            tophat,
            self.settings.top_hat_min as u8,
        );

        let mut green_mask = std::mem::take(&mut self.g_mask);
        green_mask.ensure_size(w, h);
        let g_min = self.settings.green_min as u8;
        let g_or = self.settings.green_over_red_min as u8;
        let g_ob = self.settings.green_over_blue_min as u8;
        for i in 0..n {
            green_mask.data[i] = if green.data[i] > g_min
                && green.data[i].saturating_sub(red.data[i]) > g_or
                && green.data[i].saturating_sub(blue.data[i]) > g_ob
            {
                255
            } else {
                0
            };
        }
        self.apply_tophat(
            &green,
            &mut green_mask,
            tophat,
            self.settings.green_top_hat_min as u8,
        );

        let mut mask = std::mem::take(&mut self.mask);
        mask.ensure_size(w, h);
        for i in 0..n {
            mask.data[i] = cyan_white.data[i] | red_mask.data[i] | green_mask.data[i];
        }

        if let Some((area, background, bright)) = bright_info {
            self.apply_contrast_rule(
                &value,
                &background,
                &bright,
                &mut mask,
                area,
                tophat,
                value_hat_cache.as_ref(),
            );
        }

        self.ch_b = blue;
        self.ch_g = green;
        self.ch_r = red;
        self.ch_val = value;
        self.cw_mask = cyan_white;
        self.r_mask = red_mask;
        self.g_mask = green_mask;
        self.mask = mask;
    }

    fn apply_value_tophat(
        &self,
        value: &GrayImage,
        cyan_white: &mut GrayImage,
        tophat: usize,
        bright: Option<IntRect>,
    ) -> Option<(IntRect, GrayImage)> {
        let bounds = bounding_rect_nonzero(cyan_white);
        if bounds.is_empty() {
            return None;
        }
        let area = Self::padded_rect(bounds, tophat as i32, cyan_white.width as i32, cyan_white.height as i32);
        let mut hat_area = area;
        if let Some(other) = bright {
            let u = Self::union_rect(area, other);
            if (u.width as i64) * (u.height as i64)
                <= (area.width as i64) * (area.height as i64)
                    + (other.width as i64) * (other.height as i64)
            {
                hat_area = u;
            }
        }
        let sub = value.crop(hat_area);
        let vhat = morph_tophat_rect(&sub, tophat, tophat);
        let thr = self.settings.top_hat_min as u8;
        let ox = (area.x - hat_area.x) as usize;
        let oy = (area.y - hat_area.y) as usize;
        for y in 0..(area.height as usize) {
            let cw_row = &mut cyan_white.data[(area.y as usize + y) * cyan_white.width + area.x as usize..];
            let h_row = &vhat.data[(oy + y) * vhat.width + ox..];
            for x in 0..(area.width as usize) {
                if h_row[x] <= thr {
                    cw_row[x] = 0;
                }
            }
        }
        Some((hat_area, vhat))
    }

    fn apply_tophat(&self, channel: &GrayImage, plain: &mut GrayImage, size: usize, min_val: u8) {
        let bounds = bounding_rect_nonzero(plain);
        if bounds.is_empty() {
            return;
        }
        let area = Self::padded_rect(bounds, size as i32, plain.width as i32, plain.height as i32);
        let sub = channel.crop(area);
        let hat = morph_tophat_rect(&sub, size, size);
        for y in 0..(area.height as usize) {
            let p_row = &mut plain.data[(area.y as usize + y) * plain.width + area.x as usize..];
            let h_row = &hat.data[y * hat.width..];
            for x in 0..(area.width as usize) {
                if h_row[x] <= min_val {
                    p_row[x] = 0;
                }
            }
        }
    }

    fn bright_area(
        &self,
        value: &GrayImage,
        tophat: i32,
        scale_height: i32,
    ) -> Option<(IntRect, GrayImage, GrayImage)> {
        let shrink = self.scaled(scale_height, self.settings.background_shrink_px).max(1) as usize;
        let small_w = (value.width + shrink - 1) / shrink;
        let small_h = (value.height + shrink - 1) / shrink;
        let bg_small = resize_area_u8(value, small_w.max(1), small_h.max(1));
        let bg_blur = box_blur(&bg_small, self.settings.background_blur);
        let brightest = bg_blur.data.iter().copied().max().unwrap_or(0);
        if (brightest as f64) <= self.settings.bg_dark_max {
            return None;
        }
        let background = resize_linear_u8(&bg_blur, value.width, value.height);
        let thr = self.settings.bg_dark_max as u8;
        let mut bright = GrayImage::new(value.width, value.height);
        for i in 0..bright.data.len() {
            bright.data[i] = if background.data[i] > thr { 255 } else { 0 };
        }
        let bounds = bounding_rect_nonzero(&bright);
        if bounds.is_empty() {
            return None;
        }
        let area = Self::padded_rect(bounds, tophat, value.width as i32, value.height as i32);
        Some((area, background, bright))
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_contrast_rule(
        &self,
        value: &GrayImage,
        background: &GrayImage,
        bright: &GrayImage,
        mask: &mut GrayImage,
        area: IntRect,
        tophat: usize,
        shared_hat: Option<&(IntRect, GrayImage)>,
    ) {
        let owned_hat;
        let (hat_img, ox, oy) = match shared_hat {
            Some((outer, img)) if Self::rect_contains(*outer, area) => {
                (img, (area.x - outer.x) as usize, (area.y - outer.y) as usize)
            }
            _ => {
                let sub = value.crop(area);
                owned_hat = morph_tophat_rect(&sub, tophat, tophat);
                (&owned_hat, 0usize, 0usize)
            }
        };

        let w = mask.width;
        for y in 0..(area.height as usize) {
            let gy = area.y as usize + y;
            let row_off = gy * w + area.x as usize;
            let h_off = (oy + y) * hat_img.width + ox;
            for x in 0..(area.width as usize) {
                if bright.data[row_off + x] != 0 {
                    let bg = background.data[row_off + x] as usize;
                    let lim = self.hat_limits[bg];
                    mask.data[row_off + x] = if hat_img.data[h_off + x] > lim {
                        255
                    } else {
                        0
                    };
                }
            }
        }
    }

    fn padded_rect(bounds: IntRect, size: i32, cols: i32, rows: i32) -> IntRect {
        let pad = 2 * size + 2;
        let left = (bounds.x - pad).max(0);
        let top = (bounds.y - pad).max(0);
        let right = (bounds.right() + pad).min(cols);
        let bottom = (bounds.bottom() + pad).min(rows);
        IntRect::new(left, top, right - left, bottom - top)
    }

    fn union_rect(a: IntRect, b: IntRect) -> IntRect {
        let left = a.x.min(b.x);
        let top = a.y.min(b.y);
        let right = a.right().max(b.right());
        let bottom = a.bottom().max(b.bottom());
        IntRect::new(left, top, right - left, bottom - top)
    }

    fn rect_contains(outer: IntRect, inner: IntRect) -> bool {
        !outer.is_empty()
            && inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    // --- Text lines & isolation (`NameplateDetector.Lines.cs`) ---

    fn find_text_lines(
        &mut self,
        frame_height: i32,
        mut narrow: Option<&mut Vec<IntRect>>,
    ) -> Vec<IntRect> {
        let max_height = self.scaled(frame_height, self.settings.line_max_height);
        let min_height = self.scaled(frame_height, self.settings.line_min_height);
        let min_width = self.scaled(frame_height, self.settings.line_min_width);
        let max_width = self.scaled(frame_height, NameplateDetectorSettings::MAX_LINE_WIDTH);
        let min_narrow_width = self.scaled(frame_height, self.settings.narrow_min_width);
        let ratio = frame_height as f64 / self.settings.reference_height as f64;
        let speck = ((self.settings.speck_area * ratio * ratio + 0.5).floor() as i32).max(1);

        let (labels, stats) = connected_components_with_stats(&self.mask);
        let count = stats.len();
        self.mask_labels = labels;
        self.mask_stats = stats;

        if count <= 1 {
            self.kept = self.mask.clone();
            return Vec::new();
        }

        self.solid.assign_len(count, false);
        self.tall.assign_len(count, false);
        let mut tall_count = 0usize;
        for i in 1..count {
            self.solid[i] = self.mask_stats[i].area > speck;
            let is_tall = self.mask_stats[i].h > max_height;
            self.tall[i] = is_tall;
            if is_tall {
                tall_count += 1;
            }
        }

        self.kept = self.mask.clone();
        if tall_count > 0 {
            for (idx, &lbl) in self.mask_labels.iter().enumerate() {
                if lbl > 0 && self.tall[lbl as usize] {
                    self.kept.data[idx] = 0;
                }
            }
        }

        let join_w = self.scaled(frame_height, self.settings.join_width_px).max(1) as usize;
        let joined = dilate_rect(&self.kept, join_w, 1);
        let (_, j_stats) = connected_components_with_stats(&joined);

        let mut boxes = Vec::new();
        for st in j_stats.iter().skip(1) {
            if st.h < min_height || st.h > max_height {
                continue;
            }
            let is_line = st.w >= min_width
                && st.w <= max_width
                && (st.w as f64) > (st.h as f64) * self.settings.line_aspect_min;
            if !is_line {
                if let Some(nar) = narrow.as_deref_mut() {
                    if st.w >= min_narrow_width
                        && (st.w as f64) <= self.settings.narrow_max_lines * (st.h as f64)
                    {
                        nar.push(IntRect::new(st.x, st.y, st.w, st.h));
                    }
                }
                continue;
            }

            let rect = IntRect::new(st.x, st.y, st.w, st.h);
            let fill = self.fill_ratio(&self.kept, rect);
            if fill > self.settings.fill_min && fill < self.settings.fill_max {
                boxes.push(rect);
            }
        }
        boxes
    }

    fn fill_ratio(&self, img: &GrayImage, rect: IntRect) -> f64 {
        let r = rect.clamp_to(img.width as i32, img.height as i32);
        if r.is_empty() {
            return 0.0;
        }
        let mut nz = 0usize;
        for y in (r.y as usize)..(r.bottom() as usize) {
            let row = &img.data[y * img.width + r.x as usize..y * img.width + r.right() as usize];
            for &v in row {
                if v != 0 {
                    nz += 1;
                }
            }
        }
        nz as f64 / ((r.width * r.height) as f64)
    }

    fn narrow_fill_ok(&self, rect: IntRect) -> bool {
        if self.kept.is_empty() || rect.is_empty() {
            return false;
        }
        let fill = self.fill_ratio(&self.kept, rect);
        fill > self.settings.fill_min && fill < self.settings.fill_max
    }

    fn is_clear(&self, box_rect: IntRect) -> bool {
        let reach = (self.settings.clear_lines * box_rect.height as f64).ceil() as i32;
        let mw = self.kept.width as i32;
        let mh = self.kept.height as i32;
        let row_start = box_rect.y.max(0);
        let row_end = box_rect.bottom().min(mh);
        if row_end <= row_start {
            return true;
        }
        let left_start = (box_rect.x - reach).max(0);
        let left_end = box_rect.x;
        if left_end > left_start
            && self.has_solid_ink(IntRect::new(
                left_start,
                row_start,
                left_end - left_start,
                row_end - row_start,
            ))
        {
            return false;
        }
        let right_start = box_rect.right();
        let right_end = (box_rect.right() + reach).min(mw);
        if right_end > right_start
            && self.has_solid_ink(IntRect::new(
                right_start,
                row_start,
                right_end - right_start,
                row_end - row_start,
            ))
        {
            return false;
        }
        true
    }

    fn has_solid_ink(&self, area: IntRect) -> bool {
        let w = self.kept.width;
        for y in (area.y as usize)..(area.bottom() as usize) {
            let row = &self.mask_labels[y * w + area.x as usize..y * w + area.right() as usize];
            for &lbl in row {
                if lbl > 0 && self.solid[lbl as usize] {
                    return true;
                }
            }
        }
        false
    }

    fn obstacle_window(&self, area: IntRect) -> (Vec<u8>, bool) {
        let aw = area.width as usize;
        let ah = area.height as usize;
        let mut win = vec![0u8; aw * ah];
        let mut any = false;
        let mw = self.kept.width;
        for y in 0..ah {
            let row =
                &self.mask_labels[(area.y as usize + y) * mw + area.x as usize..][..aw];
            for x in 0..aw {
                let lbl = row[x];
                if lbl > 0 && self.solid[lbl as usize] {
                    win[y * aw + x] = 1;
                    any = true;
                }
            }
        }
        (win, any)
    }

    fn speed_unions(&self, boxes: &[IntRect], frame_height: i32) -> Vec<IntRect> {
        let ht = self.scaled(frame_height, self.settings.height_tolerance_px) as f64;
        let rt = self.scaled(frame_height, self.settings.row_tolerance_px) as f64;
        let mut unions = Vec::new();
        for (l, &left) in boxes.iter().enumerate() {
            let mut best: Option<(usize, i32)> = None;
            for (r, &right) in boxes.iter().enumerate() {
                if r == l {
                    continue;
                }
                let gap = right.x - left.right();
                if !(gap >= 1
                    && (gap as f64) <= self.settings.speed_join_gap * left.height as f64)
                    || (right.width as f64)
                        > self.settings.speed_join_max_lines * left.height as f64
                {
                    continue;
                }
                if ((right.height - left.height).abs() as f64)
                    > ht.max(left.height as f64 * self.settings.height_tolerance_lines)
                {
                    continue;
                }
                let cy_l = left.y as f64 + left.height as f64 * 0.5;
                let cy_r = right.y as f64 + right.height as f64 * 0.5;
                if (cy_r - cy_l).abs()
                    > rt.max(self.settings.row_tolerance_lines * left.height as f64)
                {
                    continue;
                }
                if best.map(|(_, bg)| gap < bg).unwrap_or(true) {
                    best = Some((r, gap));
                }
            }
            if let Some((best_idx, _)) = best {
                let right = boxes[best_idx];
                let top = left.y.min(right.y);
                let bottom = left.bottom().max(right.bottom());
                unions.push(IntRect::new(
                    left.x,
                    top,
                    right.right() - left.x,
                    bottom - top,
                ));
            }
        }
        unions
    }

    // --- Pairing & Stacks (`NameplateDetector.Pairs.cs`) ---

    fn pair_lines(
        &self,
        boxes: &[IntRect],
        frame_height: i32,
        real_count: usize,
    ) -> Vec<(usize, usize)> {
        let ht = self.scaled(frame_height, self.settings.height_tolerance_px);
        let ct = self.scaled(frame_height, self.settings.center_tolerance);
        let lt = self.scaled(frame_height, self.settings.left_tolerance);
        let lwd = self.scaled(frame_height, self.settings.left_width_difference);
        let by_top = LinesByTop::new(boxes, None);
        let mut below = Vec::new();
        let mut pairs = Vec::new();
        for t in 0..real_count {
            let top = boxes[t];
            by_top.below(top, &mut below);
            for &b in &below {
                if b != t && self.pair_ok(top, boxes[b], ht, ct, lt, lwd) {
                    pairs.push((t, b));
                }
            }
        }
        pairs
    }

    fn pair_ok(
        &self,
        top: IntRect,
        bottom: IntRect,
        height_tol: i32,
        center_tol: i32,
        left_tol: i32,
        left_width_diff: i32,
    ) -> bool {
        let gap = bottom.y - top.bottom();
        if !(gap >= 1
            && (gap as f64) <= top.height as f64 * NameplateDetectorSettings::PAIR_GAP_MAX_LINES)
        {
            return false;
        }
        if ((bottom.height - top.height).abs() as f64)
            > (height_tol as f64).max(top.height as f64 * self.settings.height_tolerance_lines)
        {
            return false;
        }
        let tc = top.x as f64 + top.width as f64 * 0.5;
        let bc = bottom.x as f64 + bottom.width as f64 * 0.5;
        if (bc - tc).abs() > center_tol as f64 {
            return false;
        }
        if (top.x - bottom.x).abs() <= left_tol && (top.width - bottom.width).abs() >= left_width_diff
        {
            return false;
        }
        true
    }

    fn promote_narrow(
        &self,
        narrow: &[IntRect],
        plain: &[IntRect],
        unions: &[IntRect],
        frame_height: i32,
        row_pool: &[IntRect],
    ) -> Vec<(IntRect, Vec<IntRect>)> {
        let ht = self.scaled(frame_height, self.settings.height_tolerance_px);
        let ct = self.scaled(frame_height, self.settings.center_tolerance);
        let lt = self.scaled(frame_height, self.settings.left_tolerance);
        let lwd = self.scaled(frame_height, self.settings.left_width_difference);
        let plain_set: HashSet<IntRect> = plain.iter().copied().collect();
        let below = LinesByTop::new(plain, Some(unions));
        let mut near = Vec::new();
        let mut promoted = Vec::new();
        for &shape in narrow {
            if plain_set.contains(&shape) {
                continue;
            }
            below.below(shape, &mut near);
            let mut bottoms = Vec::new();
            for &i in &near {
                let bottom = if i < plain.len() {
                    plain[i]
                } else {
                    unions[i - plain.len()]
                };
                if !self.pair_ok(shape, bottom, ht, ct, lt, lwd) {
                    continue;
                }
                if self.stack_rows(row_pool, shape, bottom, frame_height).0.len() > 1 {
                    bottoms.push(bottom);
                }
            }
            if !bottoms.is_empty() && self.narrow_fill_ok(shape) {
                promoted.push((shape, bottoms));
            }
        }
        promoted
    }

    fn find_stacks(
        &self,
        boxes: &[IntRect],
        pairs: &[(usize, usize)],
        pool: &[IntRect],
        frame_height: i32,
    ) -> HashMap<(IntRect, IntRect), (Vec<IntRect>, Vec<IntRect>)> {
        let mut stacks = HashMap::new();
        for &(t, b) in pairs {
            let (rows, blockers) = self.stack_rows(pool, boxes[t], boxes[b], frame_height);
            if rows.len() > 1 || !blockers.is_empty() {
                stacks.insert((boxes[t], boxes[b]), (rows, blockers));
            }
        }
        stacks
    }

    fn row_candidates(&self, boxes: &[IntRect], real_count: usize, frame_height: i32) -> Vec<IntRect> {
        let s2 = self.scaled(frame_height, self.settings.row_join_top_px);
        let s3 = self.scaled(frame_height, self.settings.row_join_height_px);
        let s4 = self.scaled(frame_height, self.settings.row_join_gap_px);
        let mut parent: Vec<usize> = (0..real_count).collect();

        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }

        for i in 0..real_count {
            let a = boxes[i];
            for j in (i + 1)..real_count {
                let c = boxes[j];
                if (a.y - c.y).abs() <= s2
                    && (a.height - c.height).abs() <= s3
                    && c.x <= a.right() + s4
                    && a.x <= c.right() + s4
                {
                    let ri = find(&mut parent, i);
                    let rj = find(&mut parent, j);
                    parent[rj] = ri;
                }
            }
        }

        let mut order = Vec::new();
        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..real_count {
            let root = find(&mut parent, i);
            members.entry(root).or_insert_with(|| {
                order.push(root);
                Vec::new()
            }).push(i);
        }

        let mut pool = Vec::with_capacity(order.len());
        for root in order {
            let group = &members[&root];
            if group.len() == 1 {
                pool.push(boxes[group[0]]);
            } else {
                let x0 = group.iter().map(|&i| boxes[i].x).min().unwrap();
                let y0 = group.iter().map(|&i| boxes[i].y).min().unwrap();
                let x1 = group.iter().map(|&i| boxes[i].right()).max().unwrap();
                let y1 = group.iter().map(|&i| boxes[i].bottom()).max().unwrap();
                pool.push(IntRect::new(x0, y0, x1 - x0, y1 - y0));
            }
        }
        pool
    }

    fn stack_rows(
        &self,
        pool: &[IntRect],
        top: IntRect,
        anchor: IntRect,
        frame_height: i32,
    ) -> (Vec<IntRect>, Vec<IntRect>) {
        let ht = self.scaled(frame_height, self.settings.height_tolerance_px) as f64;
        let ct = self.scaled(frame_height, self.settings.center_tolerance) as f64;
        let lt = self.scaled(frame_height, self.settings.left_tolerance);
        let lwd = self.scaled(frame_height, self.settings.left_width_difference);
        let anchor_center = anchor.x as f64 + anchor.width as f64 * 0.5;
        let mut chain = vec![top];
        let mut current = top;

        loop {
            let mut joinable: Vec<(i32, IntRect)> = Vec::new();
            let mut broken: Vec<IntRect> = Vec::new();
            for &row in pool {
                if row == anchor || chain.contains(&row) {
                    continue;
                }
                let gap = current.y - row.bottom();
                if !(gap >= 1 && (gap as f64) <= self.settings.row_gap_max * current.height as f64)
                {
                    continue;
                }
                let pitch = (current.y - row.y) as f64;
                if !(self.settings.row_pitch_min * current.height as f64 <= pitch
                    && pitch <= self.settings.row_pitch_max * current.height as f64)
                {
                    continue;
                }
                if ((row.height - current.height).abs() as f64)
                    > ht.max(current.height as f64 * self.settings.height_tolerance_lines)
                {
                    continue;
                }
                if (row.width as f64) < self.settings.row_min_caps * row.height as f64 {
                    continue;
                }
                if row.x >= current.right() || row.right() <= current.x {
                    continue;
                }
                if (row.x - current.x).abs() <= lt
                    && (row.width - current.width).abs() >= lwd
                {
                    continue;
                }
                if (row.x as f64 + row.width as f64 * 0.5 - anchor_center).abs() > ct {
                    broken.push(row);
                    continue;
                }
                joinable.push((gap, row));
            }

            if joinable.is_empty() {
                chain.reverse();
                return (chain, broken);
            }

            joinable.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| a.1.x.cmp(&b.1.x))
                    .then_with(|| a.1.y.cmp(&b.1.y))
                    .then_with(|| a.1.width.cmp(&b.1.width))
                    .then_with(|| a.1.height.cmp(&b.1.height))
            });
            let best = joinable[0].1;
            if chain.len() >= self.settings.max_rows {
                chain.reverse();
                broken.push(best);
                return (chain, broken);
            }
            chain.push(best);
            current = best;
        }
    }

    fn sort_pairs(
        &self,
        boxes: &[IntRect],
        clear_known: &mut [i8],
        all: &[(usize, usize)],
        real_count: usize,
    ) -> (Vec<(usize, usize)>, Vec<(usize, usize)>, Vec<CrowdPair>) {
        let mut strict = Vec::new();
        let mut loose = Vec::new();
        let mut crowd = Vec::new();
        let mut pairs_of_line: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        for &(t, b) in all {
            pairs_of_line.entry(t).or_default().push((t, b));
            pairs_of_line.entry(b).or_default().push((t, b));
        }

        let check_clear = |idx: usize, known: &mut [i8]| -> bool {
            if known[idx] == 0 {
                known[idx] = if self.is_clear(boxes[idx]) { 1 } else { -1 };
            }
            known[idx] > 0
        };

        let mut neighbour_cache: HashMap<usize, Option<Vec<usize>>> = HashMap::new();

        for &(t, b) in all {
            let ct = check_clear(t, clear_known);
            let cb = check_clear(b, clear_known);
            if ct && cb {
                strict.push((t, b));
                continue;
            }

            if self.settings.crowd_exception {
                let mut need = BTreeSet::new();
                let mut blocked = false;
                for line in [t, b] {
                    let is_cl = check_clear(line, clear_known);
                    let entry = neighbour_cache.entry(line).or_insert_with(|| {
                        if is_cl {
                            Some(Vec::new())
                        } else {
                            self.neighbours(boxes, line, real_count)
                        }
                    });
                    match entry {
                        Some(near) => {
                            for &mut k in near {
                                need.insert(k);
                            }
                        }
                        None => {
                            blocked = true;
                            break;
                        }
                    }
                }
                if !blocked {
                    need.remove(&t);
                    need.remove(&b);
                    if !need.is_empty()
                        && need.iter().all(|k| {
                            pairs_of_line.get(k).is_some_and(|others| {
                                others
                                    .iter()
                                    .any(|&(ot, ob)| ot != t && ot != b && ob != t && ob != b)
                            })
                        })
                    {
                        crowd.push(CrowdPair {
                            top: t,
                            bottom: b,
                            neighbours: need.into_iter().collect(),
                        });
                        continue;
                    }
                }
            }
            loose.push((t, b));
        }

        (strict, loose, crowd)
    }

    fn neighbours(&self, boxes: &[IntRect], index: usize, real_count: usize) -> Option<Vec<usize>> {
        let box_rect = boxes[index];
        let reach = (self.settings.clear_lines * box_rect.height as f64).ceil() as i32;
        let row_start = box_rect.y.max(0);
        let row_end = box_rect.bottom().min(self.kept.height as i32);
        if row_end <= row_start {
            return Some(Vec::new());
        }
        let mut found = BTreeSet::new();
        let sides = [
            ((box_rect.x - reach).max(0), box_rect.x),
            (
                box_rect.right(),
                (box_rect.right() + reach).min(self.kept.width as i32),
            ),
        ];

        for (start, end) in sides {
            if end <= start {
                continue;
            }
            let win_w = (end - start) as usize;
            let area = IntRect::new(start, row_start, end - start, row_end - row_start);
            let (window, any_ink) = self.obstacle_window(area);
            if !any_ink {
                continue;
            }
            let mut rest = window.clone();
            for (k, &other) in boxes.iter().enumerate().take(real_count) {
                if k == index {
                    continue;
                }
                if other.x >= end
                    || other.right() <= start
                    || other.y >= row_end
                    || other.bottom() <= row_start
                {
                    continue;
                }
                let x0 = (other.x.max(start) - start) as usize;
                let x1 = (other.right().min(end) - start) as usize;
                let y0 = (other.y.max(row_start) - row_start) as usize;
                let y1 = (other.bottom().min(row_end) - row_start) as usize;
                let mut touches = false;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let at = y * win_w + x;
                        if window[at] != 0 {
                            touches = true;
                        }
                        rest[at] = 0;
                    }
                }
                if touches {
                    found.insert(k);
                }
            }
            if rest.iter().any(|&v| v != 0) {
                return None;
            }
        }
        Some(found.into_iter().collect())
    }
}

trait VecAssignExt<T: Clone> {
    fn assign_len(&mut self, len: usize, val: T);
}

impl<T: Clone> VecAssignExt<T> for Vec<T> {
    fn assign_len(&mut self, len: usize, val: T) {
        self.clear();
        self.resize(len, val);
    }
}

struct LinesByTop {
    order: Vec<usize>,
    tops: Vec<i32>,
}

impl LinesByTop {
    fn new(first: &[IntRect], second: Option<&[IntRect]>) -> Self {
        let count = first.len() + second.map_or(0, |s| s.len());
        let mut pairs: Vec<(i32, usize)> = (0..count)
            .map(|i| {
                let y = if i < first.len() {
                    first[i].y
                } else {
                    second.unwrap()[i - first.len()].y
                };
                (y, i)
            })
            .collect();
        pairs.sort_by_key(|&(y, i)| (y, i));
        Self {
            tops: pairs.iter().map(|&(y, _)| y).collect(),
            order: pairs.iter().map(|&(_, i)| i).collect(),
        }
    }

    fn below(&self, shape: IntRect, result: &mut Vec<usize>) {
        result.clear();
        let bottom = shape.bottom();
        let farthest = bottom
            + (shape.height as f64 * NameplateDetectorSettings::PAIR_GAP_MAX_LINES).floor() as i32;
        let start = self.tops.partition_point(|&t| t < bottom + 1);
        for i in start..self.tops.len() {
            if self.tops[i] > farthest {
                break;
            }
            result.push(self.order[i]);
        }
        result.sort_unstable();
    }
}
