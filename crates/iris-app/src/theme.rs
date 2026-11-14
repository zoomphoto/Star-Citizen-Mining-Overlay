use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, RichText, Rounding, Stroke, Visuals,
};
use iris_core::config::DarkMode;
use iris_core::models::OverlayColor;

const QUANTICO_BOLD_BYTES: &[u8] = include_bytes!("../../../assets/Quantico-Bold.ttf");

#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub bg: Color32,
    pub surface: Color32,
    pub elevated: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub danger: Color32,
    pub success: Color32,
}

impl ThemePalette {
    /// Exact IRIS `ThemeDark` palette
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            bg: Color32::from_rgb(0x0D, 0x0F, 0x13),
            surface: Color32::from_rgb(0x14, 0x16, 0x1C),
            elevated: Color32::from_rgb(0x1B, 0x1E, 0x26),
            border: Color32::from_rgb(0x27, 0x2B, 0x36),
            accent: Color32::from_rgb(0x00, 0xC2, 0xD1),
            text: Color32::from_rgb(0xE6, 0xEA, 0xF2),
            muted: Color32::from_rgb(0x9A, 0xA3, 0xB2),
            danger: Color32::from_rgb(0xFF, 0x45, 0x3A),
            success: Color32::from_rgb(0x2E, 0xE6, 0x5A),
        }
    }

    /// Exact IRIS `ThemeLight` palette
    pub fn light() -> Self {
        Self {
            is_dark: false,
            bg: Color32::from_rgb(0xF4, 0xF6, 0xF9),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            elevated: Color32::from_rgb(0xE9, 0xEE, 0xF5),
            border: Color32::from_rgb(0xCB, 0xD3, 0xDF),
            accent: Color32::from_rgb(0x00, 0x7B, 0x8A),
            text: Color32::from_rgb(0x18, 0x1B, 0x22),
            muted: Color32::from_rgb(0x5A, 0x64, 0x75),
            danger: Color32::from_rgb(0xD9, 0x2D, 0x20),
            success: Color32::from_rgb(0x15, 0x80, 0x3D),
        }
    }

    pub fn for_mode(mode: DarkMode, system_prefers_dark: bool) -> Self {
        match mode {
            DarkMode::On => Self::dark(),
            DarkMode::Off => Self::light(),
            DarkMode::System => {
                if system_prefers_dark {
                    Self::dark()
                } else {
                    Self::light()
                }
            }
        }
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Quantico-Bold".to_owned(),
        FontData::from_static(QUANTICO_BOLD_BYTES),
    );
    // Register a named "Quantico" family for Star Citizen HUD headings / Dossier titles,
    // while keeping default proportional fallback for full Unicode coverage.
    let mut quantico_chain = vec!["Quantico-Bold".to_owned()];
    if let Some(prop) = fonts.families.get(&FontFamily::Proportional) {
        quantico_chain.extend(prop.iter().cloned());
    }
    fonts
        .families
        .insert(FontFamily::Name("Quantico".into()), quantico_chain);
    ctx.set_fonts(fonts);
}

pub fn quantico_text(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text)
        .family(FontFamily::Name("Quantico".into()))
        .size(size)
        .color(color)
}

pub fn apply_theme(ctx: &egui::Context, mode: DarkMode, scale_percent: i32) -> ThemePalette {
    let system_dark = ctx
        .input(|i| i.raw.system_theme)
        .map(|t| t == egui::Theme::Dark)
        .unwrap_or(true);
    let p = ThemePalette::for_mode(mode, system_dark);

    let mut vis = if p.is_dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    vis.panel_fill = p.bg;
    vis.window_fill = p.surface;
    vis.faint_bg_color = p.surface;
    vis.extreme_bg_color = p.elevated;
    vis.window_stroke = Stroke::new(1.0, p.border);
    vis.override_text_color = Some(p.text);
    vis.selection.bg_fill = p.accent.gamma_multiply(0.32);
    vis.selection.stroke = Stroke::new(1.0, p.accent);
    vis.hyperlink_color = p.accent;

    let radius = Rounding::same(6.0);
    vis.window_rounding = Rounding::same(8.0);
    vis.widgets.noninteractive.bg_fill = p.surface;
    vis.widgets.noninteractive.weak_bg_fill = p.surface;
    vis.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    vis.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    vis.widgets.noninteractive.rounding = radius;

    vis.widgets.inactive.bg_fill = p.elevated;
    vis.widgets.inactive.weak_bg_fill = p.elevated;
    vis.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    vis.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    vis.widgets.inactive.rounding = radius;

    vis.widgets.hovered.bg_fill = p.elevated;
    vis.widgets.hovered.weak_bg_fill = p.elevated;
    vis.widgets.hovered.bg_stroke = Stroke::new(1.2, p.accent);
    vis.widgets.hovered.fg_stroke = Stroke::new(1.2, p.text);
    vis.widgets.hovered.rounding = radius;

    vis.widgets.active.bg_fill = p.accent.gamma_multiply(0.25);
    vis.widgets.active.weak_bg_fill = p.accent.gamma_multiply(0.25);
    vis.widgets.active.bg_stroke = Stroke::new(1.5, p.accent);
    vis.widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    vis.widgets.active.rounding = radius;

    ctx.set_visuals(vis);

    let zoom = (scale_percent.clamp(100, 175) as f32) / 100.0;
    if (ctx.zoom_factor() - zoom).abs() > 0.01 {
        ctx.set_zoom_factor(zoom);
    }

    p
}

pub fn overlay_color32(color: OverlayColor) -> Color32 {
    let (r, g, b) = color.rgb();
    Color32::from_rgb(r, g, b)
}

pub fn overlay_swatch_color32(color: OverlayColor, is_dark: bool) -> Color32 {
    if is_dark {
        let (r, g, b) = color.ui_swatch_rgb();
        Color32::from_rgb(r, g, b)
    } else {
        let (r, g, b) = color.rgb();
        Color32::from_rgb(r, g, b)
    }
}

/// Renders a compact color selector ComboBox with colored dot + name.
pub fn color_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    current: &mut OverlayColor,
    is_dark: bool,
) -> bool {
    let mut changed = false;
    let cur_c = overlay_color32(*current);
    egui::ComboBox::from_id_salt(id_salt)
        .width(112.0)
        .selected_text(RichText::new(format!("● {}", current.as_str())).color(cur_c))
        .show_ui(ui, |ui| {
            for candidate in OverlayColor::ALL {
                let c32 = overlay_swatch_color32(candidate, is_dark);
                let label = RichText::new(format!("● {}", candidate.as_str())).color(c32);
                if ui.selectable_value(current, candidate, label).changed() {
                    changed = true;
                }
            }
        });
    changed
}

/// Renders an optional color selector (`None` = Disabled / Off, `Some(color)` = Active rule).
pub fn optional_color_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    current: &mut Option<OverlayColor>,
    muted: Color32,
    is_dark: bool,
) -> bool {
    let mut changed = false;
    let selected_label = match *current {
        Some(c) => RichText::new(format!("● {}", c.as_str())).color(overlay_color32(c)),
        None => RichText::new("○ Off").color(muted),
    };
    egui::ComboBox::from_id_salt(id_salt)
        .width(116.0)
        .selected_text(selected_label)
        .show_ui(ui, |ui| {
            if ui
                .selectable_value(current, None, RichText::new("○ Off").color(muted))
                .changed()
            {
                changed = true;
            }
            for candidate in OverlayColor::ALL {
                let c32 = overlay_swatch_color32(candidate, is_dark);
                let label = RichText::new(format!("● {}", candidate.as_str())).color(c32);
                if ui
                    .selectable_value(current, Some(candidate), label)
                    .changed()
                {
                    changed = true;
                }
            }
        });
    changed
}

/// Renders the IRIS `1..9` Color Rank priority strip with `◀` / `▶` reorder controls.
pub fn color_rank_strip<F>(
    ui: &mut egui::Ui,
    palette: &ThemePalette,
    active_colors: &[OverlayColor],
    mut on_move: F,
) -> bool
where
    F: FnMut(OverlayColor, isize),
{
    let mut changed = false;
    egui::Frame::none()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .rounding(Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Color Priority (1 = highest):")
                        .size(12.5)
                        .color(palette.muted),
                );
                if active_colors.is_empty() {
                    ui.label(
                        RichText::new("Assign colors below to rank them.")
                            .italics()
                            .size(12.5)
                            .color(palette.muted),
                    );
                }
                for (idx, &color) in active_colors.iter().enumerate() {
                    let c32 = overlay_color32(color);
                    egui::Frame::none()
                        .fill(palette.elevated)
                        .stroke(Stroke::new(1.0, c32.gamma_multiply(0.65)))
                        .rounding(Rounding::same(5.0))
                        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            if idx > 0
                                && ui
                                    .small_button("◀")
                                    .on_hover_text("Raise priority")
                                    .clicked()
                            {
                                on_move(color, -1);
                                changed = true;
                            }
                            ui.label(
                                RichText::new(format!("#{} {}", idx + 1, color.as_str()))
                                    .strong()
                                    .size(12.0)
                                    .color(c32),
                            );
                            if idx + 1 < active_colors.len()
                                && ui
                                    .small_button("▶")
                                    .on_hover_text("Lower priority")
                                    .clicked()
                            {
                                on_move(color, 1);
                                changed = true;
                            }
                        });
                }
            });
        });
    changed
}
