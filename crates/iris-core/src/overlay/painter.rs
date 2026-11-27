use crate::config::settings::AppSettings;
use crate::mining::reader::MiningSettings;
use crate::models::{
    BreadcrumbOverlayInfo, DebugBoxKind, DebugOverlayInfo, MiningLabelItem, OverlayModel,
    PixelRect, ProximityZone, WaypointOverlayInfo,
};
use crate::overlay::metrics::{BracketMetrics, BracketRasterizer};
use crate::overlay::panel::{PanelLayout, PanelLayoutResult};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use std::sync::OnceLock;
use tiny_skia::{
    Color, FillRule, GradientStop, Paint, PathBuilder, Pixmap, Point, RadialGradient, SpreadMode,
    Stroke, Transform,
};

struct BreadcrumbHudLayout {
    card_rect: PixelRect,
    radar_rect: PixelRect,
    banner_rect: Option<PixelRect>,
    scale: f64,
    header_px: f32,
    body_px: f32,
    banner_px: f32,
    pad_x: f64,
    pad_y: f64,
    header_h: f64,
    footer_line_h: f64,
}

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

struct WaypointHudLayout {
    card_rect: PixelRect,
    dial_cx: f64,
    dial_cy: f64,
    dial_r: f64,
    text_x: f64,
    text_w: f64,
    scale: f64,
    title_px: f32,
    dist_px: f32,
    sub_px: f32,
    pad_y: f64,
    top_row_h: f64,
    sub_row_h: f64,
    ring_marker: Option<WaypointRingMarkerLayout>,
}

struct WaypointRingMarkerLayout {
    icon_x: f64,
    icon_y: f64,
    dir_x: f64,
    dir_y: f64,
    pill_rect: PixelRect,
    name_px: f32,
    dist_px: f32,
    line_h: f64,
    pad_x: f64,
    pad_y: f64,
    short_name: String,
    dist_line: String,
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
            Self::draw_debug_vectors(pixmap, dbg, &layout, model.boundary_rect.is_some());
            layout
        });

        let breadcrumb_layout = model.breadcrumb_info.as_ref().map(|bc| {
            let layout = Self::compute_breadcrumb_layout(bc, screen_w, screen_h);
            Self::draw_breadcrumb_vectors(pixmap, bc, &layout, screen_w, screen_h);
            layout
        });

        let top_banner_bottom = breadcrumb_layout
            .as_ref()
            .and_then(|b| b.banner_rect)
            .map(|r| r.bottom());
        let waypoint_layout = model.waypoint_info.as_ref().map(|wp| {
            let layout = Self::compute_waypoint_layout(wp, screen_w, screen_h, top_banner_bottom);
            Self::draw_waypoint_vectors(pixmap, wp, &layout);
            layout
        });

        let center_toast_layout = model.center_toast.as_deref().map(|msg| {
            let scale = (screen_h / BracketMetrics::REFERENCE_SCREEN_HEIGHT).clamp(0.75, 2.25);
            let font_px = (14.5 * scale) as f32;
            let (tw, _) = Self::measure_text(msg, font_px);
            let pad_x = 16.0 * scale;
            let pad_y = 7.0 * scale;
            let bw = ((tw as f64) + 2.0 * pad_x).clamp(240.0 * scale, screen_w * 0.65);
            let bh = (font_px as f64) * 1.35 + 2.0 * pad_y;
            let anchor_y = waypoint_layout
                .as_ref()
                .map(|wl| wl.card_rect.bottom() + 10.0 * scale)
                .or_else(|| top_banner_bottom.map(|b| b + 10.0 * scale))
                .unwrap_or_else(|| (84.0 * scale).max(screen_h * 0.075));
            let rect = PixelRect::new(((screen_w - bw) * 0.5).round(), anchor_y.round(), bw.ceil(), bh.ceil());
            let rad = 7.0 * scale;
            Self::draw_rounded_box(pixmap, rect, rad, Color::from_rgba8(0x0B, 0x10, 0x18, 0xE6));
            Self::stroke_rounded_box(
                pixmap,
                rect,
                rad,
                Color::from_rgba8(0x00, 0xE5, 0xFF, 0xEC),
                (1.5 * scale).max(1.0) as f32,
            );
            (rect, font_px, pad_x, pad_y)
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
            self.brackets
                .draw(bgra, w * 4, w, h, &model.brackets, 0.0, 0.0, screen_h);
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

        if let (Some(bc), Some(layout)) =
            (model.breadcrumb_info.as_ref(), breadcrumb_layout.as_ref())
        {
            Self::draw_breadcrumb_text(bgra, w, h, bc, layout);
        }

        if let (Some(wp), Some(layout)) = (model.waypoint_info.as_ref(), waypoint_layout.as_ref()) {
            Self::draw_waypoint_text(bgra, w, h, wp, layout);
        }

        if let (Some(msg), Some((rect, font_px, pad_x, pad_y))) =
            (model.center_toast.as_deref(), center_toast_layout)
        {
            Self::draw_outlined_text_rgb(
                bgra,
                w,
                h,
                msg,
                rect.x + pad_x,
                rect.y + pad_y,
                (rect.width - 2.0 * pad_x).max(40.0),
                (rect.height - 2.0 * pad_y).max(16.0),
                font_px,
                true,
                (0x00, 0xE5, 0xFF),
            );
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
        pixmap.stroke_path(
            &path,
            &dark_paint,
            &dark_stroke,
            Transform::identity(),
            None,
        );

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

    fn stroke_rounded_box(
        pixmap: &mut Pixmap,
        r: PixelRect,
        radius: f64,
        color: Color,
        stroke_width: f32,
    ) {
        if r.width <= 0.0 || r.height <= 0.0 || stroke_width <= 0.0 {
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
        let stroke = Stroke {
            width: stroke_width,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
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

    fn compute_mining_layout(label: &MiningLabelItem, scale: f64) -> (PixelRect, f32, f64, f64) {
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
        let card_h =
            (pad_y * 2.0 + header_h + (dbg.lines.len() as f64) * line_h + 4.0 * scale).ceil();
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

    fn draw_stroked_rect(pixmap: &mut Pixmap, r: PixelRect, stroke_w: f32, rgba: (u8, u8, u8, u8)) {
        if r.width <= 1.0 || r.height <= 1.0 {
            return;
        }
        let Some(rect) =
            tiny_skia::Rect::from_xywh(r.x as f32, r.y as f32, r.width as f32, r.height as f32)
        else {
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
        let div_y =
            (layout.card_rect.y + layout.pad_y + layout.header_h + 1.5 * layout.scale) as f32;
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
        let head_max_w =
            (layout.card_rect.width - 2.0 * layout.pad_x - layout.dot_space - layout.pips_reserve)
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
        let body_top0 = layout.card_rect.y + layout.pad_y + layout.header_h + 3.5 * layout.scale;
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

    fn compute_breadcrumb_layout(
        bc: &BreadcrumbOverlayInfo,
        screen_w: f64,
        screen_h: f64,
    ) -> BreadcrumbHudLayout {
        let scale = (screen_h / 1080.0).clamp(0.75, 2.25);
        let pad_x = 10.0 * scale;
        let pad_y = 8.0 * scale;
        let header_px = (12.5 * scale) as f32;
        let body_px = (10.5 * scale) as f32;
        let banner_px = (15.5 * scale) as f32;
        let header_h = header_px as f64 * 1.25;
        let footer_line_h = body_px as f64 * 1.28;

        let card_w = (264.0 * scale).min((screen_w * 0.28).max(180.0));
        let radar_w = (card_w - 2.0 * pad_x).max(120.0);
        let radar_h = (166.0 * scale).min((screen_h * 0.24).max(110.0));
        let card_h =
            pad_y * 2.0 + header_h + 6.0 * scale + radar_h + 6.0 * scale + footer_line_h * 2.0;

        let margin_x = (18.0 * scale).min(screen_w * 0.04);
        let margin_y = (22.0 * scale).min(screen_h * 0.05);
        let card_y = (screen_h - card_h - margin_y).max(margin_y);
        let card_rect = PixelRect::new(margin_x, card_y, card_w, card_h);
        let radar_rect = PixelRect::new(
            card_rect.x + pad_x,
            card_rect.y + pad_y + header_h + 6.0 * scale,
            radar_w,
            radar_h,
        );

        let banner_rect = if bc.show_screen_cloud && bc.zone != ProximityZone::Clear {
            let bw = (420.0 * scale).min(screen_w * 0.52);
            let bh = banner_px as f64 * 1.35 + 14.0 * scale;
            Some(PixelRect::new(
                (screen_w - bw) * 0.5,
                (16.0 * scale).min(screen_h * 0.03),
                bw,
                bh,
            ))
        } else {
            None
        };

        BreadcrumbHudLayout {
            card_rect,
            radar_rect,
            banner_rect,
            scale,
            header_px,
            body_px,
            banner_px,
            pad_x,
            pad_y,
            header_h,
            footer_line_h,
        }
    }

    fn draw_breadcrumb_vectors(
        pixmap: &mut Pixmap,
        bc: &BreadcrumbOverlayInfo,
        layout: &BreadcrumbHudLayout,
        screen_w: f64,
        screen_h: f64,
    ) {
        let (zr, zg, zb) = bc.zone.rgb();

        // 1. Ambient translucent color cloud along top-center HUD edge when entering/inside visited zone
        if bc.show_screen_cloud && bc.zone != ProximityZone::Clear {
            let cloud_w = (screen_w * 0.65).max(320.0) as f32;
            let cloud_h = (screen_h * 0.19).max(110.0) as f32;
            let cloud_x = ((screen_w as f32) - cloud_w) * 0.5;
            if let Some(rect) = tiny_skia::Rect::from_xywh(cloud_x, 0.0, cloud_w, cloud_h) {
                let (a_inner, a_mid) = match bc.zone {
                    ProximityZone::Visited => (118u8, 54u8),
                    ProximityZone::Approaching => (96u8, 42u8),
                    ProximityZone::Clear => (0u8, 0u8),
                };
                // Draw an elliptical radial gradient centered at top-center (screen_w/2, 0)
                let unit_center = Point::from_xy(0.0, 0.0);
                let stops = vec![
                    GradientStop::new(0.0, Color::from_rgba8(zr, zg, zb, a_inner)),
                    GradientStop::new(0.45, Color::from_rgba8(zr, zg, zb, a_mid)),
                    GradientStop::new(1.0, Color::from_rgba8(zr, zg, zb, 0)),
                ];
                let xform = Transform::from_scale(cloud_w * 0.52, cloud_h)
                    .post_translate(screen_w as f32 * 0.5, 0.0);
                if let Some(shader) = RadialGradient::new(
                    unit_center,
                    unit_center,
                    1.0,
                    stops,
                    SpreadMode::Pad,
                    xform,
                ) {
                    let mut paint = Paint::default();
                    paint.shader = shader;
                    paint.anti_alias = true;
                    let path = PathBuilder::from_rect(rect);
                    pixmap.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }

            if let Some(banner) = layout.banner_rect {
                let rad = 7.0 * layout.scale;
                Self::draw_rounded_box(
                    pixmap,
                    banner,
                    rad,
                    Color::from_rgba8(0x10, 0x12, 0x18, 0xCC),
                );
                Self::stroke_rounded_box(
                    pixmap,
                    banner,
                    rad,
                    Color::from_rgba8(zr, zg, zb, 0xE6),
                    (1.5 * layout.scale).max(1.0) as f32,
                );
            }
        }

        // 2. Bottom-left 2D Breadcrumb Cloud Radar Card
        let card_rad = 7.0 * layout.scale;
        Self::draw_rounded_box(
            pixmap,
            layout.card_rect,
            card_rad,
            Color::from_rgba8(0x0D, 0x11, 0x18, 0xDC),
        );
        Self::stroke_rounded_box(
            pixmap,
            layout.card_rect,
            card_rad,
            Color::from_rgba8(zr, zg, zb, 0xB8),
            (1.25 * layout.scale).max(1.0) as f32,
        );

        let r_rect = layout.radar_rect;
        let radar_rad = 5.0 * layout.scale;
        Self::draw_rounded_box(
            pixmap,
            r_rect,
            radar_rad,
            Color::from_rgba8(0x07, 0x0A, 0x10, 0xEC),
        );

        let Some(sk_radar) = tiny_skia::Rect::from_xywh(
            r_rect.x as f32,
            r_rect.y as f32,
            r_rect.width as f32,
            r_rect.height as f32,
        ) else {
            return;
        };
        let radar_clip_path = PathBuilder::from_rect(sk_radar);

        // Scale radar extent from user-selected zoom (1 km .. 1000 km), or fallback to 3x visited_radius_km.
        let max_extent_km = if bc.radar_range_km > 0.0 {
            bc.radar_range_km.clamp(0.5, 5_000.0)
        } else {
            (bc.visited_radius_km * 3.0).max(10.0)
        };
        let half_w = r_rect.width * 0.5;
        let half_h = r_rect.height * 0.5;
        let max_r = half_w.min(half_h);
        let cx = r_rect.x + half_w;
        let cy = r_rect.y + half_h;
        let px_per_km = max_r / max_extent_km;
        let cloud_radius_px =
            ((bc.visited_radius_km * px_per_km) as f32).max(2.5 * layout.scale as f32);

        // Build a lightweight clip mask for the inner radar viewport so corridor clouds never bleed
        // outside the radar border.
        let mut radar_mask = tiny_skia::Mask::new(pixmap.width(), pixmap.height());
        if let Some(mask) = radar_mask.as_mut() {
            mask.fill_path(
                &radar_clip_path,
                FillRule::Winding,
                true,
                Transform::identity(),
            );
        }

        // 3. Draw glowing color cloud corridor along the entire flight path between points
        //    so you clearly see the swept swath/direction between every pair of breadcrumbs,
        //    plus radial clouds around each recorded breadcrumb.
        if bc.points.len() >= 2 {
            let mut corridor_pb = PathBuilder::new();
            let mut started = false;
            let mut prev_xy: Option<(f32, f32)> = None;
            let mut has_segment = false;
            for pt in &bc.points {
                let px = (cx + pt.dx_km * px_per_km) as f32;
                let py = (cy - pt.dy_km * px_per_km) as f32;
                if !started {
                    corridor_pb.move_to(px, py);
                    prev_xy = Some((px, py));
                    started = true;
                } else if let Some((lx, ly)) = prev_xy {
                    if (px - lx).hypot(py - ly) >= 1.0 {
                        corridor_pb.line_to(px, py);
                        prev_xy = Some((px, py));
                        has_segment = true;
                    }
                }
            }
            if has_segment {
                if let Some(corridor_path) = corridor_pb.finish() {
                    let (cr, cg, cb) = match bc.zone {
                        ProximityZone::Visited => (0xFFu8, 0x55u8, 0x38u8),
                        ProximityZone::Approaching => (0xFFu8, 0xAEu8, 0x18u8),
                        ProximityZone::Clear => (0x18u8, 0xC8u8, 0xFFu8),
                    };
                    let mut outer_p = Paint::default();
                    outer_p.set_color_rgba8(cr, cg, cb, 40);
                    outer_p.anti_alias = true;
                    let s = layout.scale as f32;
                    let outer_w =
                        (cloud_radius_px * 1.45).clamp(4.0 * s, (r_rect.height as f32) * 0.45);
                    let outer_stroke = Stroke {
                        width: outer_w,
                        line_cap: tiny_skia::LineCap::Round,
                        line_join: tiny_skia::LineJoin::Round,
                        ..Stroke::default()
                    };
                    pixmap.stroke_path(
                        &corridor_path,
                        &outer_p,
                        &outer_stroke,
                        Transform::identity(),
                        radar_mask.as_ref(),
                    );

                    let mut mid_p = Paint::default();
                    mid_p.set_color_rgba8(cr, cg, cb, 70);
                    mid_p.anti_alias = true;
                    let mid_w =
                        (cloud_radius_px * 0.72).clamp(2.5 * s, (r_rect.height as f32) * 0.25);
                    let mid_stroke = Stroke {
                        width: mid_w,
                        line_cap: tiny_skia::LineCap::Round,
                        line_join: tiny_skia::LineJoin::Round,
                        ..Stroke::default()
                    };
                    pixmap.stroke_path(
                        &corridor_path,
                        &mid_p,
                        &mid_stroke,
                        Transform::identity(),
                        radar_mask.as_ref(),
                    );
                }
            }
        }

        // Radial color clouds around each prior visited breadcrumb (clipped to radar rect)
        for pt in &bc.points {
            if pt.is_current {
                continue;
            }
            let px = (cx + pt.dx_km * px_per_km) as f32;
            let py = (cy - pt.dy_km * px_per_km) as f32;
            let dist_km = (pt.dx_km * pt.dx_km + pt.dy_km * pt.dy_km + pt.dz_km * pt.dz_km).sqrt();
            let (cr, cg, cb, a0, a1) = if dist_km <= bc.visited_radius_km {
                (0xFFu8, 0x45u8, 0x3Au8, 108u8, 46u8)
            } else if dist_km <= bc.visited_radius_km * 1.6 {
                (0xFFu8, 0xAAu8, 0x10u8, 88u8, 36u8)
            } else {
                (0x18u8, 0xC8u8, 0xFFu8, 68u8, 26u8)
            };

            let center = Point::from_xy(px, py);
            let stops = vec![
                GradientStop::new(0.0, Color::from_rgba8(cr, cg, cb, a0)),
                GradientStop::new(0.55, Color::from_rgba8(cr, cg, cb, a1)),
                GradientStop::new(1.0, Color::from_rgba8(cr, cg, cb, 0)),
            ];
            if let Some(shader) = RadialGradient::new(
                center,
                center,
                cloud_radius_px,
                stops,
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                let mut paint = Paint::default();
                paint.shader = shader;
                paint.anti_alias = true;
                pixmap.fill_path(
                    &radar_clip_path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        // 4. Subtle crosshair & range rings (1x visited radius and 2x visited radius)
        let mut grid_paint = Paint::default();
        grid_paint.set_color_rgba8(0x90, 0xA4, 0xB8, 0x30);
        grid_paint.anti_alias = true;
        let thin_stroke = Stroke {
            width: (1.0 * layout.scale).max(1.0) as f32,
            ..Stroke::default()
        };
        let mut h_line = PathBuilder::new();
        h_line.move_to(r_rect.x as f32, cy as f32);
        h_line.line_to(r_rect.right() as f32, cy as f32);
        if let Some(p) = h_line.finish() {
            pixmap.stroke_path(&p, &grid_paint, &thin_stroke, Transform::identity(), None);
        }
        let mut v_line = PathBuilder::new();
        v_line.move_to(cx as f32, r_rect.y as f32);
        v_line.line_to(cx as f32, r_rect.bottom() as f32);
        if let Some(p) = v_line.finish() {
            pixmap.stroke_path(&p, &grid_paint, &thin_stroke, Transform::identity(), None);
        }

        // 50% and 100% radar scale rings (always reflect active zoom: 1 km, 5 km, 10 km, 100 km...)
        for frac in [0.5f32, 0.96f32] {
            let ring_r = (max_r as f32) * frac;
            if ring_r >= 4.0 {
                if let Some(ring_path) = PathBuilder::from_circle(cx as f32, cy as f32, ring_r) {
                    let mut rp = Paint::default();
                    rp.set_color(Color::from_rgba8(0x90, 0xA4, 0xB8, 0x2E));
                    rp.anti_alias = true;
                    pixmap.stroke_path(&ring_path, &rp, &thin_stroke, Transform::identity(), None);
                }
            }
        }
        // Visited cloud boundary ring (colored by proximity zone when visible inside current zoom)
        let visited_ring_r = (bc.visited_radius_km * px_per_km) as f32;
        if visited_ring_r >= 3.5 && visited_ring_r < (max_r as f32) - 2.0 {
            if let Some(ring_path) = PathBuilder::from_circle(cx as f32, cy as f32, visited_ring_r)
            {
                let mut rp = Paint::default();
                rp.set_color(Color::from_rgba8(zr, zg, zb, 0x6E));
                rp.anti_alias = true;
                pixmap.stroke_path(&ring_path, &rp, &thin_stroke, Transform::identity(), None);
            }
        }

        // 5. Trail polyline connecting recorded breadcrumbs in order (clipped to radar viewport)
        if bc.points.len() >= 2 {
            let mut trail_pb = PathBuilder::new();
            let mut started = false;
            let mut prev_xy: Option<(f32, f32)> = None;
            let mut has_segment = false;
            for pt in &bc.points {
                let px = (cx + pt.dx_km * px_per_km) as f32;
                let py = (cy - pt.dy_km * px_per_km) as f32;
                if !started {
                    trail_pb.move_to(px, py);
                    prev_xy = Some((px, py));
                    started = true;
                } else if let Some((lx, ly)) = prev_xy {
                    if (px - lx).hypot(py - ly) >= 1.0 {
                        trail_pb.line_to(px, py);
                        prev_xy = Some((px, py));
                        has_segment = true;
                    }
                }
            }
            if has_segment {
                if let Some(tp) = trail_pb.finish() {
                    let mut lp = Paint::default();
                    lp.set_color_rgba8(0xD8, 0xF4, 0xFF, 0xA8);
                    lp.anti_alias = true;
                    let trail_stroke = Stroke {
                        width: (1.5 * layout.scale).max(1.0) as f32,
                        line_cap: tiny_skia::LineCap::Round,
                        line_join: tiny_skia::LineJoin::Round,
                        ..Stroke::default()
                    };
                    pixmap.stroke_path(
                        &tp,
                        &lp,
                        &trail_stroke,
                        Transform::identity(),
                        radar_mask.as_ref(),
                    );
                }
            }
        }

        // 6. Crisp dots for prior points + heading arrow / marker for current position
        let dot_r = (2.6 * layout.scale).max(2.0) as f32;
        for pt in &bc.points {
            if pt.is_current {
                continue;
            }
            let px = cx + pt.dx_km * px_per_km;
            let py = cy - pt.dy_km * px_per_km;
            if px < r_rect.x + 2.0
                || px > r_rect.right() - 2.0
                || py < r_rect.y + 2.0
                || py > r_rect.bottom() - 2.0
            {
                continue;
            }
            if let Some(dot) = PathBuilder::from_circle(px as f32, py as f32, dot_r) {
                let mut dp = Paint::default();
                dp.set_color_rgba8(0xE8, 0xF6, 0xFF, 0xCC);
                dp.anti_alias = true;
                pixmap.fill_path(&dot, &dp, FillRule::Winding, Transform::identity(), None);
            }
        }

        // Active waypoint beacon diamond + guide line on 2D radar
        if let Some(pin) = &bc.active_waypoint {
            let (pr, pg, pb) = pin.color.rgb();
            let raw_x = cx + pin.dx_km * px_per_km;
            let raw_y = cy - pin.dy_km * px_per_km;
            let inset = 8.0 * layout.scale;
            let min_x = r_rect.x + inset;
            let max_x = r_rect.right() - inset;
            let min_y = r_rect.y + inset;
            let max_y = r_rect.bottom() - inset;

            // Clamp vector onto radar viewport border if waypoint is beyond local zoom
            let mut px = raw_x;
            let mut py = raw_y;
            if px < min_x || px > max_x || py < min_y || py > max_y {
                let vx = raw_x - cx;
                let vy = raw_y - cy;
                let mut t = 1.0f64;
                if vx > 0.0 {
                    t = t.min((max_x - cx) / vx);
                } else if vx < 0.0 {
                    t = t.min((min_x - cx) / vx);
                }
                if vy > 0.0 {
                    t = t.min((max_y - cy) / vy);
                } else if vy < 0.0 {
                    t = t.min((min_y - cy) / vy);
                }
                t = t.clamp(0.0, 1.0);
                px = cx + vx * t;
                py = cy + vy * t;
            }

            let mut guide_pb = PathBuilder::new();
            guide_pb.move_to(cx as f32, cy as f32);
            guide_pb.line_to(px as f32, py as f32);
            if let Some(gpath) = guide_pb.finish() {
                let mut gp = Paint::default();
                gp.set_color_rgba8(pr, pg, pb, 0x99);
                gp.anti_alias = true;
                let gstroke = Stroke {
                    width: (1.4 * layout.scale).max(1.0) as f32,
                    ..Stroke::default()
                };
                pixmap.stroke_path(
                    &gpath,
                    &gp,
                    &gstroke,
                    Transform::identity(),
                    radar_mask.as_ref(),
                );
            }

            let dr = (5.2 * layout.scale).max(4.0) as f32;
            let mut dpb = PathBuilder::new();
            dpb.move_to(px as f32, py as f32 - dr);
            dpb.line_to(px as f32 + dr, py as f32);
            dpb.line_to(px as f32, py as f32 + dr);
            dpb.line_to(px as f32 - dr, py as f32);
            dpb.close();
            if let Some(dpath) = dpb.finish() {
                let mut dp = Paint::default();
                dp.set_color_rgba8(pr, pg, pb, 0xFF);
                dp.anti_alias = true;
                pixmap.fill_path(&dpath, &dp, FillRule::Winding, Transform::identity(), None);
            }
        }

        // Current ship marker at radar center
        let ship_r = (4.8 * layout.scale).max(3.5) as f32;
        if let Some(heading) = bc.heading_rad {
            // Note: screen Y is inverted relative to space Y
            let angle = -heading;
            let tip_x = cx as f32 + angle.cos() as f32 * ship_r * 1.35;
            let tip_y = cy as f32 + angle.sin() as f32 * ship_r * 1.35;
            let left_a = angle + 2.45;
            let right_a = angle - 2.45;
            let mut pb = PathBuilder::new();
            pb.move_to(tip_x, tip_y);
            pb.line_to(
                cx as f32 + left_a.cos() as f32 * ship_r,
                cy as f32 + left_a.sin() as f32 * ship_r,
            );
            pb.line_to(
                cx as f32 + right_a.cos() as f32 * ship_r,
                cy as f32 + right_a.sin() as f32 * ship_r,
            );
            pb.close();
            if let Some(tri) = pb.finish() {
                let mut sp = Paint::default();
                sp.set_color_rgba8(0xFF, 0xFF, 0xFF, 0xFF);
                sp.anti_alias = true;
                pixmap.fill_path(&tri, &sp, FillRule::Winding, Transform::identity(), None);
            }
        } else if let Some(dot) = PathBuilder::from_circle(cx as f32, cy as f32, ship_r * 0.85) {
            let mut sp = Paint::default();
            sp.set_color_rgba8(0xFF, 0xFF, 0xFF, 0xFF);
            sp.anti_alias = true;
            pixmap.fill_path(&dot, &sp, FillRule::Winding, Transform::identity(), None);
        }

        // Radar inner border
        Self::stroke_rounded_box(
            pixmap,
            r_rect,
            radar_rad,
            Color::from_rgba8(0x64, 0x74, 0x8B, 0x60),
            (1.0 * layout.scale).max(1.0) as f32,
        );
    }

    fn draw_breadcrumb_text(
        bgra: &mut [u8],
        w: usize,
        h: usize,
        bc: &BreadcrumbOverlayInfo,
        layout: &BreadcrumbHudLayout,
    ) {
        let (zr, zg, zb) = bc.zone.rgb();

        if let Some(banner) = layout.banner_rect {
            Self::draw_outlined_text_rgb(
                bgra,
                w,
                h,
                &bc.headline,
                banner.x + 12.0 * layout.scale,
                banner.y + 4.0 * layout.scale,
                (banner.width - 24.0 * layout.scale).max(40.0),
                banner.height - 8.0 * layout.scale,
                layout.banner_px,
                true,
                (zr, zg, zb),
            );
        }

        let header_w = (layout.card_rect.width - 2.0 * layout.pad_x).max(40.0);
        let badge = match bc.zone {
            ProximityZone::Clear => "BREADCRUMB CLOUD  [FRESH]",
            ProximityZone::Approaching => "BREADCRUMB CLOUD  [NEAR]",
            ProximityZone::Visited => "BREADCRUMB CLOUD  [VISITED]",
        };
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            badge,
            layout.card_rect.x + layout.pad_x,
            layout.card_rect.y + layout.pad_y,
            header_w,
            layout.header_h,
            layout.header_px,
            false,
            (zr, zg, zb),
        );

        // Crisp radar range readout in the top-right corner of the radar viewport (e.g. "R 100 km")
        let eff_range_km = if bc.radar_range_km > 0.0 {
            bc.radar_range_km.clamp(0.5, 5_000.0)
        } else {
            (bc.visited_radius_km * 3.0).max(10.0)
        };
        let range_label = if eff_range_km >= 10.0 || (eff_range_km - eff_range_km.round()).abs() < 0.05
        {
            format!("R {:.0} km", eff_range_km.round())
        } else {
            format!("R {:.1} km", eff_range_km)
        };
        let scale_px = (9.5 * layout.scale) as f32;
        let (sw, sh) = Self::measure_text(&range_label, scale_px);
        let inset = 5.0 * layout.scale;
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            &range_label,
            (layout.radar_rect.right() - inset - sw as f64).max(layout.radar_rect.x + inset),
            layout.radar_rect.y + inset,
            (sw as f64 + 4.0).min(layout.radar_rect.width - 2.0 * inset),
            (sh as f64).max(12.0 * layout.scale),
            scale_px,
            false,
            (168, 214, 236),
        );

        let footer_y0 = layout.radar_rect.bottom() + 5.0 * layout.scale;
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            &bc.headline,
            layout.card_rect.x + layout.pad_x,
            footer_y0,
            header_w,
            layout.footer_line_h,
            layout.body_px,
            false,
            (238, 244, 252),
        );
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            &bc.subline,
            layout.card_rect.x + layout.pad_x,
            footer_y0 + layout.footer_line_h,
            header_w,
            layout.footer_line_h,
            layout.body_px,
            false,
            (168, 184, 204),
        );
    }

    fn compute_waypoint_layout(
        wp: &WaypointOverlayInfo,
        screen_w: f64,
        screen_h: f64,
        top_banner_bottom: Option<f64>,
    ) -> WaypointHudLayout {
        let scale = (screen_h / BracketMetrics::REFERENCE_SCREEN_HEIGHT).clamp(0.75, 2.25);
        let pad_x = 12.0 * scale;
        let pad_y = 8.0 * scale;
        let dial_r = 22.0 * scale;
        let dial_gap = 12.0 * scale;
        let title_px = (15.0 * scale) as f32;
        let dist_px = (18.0 * scale) as f32;
        let sub_px = (11.5 * scale) as f32;
        let top_row_h = (dist_px as f64) * 1.25;
        let sub_row_h = (sub_px as f64) * 1.32;

        let (name_w, _) = Self::measure_text(&wp.name, title_px);
        let (dist_w, _) = Self::measure_text(&wp.distance_text, dist_px);
        let (sub_w, _) = Self::measure_text(&wp.subline, sub_px);

        let top_w = name_w as f64 + 18.0 * scale + dist_w as f64;
        let text_w = top_w
            .max(sub_w as f64)
            .clamp(210.0 * scale, (screen_w * 0.44).max(220.0));
        let card_w = (pad_x * 2.0 + dial_r * 2.0 + dial_gap + text_w)
            .min((screen_w * 0.56).max(280.0))
            .ceil();
        let content_h = (top_row_h + sub_row_h + 2.0 * scale).max(dial_r * 2.0);
        let card_h = (pad_y * 2.0 + content_h).ceil();

        let card_x = ((screen_w - card_w) * 0.5).round();
        // Sit cleanly below Star Citizen's top-center helmet/ship compass bar (~7.5% from top)
        let default_y = (84.0 * scale).max(screen_h * 0.075);
        let card_y = match top_banner_bottom {
            Some(b) => (b + 10.0 * scale).max(default_y),
            None => default_y,
        }
        .round();

        let card_rect = PixelRect::new(card_x, card_y, card_w, card_h);
        let dial_cx = card_rect.x + pad_x + dial_r;
        let dial_cy = card_rect.y + card_rect.height * 0.5;
        let text_x = card_rect.x + pad_x + dial_r * 2.0 + dial_gap;
        let actual_text_w = (card_rect.right() - pad_x - text_x).max(60.0);

        let ring_marker = if wp.show_ring_marker {
            wp.rel_yaw_rad.map(|rel_yaw| {
                // rel_yaw: 0 = ahead (UP), <0 = RIGHT, >0 = LEFT, +/-pi = behind (DOWN)
                let screen_angle = -rel_yaw;
                let dir_x = screen_angle.sin();
                let dir_y = -screen_angle.cos();

                let cx = screen_w * 0.5;
                let cy = screen_h * 0.5;
                let rx = (screen_w * 0.29).min(screen_h * 0.46);
                let ry = screen_h * 0.30;

                let mut icon_x = cx + dir_x * rx;
                let mut icon_y = cy + dir_y * ry;

                // Keep well clear of the top-center guidance card when aiming straight up
                let min_top_y = card_rect.bottom() + 34.0 * scale;
                if icon_y < min_top_y {
                    icon_y = min_top_y;
                }
                let margin_x = 95.0 * scale;
                let margin_bot = 68.0 * scale;
                icon_x = icon_x.clamp(margin_x, (screen_w - margin_x).max(margin_x));
                icon_y = icon_y.clamp(min_top_y, (screen_h - margin_bot).max(min_top_y));

                let short_name = if wp.name.chars().count() > 20 {
                    let cut: String = wp.name.chars().take(18).collect();
                    format!("{}..", cut)
                } else {
                    wp.name.clone()
                };

                let dist_line = if wp.dz_m.abs() >= 100.0 {
                    let sign = if wp.dz_m > 0.0 { "+" } else { "-" };
                    let dz_txt = crate::waypoints::format_distance_m(wp.dz_m.abs());
                    format!("{}  ({}{}Z)", wp.distance_text, sign, dz_txt)
                } else {
                    wp.distance_text.clone()
                };

                let r_name_px = (12.0 * scale) as f32;
                let r_dist_px = (13.5 * scale) as f32;
                let r_line_h = (r_dist_px as f64) * 1.24;
                let r_pad_x = 8.0 * scale;
                let r_pad_y = 4.0 * scale;
                let (nw, _) = Self::measure_text(&short_name, r_name_px);
                let (dw, _) = Self::measure_text(&dist_line, r_dist_px);
                let pill_w = ((nw.max(dw) as f64) + 2.0 * r_pad_x)
                    .clamp(92.0 * scale, 220.0 * scale)
                    .ceil();
                let pill_h = (r_pad_y * 2.0 + r_line_h * 2.0).ceil();

                // Place label pill slightly inward from the directional chevron so it never clips screen edges
                let offset_dist = 22.0 * scale;
                let mut pill_cx = icon_x - dir_x * (offset_dist + pill_w * 0.45);
                let pill_cy = icon_y - dir_y * (offset_dist + pill_h * 0.55);
                if dir_y.abs() > 0.75 {
                    pill_cx = icon_x;
                }
                let pill_x = (pill_cx - pill_w * 0.5).clamp(
                    12.0 * scale,
                    (screen_w - pill_w - 12.0 * scale).max(12.0 * scale),
                );
                let pill_y = (pill_cy - pill_h * 0.5).clamp(
                    card_rect.bottom() + 8.0 * scale,
                    (screen_h - pill_h - 12.0 * scale).max(card_rect.bottom() + 8.0 * scale),
                );

                WaypointRingMarkerLayout {
                    icon_x,
                    icon_y,
                    dir_x,
                    dir_y,
                    pill_rect: PixelRect::new(pill_x, pill_y, pill_w, pill_h),
                    name_px: r_name_px,
                    dist_px: r_dist_px,
                    line_h: r_line_h,
                    pad_x: r_pad_x,
                    pad_y: r_pad_y,
                    short_name,
                    dist_line,
                }
            })
        } else {
            None
        };

        WaypointHudLayout {
            card_rect,
            dial_cx,
            dial_cy,
            dial_r,
            text_x,
            text_w: actual_text_w,
            scale,
            title_px,
            dist_px,
            sub_px,
            pad_y,
            top_row_h,
            sub_row_h,
            ring_marker,
        }
    }

    fn draw_waypoint_vectors(
        pixmap: &mut Pixmap,
        wp: &WaypointOverlayInfo,
        layout: &WaypointHudLayout,
    ) {
        let (cr, cg, cb) = if wp.arrived {
            (0x2E, 0xE6, 0x5A)
        } else {
            wp.color.rgb()
        };
        let s = layout.scale as f32;

        // 1. Top-center Waypoint Beacon Guidance Card
        let card_rad = 8.0 * layout.scale;
        Self::draw_rounded_box(
            pixmap,
            layout.card_rect,
            card_rad,
            Color::from_rgba8(0x0B, 0x10, 0x18, 0xDC),
        );
        Self::stroke_rounded_box(
            pixmap,
            layout.card_rect,
            card_rad,
            Color::from_rgba8(cr, cg, cb, 0xD8),
            (1.5 * s).max(1.0),
        );

        // 2. Rotating 360° Directional Compass Dial on card left
        let dcx = layout.dial_cx as f32;
        let dcy = layout.dial_cy as f32;
        let dr = layout.dial_r as f32;

        if let Some(bg_circle) = PathBuilder::from_circle(dcx, dcy, dr) {
            let mut bgp = Paint::default();
            bgp.set_color_rgba8(0x06, 0x09, 0x0F, 0xEB);
            bgp.anti_alias = true;
            pixmap.fill_path(
                &bg_circle,
                &bgp,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            let mut ring_p = Paint::default();
            ring_p.set_color_rgba8(cr, cg, cb, 0xA8);
            ring_p.anti_alias = true;
            let ring_stroke = Stroke {
                width: (1.5 * s).max(1.0),
                ..Stroke::default()
            };
            pixmap.stroke_path(
                &bg_circle,
                &ring_p,
                &ring_stroke,
                Transform::identity(),
                None,
            );
        }

        // Subtle cardinal tick marks (Forward / Right / Aft / Left relative to ship nose)
        let mut tick_paint = Paint::default();
        tick_paint.set_color_rgba8(0x90, 0xA8, 0xC4, 0x70);
        tick_paint.anti_alias = true;
        let tick_stroke = Stroke {
            width: (1.1 * s).max(1.0),
            ..Stroke::default()
        };
        for &(tx, ty) in &[(0.0f32, -1.0f32), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)] {
            let mut pb = PathBuilder::new();
            pb.move_to(dcx + tx * (dr - 3.5 * s), dcy + ty * (dr - 3.5 * s));
            pb.line_to(dcx + tx * (dr - 0.5 * s), dcy + ty * (dr - 0.5 * s));
            if let Some(tp) = pb.finish() {
                pixmap.stroke_path(&tp, &tick_paint, &tick_stroke, Transform::identity(), None);
            }
        }

        if let Some(rel_yaw) = wp.rel_yaw_rad {
            let screen_angle = -rel_yaw;
            let dx = screen_angle.sin() as f32;
            let dy = -screen_angle.cos() as f32;
            let px = -dy; // perpendicular right
            let py = dx;

            // Bold directional pointer arrow inside dial
            let tip_x = dcx + dx * (dr * 0.78);
            let tip_y = dcy + dy * (dr * 0.78);
            let tail_x = dcx - dx * (dr * 0.16);
            let tail_y = dcy - dy * (dr * 0.16);
            let wing_back = dr * 0.48;
            let wing_side = dr * 0.44;
            let lw_x = dcx - dx * wing_back - px * wing_side;
            let lw_y = dcy - dy * wing_back - py * wing_side;
            let rw_x = dcx - dx * wing_back + px * wing_side;
            let rw_y = dcy - dy * wing_back + py * wing_side;

            let mut pb = PathBuilder::new();
            pb.move_to(tip_x, tip_y);
            pb.line_to(rw_x, rw_y);
            pb.line_to(tail_x, tail_y);
            pb.line_to(lw_x, lw_y);
            pb.close();
            if let Some(arrow) = pb.finish() {
                let mut ap = Paint::default();
                ap.set_color_rgba8(cr, cg, cb, 0xFF);
                ap.anti_alias = true;
                pixmap.fill_path(&arrow, &ap, FillRule::Winding, Transform::identity(), None);
            }

            // Bright pip on dial rim indicating direction
            let rim_x = dcx + dx * dr;
            let rim_y = dcy + dy * dr;
            if let Some(pip) = PathBuilder::from_circle(rim_x, rim_y, (2.6 * s).max(2.0)) {
                let mut pp = Paint::default();
                pp.set_color_rgba8(0xFF, 0xFF, 0xFF, 0xFF);
                pp.anti_alias = true;
                pixmap.fill_path(&pip, &pp, FillRule::Winding, Transform::identity(), None);
            }
        } else {
            // Arrived or waiting for movement lock: crisp glowing waypoint diamond
            let d = dr * 0.46;
            let mut pb = PathBuilder::new();
            pb.move_to(dcx, dcy - d);
            pb.line_to(dcx + d, dcy);
            pb.line_to(dcx, dcy + d);
            pb.line_to(dcx - d, dcy);
            pb.close();
            if let Some(diamond) = pb.finish() {
                let mut dp = Paint::default();
                dp.set_color_rgba8(cr, cg, cb, if wp.arrived { 0xFF } else { 0xC8 });
                dp.anti_alias = true;
                pixmap.fill_path(
                    &diamond,
                    &dp,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        // 3. Floating HUD Directional Ring Marker ("9.9 km THIS WAY")
        if let Some(rm) = &layout.ring_marker {
            let ix = rm.icon_x as f32;
            let iy = rm.icon_y as f32;
            let dx = rm.dir_x as f32;
            let dy = rm.dir_y as f32;
            let px = -dy;
            let py = dx;

            // Soft radial glow behind the floating beacon chevron
            let glow_r = 22.0 * s;
            let glow_center = Point::from_xy(ix, iy);
            let stops = vec![
                GradientStop::new(0.0, Color::from_rgba8(cr, cg, cb, 0x70)),
                GradientStop::new(1.0, Color::from_rgba8(cr, cg, cb, 0x00)),
            ];
            if let Some(shader) = RadialGradient::new(
                glow_center,
                glow_center,
                glow_r,
                stops,
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                if let Some(glow_circle) = PathBuilder::from_circle(ix, iy, glow_r) {
                    let mut gp = Paint::default();
                    gp.shader = shader;
                    gp.anti_alias = true;
                    pixmap.fill_path(
                        &glow_circle,
                        &gp,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }

            // Directional chevron arrow pointing outward toward POI
            let tip_x = ix + dx * (15.0 * s);
            let tip_y = iy + dy * (15.0 * s);
            let notch_x = ix - dx * (2.5 * s);
            let notch_y = iy - dy * (2.5 * s);
            let wing_b = 9.0 * s;
            let wing_s = 10.5 * s;
            let lw_x = ix - dx * wing_b - px * wing_s;
            let lw_y = iy - dy * wing_b - py * wing_s;
            let rw_x = ix - dx * wing_b + px * wing_s;
            let rw_y = iy - dy * wing_b + py * wing_s;

            let mut pb = PathBuilder::new();
            pb.move_to(tip_x, tip_y);
            pb.line_to(rw_x, rw_y);
            pb.line_to(notch_x, notch_y);
            pb.line_to(lw_x, lw_y);
            pb.close();
            if let Some(chev) = pb.finish() {
                // Dark outline for contrast against bright skies/planets
                let mut out_p = Paint::default();
                out_p.set_color_rgba8(0x0A, 0x0A, 0x0F, 0xE6);
                out_p.anti_alias = true;
                let out_s = Stroke {
                    width: (3.2 * s).max(2.0),
                    line_join: tiny_skia::LineJoin::Round,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&chev, &out_p, &out_s, Transform::identity(), None);

                let mut fill_p = Paint::default();
                fill_p.set_color_rgba8(cr, cg, cb, 0xFF);
                fill_p.anti_alias = true;
                pixmap.fill_path(
                    &chev,
                    &fill_p,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }

            // Compact label pill carrying POI name & live distance
            let pill_rad = 6.0 * layout.scale;
            Self::draw_rounded_box(
                pixmap,
                rm.pill_rect,
                pill_rad,
                Color::from_rgba8(0x0B, 0x10, 0x18, 0xD4),
            );
            Self::stroke_rounded_box(
                pixmap,
                rm.pill_rect,
                pill_rad,
                Color::from_rgba8(cr, cg, cb, 0xC8),
                (1.2 * s).max(1.0),
            );
        }
    }

    fn draw_waypoint_text(
        bgra: &mut [u8],
        w: usize,
        h: usize,
        wp: &WaypointOverlayInfo,
        layout: &WaypointHudLayout,
    ) {
        let (cr, cg, cb) = if wp.arrived {
            (0x2E, 0xE6, 0x5A)
        } else {
            wp.color.rgb()
        };

        let top_y = layout.card_rect.y + layout.pad_y;
        let (dist_w, _) = Self::measure_text(&wp.distance_text, layout.dist_px);
        let dist_box_w = (dist_w as f64 + 6.0 * layout.scale).min(layout.text_w * 0.48);
        let name_max_w = (layout.text_w - dist_box_w - 10.0 * layout.scale).max(40.0);

        // POI Name (left of top row)
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            &wp.name,
            layout.text_x,
            top_y,
            name_max_w,
            layout.top_row_h,
            layout.title_px,
            false,
            (cr, cg, cb),
        );

        // Live Distance Readout (right-aligned in top row)
        let dist_x = layout.text_x + layout.text_w - dist_box_w;
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            &wp.distance_text,
            dist_x,
            top_y,
            dist_box_w,
            layout.top_row_h,
            layout.dist_px,
            true,
            if wp.arrived {
                (0x2E, 0xE6, 0x5A)
            } else {
                (255, 255, 255)
            },
        );

        // Steering / Vertical / ETA Subline
        let sub_y = top_y + layout.top_row_h + 1.0 * layout.scale;
        Self::draw_outlined_text_rgb(
            bgra,
            w,
            h,
            &wp.subline,
            layout.text_x,
            sub_y,
            layout.text_w,
            layout.sub_row_h,
            layout.sub_px,
            false,
            (206, 222, 238),
        );

        // Floating HUD Directional Ring Marker text
        if let Some(rm) = &layout.ring_marker {
            let inner_w = (rm.pill_rect.width - 2.0 * rm.pad_x).max(24.0);
            Self::draw_outlined_text_rgb(
                bgra,
                w,
                h,
                &rm.short_name,
                rm.pill_rect.x + rm.pad_x,
                rm.pill_rect.y + rm.pad_y,
                inner_w,
                rm.line_h,
                rm.name_px,
                true,
                (cr, cg, cb),
            );
            Self::draw_outlined_text_rgb(
                bgra,
                w,
                h,
                &rm.dist_line,
                rm.pill_rect.x + rm.pad_x,
                rm.pill_rect.y + rm.pad_y + rm.line_h,
                inner_w,
                rm.line_h,
                rm.dist_px,
                true,
                (255, 255, 255),
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
                    bgra[idx + 1] = (cg * a + bgra[idx + 1] as f32 * inv)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                    bgra[idx + 2] = (cr * a + bgra[idx + 2] as f32 * inv)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                    bgra[idx + 3] = (255.0 * a + bgra[idx + 3] as f32 * inv)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                });
            }
        }
    }
}
