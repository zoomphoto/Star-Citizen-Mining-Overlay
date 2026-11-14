use crate::detection::image_ops::{
    morph_open_rect, morph_tophat_rect, otsu_threshold, phase_correlate_hanning, resize_area_f32,
    resize_cubic_bgr, BgrImage, GrayImage,
};
use crate::models::IntRect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapBand {
    pub top: usize,
    pub bottom: usize,
}

impl CapBand {
    pub fn height(&self) -> usize {
        self.bottom.saturating_sub(self.top) + 1
    }
}

#[derive(Debug, Clone)]
pub struct LineMask {
    pub height: usize,
    pub width: usize,
    pub data: Vec<u8>,
    pub origin_x: i32,
    pub origin_y: i32,
    pub column_offset: usize,
    pub row_offset: usize,
    pub ink_colors: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerRule {
    Any,
    Agree,
}

pub struct LayerCheck<'a> {
    pub active: bool,
    pub rule: LayerRule,
    pub on_note: Option<&'a mut dyn FnMut(bool)>,
}

pub struct NameplateFeatures;

impl NameplateFeatures {
    pub const SCALE: usize = 4;
    pub const GLYPH_WIDTH: usize = 16;
    pub const GLYPH_HEIGHT: usize = 24;
    pub const VECTOR_LENGTH: usize = Self::GLYPH_WIDTH * Self::GLYPH_HEIGHT + 4;

    pub const MULTI_LAYER_SHARE: f64 = 0.5;
    pub const LAYER_SHIFT_MIN_PX: f64 = 0.6;
    pub const LAYER_SHIFT_MAX_PX: f64 = 3.5;
    pub const LAYER_SHIFT_MIN_RESPONSE: f64 = 0.5;

    pub fn create_mask(
        bgr: &BgrImage,
        box_rect: IntRect,
        threshold: Option<f64>,
        pad: i32,
        keep_colors: bool,
        mut layers: Option<&mut LayerCheck<'_>>,
    ) -> Option<LineMask> {
        if bgr.is_empty() {
            return None;
        }
        let x0 = (box_rect.x - pad).max(0);
        let y0 = (box_rect.y - pad).max(0);
        let x1 = (box_rect.right() + pad).min(bgr.width as i32);
        let y1 = (box_rect.bottom() + pad).min(bgr.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let crop = bgr.crop(IntRect::new(x0, y0, x1 - x0, y1 - y0));
        Self::create_mask_from_crop(&crop, threshold, x0, y0, keep_colors, layers.as_deref_mut())
    }

    fn create_mask_from_crop(
        crop: &BgrImage,
        threshold: Option<f64>,
        origin_x: i32,
        origin_y: i32,
        keep_colors: bool,
        layers: Option<&mut LayerCheck<'_>>,
    ) -> Option<LineMask> {
        let big = resize_cubic_bgr(crop, Self::SCALE);
        let (b_raw, g_raw, r_raw) = big.split_channels();

        let b_hat = morph_tophat_rect(&b_raw, 31, 31);
        let g_hat = morph_tophat_rect(&g_raw, 31, 31);
        let r_hat = morph_tophat_rect(&r_raw, 31, 31);

        let n = big.width * big.height;
        let mut high = vec![0u8; n];
        let mut tophat = vec![0u8; n];
        for i in 0..n {
            let bg = b_hat.data[i].max(g_hat.data[i]);
            high[i] = bg;
            tophat[i] = bg.max(r_hat.data[i]);
        }

        if let Some(lc) = layers {
            if lc.active {
                let mut middle = vec![0u8; n];
                for i in 0..n {
                    let low = b_hat.data[i].min(g_hat.data[i]);
                    middle[i] = high[i].min(r_hat.data[i]).max(low);
                }
                if is_multi_layer_text(&tophat, &middle) {
                    let shifted = has_shifted_layers(&r_hat, &b_hat);
                    if let Some(cb) = lc.on_note.as_deref_mut() {
                        cb(shifted);
                    }
                    if shifted && lc.rule == LayerRule::Agree {
                        tophat = middle;
                    }
                }
            }
        }

        let thr = threshold.unwrap_or_else(|| otsu_threshold(&tophat).clamp(25.0, 90.0));
        let mut binary = GrayImage::new(big.width, big.height);
        for i in 0..n {
            if (tophat[i] as f64) > thr {
                binary.data[i] = 1;
            }
        }

        let opened = morph_open_rect(&binary, 1, 3);
        let colors = if keep_colors {
            Some(big.data)
        } else {
            None
        };
        Self::trim_to_text(&opened.data, opened.height, opened.width, origin_x, origin_y, colors.as_deref())
    }

    pub fn trim_to_text(
        data: &[u8],
        h: usize,
        w: usize,
        origin_x: i32,
        origin_y: i32,
        colors_4x: Option<&[u8]>,
    ) -> Option<LineMask> {
        let row_limit = 0.02 * w as f64;
        let mut first_row: Option<usize> = None;
        let mut last_row: usize = 0;

        for y in 0..h {
            let sum: u32 = data[y * w..(y + 1) * w].iter().map(|&v| v as u32).sum();
            if (sum as f64) > row_limit {
                if first_row.is_none() {
                    first_row = Some(y);
                }
                last_row = y;
            }
        }
        let first_row = first_row?;

        let mut first_col = usize::MAX;
        let mut last_col = 0usize;
        let mut any_col = false;
        for y in first_row..=last_row {
            for x in 0..w {
                if data[y * w + x] != 0 {
                    any_col = true;
                    first_col = first_col.min(x);
                    last_col = last_col.max(x);
                }
            }
        }
        if !any_col {
            return None;
        }

        let out_h = last_row - first_row + 1;
        let out_w = last_col - first_col + 1;
        let mut result = vec![0u8; out_h * out_w];
        for y in 0..out_h {
            let src = (first_row + y) * w + first_col;
            result[y * out_w..(y + 1) * out_w].copy_from_slice(&data[src..src + out_w]);
        }

        let ink_colors = colors_4x.map(|cols| {
            let count: usize = result.iter().map(|&v| v as usize).sum();
            let mut ink = Vec::with_capacity(count * 3);
            for y in 0..out_h {
                for x in 0..out_w {
                    if result[y * out_w + x] != 0 {
                        let s = ((first_row + y) * w + first_col + x) * 3;
                        ink.push(cols[s]);
                        ink.push(cols[s + 1]);
                        ink.push(cols[s + 2]);
                    }
                }
            }
            ink
        });

        Some(LineMask {
            height: out_h,
            width: out_w,
            data: result,
            origin_x,
            origin_y,
            column_offset: first_col,
            row_offset: first_row,
            ink_colors,
        })
    }

    pub fn gap_cuts(m: &LineMask) -> Vec<(usize, usize)> {
        let mut cuts = Vec::new();
        let mut start: Option<usize> = None;
        for x in 0..m.width {
            let mut has_ink = false;
            for y in 0..m.height {
                if m.data[y * m.width + x] != 0 {
                    has_ink = true;
                    break;
                }
            }
            if has_ink && start.is_none() {
                start = Some(x);
            } else if !has_ink {
                if let Some(s) = start.take() {
                    cuts.push((s, x));
                }
            }
        }
        if let Some(s) = start {
            cuts.push((s, m.width));
        }
        cuts
    }

    pub fn find_cap_band(m: &LineMask) -> CapBand {
        let mut tops = Vec::new();
        let mut bottoms = Vec::new();
        for (a, b) in Self::gap_cuts(m) {
            let mut min_y: Option<usize> = None;
            let mut max_y = 0usize;
            for y in 0..m.height {
                for x in a..b {
                    if m.data[y * m.width + x] != 0 {
                        if min_y.is_none() {
                            min_y = Some(y);
                        }
                        max_y = y;
                        break;
                    }
                }
            }
            if let Some(mn) = min_y {
                if (max_y - mn) as f64 > 0.5 * m.height as f64 {
                    tops.push(mn);
                    bottoms.push(max_y);
                }
            }
        }
        if tops.is_empty() {
            return CapBand {
                top: 0,
                bottom: m.height.saturating_sub(1),
            };
        }
        CapBand {
            top: median_usize(&mut tops),
            bottom: median_usize(&mut bottoms),
        }
    }

    pub fn glyph_vector(
        m: &LineMask,
        x0: usize,
        x1: usize,
        band: CapBand,
        out: &mut [f64],
    ) -> bool {
        if out.len() < Self::VECTOR_LENGTH {
            return false;
        }
        let x1 = x1.min(m.width);
        if x1 <= x0 {
            return false;
        }
        let cap = band.height().max(1) as f64;
        let mut row_min: Option<usize> = None;
        let mut row_max = 0usize;
        let mut col_min = usize::MAX;
        let mut col_max = 0usize;

        for y in 0..m.height {
            for x in x0..x1 {
                if m.data[y * m.width + x] != 0 {
                    if row_min.is_none() {
                        row_min = Some(y);
                    }
                    row_max = y;
                    col_min = col_min.min(x);
                    col_max = col_max.max(x);
                }
            }
        }
        let Some(row_min) = row_min else {
            return false;
        };

        let gh = row_max - row_min + 1;
        let gw = col_max - col_min + 1;
        let mut sub = vec![0.0f32; gh * gw];
        for y in 0..gh {
            for x in 0..gw {
                sub[y * gw + x] = m.data[(row_min + y) * m.width + col_min + x] as f32;
            }
        }

        let small = resize_area_f32(&sub, gw, gh, Self::GLYPH_WIDTH, Self::GLYPH_HEIGHT);
        for (i, &v) in small.iter().enumerate() {
            out[i] = v as f64;
        }
        let n = Self::GLYPH_WIDTH * Self::GLYPH_HEIGHT;
        out[n] = (gw as f64) / cap * 4.0;
        out[n + 1] = (gh as f64) / cap * 4.0;
        out[n + 2] = (row_min as isize - band.top as isize) as f64 / cap * 4.0;
        out[n + 3] = (row_max as isize - band.bottom as isize) as f64 / cap * 4.0;
        true
    }
}

fn median_usize(values: &mut [usize]) -> usize {
    values.sort_unstable();
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2
    }
}

fn is_multi_layer_text(brightest: &[u8], second: &[u8]) -> bool {
    let thr = otsu_threshold(brightest).clamp(25.0, 90.0);
    let mut sum_b = 0.0f64;
    let mut sum_s = 0.0f64;
    let mut count = 0usize;
    for i in 0..brightest.len() {
        if (brightest[i] as f64) > thr {
            sum_b += brightest[i] as f64;
            sum_s += second[i] as f64;
            count += 1;
        }
    }
    count > 0 && sum_s >= NameplateFeatures::MULTI_LAYER_SHARE * sum_b
}

fn has_shifted_layers(red: &GrayImage, blue: &GrayImage) -> bool {
    let (dx, dy, response) = phase_correlate_hanning(red, blue);
    let px = (dx * dx + dy * dy).sqrt() / NameplateFeatures::SCALE as f64;
    response >= NameplateFeatures::LAYER_SHIFT_MIN_RESPONSE
        && px >= NameplateFeatures::LAYER_SHIFT_MIN_PX
        && px <= NameplateFeatures::LAYER_SHIFT_MAX_PX
}

