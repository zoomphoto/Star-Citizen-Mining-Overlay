use crate::config::settings::AppSettings;
use crate::mining::reader::MiningSettings;
use crate::models::{MiningLabelItem, OverlayModel, PixelRect};
use crate::overlay::metrics::{BracketMetrics, BracketRasterizer};
use crate::overlay::panel::{PanelLayout, PanelLayoutResult};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use std::sync::OnceLock;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

const QUANTICO_BOLD_TTF: &[u8] = include_bytes!("../../../../assets/Quantico-Bold.ttf");

pub fn embedded_quantico_font() -> &'static FontArc {
    static FONT: OnceLock<FontArc> = OnceLock::new();
    FONT.get_or_init(|| {
        FontArc::try_from_slice(QUANTICO_BOLD_TTF)
            .expect("Embedded assets/Quantico-Bold.ttf must be valid TrueType font")
    })
}

/// Cross-platform software compositor rendering brackets, side panel, boundary, Dossier test box,
/// and mining scan labels into premultiplied BGRA8 (`[B, G, R, A]` — native format for both
/// Win32 `UpdateLayeredWindow` and Wayland `wl_shm::Format::Argb8888` / X11 32-bit ARGB).
pub struct SoftwareOverlayPainter {
    brackets: BracketRasterizer,
    pixmap: Option<Pixmap>,
}

impl Default for SoftwareOverlayPainter {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftwareOverlayPainter {
    const CAP_HEIGHT_EM: f64 = 0.7;

    pub fn new() -> Self {
        Self {
            brackets: BracketRasterizer::new(),
            pixmap: None,
        }
    }

    pub fn paint(
        &mut self,
        model: &OverlayModel,
        settings: &AppSettings,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let w = width as usize;
        let h = height as usize;
        if model.is_empty() {
            return vec![0u8; w * h * 4];
        }

        let need_new = self
            .pixmap
            .as_ref()
            .map(|p| p.width() != width || p.height() != height)
            .unwrap_or(true);
        if need_new {
            self.pixmap = Pixmap::new(width, height);
        }
        let Some(pixmap) = self.pixmap.as_mut() else {
            return vec![0u8; w * h * 4];
        };
        pixmap.fill(Color::TRANSPARENT);

        let screen_w = width as f64;
        let screen_h = height as f64;
        let panel = PanelLayout::compute(
            &model.panel_entries,
            screen_w,
            screen_h,
            settings,
            model.note.as_deref(),
            model.toast.as_deref(),
        );

        // 1. Vector shapes via tiny-skia (premultiplied RGBA8 internally)
        if let Some(area) = model.boundary_rect {
            Self::draw_boundary(pixmap, area, screen_h);
        }
        if let Some(dossier_rect) = model.dossier_placeholder {
            Self::draw_rounded_box(
                pixmap,
                dossier_rect,
                if panel.is_empty() {
                    6.0 * (screen_h / BracketMetrics::REFERENCE_SCREEN_HEIGHT)
                } else {
                    panel.corner_radius
                },
                Color::from_rgba8(0x12, 0x12, 0x1A, 0xCC),
            );
        }
        if !panel.is_empty() {
            for msg in &panel.messages {
                Self::draw_rounded_box(
                    pixmap,
                    msg.rect,
                    panel.corner_radius,
                    Color::from_rgba8(0x12, 0x12, 0x1A, 0xCC),
                );
            }
            for slot in &panel.slots {
                let (r, g, b) = slot.entry.color.rgb();
                Self::draw_rounded_box(
                    pixmap,
                    slot.rect,
                    panel.corner_radius,
                    Color::from_rgba8(r, g, b, 0x66),
                );
            }
        }

        let mining_scale = (settings.mining_label_zoom_percent as f64 / 100.0).clamp(1.0, 2.5);
        let mut mining_layouts = Vec::with_capacity(model.mining_labels.len());
        for m_item in &model.mining_labels {
            let layout = Self::compute_mining_layout(m_item, mining_scale);
            let radius = (layout.0.height * 0.25).max(2.0);
            Self::draw_rounded_box(
                pixmap,
                layout.0,
                radius,
                Color::from_rgba8(
                    m_item.back_colour.r,
                    m_item.back_colour.g,
                    m_item.back_colour.b,
                    MiningSettings::default().label_back_alpha,
                ),
            );
            mining_layouts.push((m_item, layout));
        }

        // Convert tiny-skia's premultiplied RGBA8 buffer to premultiplied BGRA8
        let mut bgra = pixmap.data().to_vec();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        // 2. Brackets (`{ }`, `[ ]`, `< >`) rasterized directly into premultiplied BGRA8
        if !model.brackets.is_empty() {
            self.brackets.draw(
                &mut bgra,
                w * 4,
                w,
                h,
                &model.brackets,
                0.0,
                0.0,
                screen_h,
            );
        }

        // 3. Crisp outlined Quantico-Bold typography into premultiplied BGRA8
        if let Some(place) = model.dossier_placeholder {
            let scale = screen_h / BracketMetrics::REFERENCE_SCREEN_HEIGHT;
            let size = (if panel.is_empty() {
                22.0 * scale
            } else {
                panel.font_size
            }) * 1.5;
            Self::draw_outlined_text(
                &mut bgra,
                w,
                h,
                "Dossier",
                place.x,
                place.y,
                place.width,
                place.height,
                size as f32,
                true,
            );
        }

        if !panel.is_empty() {
            Self::draw_panel_text(&mut bgra, w, h, &panel);
        }

        for (m_item, (box_rect, font_px, pad_x, pad_y)) in mining_layouts {
            Self::draw_colored_text(
                &mut bgra,
                w,
                h,
                &m_item.text,
                (box_rect.x + pad_x) as f32,
                (box_rect.y + pad_y) as f32,
                font_px,
                (
                    m_item.text_colour.r,
                    m_item.text_colour.g,
                    m_item.text_colour.b,
                    255,
                ),
                false,
            );
        }

        bgra
    }

    fn draw_boundary(pixmap: &mut Pixmap, area: PixelRect, screen_h: f64) {
        let stroke_w = ((screen_h / BracketMetrics::REFERENCE_SCREEN_HEIGHT) * 1.5).max(1.0) as f32;
        let inset = (stroke_w + 2.0) * 0.5;
        let rx = area.x as f32 + inset;
        let ry = area.y as f32 + inset;
        let rw = (area.width as f32 - 2.0 * inset).max(0.0);
        let rh = (area.height as f32 - 2.0 * inset).max(0.0);
        if rw <= 0.0 || rh <= 0.0 {
            return;
        }
        let Some(rect) = tiny_skia::Rect::from_xywh(rx, ry, rw, rh) else {
            return;
        };
        let path = PathBuilder::from_rect(rect);

        let mut dark_paint = Paint::default();
        dark_paint.set_color_rgba8(0x0A, 0x0A, 0x0F, 0xE6);
        dark_paint.anti_alias = true;
        let mut dark_stroke = Stroke {
            width: stroke_w + 2.0,
            ..Stroke::default()
        };
        dark_stroke.line_join = tiny_skia::LineJoin::Round;
        pixmap.stroke_path(&path, &dark_paint, &dark_stroke, Transform::identity(), None);

        let mut yellow_paint = Paint::default();
        yellow_paint.set_color_rgba8(0xFF, 0xD5, 0x00, 0xFF);
        yellow_paint.anti_alias = true;
        let mut yellow_stroke = Stroke {
            width: stroke_w,
            ..Stroke::default()
        };
        yellow_stroke.line_join = tiny_skia::LineJoin::Round;
        pixmap.stroke_path(
            &path,
            &yellow_paint,
            &yellow_stroke,
            Transform::identity(),
            None,
        );
    }

    fn draw_rounded_box(pixmap: &mut Pixmap, r: PixelRect, radius: f64, color: Color) {
        if r.width <= 0.0 || r.height <= 0.0 {
            return;
        }
        let x = r.x as f32;
        let y = r.y as f32;
        let w = r.width as f32;
        let h = r.height as f32;
        let rad = (radius as f32).min(w * 0.5).min(h * 0.5).max(0.0);

        let mut pb = PathBuilder::new();
        pb.move_to(x + rad, y);
        pb.line_to(x + w - rad, y);
        pb.quad_to(x + w, y, x + w, y + rad);
        pb.line_to(x + w, y + h - rad);
        pb.quad_to(x + w, y + h, x + w - rad, y + h);
        pb.line_to(x + rad, y + h);
        pb.quad_to(x, y + h, x, y + h - rad);
        pb.line_to(x, y + rad);
        pb.quad_to(x, y, x + rad, y);
        pb.close();

        let Some(path) = pb.finish() else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    fn draw_panel_text(bgra: &mut [u8], w: usize, h: usize, panel: &PanelLayoutResult) {
        let fs = panel.font_size as f32;
        for msg in &panel.messages {
            let max_w = (msg.rect.width - 2.0 * panel.padding_x).max(1.0);
            for (i, line) in msg.lines.iter().enumerate() {
                Self::draw_outlined_text(
                    bgra,
                    w,
                    h,
                    line,
                    msg.rect.x + panel.padding_x,
                    msg.rect.y + panel.padding_y + i as f64 * panel.line_height,
                    max_w,
                    panel.line_height,
                    fs,
                    msg.centered,
                );
            }
        }

        for slot in &panel.slots {
            let tx = slot.rect.x + panel.padding_x;
            let max_w = (slot.rect.width - 2.0 * panel.padding_x).max(1.0);
            Self::draw_outlined_text(
                bgra,
                w,
                h,
                &slot.entry.handle,
                tx,
                slot.rect.y + panel.padding_y,
                max_w,
                panel.line_height,
                fs,
                false,
            );
            for (i, line) in slot.org_lines.iter().enumerate() {
                Self::draw_outlined_text(
                    bgra,
                    w,
                    h,
                    line,
                    tx,
                    slot.rect.y + panel.padding_y + (i + 1) as f64 * panel.line_height,
                    max_w,
                    panel.line_height,
                    fs,
                    false,
                );
            }
        }

        if let Some(more) = panel.more_text.as_deref() {
            let r = panel.more_rect;
            Self::draw_outlined_text(
                bgra,
                w,
                h,
                more,
                r.x + panel.padding_x,
                r.y,
                (r.width - 2.0 * panel.padding_x).max(1.0),
                panel.line_height,
                fs,
                false,
            );
        }
    }

    fn compute_mining_layout(
        label: &MiningLabelItem,
        scale: f64,
    ) -> (PixelRect, f32, f64, f64) {
        let settings = MiningSettings::default();
        let h = (label.digit_height * scale).max(6.0);
        let font_px = (h / Self::CAP_HEIGHT_EM) as f32;
        let (tw, th) = Self::measure_text(&label.text, font_px);
        let pad_x = settings.label_pad_x_lines * h;
        let pad_y = settings.label_pad_y_lines * h;
        let bw = tw as f64 + 2.0 * pad_x;
        let bh = th as f64 + 2.0 * pad_y;
        let centre = label.signal_box.x + label.signal_box.width * 0.5;
        let top = label.signal_box.bottom() + settings.label_gap_lines * h;
        let box_rect = PixelRect::new(
            (centre - bw * 0.5).round(),
            top.round(),
            bw.ceil(),
            bh.ceil(),
        );
        (box_rect, font_px, pad_x, pad_y)
    }

    fn measure_text(text: &str, px_size: f32) -> (f32, f32) {
        let font = embedded_quantico_font();
        let scaled = font.as_scaled(PxScale::from(px_size.max(4.0)));
        let mut width = 0.0f32;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        for ch in text.chars() {
            let gid = scaled.glyph_id(ch);
            if let Some(p) = prev {
                width += scaled.kern(p, gid);
            }
            width += scaled.h_advance(gid);
            prev = Some(gid);
        }
        let height = scaled.ascent() - scaled.descent();
        (width, height.max(px_size))
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_outlined_text(
        bgra: &mut [u8],
        w: usize,
        h: usize,
        text: &str,
        x: f64,
        box_top: f64,
        max_width: f64,
        box_height: f64,
        mut px_size: f32,
        center: bool,
    ) {
        let (mut tw, mut th) = Self::measure_text(text, px_size);
        if tw as f64 > max_width && tw > 0.0 {
            let ratio = (max_width / tw as f64).max(0.7) as f32;
            px_size *= ratio;
            (tw, th) = Self::measure_text(text, px_size);
        }
        let draw_x = if center {
            x + ((max_width - tw as f64).max(0.0) * 0.5)
        } else {
            x
        } as f32;
        let draw_y = (box_top + (box_height - th as f64) * 0.5) as f32;

        // 8-direction dark outline (`#E60A0A0F`) followed by crisp white fill
        let edge = (px_size * 0.08).max(1.0).round() as i32;
        for dy in -edge..=edge {
            for dx in -edge..=edge {
                if dx == 0 && dy == 0 {
                    continue;
                }
                if dx * dx + dy * dy > edge * edge + 1 {
                    continue;
                }
                Self::draw_colored_text(
                    bgra,
                    w,
                    h,
                    text,
                    draw_x + dx as f32,
                    draw_y + dy as f32,
                    px_size,
                    (0x0A, 0x0A, 0x0F, 0xE6),
                    true,
                );
            }
        }
        Self::draw_colored_text(
            bgra,
            w,
            h,
            text,
            draw_x,
            draw_y,
            px_size,
            (255, 255, 255, 255),
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_colored_text(
        bgra: &mut [u8],
        w: usize,
        h: usize,
        text: &str,
        x: f32,
        y_top: f32,
        px_size: f32,
        rgba: (u8, u8, u8, u8),
        _is_outline: bool,
    ) {
        let font = embedded_quantico_font();
        let scale = PxScale::from(px_size.max(4.0));
        let scaled = font.as_scaled(scale);
        let baseline_y = y_top + scaled.ascent();

        let mut cursor_x = x;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        let (cr, cg, cb, ca) = (
            rgba.0 as f32,
            rgba.1 as f32,
            rgba.2 as f32,
            rgba.3 as f32 / 255.0,
        );

        for ch in text.chars() {
            let gid = scaled.glyph_id(ch);
            if let Some(p) = prev {
                cursor_x += scaled.kern(p, gid);
            }
            let glyph = gid.with_scale_and_position(scale, ab_glyph::point(cursor_x, baseline_y));
            cursor_x += scaled.h_advance(gid);
            prev = Some(gid);

            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                let bx0 = bounds.min.x.floor() as i32;
                let by0 = bounds.min.y.floor() as i32;
                outlined.draw(|gx, gy, cov| {
                    if cov <= 0.01 {
                        return;
                    }
                    let px = bx0 + gx as i32;
                    let py = by0 + gy as i32;
                    if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                        return;
                    }
                    let a = (cov * ca).clamp(0.0, 1.0);
                    let inv = 1.0 - a;
                    let idx = (py as usize * w + px as usize) * 4;
                    bgra[idx] = (cb * a + bgra[idx] as f32 * inv).round().clamp(0.0, 255.0) as u8;
                    bgra[idx + 1] =
                        (cg * a + bgra[idx + 1] as f32 * inv).round().clamp(0.0, 255.0) as u8;
                    bgra[idx + 2] =
                        (cr * a + bgra[idx + 2] as f32 * inv).round().clamp(0.0, 255.0) as u8;
                    bgra[idx + 3] = (255.0 * a + bgra[idx + 3] as f32 * inv)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                });
            }
        }
    }
}
