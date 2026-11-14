use crate::config::settings::AppSettings;
use crate::mining::reader::MiningSettings;
use crate::models::{
    DebugBoxKind, DebugOverlayInfo, MiningLabelItem, OverlayModel, PixelRect,
};
use crate::overlay::metrics::{BracketMetrics, BracketRasterizer};
use crate::overlay::panel::{PanelLayout, PanelLayoutResult};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use std::sync::OnceLock;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

struct DebugHudLayout {
    card_rect: PixelRect,
    scale: f64,
    header_px: f32,
    body_px: f32,
    header_h: f64,
    line_h: f64,
    pad_x: f64,
    pad_y: f64,
    dot_space: f64,
    pips_reserve: f64,
}

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
    bgra_buf: Vec<u8>,
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
            bgra_buf: Vec::new(),
        }
    }

    pub fn paint(
        &mut self,
        model: &OverlayModel,
        settings: &AppSettings,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        self.paint_slice(model, settings, width, height).to_vec()
    }

    /// Renders into an internal reusable BGRA8 buffer (`width * height * 4` bytes) with zero per-frame heap churn.
    pub fn paint_slice(
        &mut self,
        model: &OverlayModel,
        settings: &AppSettings,
        width: u32,
        height: u32,
    ) -> &[u8] {
        if width == 0 || height == 0 {
            self.bgra_buf.clear();
            return &self.bgra_buf;
        }
        let w = width as usize;
        let h = height as usize;
        let total_bytes = w * h * 4;
        if model.is_empty() {
            self.bgra_buf.resize(total_bytes, 0);
            self.bgra_buf[..total_bytes].fill(0);
            return &self.bgra_buf[..total_bytes];
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
            self.bgra_buf.resize(total_bytes, 0);
            self.bgra_buf[..total_bytes].fill(0);
            return &self.bgra_buf[..total_bytes];
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

        let debug_layout = model.debug_info.as_ref().map(|dbg| {
            let layout = Self::compute_debug_layout(dbg, screen_w, screen_h);
            Self::draw_debug_vectors(
                pixmap,
                dbg,
                &layout,
                model.boundary_rect.is_some(),
            );
            layout
        });

        // Copy tiny-skia's premultiplied RGBA8 buffer into reusable BGRA8 buffer,
        // swapping R <-> B only on non-transparent pixels (98%+ of overlay is transparent 0x00000000).
        let src_rgba = pixmap.data();
        self.bgra_buf.resize(total_bytes, 0);
        let bgra = &mut self.bgra_buf[..total_bytes];
        bgra.copy_from_slice(src_rgba);
        for px in bgra.chunks_exact_mut(4) {
            if px[3] != 0 {
                px.swap(0, 2);
            }
        }

        // 2. Brackets (`{ }`, `[ ]`, `< >`) rasterized directly into premultiplied BGRA8
        if !model.brackets.is_empty() {
            self.brackets.draw(
                bgra,
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
                bgra,
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
            Self::draw_panel_text(bgra, w, h, &panel);
        }

        for (m_item, (box_rect, font_px, pad_x, pad_y)) in mining_layouts {
            Self::draw_colored_text(
                bgra,
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

        if let (Some(dbg), Some(layout)) = (model.debug_info.as_ref(), debug_layout.as_ref()) {
            Self::draw_debug_text(bgra, w, h, dbg, layout);
        }

        &self.bgra_buf[..total_bytes]
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
        px_size: f32,
        center: bool,
    ) {
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            text,
            x,
            box_top,
            max_width,
            box_height,
            px_size,
            center,
            (255, 255, 255),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_outlined_text_rgb(
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
        rgb: (u8, u8, u8),
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

        // 8-direction dark outline (`#E60A0A0F`) followed by crisp fill
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
            (rgb.0, rgb.1, rgb.2, 255),
            false,
        );
    }

    fn compute_debug_layout(
        dbg: &DebugOverlayInfo,
        screen_w: f64,
        screen_h: f64,
    ) -> DebugHudLayout {
        let scale = (screen_h / BracketMetrics::REFERENCE_SCREEN_HEIGHT).clamp(0.65, 2.2);
        let header_px = (14.5 * scale) as f32;
        let body_px = (12.5 * scale) as f32;
        let header_h = (header_px as f64) * 1.45;
        let line_h = (body_px as f64) * 1.35;
        let pad_x = 12.0 * scale;
        let pad_y = 8.0 * scale;
        let dot_space = 18.0 * scale;
        let pips_reserve = 50.0 * scale;

        let (hw, _) = Self::measure_text(&dbg.headline, header_px);
        let mut max_content_w = (hw as f64) + dot_space + pips_reserve;
        for line in &dbg.lines {
            let (lw, _) = Self::measure_text(line, body_px);
            max_content_w = max_content_w.max(lw as f64);
        }
        let min_w = 410.0 * scale;
        let card_w = (max_content_w.max(min_w) + 2.0 * pad_x)
            .min((screen_w * 0.65).max(160.0))
            .ceil();
        let card_h = (pad_y * 2.0 + header_h + (dbg.lines.len() as f64) * line_h + 4.0 * scale)
            .ceil();
        let card_x = (screen_w * 0.015).round();
        let card_y = (screen_h * 0.02).round();

        DebugHudLayout {
            card_rect: PixelRect::new(card_x, card_y, card_w, card_h),
            scale,
            header_px,
            body_px,
            header_h,
            line_h,
            pad_x,
            pad_y,
            dot_space,
            pips_reserve,
        }
    }

    fn draw_stroked_rect(
        pixmap: &mut Pixmap,
        r: PixelRect,
        stroke_w: f32,
        rgba: (u8, u8, u8, u8),
    ) {
        if r.width <= 1.0 || r.height <= 1.0 {
            return;
        }
        let Some(rect) = tiny_skia::Rect::from_xywh(
            r.x as f32,
            r.y as f32,
            r.width as f32,
            r.height as f32,
        ) else {
            return;
        };
        let path = PathBuilder::from_rect(rect);
        let mut paint = Paint::default();
        paint.set_color_rgba8(rgba.0, rgba.1, rgba.2, rgba.3);
        paint.anti_alias = true;
        let mut stroke = Stroke {
            width: stroke_w.max(1.0),
            ..Stroke::default()
        };
        stroke.line_join = tiny_skia::LineJoin::Round;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    fn draw_debug_vectors(
        pixmap: &mut Pixmap,
        dbg: &DebugOverlayInfo,
        layout: &DebugHudLayout,
        boundary_already_drawn: bool,
    ) {
        let s = layout.scale as f32;
        if !boundary_already_drawn {
            if let Some(scan_area) = dbg.scan_boundary {
                Self::draw_stroked_rect(
                    pixmap,
                    scan_area,
                    (1.2 * s).max(1.0),
                    (0x00, 0xE5, 0xFF, 0x48),
                );
            }
        }

        for cand in &dbg.candidate_boxes {
            let (rgba, sw) = match cand.kind {
                DebugBoxKind::Candidate => ((0x00, 0xE5, 0xFF, 0x80), 1.2 * s),
                DebugBoxKind::PairedCandidate => ((0xFF, 0xD5, 0x00, 0xDC), 1.6 * s),
                DebugBoxKind::TentativeTrack => ((0xFF, 0x3D, 0xDB, 0xEB), 2.0 * s),
            };
            Self::draw_stroked_rect(pixmap, cand.rect, sw.max(1.0), rgba);
        }

        // Card background & colored status border
        let (sr, sg, sb) = dbg.level.rgb();
        let radius = 6.0 * layout.scale;
        Self::draw_rounded_box(
            pixmap,
            layout.card_rect,
            radius,
            Color::from_rgba8(0x0B, 0x0F, 0x17, 0xE0),
        );
        Self::draw_stroked_rect(
            pixmap,
            PixelRect::new(
                layout.card_rect.x + 0.5,
                layout.card_rect.y + 0.5,
                (layout.card_rect.width - 1.0).max(1.0),
                (layout.card_rect.height - 1.0).max(1.0),
            ),
            (1.2 * s).max(1.0),
            (sr, sg, sb, 0xB8),
        );

        // Subtle divider line below headline
        let div_y = (layout.card_rect.y + layout.pad_y + layout.header_h + 1.5 * layout.scale) as f32;
        if let Some(div_rect) = tiny_skia::Rect::from_xywh(
            (layout.card_rect.x + layout.pad_x) as f32,
            div_y,
            (layout.card_rect.width - 2.0 * layout.pad_x).max(1.0) as f32,
            (1.0 * s).max(1.0),
        ) {
            let mut div_paint = Paint::default();
            div_paint.set_color_rgba8(sr, sg, sb, 0x3A);
            pixmap.fill_rect(div_rect, &div_paint, Transform::identity(), None);
        }

        // Pulsing status dot on header left
        let dot_cx = (layout.card_rect.x + layout.pad_x + 5.5 * layout.scale) as f32;
        let dot_cy = (layout.card_rect.y + layout.pad_y + layout.header_h * 0.5) as f32;
        let pulse_on = (dbg.heartbeat_phase & 1) == 0;
        let pulse_expand = if pulse_on { 6.8 * s } else { 5.2 * s };
        let pulse_alpha = if pulse_on { 0x58 } else { 0x28 };
        if let Some(outer) = PathBuilder::from_circle(dot_cx, dot_cy, pulse_expand.max(2.0)) {
            let mut p = Paint::default();
            p.set_color_rgba8(sr, sg, sb, pulse_alpha);
            p.anti_alias = true;
            pixmap.fill_path(&outer, &p, FillRule::Winding, Transform::identity(), None);
        }
        if let Some(inner) = PathBuilder::from_circle(dot_cx, dot_cy, (3.8 * s).max(1.5)) {
            let mut p = Paint::default();
            p.set_color_rgba8(sr, sg, sb, 0xFF);
            p.anti_alias = true;
            pixmap.fill_path(&inner, &p, FillRule::Winding, Transform::identity(), None);
        }

        // Marching 4-pip heartbeat scanner bar on header right
        let pip_w = 7.0 * layout.scale;
        let pip_h = 5.5 * layout.scale;
        let pip_gap = 3.5 * layout.scale;
        let total_pips_w = 4.0 * pip_w + 3.0 * pip_gap;
        let pips_x0 = layout.card_rect.right() - layout.pad_x - total_pips_w;
        let pips_y = layout.card_rect.y + layout.pad_y + (layout.header_h - pip_h) * 0.5;
        let active_pip = (dbg.heartbeat_phase % 4) as usize;
        let trail_pip = ((dbg.heartbeat_phase + 3) % 4) as usize;

        for idx in 0..4 {
            let px = pips_x0 + idx as f64 * (pip_w + pip_gap);
            let col = if idx == active_pip {
                Color::from_rgba8(sr, sg, sb, 0xFF)
            } else if idx == trail_pip {
                Color::from_rgba8(sr, sg, sb, 0x70)
            } else {
                Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x24)
            };
            Self::draw_rounded_box(
                pixmap,
                PixelRect::new(px, pips_y, pip_w, pip_h),
                1.5 * layout.scale,
                col,
            );
        }
    }

    fn draw_debug_text(
        bgra: &mut [u8],
        w: usize,
        h: usize,
        dbg: &DebugOverlayInfo,
        layout: &DebugHudLayout,
    ) {
        let (sr, sg, sb) = dbg.level.rgb();
        let head_max_w = (layout.card_rect.width
            - 2.0 * layout.pad_x
            - layout.dot_space
            - layout.pips_reserve)
            .max(20.0);
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            &dbg.headline,
            layout.card_rect.x + layout.pad_x + layout.dot_space,
            layout.card_rect.y + layout.pad_y,
            head_max_w,
            layout.header_h,
            layout.header_px,
            false,
            (sr, sg, sb),
        );

        let body_max_w = (layout.card_rect.width - 2.0 * layout.pad_x).max(20.0);
        let body_top0 =
            layout.card_rect.y + layout.pad_y + layout.header_h + 3.5 * layout.scale;
        for (i, line) in dbg.lines.iter().enumerate() {
            Self::draw_outlined_text_rgb(
                bgra,
                w,
                h,
                line,
                layout.card_rect.x + layout.pad_x,
                body_top0 + i as f64 * layout.line_h,
                body_max_w,
                layout.line_h,
                layout.body_px,
                false,
                (232, 238, 246),
            );
        }
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
