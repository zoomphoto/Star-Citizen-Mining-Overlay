use crate::models::IntRect;
use rustfft::{num_complex::Complex, FftPlanner};
use std::f32::consts::PI;

#[derive(Debug, Clone, Default)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl GrayImage {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0u8; width * height],
        }
    }

    pub fn from_vec(width: usize, height: usize, data: Vec<u8>) -> Self {
        debug_assert_eq!(data.len(), width * height);
        Self {
            width,
            height,
            data,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || self.data.is_empty()
    }

    pub fn crop(&self, rect: IntRect) -> Self {
        let r = rect.clamp_to(self.width as i32, self.height as i32);
        if r.is_empty() {
            return Self::default();
        }
        let w = r.width as usize;
        let h = r.height as usize;
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            let src_row = (r.y as usize + y) * self.width + r.x as usize;
            out[y * w..(y + 1) * w].copy_from_slice(&self.data[src_row..src_row + w]);
        }
        Self {
            width: w,
            height: h,
            data: out,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BgrImage {
    pub width: usize,
    pub height: usize,
    /// Packed `B, G, R` bytes (`width * height * 3`)
    pub data: Vec<u8>,
}

impl BgrImage {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0u8; width * height * 3],
        }
    }

    pub fn from_bgra(bgra: &[u8], width: usize, height: usize, stride: usize) -> Self {
        let mut data = vec![0u8; width * height * 3];
        for y in 0..height {
            let src = &bgra[y * stride..y * stride + width * 4];
            let dst = &mut data[y * width * 3..(y + 1) * width * 3];
            for x in 0..width {
                dst[x * 3] = src[x * 4];
                dst[x * 3 + 1] = src[x * 4 + 1];
                dst[x * 3 + 2] = src[x * 4 + 2];
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || self.data.is_empty()
    }

    pub fn crop(&self, rect: IntRect) -> Self {
        let r = rect.clamp_to(self.width as i32, self.height as i32);
        if r.is_empty() {
            return Self::default();
        }
        let w = r.width as usize;
        let h = r.height as usize;
        let mut out = vec![0u8; w * h * 3];
        for y in 0..h {
            let src_off = ((r.y as usize + y) * self.width + r.x as usize) * 3;
            out[y * w * 3..(y + 1) * w * 3]
                .copy_from_slice(&self.data[src_off..src_off + w * 3]);
        }
        Self {
            width: w,
            height: h,
            data: out,
        }
    }

    pub fn split_channels(&self) -> (GrayImage, GrayImage, GrayImage) {
        let n = self.width * self.height;
        let mut b = vec![0u8; n];
        let mut g = vec![0u8; n];
        let mut r = vec![0u8; n];
        for i in 0..n {
            b[i] = self.data[i * 3];
            g[i] = self.data[i * 3 + 1];
            r[i] = self.data[i * 3 + 2];
        }
        (
            GrayImage::from_vec(self.width, self.height, b),
            GrayImage::from_vec(self.width, self.height, g),
            GrayImage::from_vec(self.width, self.height, r),
        )
    }
}

/// Van Herk / Gil-Werman O(1)-per-pixel 1D sliding window min/max along rows and columns.
fn morph_1d_line(src: &[u8], dst: &mut [u8], k: usize, is_max: bool) {
    let n = src.len();
    if n == 0 {
        return;
    }
    if k <= 1 {
        dst.copy_from_slice(src);
        return;
    }
    let anchor = k / 2;
    let cmp = |a: u8, b: u8| if is_max { a.max(b) } else { a.min(b) };

    // Padded length so every window [i, i + k - 1] is in bounds with clamped borders
    let pad_left = anchor;
    let pad_right = k - 1 - anchor;
    let m = n + pad_left + pad_right;
    let mut padded = vec![0u8; m];
    for i in 0..pad_left {
        padded[i] = src[0];
    }
    padded[pad_left..pad_left + n].copy_from_slice(src);
    for i in 0..pad_right {
        padded[pad_left + n + i] = src[n - 1];
    }

    let mut prefix = vec![0u8; m];
    let mut suffix = vec![0u8; m];

    for i in 0..m {
        if i % k == 0 {
            prefix[i] = padded[i];
        } else {
            prefix[i] = cmp(prefix[i - 1], padded[i]);
        }
    }
    for i in (0..m).rev() {
        if i == m - 1 || (i + 1) % k == 0 {
            suffix[i] = padded[i];
        } else {
            suffix[i] = cmp(suffix[i + 1], padded[i]);
        }
    }

    for i in 0..n {
        let right = i + k - 1;
        dst[i] = cmp(suffix[i], prefix[right]);
    }
}

fn morph_rect(src: &GrayImage, kw: usize, kh: usize, is_max: bool) -> GrayImage {
    let w = src.width;
    let h = src.height;
    if w == 0 || h == 0 {
        return GrayImage::default();
    }
    let kw = kw.max(1);
    let kh = kh.max(1);

    let mut horiz = vec![0u8; w * h];
    if kw == 1 {
        horiz.copy_from_slice(&src.data);
    } else {
        for y in 0..h {
            morph_1d_line(
                &src.data[y * w..(y + 1) * w],
                &mut horiz[y * w..(y + 1) * w],
                kw,
                is_max,
            );
        }
    }

    if kh == 1 {
        return GrayImage::from_vec(w, h, horiz);
    }

    let mut out = vec![0u8; w * h];
    let mut col_in = vec![0u8; h];
    let mut col_out = vec![0u8; h];
    for x in 0..w {
        for y in 0..h {
            col_in[y] = horiz[y * w + x];
        }
        morph_1d_line(&col_in, &mut col_out, kh, is_max);
        for y in 0..h {
            out[y * w + x] = col_out[y];
        }
    }
    GrayImage::from_vec(w, h, out)
}

pub fn erode_rect(src: &GrayImage, kw: usize, kh: usize) -> GrayImage {
    morph_rect(src, kw, kh, false)
}

pub fn dilate_rect(src: &GrayImage, kw: usize, kh: usize) -> GrayImage {
    morph_rect(src, kw, kh, true)
}

pub fn morph_open_rect(src: &GrayImage, kw: usize, kh: usize) -> GrayImage {
    let eroded = erode_rect(src, kw, kh);
    dilate_rect(&eroded, kw, kh)
}

pub fn morph_tophat_rect(src: &GrayImage, kw: usize, kh: usize) -> GrayImage {
    let opened = morph_open_rect(src, kw, kh);
    let mut out = vec![0u8; src.data.len()];
    for i in 0..out.len() {
        out[i] = src.data[i].saturating_sub(opened.data[i]);
    }
    GrayImage::from_vec(src.width, src.height, out)
}

pub fn box_blur(src: &GrayImage, ksize: usize) -> GrayImage {
    let w = src.width;
    let h = src.height;
    if w == 0 || h == 0 || ksize <= 1 {
        return src.clone();
    }
    let r = (ksize / 2) as isize;
    let mut integral = vec![0u64; (w + 1) * (h + 1)];
    let stride = w + 1;
    for y in 0..h {
        let mut row_sum = 0u64;
        for x in 0..w {
            row_sum += src.data[y * w + x] as u64;
            integral[(y + 1) * stride + (x + 1)] = integral[y * stride + (x + 1)] + row_sum;
        }
    }

    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let y0 = (y as isize - r).clamp(0, h as isize - 1) as usize;
        let y1 = (y as isize + r).clamp(0, h as isize - 1) as usize + 1;
        for x in 0..w {
            let x0 = (x as isize - r).clamp(0, w as isize - 1) as usize;
            let x1 = (x as isize + r).clamp(0, w as isize - 1) as usize + 1;
            let sum = integral[y1 * stride + x1] + integral[y0 * stride + x0]
                - integral[y0 * stride + x1]
                - integral[y1 * stride + x0];
            let area = ((y1 - y0) * (x1 - x0)) as u64;
            out[y * w + x] = ((sum + area / 2) / area) as u8;
        }
    }
    GrayImage::from_vec(w, h, out)
}

/// Computes OpenCV-compatible Otsu threshold on 8-bit grayscale data.
pub fn otsu_threshold(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut hist = [0u64; 256];
    for &b in data {
        hist[b as usize] += 1;
    }
    let total = data.len() as f64;
    let mut sum_all = 0.0;
    for i in 0..256 {
        sum_all += (i as f64) * (hist[i] as f64);
    }

    let mut sum_b = 0.0;
    let mut w_b = 0.0;
    let mut max_var = -1.0;
    let mut best_t = 0usize;

    for t in 0..256 {
        w_b += hist[t] as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += (t as f64) * (hist[t] as f64);
        let m_b = sum_b / w_b;
        let m_f = (sum_all - sum_b) / w_f;
        let diff = m_b - m_f;
        let between = w_b * w_f * diff * diff;
        if between > max_var {
            max_var = between;
            best_t = t;
        }
    }
    best_t as f64
}

pub fn bounding_rect_nonzero(img: &GrayImage) -> IntRect {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = -1i32;
    let mut max_y = -1i32;

    for y in 0..img.height {
        let row = &img.data[y * img.width..(y + 1) * img.width];
        for (x, &v) in row.iter().enumerate() {
            if v != 0 {
                let xi = x as i32;
                let yi = y as i32;
                if xi < min_x {
                    min_x = xi;
                }
                if xi > max_x {
                    max_x = xi;
                }
                if yi < min_y {
                    min_y = yi;
                }
                if yi > max_y {
                    max_y = yi;
                }
            }
        }
    }

    if max_x < 0 {
        IntRect::default()
    } else {
        IntRect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ComponentStats {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub area: i32,
}

/// 8-connected components labeling with bounding box & pixel area stats (label 0 is background).
pub fn connected_components_with_stats(binary: &GrayImage) -> (Vec<i32>, Vec<ComponentStats>) {
    let w = binary.width;
    let h = binary.height;
    let mut labels = vec![0i32; w * h];
    let mut stats = vec![ComponentStats {
        x: 0,
        y: 0,
        w: w as i32,
        h: h as i32,
        area: 0,
    }];
    if w == 0 || h == 0 {
        return (labels, stats);
    }

    let mut queue = Vec::with_capacity(256);
    let mut next_label = 0i32;

    for sy in 0..h {
        for sx in 0..w {
            let start = sy * w + sx;
            if binary.data[start] == 0 || labels[start] != 0 {
                continue;
            }
            next_label += 1;
            labels[start] = next_label;
            queue.clear();
            queue.push((sx, sy));

            let mut min_x = sx as i32;
            let mut max_x = sx as i32;
            let mut min_y = sy as i32;
            let mut max_y = sy as i32;
            let mut area = 0i32;

            let mut head = 0usize;
            while head < queue.len() {
                let (cx, cy) = queue[head];
                head += 1;
                area += 1;
                min_x = min_x.min(cx as i32);
                max_x = max_x.max(cx as i32);
                min_y = min_y.min(cy as i32);
                max_y = max_y.max(cy as i32);

                let y0 = cy.saturating_sub(1);
                let y1 = (cy + 1).min(h - 1);
                let x0 = cx.saturating_sub(1);
                let x1 = (cx + 1).min(w - 1);
                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let idx = ny * w + nx;
                        if binary.data[idx] != 0 && labels[idx] == 0 {
                            labels[idx] = next_label;
                            queue.push((nx, ny));
                        }
                    }
                }
            }

            stats.push(ComponentStats {
                x: min_x,
                y: min_y,
                w: max_x - min_x + 1,
                h: max_y - min_y + 1,
                area,
            });
        }
    }

    (labels, stats)
}

/// Area-averaging resampling (`INTER_AREA` equivalent) for `f32` buffers (used in `GlyphVector` and `PinTemplate`).
pub fn resize_area_f32(
    src: &[f32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<f32> {
    let mut dst = vec![0.0f32; dst_w * dst_h];
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return dst;
    }
    if dst_w > src_w || dst_h > src_h {
        return resize_linear_f32(src, src_w, src_h, dst_w, dst_h);
    }

    let scale_x = src_w as f64 / dst_w as f64;
    let scale_y = src_h as f64 / dst_h as f64;

    for dy in 0..dst_h {
        let sy0 = dy as f64 * scale_y;
        let sy1 = (dy + 1) as f64 * scale_y;
        let iy0 = sy0.floor() as usize;
        let iy1 = (sy1.ceil() as usize).min(src_h);

        for dx in 0..dst_w {
            let sx0 = dx as f64 * scale_x;
            let sx1 = (dx + 1) as f64 * scale_x;
            let ix0 = sx0.floor() as usize;
            let ix1 = (sx1.ceil() as usize).min(src_w);

            let mut sum = 0.0f64;
            let mut weight_sum = 0.0f64;

            for sy in iy0..iy1 {
                let wy = (sy1.min((sy + 1) as f64) - sy0.max(sy as f64)).max(0.0);
                if wy <= 0.0 {
                    continue;
                }
                for sx in ix0..ix1 {
                    let wx = (sx1.min((sx + 1) as f64) - sx0.max(sx as f64)).max(0.0);
                    let w = wx * wy;
                    sum += src[sy * src_w + sx] as f64 * w;
                    weight_sum += w;
                }
            }
            dst[dy * dst_w + dx] = if weight_sum > 0.0 {
                (sum / weight_sum) as f32
            } else {
                0.0
            };
        }
    }
    dst
}

pub fn resize_area_u8(src: &GrayImage, dst_w: usize, dst_h: usize) -> GrayImage {
    let f: Vec<f32> = src.data.iter().map(|&b| b as f32).collect();
    let resized = resize_area_f32(&f, src.width, src.height, dst_w, dst_h);
    GrayImage::from_vec(
        dst_w,
        dst_h,
        resized
            .into_iter()
            .map(|v| v.round().clamp(0.0, 255.0) as u8)
            .collect(),
    )
}

pub fn resize_linear_f32(
    src: &[f32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<f32> {
    let mut dst = vec![0.0f32; dst_w * dst_h];
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return dst;
    }
    let scale_x = src_w as f64 / dst_w as f64;
    let scale_y = src_h as f64 / dst_h as f64;

    for dy in 0..dst_h {
        let fy = ((dy as f64 + 0.5) * scale_y - 0.5).clamp(0.0, (src_h - 1) as f64);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let wy = (fy - y0 as f64) as f32;

        for dx in 0..dst_w {
            let fx = ((dx as f64 + 0.5) * scale_x - 0.5).clamp(0.0, (src_w - 1) as f64);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let wx = (fx - x0 as f64) as f32;

            let v00 = src[y0 * src_w + x0];
            let v01 = src[y0 * src_w + x1];
            let v10 = src[y1 * src_w + x0];
            let v11 = src[y1 * src_w + x1];
            let top = v00 + (v01 - v00) * wx;
            let bot = v10 + (v11 - v10) * wx;
            dst[dy * dst_w + dx] = top + (bot - top) * wy;
        }
    }
    dst
}

pub fn resize_linear_u8(src: &GrayImage, dst_w: usize, dst_h: usize) -> GrayImage {
    let f: Vec<f32> = src.data.iter().map(|&b| b as f32).collect();
    let resized = resize_linear_f32(&f, src.width, src.height, dst_w, dst_h);
    GrayImage::from_vec(
        dst_w,
        dst_h,
        resized
            .into_iter()
            .map(|v| v.round().clamp(0.0, 255.0) as u8)
            .collect(),
    )
}

#[inline]
fn cubic_weights(t: f32) -> [f32; 4] {
    // OpenCV INTER_CUBIC uses Catmull-Rom a = -0.75
    const A: f32 = -0.75;
    let t2 = t * t;
    let t3 = t2 * t;
    let w0 = A * (t3 - 2.0 * t2 + t);
    let w1 = (A + 2.0) * t3 - (A + 3.0) * t2 + 1.0;
    let w2 = -(A + 2.0) * t3 + (2.0 * A + 3.0) * t2 - A * t;
    let w3 = -A * (t3 - t2);
    [w0, w1, w2, w3]
}

/// Separable bicubic (`INTER_CUBIC`) upscaling for `f32` single-channel images (used by `O0Rule` 8x).
pub fn resize_cubic_f32(
    src: &[f32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<f32> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return vec![0.0; dst_w * dst_h];
    }
    let scale_x = src_w as f32 / dst_w as f32;
    let scale_y = src_h as f32 / dst_h as f32;

    let mut horiz = vec![0.0f32; src_h * dst_w];
    let mut x_idx = Vec::with_capacity(dst_w);
    let mut x_w = Vec::with_capacity(dst_w);
    for dx in 0..dst_w {
        let fx = (dx as f32 + 0.5) * scale_x - 0.5;
        let ix = fx.floor() as isize;
        let t = fx - ix as f32;
        let clamp_x = |k: isize| k.clamp(0, src_w as isize - 1) as usize;
        x_idx.push([
            clamp_x(ix - 1),
            clamp_x(ix),
            clamp_x(ix + 1),
            clamp_x(ix + 2),
        ]);
        x_w.push(cubic_weights(t));
    }

    for y in 0..src_h {
        let row = &src[y * src_w..(y + 1) * src_w];
        let out_row = &mut horiz[y * dst_w..(y + 1) * dst_w];
        for dx in 0..dst_w {
            let idx = x_idx[dx];
            let w = x_w[dx];
            out_row[dx] =
                row[idx[0]] * w[0] + row[idx[1]] * w[1] + row[idx[2]] * w[2] + row[idx[3]] * w[3];
        }
    }

    let mut dst = vec![0.0f32; dst_w * dst_h];
    for dy in 0..dst_h {
        let fy = (dy as f32 + 0.5) * scale_y - 0.5;
        let iy = fy.floor() as isize;
        let t = fy - iy as f32;
        let clamp_y = |k: isize| k.clamp(0, src_h as isize - 1) as usize;
        let r0 = &horiz[clamp_y(iy - 1) * dst_w..][..dst_w];
        let r1 = &horiz[clamp_y(iy) * dst_w..][..dst_w];
        let r2 = &horiz[clamp_y(iy + 1) * dst_w..][..dst_w];
        let r3 = &horiz[clamp_y(iy + 2) * dst_w..][..dst_w];
        let w = cubic_weights(t);
        let out_row = &mut dst[dy * dst_w..(dy + 1) * dst_w];
        for dx in 0..dst_w {
            out_row[dx] = r0[dx] * w[0] + r1[dx] * w[1] + r2[dx] * w[2] + r3[dx] * w[3];
        }
    }
    dst
}

/// Separable bicubic (`INTER_CUBIC`) upscaling for 3-channel `BgrImage` (used by `NameplateFeatures` 4x).
pub fn resize_cubic_bgr(src: &BgrImage, scale: usize) -> BgrImage {
    let dst_w = src.width * scale;
    let dst_h = src.height * scale;
    if src.is_empty() || scale == 0 {
        return BgrImage::default();
    }
    let (b, g, r) = src.split_channels();
    let up = |ch: &GrayImage| -> Vec<u8> {
        let f: Vec<f32> = ch.data.iter().map(|&v| v as f32).collect();
        let res = resize_cubic_f32(&f, ch.width, ch.height, dst_w, dst_h);
        res.into_iter()
            .map(|v| v.round().clamp(0.0, 255.0) as u8)
            .collect()
    };
    let bb = up(&b);
    let gg = up(&g);
    let rr = up(&r);
    let mut data = vec![0u8; dst_w * dst_h * 3];
    for i in 0..dst_w * dst_h {
        data[i * 3] = bb[i];
        data[i * 3 + 1] = gg[i];
        data[i * 3 + 2] = rr[i];
    }
    BgrImage {
        width: dst_w,
        height: dst_h,
        data,
    }
}

/// Normalized cross-correlation template matching (`TM_CCOEFF_NORMED`) accelerated with integral images.
pub fn match_template_ccoeff_normed(
    img: &[f32],
    img_w: usize,
    img_h: usize,
    tpl: &[f32],
    tpl_w: usize,
    tpl_h: usize,
) -> (f64, usize, usize) {
    if tpl_w == 0 || tpl_h == 0 || img_w < tpl_w || img_h < tpl_h {
        return (f64::NEG_INFINITY, 0, 0);
    }
    let n = (tpl_w * tpl_h) as f64;
    let mean_t: f64 = tpl.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mut tpl_zero = vec![0.0f64; tpl.len()];
    let mut norm_t_sq = 0.0f64;
    for (i, &v) in tpl.iter().enumerate() {
        let d = v as f64 - mean_t;
        tpl_zero[i] = d;
        norm_t_sq += d * d;
    }
    if norm_t_sq <= 1e-12 {
        return (0.0, 0, 0);
    }

    // Integral of I and I^2 for O(1) window variance
    let stride = img_w + 1;
    let mut int_s = vec![0.0f64; (img_w + 1) * (img_h + 1)];
    let mut int_sq = vec![0.0f64; (img_w + 1) * (img_h + 1)];
    for y in 0..img_h {
        let mut rs = 0.0f64;
        let mut rsq = 0.0f64;
        for x in 0..img_w {
            let v = img[y * img_w + x] as f64;
            rs += v;
            rsq += v * v;
            int_s[(y + 1) * stride + x + 1] = int_s[y * stride + x + 1] + rs;
            int_sq[(y + 1) * stride + x + 1] = int_sq[y * stride + x + 1] + rsq;
        }
    }

    let out_w = img_w - tpl_w + 1;
    let out_h = img_h - tpl_h + 1;
    let mut best_score = f64::NEG_INFINITY;
    let mut best_x = 0usize;
    let mut best_y = 0usize;

    for y in 0..out_h {
        let y1 = y + tpl_h;
        for x in 0..out_w {
            let x1 = x + tpl_w;
            let sum_i = int_s[y1 * stride + x1] + int_s[y * stride + x]
                - int_s[y * stride + x1]
                - int_s[y1 * stride + x];
            let sum_i_sq = int_sq[y1 * stride + x1] + int_sq[y * stride + x]
                - int_sq[y * stride + x1]
                - int_sq[y1 * stride + x];
            let var_i = (sum_i_sq - (sum_i * sum_i) / n).max(0.0);
            let denom = (var_i * norm_t_sq).sqrt();
            let score = if denom <= 1e-9 {
                0.0
            } else {
                let mut num = 0.0f64;
                for ty in 0..tpl_h {
                    let img_row = &img[(y + ty) * img_w + x..][..tpl_w];
                    let tpl_row = &tpl_zero[ty * tpl_w..(ty + 1) * tpl_w];
                    for tx in 0..tpl_w {
                        num += (img_row[tx] as f64) * tpl_row[tx];
                    }
                }
                (num / denom).clamp(-1.0, 1.0)
            };
            if score > best_score {
                best_score = score;
                best_x = x;
                best_y = y;
            }
        }
    }

    (best_score, best_x, best_y)
}

/// 2D FFT Phase Correlation with 2D Hanning window matching `Cv2.PhaseCorrelate` (for Chromatic Aberration check).
pub fn phase_correlate_hanning(a: &GrayImage, b: &GrayImage) -> (f64, f64, f64) {
    let w = a.width;
    let h = a.height;
    if w < 8 || h < 8 || b.width != w || b.height != h {
        return (0.0, 0.0, 0.0);
    }

    let mut wx = vec![0.0f32; w];
    let mut wy = vec![0.0f32; h];
    for x in 0..w {
        wx[x] = 0.5 * (1.0 - (2.0 * PI * x as f32 / (w - 1) as f32).cos());
    }
    for y in 0..h {
        wy[y] = 0.5 * (1.0 - (2.0 * PI * y as f32 / (h - 1) as f32).cos());
    }

    let mut fa = vec![Complex::new(0.0f32, 0.0f32); w * h];
    let mut fb = vec![Complex::new(0.0f32, 0.0f32); w * h];
    for y in 0..h {
        for x in 0..w {
            let win = wx[x] * wy[y];
            let i = y * w + x;
            fa[i].re = a.data[i] as f32 * win;
            fb[i].re = b.data[i] as f32 * win;
        }
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft_x = planner.plan_fft_forward(w);
    let fft_y = planner.plan_fft_forward(h);
    let ifft_x = planner.plan_fft_inverse(w);
    let ifft_y = planner.plan_fft_inverse(h);

    fft_2d(&mut fa, w, h, &*fft_x, &*fft_y);
    fft_2d(&mut fb, w, h, &*fft_x, &*fft_y);

    for i in 0..w * h {
        let cross = fa[i] * fb[i].conj();
        let mag = cross.norm().max(1e-12);
        fa[i] = cross / mag;
    }

    fft_2d(&mut fa, w, h, &*ifft_x, &*ifft_y);

    let inv_n = 1.0 / ((w * h) as f64);
    let mut best_val = f64::NEG_INFINITY;
    let mut peak_x = 0usize;
    let mut peak_y = 0usize;

    for y in 0..h {
        for x in 0..w {
            let val = fa[y * w + x].re as f64 * inv_n;
            if val > best_val {
                best_val = val;
                peak_x = x;
                peak_y = y;
            }
        }
    }

    // 5x5 weighted sub-pixel centroid around peak (with periodic wrap)
    let mut sum_w = 0.0f64;
    let mut sum_dx = 0.0f64;
    let mut sum_dy = 0.0f64;
    for dy in -2isize..=2 {
        let yy = ((peak_y as isize + dy).rem_euclid(h as isize)) as usize;
        for dx in -2isize..=2 {
            let xx = ((peak_x as isize + dx).rem_euclid(w as isize)) as usize;
            let v = (fa[yy * w + xx].re as f64 * inv_n).max(0.0);
            sum_w += v;
            sum_dx += dx as f64 * v;
            sum_dy += dy as f64 * v;
        }
    }

    let mut shift_x = peak_x as f64 + if sum_w > 0.0 { sum_dx / sum_w } else { 0.0 };
    let mut shift_y = peak_y as f64 + if sum_w > 0.0 { sum_dy / sum_w } else { 0.0 };
    if shift_x > w as f64 / 2.0 {
        shift_x -= w as f64;
    }
    if shift_y > h as f64 / 2.0 {
        shift_y -= h as f64;
    }
    (shift_x, shift_y, sum_w.clamp(0.0, 1.0))
}

fn fft_2d(
    buf: &mut [Complex<f32>],
    w: usize,
    h: usize,
    fft_x: &dyn rustfft::Fft<f32>,
    fft_y: &dyn rustfft::Fft<f32>,
) {
    for y in 0..h {
        fft_x.process(&mut buf[y * w..(y + 1) * w]);
    }
    let mut col = vec![Complex::new(0.0f32, 0.0f32); h];
    for x in 0..w {
        for y in 0..h {
            col[y] = buf[y * w + x];
        }
        fft_y.process(&mut col);
        for y in 0..h {
            buf[y * w + x] = col[y];
        }
    }
}

