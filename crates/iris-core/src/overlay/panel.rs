use crate::config::settings::{AppSettings, OrgDisplay};
use crate::config::social::{
    AttributeKind, AttributeTarget, ColorResolver, OrgAttributeRule, OrgEntry, PlayerEntry,
    SocialConfig,
};
use crate::models::{
    BracketItem, BreadcrumbOverlayInfo, BreadcrumbRenderPoint, BreadcrumbWaypointPin, ColorMatch,
    DebugBoxKind, DebugCandidateBox, DebugOverlayInfo, DebugStatusLevel, MatchKind, OrgMembership,
    OverlayColor, OverlayModel, PanelEntry, PanelOrgs, PixelRect, ProximityZone,
    WaypointOverlayInfo,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackingBoundary {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Default for TrackingBoundary {
    fn default() -> Self {
        Self::from_percents(10, 30, 15, 15)
    }
}

impl TrackingBoundary {
    pub const BOUNDARY_DROP_LINES: f64 = 1.5;
    pub const LINE_HEIGHT_AT_1440: f64 = 16.0;

    pub fn from_percents(top: i32, bottom: i32, left: i32, right: i32) -> Self {
        let lim = |v: i32| v.clamp(AppSettings::BOUNDARY_MIN, AppSettings::BOUNDARY_MAX);
        Self {
            left: lim(left) as f64 / 100.0,
            top: lim(top) as f64 / 100.0,
            right: (100 - lim(right)) as f64 / 100.0,
            bottom: (100 - lim(bottom)) as f64 / 100.0,
        }
    }

    pub fn from_settings(settings: &AppSettings) -> Self {
        Self::from_percents(
            settings.boundary_top,
            settings.boundary_bottom,
            settings.boundary_left,
            settings.boundary_right,
        )
    }

    pub fn to_pixels(&self, window_width: f64, window_height: f64) -> PixelRect {
        let x0 = self.left * window_width;
        let y0 = self.top * window_height;
        let x1 = self.right * window_width;
        let y1 = self.bottom * window_height;
        PixelRect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    pub fn drop_distance_px(window_height: f64) -> f64 {
        Self::BOUNDARY_DROP_LINES * Self::LINE_HEIGHT_AT_1440 * window_height / 1440.0
    }

    pub fn contains_center_of(&self, box_rect: PixelRect, ww: f64, wh: f64) -> bool {
        self.distance_outside(box_rect, ww, wh) <= 0.0
    }

    pub fn distance_outside(&self, box_rect: PixelRect, ww: f64, wh: f64) -> f64 {
        let cx = box_rect.center_x();
        let cy = box_rect.center_y();
        let dx = (self.left * ww - cx).max(cx - self.right * ww).max(0.0);
        let dy = (self.top * wh - cy).max(cy - self.bottom * wh).max(0.0);
        dx.max(dy)
    }
}

#[derive(Debug, Clone)]
pub struct PanelSlot {
    pub entry: PanelEntry,
    pub rect: PixelRect,
    pub org_lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PanelMessage {
    pub lines: Vec<String>,
    pub rect: PixelRect,
    pub centered: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PanelLayoutResult {
    pub slots: Vec<PanelSlot>,
    pub more_text: Option<String>,
    pub more_rect: PixelRect,
    pub font_size: f64,
    pub line_height: f64,
    pub padding_x: f64,
    pub padding_y: f64,
    pub corner_radius: f64,
    pub messages: Vec<PanelMessage>,
}

impl PanelLayoutResult {
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty() && self.more_text.is_none() && self.messages.is_empty()
    }
}

pub struct PanelLayout;

impl PanelLayout {
    pub const BOTTOM_RESERVE_PERCENT: f64 = 10.0;
    pub const REFERENCE_HEIGHT: f64 = 1440.0;
    pub const NOTE_CHAR_WIDTH: f64 = 0.62;

    pub fn scaled_text_size(text_size_at_1440: i32, window_height: f64) -> f64 {
        text_size_at_1440 as f64 * window_height / Self::REFERENCE_HEIGHT
    }

    pub fn compute(
        entries: &[PanelEntry],
        window_width: f64,
        window_height: f64,
        settings: &AppSettings,
        note: Option<&str>,
        toast: Option<&str>,
    ) -> PanelLayoutResult {
        if window_width <= 0.0 || window_height <= 0.0 {
            return PanelLayoutResult::default();
        }
        if entries.is_empty() && note.is_none() && toast.is_none() {
            return PanelLayoutResult::default();
        }

        let font_size = Self::scaled_text_size(settings.panel_text_size, window_height);
        let line_height = font_size * 1.25;
        let padding_x = font_size * 0.7;
        let padding_y = font_size * 0.45;
        let corner_radius = font_size * 0.5;
        let gap = font_size * 0.4;

        let panel_width = window_width * settings.panel_width_percent as f64 / 100.0;
        let right = window_width - window_width * settings.panel_right_gap_percent as f64 / 100.0;
        let left = (right - panel_width).max(0.0);
        let top = window_height * settings.panel_top_gap_percent as f64 / 100.0;
        let limit = window_height - window_height * Self::BOTTOM_RESERVE_PERCENT / 100.0;

        let mut messages = Vec::new();
        let mut msg_top = top;

        if let Some(t) = toast {
            let h = line_height + 2.0 * padding_y;
            messages.push(PanelMessage {
                lines: vec![t.to_string()],
                rect: PixelRect::new(left, msg_top, panel_width, h),
                centered: true,
            });
            msg_top += h + gap;
        }

        if let Some(n) = note {
            let lines = Self::wrap_note(n, panel_width - 2.0 * padding_x, font_size);
            if !lines.is_empty() {
                let h = lines.len() as f64 * line_height + 2.0 * padding_y;
                messages.push(PanelMessage {
                    lines,
                    rect: PixelRect::new(left, msg_top, panel_width, h),
                    centered: false,
                });
                msg_top += h + gap;
            }
        }

        let entries_top = msg_top;
        let per_line = Self::chars_per_line(panel_width - 2.0 * padding_x, font_size);
        let mut org_lines: Vec<Vec<String>> = Vec::with_capacity(entries.len());
        let mut tops = vec![0.0f64; entries.len() + 1];
        tops[0] = entries_top;

        for (i, entry) in entries.iter().enumerate() {
            let mut lines = PanelText::lines(
                &entry.orgs,
                settings.panel_org_display,
                settings.panel_main_org_only,
                per_line,
            );
            if let Some(en) = entry.note.as_deref() {
                if !en.trim().is_empty() {
                    lines.push(PanelText::notes_line(en, per_line));
                }
            }
            let eh = (1 + lines.len()) as f64 * line_height + 2.0 * padding_y;
            tops[i + 1] = tops[i] + eh + gap;
            org_lines.push(lines);
        }

        let total = entries.len();
        let mut shown = total;
        if total > 0 && tops[total] - gap > limit {
            shown = 0;
            for k in (0..total).rev() {
                if tops[k] + line_height <= limit {
                    shown = k;
                    break;
                }
            }
        }

        let mut slots = Vec::with_capacity(shown);
        for i in 0..shown {
            let eh = (1 + org_lines[i].len()) as f64 * line_height + 2.0 * padding_y;
            slots.push(PanelSlot {
                entry: entries[i].clone(),
                rect: PixelRect::new(left, tops[i], panel_width, eh),
                org_lines: org_lines[i].clone(),
            });
        }

        let (more_text, more_rect) = if shown < total {
            let mut more_top = tops[shown];
            if more_top + line_height > window_height {
                more_top = (window_height - line_height).max(0.0);
            }
            (
                Some(format!("+{} more", total - shown)),
                PixelRect::new(left, more_top, panel_width, line_height),
            )
        } else {
            (None, PixelRect::default())
        };

        PanelLayoutResult {
            slots,
            more_text,
            more_rect,
            font_size,
            line_height,
            padding_x,
            padding_y,
            corner_radius,
            messages,
        }
    }

    pub fn chars_per_line(width: f64, font_size: f64) -> usize {
        ((width / (font_size * Self::NOTE_CHAR_WIDTH)).floor() as isize).max(8) as usize
    }

    pub fn wrap_note(text: &str, width: f64, font_size: f64) -> Vec<String> {
        let per_line = Self::chars_per_line(width, font_size);
        let mut lines = Vec::new();
        let mut current = String::new();
        for word in text.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.chars().count() + 1 + word.chars().count() <= per_line {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        lines
    }
}

pub struct PanelText;

impl PanelText {
    pub const NOTES_PREFIX: &'static str = "Notes: ";
    pub const MAX_LINES: usize = 2;
    pub const NAME_MAX_LENGTH: usize = 20;
    pub const NAME_CUT_LENGTH: usize = 18;
    pub const NAME_CUT: &'static str = "..";
    pub const PREFIX: &'static str = "Orgs: ";
    pub const NOT_LOOKED_UP_TEXT: &'static str = "Orgs: not looked up";
    pub const NONE_FOUND_TEXT: &'static str = "Orgs: none found";
    pub const HIDDEN_MAIN_ORG_TEXT: &'static str = "Hidden main org";
    pub const REDACTED_TEXT: &'static str = "REDACTED";
    const ETC: &'static str = ", etc.";
    const SEPARATOR: &'static str = ", ";

    pub fn notes_line(note: &str, chars_per_line: usize) -> String {
        let first_line = note.lines().next().unwrap_or("").trim();
        let full = format!("{}{}", Self::NOTES_PREFIX, first_line);
        if full.chars().count() <= chars_per_line {
            full
        } else {
            let take = chars_per_line.saturating_sub(2).max(Self::NOTES_PREFIX.len());
            let truncated: String = full.chars().take(take).collect();
            format!("{}..", truncated)
        }
    }

    pub fn clean_name(name: Option<&str>) -> String {
        let Some(name) = name else {
            return String::new();
        };
        let mut out = String::with_capacity(name.len());
        let mut pending_space = false;
        for raw in name.chars() {
            let c = match raw {
                '‘' | '’' => '\'',
                '“' | '”' => '"',
                '–' | '—' => '-',
                _ => raw,
            };
            if c.is_whitespace() {
                pending_space = !out.is_empty();
                continue;
            }
            if ('!'..='~').contains(&c) || ('\u{00C0}'..='\u{00FF}').contains(&c) {
                if pending_space {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
            }
        }
        out
    }

    pub fn lines(
        orgs: &PanelOrgs,
        display: OrgDisplay,
        main_org_only: bool,
        chars_per_line: usize,
    ) -> Vec<String> {
        if !orgs.looked_up {
            return vec![Self::NOT_LOOKED_UP_TEXT.to_string()];
        }
        if orgs.orgs.is_empty() {
            return vec![Self::NONE_FOUND_TEXT.to_string()];
        }

        let items = Self::build_items(orgs, display, main_org_only);
        let per_line = chars_per_line.max(Self::PREFIX.len() + 2);

        let fulls: Vec<String> = items.iter().map(|(f, _)| f.clone()).collect();
        if let Some(w) = Self::wrap(&fulls, per_line, 0) {
            return w;
        }
        let cappeds: Vec<String> = items.iter().map(|(_, c)| c.clone()).collect();
        if let Some(w) = Self::wrap(&cappeds, per_line, 0) {
            return w;
        }
        Self::with_etc(&cappeds, per_line)
    }

    fn build_items(
        orgs: &PanelOrgs,
        display: OrgDisplay,
        main_org_only: bool,
    ) -> Vec<(String, String)> {
        let mut ordered: Vec<OrgMembership> = orgs
            .orgs
            .iter()
            .filter(|o| o.is_main)
            .chain(orgs.orgs.iter().filter(|o| !o.is_main))
            .cloned()
            .collect();

        let deciding = Self::deciding_index(&ordered, orgs);
        let main_idx = ordered.iter().position(|o| o.is_main);

        let (shown, mark_main) = if main_org_only && main_idx.is_some() {
            let mi = main_idx.unwrap();
            let mut s = vec![ordered[mi].clone()];
            if let Some(di) = deciding {
                if di != mi
                    && matches!(
                        orgs.match_kind,
                        Some(MatchKind::OrgList | MatchKind::OrgAttribute)
                    )
                {
                    s.push(ordered[di].clone());
                }
            }
            (s, false)
        } else {
            if let Some(di) = deciding {
                if di > 0 {
                    let first = ordered.remove(di);
                    ordered.insert(0, first);
                }
            }
            (ordered, !main_org_only)
        };

        shown
            .into_iter()
            .map(|o| {
                let label = Self::label(&o, display);
                let mark = if mark_main && o.is_main && !o.is_redacted() {
                    "*"
                } else {
                    ""
                };
                let suffix = if orgs.match_kind == Some(MatchKind::OrgAttribute)
                    && o.sid.is_some()
                    && orgs.deciding_sid.as_deref().is_some_and(|ds| {
                        o.sid.as_deref().unwrap_or_default().eq_ignore_ascii_case(ds)
                    })
                    && orgs.attribute_match.is_some()
                {
                    format!(" ({})", orgs.attribute_match.as_deref().unwrap())
                } else {
                    String::new()
                };
                let capped = if label.chars().count() > Self::NAME_MAX_LENGTH {
                    let cut: String = label.chars().take(Self::NAME_CUT_LENGTH).collect();
                    format!("{}{}", cut, Self::NAME_CUT)
                } else {
                    label.clone()
                };
                (
                    format!("{}{}{}", label, mark, suffix),
                    format!("{}{}{}", capped, mark, suffix),
                )
            })
            .collect()
    }

    fn label(o: &OrgMembership, display: OrgDisplay) -> String {
        if o.is_redacted() {
            return if o.is_main {
                Self::HIDDEN_MAIN_ORG_TEXT.to_string()
            } else {
                Self::REDACTED_TEXT.to_string()
            };
        }
        if display == OrgDisplay::FullName {
            let clean = Self::clean_name(o.name.as_deref());
            if !clean.is_empty() {
                return clean;
            }
        }
        o.sid.clone().unwrap_or_default()
    }

    fn deciding_index(ordered: &[OrgMembership], orgs: &PanelOrgs) -> Option<usize> {
        if let Some(sid) = orgs.deciding_sid.as_deref() {
            return ordered
                .iter()
                .position(|o| o.sid.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(sid)));
        }
        if orgs.match_kind == Some(MatchKind::OrgAttribute) {
            if orgs.attribute_match.as_deref() == Some(Self::REDACTED_TEXT) {
                return ordered.iter().position(|o| o.is_redacted());
            }
            if orgs
                .attribute_match
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case("Hidden Main Org"))
            {
                return ordered.iter().position(|o| o.is_main && o.is_redacted());
            }
        }
        None
    }

    fn wrap(items: &[String], per_line: usize, reserve: usize) -> Option<Vec<String>> {
        let mut lines = Vec::with_capacity(Self::MAX_LINES);
        let mut cur = String::from(Self::PREFIX);
        let mut first = true;

        for item in items {
            let cur_len = cur.chars().count();
            let item_len = item.chars().count();
            if first {
                if cur_len + item_len > per_line {
                    return None;
                }
                cur.push_str(item);
                first = false;
                continue;
            }
            if cur_len + Self::SEPARATOR.len() + item_len <= per_line {
                cur.push_str(Self::SEPARATOR);
                cur.push_str(item);
                continue;
            }
            if lines.len() + 1 >= Self::MAX_LINES || cur_len + 1 > per_line || item_len > per_line {
                return None;
            }
            cur.push(',');
            lines.push(cur);
            cur = item.clone();
        }
        if cur.chars().count() + reserve > per_line {
            return None;
        }
        lines.push(cur);
        Some(lines)
    }

    fn with_etc(items: &[String], per_line: usize) -> Vec<String> {
        for k in (1..items.len()).rev() {
            if let Some(mut lines) = Self::wrap(&items[..k], per_line, Self::ETC.len()) {
                if let Some(last) = lines.last_mut() {
                    last.push_str(Self::ETC);
                }
                return lines;
            }
        }
        let tail = if items.len() > 1 { Self::ETC } else { "" };
        let room = per_line
            .saturating_sub(Self::PREFIX.len() + tail.len())
            .max(1);
        let first = &items[0];
        let text = if first.chars().count() <= room {
            first.clone()
        } else {
            let cut: String = first
                .chars()
                .take(room.saturating_sub(Self::NAME_CUT.len()).max(1))
                .collect();
            format!("{}{}", cut, Self::NAME_CUT)
        };
        vec![format!("{}{}{}", Self::PREFIX, text, tail)]
    }
}

#[derive(Debug, Clone)]
pub struct DossierOrgBox {
    pub title: String,
    pub full_name: Option<String>,
    pub archetype: Option<String>,
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub sid: Option<String>,
    pub target: Option<AttributeTarget>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DossierPlacement {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub bottom: f64,
}

impl DossierPlacement {
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.bottom <= self.top
    }
}

pub struct DossierText;

impl DossierText {
    pub const PANEL_GAP_SHARE: f64 = 0.10;
    pub const LEFT_MARGIN_SHARE: f64 = 0.05;
    pub const NO_CITIZEN_RECORD: &'static str = "n/a";

    pub fn org_boxes(orgs: &[OrgMembership]) -> Vec<DossierOrgBox> {
        let mut boxes = Vec::new();
        if let Some(main) = orgs.iter().find(|o| o.is_main) {
            boxes.push(Self::make_box(main, "Main"));
        } else {
            boxes.push(DossierOrgBox {
                title: "Main: NONE".to_string(),
                full_name: None,
                archetype: None,
                primary: None,
                secondary: None,
                sid: None,
                target: Some(AttributeTarget {
                    kind: AttributeKind::NoOrg,
                    value: None,
                }),
            });
        }
        for aff in orgs.iter().filter(|o| !o.is_main) {
            boxes.push(Self::make_box(aff, "Affiliate"));
        }
        boxes
    }

    fn make_box(org: &OrgMembership, prefix: &str) -> DossierOrgBox {
        let Some(sid) = org.sid.as_deref() else {
            return if org.is_main {
                DossierOrgBox {
                    title: "Main: HIDDEN".to_string(),
                    full_name: None,
                    archetype: None,
                    primary: None,
                    secondary: None,
                    sid: None,
                    target: Some(AttributeTarget {
                        kind: AttributeKind::HiddenMainOrg,
                        value: None,
                    }),
                }
            } else {
                DossierOrgBox {
                    title: "Affiliate: REDACTED".to_string(),
                    full_name: None,
                    archetype: None,
                    primary: None,
                    secondary: None,
                    sid: None,
                    target: Some(AttributeTarget {
                        kind: AttributeKind::Redacted,
                        value: None,
                    }),
                }
            };
        };
        DossierOrgBox {
            title: format!("{}: {}", prefix, sid),
            full_name: org.name.clone(),
            archetype: org.archetype.clone(),
            primary: org.activities.first().cloned(),
            secondary: org.activities.get(1).cloned(),
            sid: Some(sid.to_string()),
            target: None,
        }
    }

    pub fn classification_from(
        kind: MatchKind,
        color_match: Option<&ColorMatch>,
        config: &SocialConfig,
    ) -> String {
        match kind {
            MatchKind::PlayerList => "Named List".to_string(),
            MatchKind::OrgList => match color_match.and_then(|m| m.deciding_org_sid.as_deref()) {
                Some(sid) if !sid.is_empty() => format!("Org Affiliation ({})", sid),
                _ => "Org Affiliation".to_string(),
            },
            MatchKind::OrgAttribute => {
                let attr = color_match.and_then(|m| m.deciding_attribute.as_deref());
                format!("Org-Attribute ({})", Self::attribute_text(attr, config))
            }
            MatchKind::NoMatch => "No match".to_string(),
        }
    }

    fn attribute_text(text: Option<&str>, config: &SocialConfig) -> String {
        let Some(t) = text.filter(|s| !s.is_empty()) else {
            return "?".to_string();
        };
        for rule in &config.org_rules {
            if matches!(rule.kind, AttributeKind::Archetype | AttributeKind::Activity)
                && rule
                    .value
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .eq_ignore_ascii_case(t)
            {
                let pfx = if rule.kind == AttributeKind::Archetype {
                    "Archetype: "
                } else {
                    "Activity: "
                };
                return format!("{}{}", pfx, t);
            }
        }
        t.to_string()
    }

    pub fn panel_area(settings: &AppSettings, ww: f64, wh: f64) -> (f64, f64, f64, f64) {
        let width = ww * settings.panel_width_percent as f64 / 100.0;
        let right = ww - ww * settings.panel_right_gap_percent as f64 / 100.0;
        let top = wh * settings.panel_top_gap_percent as f64 / 100.0;
        let bottom = wh * (1.0 - PanelLayout::BOTTOM_RESERVE_PERCENT / 100.0);
        ((right - width).max(0.0), top, width, bottom)
    }

    pub fn place_for(settings: &AppSettings, ww: f64, wh: f64) -> DossierPlacement {
        let (left, top, _, bottom) = Self::panel_area(settings, ww, wh);
        let right = left - ww * Self::PANEL_GAP_SHARE;
        let width = ww * settings.dossier_width_percent as f64 / 100.0;
        if right <= 0.0 || width <= 0.0 {
            DossierPlacement {
                left: 0.0,
                top,
                width: 0.0,
                bottom,
            }
        } else {
            DossierPlacement {
                left: right - width,
                top,
                width,
                bottom,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    Windowed = 0,
    Borderless = 1,
    Fullscreen = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameWindowState {
    GameNotActive,
    NotBorderless,
    Ready,
}

pub struct GameWindowRules;

impl GameWindowRules {
    pub fn decide(is_star_citizen_in_front: bool, game_mode: Option<WindowMode>) -> GameWindowState {
        if !is_star_citizen_in_front {
            return GameWindowState::GameNotActive;
        }
        match game_mode {
            None => GameWindowState::GameNotActive,
            Some(WindowMode::Borderless) => GameWindowState::Ready,
            Some(_) => GameWindowState::NotBorderless,
        }
    }
}

#[derive(Debug, Default)]
pub struct BorderlessNoticeTracker {
    pub was_shown: bool,
}

impl BorderlessNoticeTracker {
    pub const TEXT: &'static str =
        "SC Overlay requires Borderless mode to function. Change in Options > Graphics > Window Mode.";

    pub fn should_show(
        &self,
        state: GameWindowState,
        overlay_disabled: bool,
        other_popup_open: bool,
        game_mode: Option<WindowMode>,
    ) -> bool {
        state == GameWindowState::NotBorderless
            && game_mode != Some(WindowMode::Borderless)
            && !self.was_shown
            && !overlay_disabled
            && !other_popup_open
    }

    pub fn mark_shown(&mut self) {
        self.was_shown = true;
    }
}

pub struct StarCitizenDisplaySettings;

impl StarCitizenDisplaySettings {
    /// Locates `<Install>/user/client/0/Profiles/default/attributes.xml` (case-insensitive on Linux Wine/LUG prefixes).
    pub fn attributes_path_for_exe(exe_path: &Path) -> Option<PathBuf> {
        let bin_dir = exe_path.parent()?;
        let bin_name = bin_dir.file_name()?.to_str()?;
        if !bin_name.eq_ignore_ascii_case("Bin64") {
            return None;
        }
        let install_dir = bin_dir.parent()?;
        let user_dir = Self::find_child_ci(install_dir, "user")?;
        let client_dir = Self::find_child_ci(&user_dir, "client")?;

        if let Some(p0) = Self::find_child_ci(&client_dir, "0")
            .and_then(|d| Self::find_child_ci(&d, "Profiles"))
            .and_then(|d| Self::find_child_ci(&d, "default"))
            .and_then(|d| Self::find_child_ci(&d, "attributes.xml"))
        {
            if p0.is_file() {
                return Some(p0);
            }
        }

        // Fallback: newest attributes.xml under user/client/*/Profiles/default/attributes.xml
        let mut best: Option<(PathBuf, SystemTime)> = None;
        if let Ok(entries) = std::fs::read_dir(&client_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(attr) = Self::find_child_ci(&p, "Profiles")
                    .and_then(|d| Self::find_child_ci(&d, "default"))
                    .and_then(|d| Self::find_child_ci(&d, "attributes.xml"))
                {
                    if let Ok(meta) = std::fs::metadata(&attr) {
                        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        if best.as_ref().map(|(_, bt)| mtime > *bt).unwrap_or(true) {
                            best = Some((attr, mtime));
                        }
                    }
                }
            }
        }
        best.map(|(p, _)| p)
    }

    fn find_child_ci(dir: &Path, target: &str) -> Option<PathBuf> {
        let direct = dir.join(target);
        if direct.exists() {
            return Some(direct);
        }
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(target))
            {
                return Some(entry.path());
            }
        }
        None
    }

    /// Parses `<Attr name="WindowMode" value="1"/>` from Star Citizen's `attributes.xml`.
    pub fn parse_window_mode(xml_text: &str) -> Option<WindowMode> {
        if xml_text.trim().is_empty() {
            return None;
        }
        for raw_tag in xml_text.split('<') {
            let tag = raw_tag.trim();
            if !tag.starts_with("Attr") {
                continue;
            }
            let Some(end) = tag.find('>') else {
                continue;
            };
            let inside = &tag[..end];
            if Self::extract_xml_attr(inside, "name") == Some("WindowMode") {
                let val_str = Self::extract_xml_attr(inside, "value")?.trim();
                let num: i32 = val_str.parse().ok()?;
                return match num {
                    0 => Some(WindowMode::Windowed),
                    1 => Some(WindowMode::Borderless),
                    2 => Some(WindowMode::Fullscreen),
                    _ => None,
                };
            }
        }
        Some(WindowMode::Windowed)
    }

    fn extract_xml_attr<'a>(tag: &'a str, attr_name: &str) -> Option<&'a str> {
        let needle = format!("{}=\"", attr_name);
        if let Some(pos) = tag.find(&needle) {
            let rest = &tag[pos + needle.len()..];
            let end = rest.find('"')?;
            return Some(&rest[..end]);
        }
        let needle_sq = format!("{}='", attr_name);
        if let Some(pos) = tag.find(&needle_sq) {
            let rest = &tag[pos + needle_sq.len()..];
            let end = rest.find('\'')?;
            return Some(&rest[..end]);
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOverlayPart {
    Boundary,
    Panel,
    DebugHud,
    Breadcrumbs,
    Waypoint,
}

pub struct TestOverlayBuilder;

impl TestOverlayBuilder {
    pub const GENERIC_NAMES: [&'static str; 8] = [
        "John", "Adam", "Steve", "Mike", "Chris", "David", "Paul", "Mark",
    ];
    const JITTER: [(f64, f64); 8] = [
        (-0.10, 0.12),
        (0.08, -0.10),
        (-0.04, 0.05),
        (0.12, 0.10),
        (0.06, -0.12),
        (-0.12, 0.08),
        (0.10, -0.04),
        (-0.06, -0.10),
    ];

    pub fn build(
        config: &SocialConfig,
        screen_width: f64,
        screen_height: f64,
        boundary: Option<TrackingBoundary>,
        part: TestOverlayPart,
        settings: Option<&AppSettings>,
    ) -> OverlayModel {
        let area = boundary.unwrap_or_default();
        let mut name_idx = 0usize;
        let mut next_name = || -> String {
            let n = Self::GENERIC_NAMES
                .get(name_idx)
                .copied()
                .unwrap_or("Pilot");
            name_idx += 1;
            n.to_string()
        };

        let mut shown = SocialConfig {
            name: config.name.clone(),
            iris_version: config.iris_version.clone(),
            orgs_main_org_only: config.orgs_main_org_only,
            org_rules_main_org_only: config.org_rules_main_org_only,
            players: Vec::new(),
            orgs: Vec::new(),
            org_rules: config.org_rules.clone(),
        };
        shown.ensure_normalized();

        let mut made: HashMap<String, Vec<OrgMembership>> = HashMap::new();
        let mut handles: Vec<String> = Vec::new();

        for p in config.players.iter().take(3) {
            let h = next_name();
            shown.players.push(PlayerEntry {
                handle: h.clone(),
                color: p.color,
            });
            handles.push(h);
        }

        let mut org_num = 0usize;
        for org in config.orgs.iter().take(2) {
            org_num += 1;
            let h = next_name();
            let sid = format!("COMPANY{}", org_num);
            shown.orgs.push(OrgEntry {
                sid: sid.clone(),
                priority: org.priority,
                color: org.color,
            });
            made.insert(
                h.to_ascii_lowercase(),
                vec![OrgMembership {
                    sid: Some(sid),
                    is_main: true,
                    archetype: None,
                    activities: Vec::new(),
                    name: Some(format!("Example Company {}", org_num)),
                }],
            );
            handles.push(h);
        }

        if let Some(rule) = config
            .org_rules
            .iter()
            .find(|r| r.kind != AttributeKind::NoMatch)
        {
            org_num += 1;
            let h = next_name();
            let sid = format!("COMPANY{}", org_num);
            let full = format!("Example Company {}", org_num);
            made.insert(h.to_ascii_lowercase(), Self::orgs_for_rule(rule, &sid, &full));
            handles.push(h);
        }

        handles.push(next_name());
        handles.push(next_name());

        let mut brackets = Vec::new();
        let mut panel_entries = Vec::new();
        let mut seen = HashSet::new();

        for (i, handle) in handles.iter().enumerate() {
            let box_rect = Self::box_for(i, handle, screen_width, screen_height, area);
            let orgs_opt = made.get(&handle.to_ascii_lowercase());
            let cm = match orgs_opt {
                Some(orgs) => ColorResolver::resolve(&shown, handle, orgs),
                None => ColorResolver::resolve_unknown(&shown, handle),
            };
            brackets.push(BracketItem {
                marker_id: (i + 1) as u64,
                handle: handle.clone(),
                name_box: box_rect,
                color: cm.color,
                kind: cm.kind,
            });
            if seen.insert(handle.to_ascii_lowercase()) {
                let p_orgs = match orgs_opt {
                    Some(orgs) => PanelOrgs {
                        looked_up: true,
                        orgs: orgs.clone(),
                        deciding_sid: cm.deciding_org_sid.clone(),
                        match_kind: Some(cm.kind),
                        attribute_match: cm.deciding_attribute.clone(),
                    },
                    None => PanelOrgs::default(),
                };
                panel_entries.push(PanelEntry {
                    handle: handle.clone(),
                    color: cm.color,
                    kind: cm.kind,
                    orgs: p_orgs,
                    note: None,
                    live: true,
                });
            }
        }
        panel_entries.sort_by_key(|e| e.kind);

        match part {
            TestOverlayPart::Panel => {
                let dossier_placeholder = settings.and_then(|s| {
                    let place = DossierText::place_for(s, screen_width, screen_height);
                    if place.is_empty() {
                        None
                    } else {
                        Some(PixelRect::new(
                            place.left,
                            place.top,
                            place.width,
                            place.bottom - place.top,
                        ))
                    }
                });
                OverlayModel {
                    window_width: screen_width as i32,
                    window_height: screen_height as i32,
                    brackets: Vec::new(),
                    panel_entries,
                    mining_labels: Vec::new(),
                    toast: None,
                    center_toast: None,
                    note: None,
                    boundary_rect: None,
                    dossier_placeholder,
                    debug_info: None,
                    breadcrumb_info: None,
                    waypoint_info: None,
                }
            }
            TestOverlayPart::Boundary => OverlayModel {
                window_width: screen_width as i32,
                window_height: screen_height as i32,
                brackets,
                panel_entries: Vec::new(),
                mining_labels: Vec::new(),
                toast: None,
                center_toast: None,
                note: None,
                boundary_rect: Some(area.to_pixels(screen_width, screen_height)),
                dossier_placeholder: None,
                debug_info: None,
                breadcrumb_info: None,
                waypoint_info: None,
            },
            TestOverlayPart::DebugHud => {
                let show_cands = settings.map(|s| s.debug_show_candidates).unwrap_or(false);
                let mut candidate_boxes = Vec::new();
                if show_cands {
                    for (idx, b) in brackets.iter().take(4).enumerate() {
                        let kind = match idx {
                            0 => DebugBoxKind::TentativeTrack,
                            1 => DebugBoxKind::PairedCandidate,
                            _ => DebugBoxKind::Candidate,
                        };
                        candidate_boxes.push(DebugCandidateBox {
                            rect: b.name_box,
                            kind,
                        });
                    }
                }
                let scan_boundary = if show_cands {
                    Some(area.to_pixels(screen_width, screen_height))
                } else {
                    None
                };
                OverlayModel {
                    window_width: screen_width as i32,
                    window_height: screen_height as i32,
                    brackets: Vec::new(),
                    panel_entries: Vec::new(),
                    mining_labels: Vec::new(),
                    toast: None,
                    center_toast: None,
                    note: None,
                    boundary_rect: None,
                    dossier_placeholder: None,
                    debug_info: Some(DebugOverlayInfo {
                        level: DebugStatusLevel::Active,
                        headline: "PREVIEW - SCANNING (IDLE)  #0042".to_string(),
                        lines: vec![
                            format!(
                                "Loop: 90 Hz  |  Cap: 60 FPS ({}x{})  |  Scan: 1.4 ms",
                                screen_width as i32, screen_height as i32
                            ),
                            "Plates: 4 boxes, 1 pair, 1 OCR  |  Tracks: 0 conf, 1 tent  |  RSI: 0"
                                .to_string(),
                            "Mining: Searching  |  Last OCR: none (preview mode)".to_string(),
                        ],
                        heartbeat_phase: 0,
                        candidate_boxes,
                        scan_boundary,
                    }),
                    breadcrumb_info: None,
                    waypoint_info: None,
                }
            }
            TestOverlayPart::Breadcrumbs => {
                let radius = settings
                    .map(|s| s.breadcrumbs_visited_radius_km as f64)
                    .unwrap_or(5.0);
                let show_cloud = settings.map(|s| s.breadcrumbs_screen_cloud).unwrap_or(true);
                OverlayModel {
                    window_width: screen_width as i32,
                    window_height: screen_height as i32,
                    brackets: Vec::new(),
                    panel_entries: Vec::new(),
                    mining_labels: Vec::new(),
                    toast: None,
                    center_toast: None,
                    note: None,
                    boundary_rect: None,
                    dossier_placeholder: None,
                    debug_info: None,
                    breadcrumb_info: Some(BreadcrumbOverlayInfo {
                        zone: ProximityZone::Visited,
                        headline: "VISITED CLOUD - 2.1 km from #3 (14m ago)".to_string(),
                        subline: format!("6 pts  |  Cloud {:.0} km  |  dZ +0.2 km", radius),
                        visited_radius_km: radius,
                        heading_rad: Some(0.65),
                        show_screen_cloud: show_cloud,
                        active_waypoint: Some(BreadcrumbWaypointPin {
                            name: "Quant Cluster Alpha".to_string(),
                            dx_km: 7.4,
                            dy_km: 6.6,
                            color: OverlayColor::Cyan,
                        }),
                        points: vec![
                            BreadcrumbRenderPoint {
                                id: 1,
                                dx_km: -9.5,
                                dy_km: -6.0,
                                dz_km: -0.4,
                                age_mins: 32,
                                is_current: false,
                            },
                            BreadcrumbRenderPoint {
                                id: 2,
                                dx_km: -5.2,
                                dy_km: -1.8,
                                dz_km: -0.1,
                                age_mins: 24,
                                is_current: false,
                            },
                            BreadcrumbRenderPoint {
                                id: 3,
                                dx_km: -1.6,
                                dy_km: 1.4,
                                dz_km: 0.0,
                                age_mins: 14,
                                is_current: false,
                            },
                            BreadcrumbRenderPoint {
                                id: 4,
                                dx_km: 4.5,
                                dy_km: 5.0,
                                dz_km: 0.3,
                                age_mins: 9,
                                is_current: false,
                            },
                            BreadcrumbRenderPoint {
                                id: 5,
                                dx_km: 3.0,
                                dy_km: -2.5,
                                dz_km: -0.2,
                                age_mins: 4,
                                is_current: false,
                            },
                            BreadcrumbRenderPoint {
                                id: 6,
                                dx_km: 0.0,
                                dy_km: 0.0,
                                dz_km: 0.0,
                                age_mins: 0,
                                is_current: true,
                            },
                        ],
                    }),
                    waypoint_info: None,
                }
            }
            TestOverlayPart::Waypoint => {
                let show_ring = settings
                    .map(|s| s.waypoints_show_ring_marker)
                    .unwrap_or(true);
                OverlayModel {
                    window_width: screen_width as i32,
                    window_height: screen_height as i32,
                    brackets: Vec::new(),
                    panel_entries: Vec::new(),
                    mining_labels: Vec::new(),
                    toast: None,
                    center_toast: None,
                    note: None,
                    boundary_rect: None,
                    dossier_placeholder: None,
                    debug_info: None,
                    breadcrumb_info: None,
                    waypoint_info: Some(WaypointOverlayInfo {
                        waypoint_id: "preview".to_string(),
                        name: "Quant Cluster Alpha".to_string(),
                        color: OverlayColor::Cyan,
                        has_position_fix: true,
                        arrived: false,
                        distance_m: 9_900.0,
                        distance_text: "9.9 km".to_string(),
                        steering_text: "RIGHT 28°".to_string(),
                        vertical_text: "UP +420 m".to_string(),
                        subline: "RIGHT 28°  |  UP +420 m  |  ETA 0:46  |  ZONE PYRO6".to_string(),
                        rel_yaw_rad: Some(-28.0_f64.to_radians()),
                        dz_m: 420.0,
                        world_bearing_deg: 62.0,
                        closing_mps: Some(215.0),
                        zone_lock: Some("pyro6".to_string()),
                        show_ring_marker: show_ring,
                    }),
                }
            }
        }
    }

    fn orgs_for_rule(rule: &OrgAttributeRule, sid: &str, name: &str) -> Vec<OrgMembership> {
        match rule.kind {
            AttributeKind::NoOrg | AttributeKind::NoMatch => Vec::new(),
            AttributeKind::Redacted | AttributeKind::HiddenMainOrg => {
                vec![OrgMembership::new(None, true)]
            }
            AttributeKind::Archetype => vec![OrgMembership {
                sid: Some(sid.to_string()),
                is_main: true,
                archetype: rule.value.clone(),
                activities: Vec::new(),
                name: Some(name.to_string()),
            }],
            AttributeKind::Activity => vec![OrgMembership {
                sid: Some(sid.to_string()),
                is_main: true,
                archetype: None,
                activities: vec![rule.value.clone().unwrap_or_default()],
                name: Some(name.to_string()),
            }],
        }
    }

    pub fn box_for(
        index: usize,
        handle: &str,
        screen_width: f64,
        screen_height: f64,
        boundary: TrackingBoundary,
    ) -> PixelRect {
        const COLS: usize = 4;
        const ROWS: usize = 2;
        let area = boundary.to_pixels(screen_width, screen_height);
        let cell_w = area.width / COLS as f64;
        let cell_h = area.height / ROWS as f64;
        let box_h = screen_height * 0.032;
        let box_w = (screen_height * (0.04 + 0.0055 * handle.len() as f64)).min(cell_w * 0.9);
        let slot = index % (COLS * ROWS);
        let col = slot % COLS;
        let row = slot / COLS;
        let (jx, jy) = Self::JITTER[slot];
        let cx = area.x + (col as f64 + 0.5 + jx) * cell_w;
        let cy = area.y + (row as f64 + 0.5 + jy) * cell_h;
        let x = (cx - box_w * 0.5).clamp(area.x, (area.right() - box_w).max(area.x));
        let y = (cy - box_h * 0.5).clamp(area.y, (area.bottom() - box_h).max(area.y));
        PixelRect::new(x, y, box_w, box_h)
    }
}

