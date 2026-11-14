use crate::models::{BracketItem, MatchKind, PixelRect};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BracketMetrics {
    pub top: f64,
    pub height: f64,
    pub width: f64,
    pub gap: f64,
    pub stroke: f64,
    pub outline: f64,
}

impl BracketMetrics {
    pub const REFERENCE_SCREEN_HEIGHT: f64 = 1440.0;
    pub const REFERENCE_BOX_HEIGHT: f64 = 41.0;
    pub const MIN_CLEAR_PX: f64 = 20.0;

    pub fn scale(screen_height: f64) -> f64 {
        if screen_height > 0.0 {
            screen_height / Self::REFERENCE_SCREEN_HEIGHT
        } else {
            1.0
        }
    }

    pub fn for_box(plate_box: PixelRect, screen_height: f64) -> Self {
        let scale = Self::scale(screen_height);
        let mut height = Self::REFERENCE_BOX_HEIGHT * 1.24;
        let mut stroke = (height * 0.06).max(2.0);
        let mut outline = (stroke * 0.5).max(1.0);
        let mut width = height * 0.24;

        height *= scale;
        width *= scale;
        stroke *= scale;
        outline *= scale;

        Self {
            top: plate_box.y + plate_box.height * 0.5 - height * 0.5,
            height,
            width,
            gap: Self::MIN_CLEAR_PX * scale + stroke * 0.5 + outline,
            stroke,
            outline,
        }
    }

    pub fn left_x(&self, box_rect: PixelRect) -> f64 {
        box_rect.x - self.gap - self.width
    }

    pub fn right_x(&self, box_rect: PixelRect) -> f64 {
        box_rect.right() + self.gap
    }

    pub fn bleed(&self) -> f64 {
        self.stroke * 0.5 + self.outline
    }

    pub fn extent(&self, box_rect: PixelRect) -> PixelRect {
        let half = self.bleed();
        let left = self.left_x(box_rect) - half;
        let right = self.right_x(box_rect) + self.width + half;
        PixelRect::new(left, self.top - half, right - left, self.height + 2.0 * half)
    }

    pub fn extent_of(plate_box: PixelRect, screen_height: f64) -> PixelRect {
        Self::for_box(plate_box, screen_height).extent(plate_box)
    }
}

/// Anti-aliased distance-field rasterizer drawing `{ }`, `[ ]`, and `< >` brackets directly
/// into premultiplied BGRA8 (`[B, G, R, A]`) pixel buffers.
#[derive(Debug, Default)]
pub struct BracketRasterizer {
    left_segs: Vec<[f32; 4]>,
    right_segs: Vec<[f32; 4]>,
    coverage: Vec<f32>,
}

impl BracketRasterizer {
    pub const OPACITY: f32 = 0.6;
    const CURVE_STEPS: usize = 8;
    const DARK_ALPHA: f32 = (0xE6 as f32 / 255.0) * Self::OPACITY;
    const COLOR_ALPHA: f32 = Self::OPACITY;
    const DARK_RGB: (f32, f32, f32) = (0x0A as f32, 0x0A as f32, 0x0F as f32);

    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pixels: &mut [u8],
        stride: usize,
        width: usize,
        height: usize,
        brackets: &[BracketItem],
        origin_x: f64,
        origin_y: f64,
        screen_height: f64,
    ) {
        if width == 0
            || height == 0
            || brackets.is_empty()
            || stride < width * 4
            || pixels.len() < stride * (height - 1) + width * 4
        {
            return;
        }
        if self.coverage.len() < width * height {
            self.coverage.resize(width * height, 0.0);
        }

        for item in brackets {
            let b = item.name_box;
            let m = BracketMetrics::for_box(b, screen_height);
            Self::build_shape(
                &mut self.left_segs,
                item.kind,
                m.left_x(b) - origin_x,
                m.top - origin_y,
                m.width,
                m.height,
                false,
            );
            Self::build_shape(
                &mut self.right_segs,
                item.kind,
                m.right_x(b) - origin_x,
                m.top - origin_y,
                m.width,
                m.height,
                true,
            );

            let (r, g, bl) = item.color.rgb();
            let dark_radius = ((m.stroke + 2.0 * m.outline) * 0.5) as f32;
            let color_radius = (m.stroke * 0.5) as f32;

            let (dr, dg, db) = Self::DARK_RGB;
            Self::stroke_segments(
                &mut self.coverage,
                pixels,
                stride,
                width,
                height,
                &self.left_segs,
                dark_radius,
                dr,
                dg,
                db,
                Self::DARK_ALPHA,
            );
            Self::stroke_segments(
                &mut self.coverage,
                pixels,
                stride,
                width,
                height,
                &self.right_segs,
                dark_radius,
                dr,
                dg,
                db,
                Self::DARK_ALPHA,
            );
            Self::stroke_segments(
                &mut self.coverage,
                pixels,
                stride,
                width,
                height,
                &self.left_segs,
                color_radius,
                r as f32,
                g as f32,
                bl as f32,
                Self::COLOR_ALPHA,
            );
            Self::stroke_segments(
                &mut self.coverage,
                pixels,
                stride,
                width,
                height,
                &self.right_segs,
                color_radius,
                r as f32,
                g as f32,
                bl as f32,
                Self::COLOR_ALPHA,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn stroke_segments(
        coverage: &mut [f32],
        pixels: &mut [u8],
        stride: usize,
        width: usize,
        height: usize,
        segs: &[[f32; 4]],
        radius: f32,
        r: f32,
        g: f32,
        b: f32,
        alpha: f32,
    ) {
        if segs.is_empty() {
            return;
        }
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let mut any = false;

        for &[x0, y0, x1, y1] in segs {
            let left_i = (x0.min(x1) - radius - 1.0).floor() as isize;
            let right_i = (x0.max(x1) + radius + 1.0).ceil() as isize;
            let top_i = (y0.min(y1) - radius - 1.0).floor() as isize;
            let bottom_i = (y0.max(y1) + radius + 1.0).ceil() as isize;

            if right_i < 0 || bottom_i < 0 || left_i >= width as isize || top_i >= height as isize {
                continue;
            }
            let left = left_i.max(0) as usize;
            let right = right_i.min(width as isize - 1) as usize;
            let top = top_i.max(0) as usize;
            let bottom = bottom_i.min(height as isize - 1) as usize;
            if left > right || top > bottom {
                continue;
            }

            min_x = min_x.min(left);
            max_x = max_x.max(right);
            min_y = min_y.min(top);
            max_y = max_y.max(bottom);
            any = true;

            let dx = x1 - x0;
            let dy = y1 - y0;
            let len_sq = dx * dx + dy * dy;

            for y in top..=bottom {
                let py = y as f32 + 0.5;
                let row = y * width;
                for x in left..=right {
                    let px = x as f32 + 0.5;
                    let t = if len_sq > 0.0 {
                        (((px - x0) * dx + (py - y0) * dy) / len_sq).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let ex = px - (x0 + t * dx);
                    let ey = py - (y0 + t * dy);
                    let c = (radius + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0);
                    if c > coverage[row + x] {
                        coverage[row + x] = c;
                    }
                }
            }
        }

        if !any {
            return;
        }
        let to_u8 = |v: f32| v.clamp(0.0, 255.0).round() as u8;
        for y in min_y..=max_y {
            let row = y * width;
            for x in min_x..=max_x {
                let c = coverage[row + x];
                if c <= 0.0 {
                    continue;
                }
                coverage[row + x] = 0.0;
                let a = c * alpha;
                let inv = 1.0 - a;
                let i = y * stride + x * 4;
                pixels[i] = to_u8(b * a + pixels[i] as f32 * inv);
                pixels[i + 1] = to_u8(g * a + pixels[i + 1] as f32 * inv);
                pixels[i + 2] = to_u8(r * a + pixels[i + 2] as f32 * inv);
                pixels[i + 3] = to_u8(255.0 * a + pixels[i + 3] as f32 * inv);
            }
        }
    }

    fn build_shape(
        segs: &mut Vec<[f32; 4]>,
        kind: MatchKind,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        mirrored: bool,
    ) {
        segs.clear();
        let pt = |u: f64, v: f64| -> (f32, f32) {
            let px = if mirrored {
                x + (1.0 - u) * width
            } else {
                x + u * width
            };
            let py = y + v * height;
            (px as f32, py as f32)
        };
        let add_line = |segs: &mut Vec<[f32; 4]>, from: (f32, f32), to: (f32, f32)| -> (f32, f32) {
            segs.push([from.0, from.1, to.0, to.1]);
            to
        };
        let add_curve = |segs: &mut Vec<[f32; 4]>,
                         from: (f32, f32),
                         ctrl: (f32, f32),
                         to: (f32, f32)|
         -> (f32, f32) {
            let mut prev = from;
            for i in 1..=Self::CURVE_STEPS {
                let t = i as f32 / Self::CURVE_STEPS as f32;
                let mt = 1.0 - t;
                let next = (
                    mt * mt * from.0 + 2.0 * mt * t * ctrl.0 + t * t * to.0,
                    mt * mt * from.1 + 2.0 * mt * t * ctrl.1 + t * t * to.1,
                );
                segs.push([prev.0, prev.1, next.0, next.1]);
                prev = next;
            }
            to
        };

        match kind {
            MatchKind::PlayerList => {
                let mut cur = pt(1.0, 0.0);
                cur = add_curve(segs, cur, pt(0.5, 0.0), pt(0.5, 0.12));
                cur = add_line(segs, cur, pt(0.5, 0.38));
                cur = add_curve(segs, cur, pt(0.5, 0.5), pt(0.0, 0.5));
                cur = add_curve(segs, cur, pt(0.5, 0.5), pt(0.5, 0.62));
                cur = add_line(segs, cur, pt(0.5, 0.88));
                add_curve(segs, cur, pt(0.5, 1.0), pt(1.0, 1.0));
            }
            MatchKind::OrgList => {
                let mut cur = pt(1.0, 0.0);
                cur = add_line(segs, cur, pt(0.0, 0.0));
                cur = add_line(segs, cur, pt(0.0, 1.0));
                add_line(segs, cur, pt(1.0, 1.0));
            }
            MatchKind::OrgAttribute | MatchKind::NoMatch => {
                let mut cur = pt(1.0, 0.0);
                cur = add_line(segs, cur, pt(0.0, 0.5));
                add_line(segs, cur, pt(1.0, 1.0));
            }
        }
    }
}

pub struct OverlayWindowFit;

impl OverlayWindowFit {
    pub const SLACK_AREA: f64 = 256.0 * 256.0;
    pub const SHRINK_FACTOR: f64 = 2.0;

    pub fn union(rects: impl IntoIterator<Item = PixelRect>) -> Option<PixelRect> {
        let mut left = 0.0f64;
        let mut top = 0.0f64;
        let mut right = 0.0f64;
        let mut bottom = 0.0f64;
        let mut any = false;
        for r in rects {
            if r.is_empty() {
                continue;
            }
            if !any {
                left = r.x;
                top = r.y;
                right = r.right();
                bottom = r.bottom();
                any = true;
            } else {
                left = left.min(r.x);
                top = top.min(r.y);
                right = right.max(r.right());
                bottom = bottom.max(r.bottom());
            }
        }
        if any {
            Some(PixelRect::new(left, top, right - left, bottom - top))
        } else {
            None
        }
    }

    pub fn inflate(r: PixelRect, by: f64) -> PixelRect {
        PixelRect::new(r.x - by, r.y - by, r.width + 2.0 * by, r.height + 2.0 * by)
    }

    pub fn clip(a: PixelRect, bounds: PixelRect) -> PixelRect {
        let left = a.x.max(bounds.x);
        let top = a.y.max(bounds.y);
        let right = a.right().min(bounds.right());
        let bottom = a.bottom().min(bounds.bottom());
        if right <= left || bottom <= top {
            PixelRect::default()
        } else {
            PixelRect::new(left, top, right - left, bottom - top)
        }
    }

    pub fn contains(outer: PixelRect, inner: PixelRect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    pub fn choose(
        current: Option<PixelRect>,
        needed: PixelRect,
        bounds: PixelRect,
        margin: f64,
    ) -> PixelRect {
        let clipped = Self::clip(needed, bounds);
        if clipped.is_empty() {
            return PixelRect::default();
        }
        let grown = Self::clip(Self::inflate(clipped, margin), bounds);
        if let Some(have) = current {
            if !have.is_empty()
                && Self::contains(have, clipped)
                && Self::contains(bounds, have)
                && have.width * have.height
                    <= Self::SHRINK_FACTOR * grown.width * grown.height + Self::SLACK_AREA
            {
                return have;
            }
        }
        grown
    }
}
