use crate::state::{AppSharedState, CoordinatorCommand};
use crate::theme::{
    color_combo, color_rank_strip, optional_color_combo, overlay_color32, quantico_text,
    ThemePalette,
};
use eframe::egui::{
    self, Align, Key, Layout, Margin, RichText, Rounding, ScrollArea, Slider, Stroke, TextEdit,
};
use iris_core::breadcrumbs::SpaceCoord;
use iris_core::config::{
    AppSettings, AttributeKind, AttributeTarget, CaptureMethod, ColorRanks, DarkMode,
    HostedConfigUrl, HotkeyChord, HotkeyText, ImportMode, OrgDisplay, OrgVocabulary,
    SocialConfigMerger, SocialConfigToml,
};
use iris_core::lookup::RsiNames;
use iris_core::models::OverlayColor;
use iris_core::overlay::{GameWindowRules, GameWindowState, TestOverlayPart};
use iris_core::waypoints::format_distance_m;
use iris_platform::{open_folder_or_file, open_https_url};
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Players,
    Orgs,
    OrgAttribute,
    Waypoints,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingHotkey {
    Toggle,
    Timed,
    Interactive,
    SaveWaypoint,
}

pub struct SettingsWindowState {
    active_tab: SettingsTab,
    new_player_handle: String,
    new_player_color: OverlayColor,
    player_filter: String,
    new_org_sid: String,
    new_org_color: OverlayColor,
    org_filter: String,
    new_poi_name: String,
    new_poi_color: OverlayColor,
    paste_poi_coords: String,
    poi_filter: String,
    editing_poi_id: Option<String>,
    editing_poi_name: String,
    editing_poi_notes: String,
    hosted_url_draft: String,
    hosted_url_initialized: bool,
    recording_hotkey: Option<RecordingHotkey>,
}

impl Default for SettingsWindowState {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsWindowState {
    pub fn new() -> Self {
        Self {
            active_tab: SettingsTab::Players,
            new_player_handle: String::new(),
            new_player_color: OverlayColor::Red,
            player_filter: String::new(),
            new_org_sid: String::new(),
            new_org_color: OverlayColor::Red,
            org_filter: String::new(),
            new_poi_name: String::new(),
            new_poi_color: OverlayColor::Cyan,
            paste_poi_coords: String::new(),
            poi_filter: String::new(),
            editing_poi_id: None,
            editing_poi_name: String::new(),
            editing_poi_notes: String::new(),
            hosted_url_draft: String::new(),
            hosted_url_initialized: false,
            recording_hotkey: None,
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        if !self.hosted_url_initialized {
            self.hosted_url_draft = shared.settings.read().hosted_config_url.clone();
            self.hosted_url_initialized = true;
        }

        // Handle active hotkey recording before standard widgets consume keys
        if let Some(target_hk) = self.recording_hotkey {
            self.capture_hotkey_input(ui.ctx(), target_hk, shared);
        }

        self.render_header(ui, p, shared);
        ui.add_space(6.0);
        self.render_banners(ui, p, shared);
        self.render_tab_bar(ui, p);
        ui.add_space(8.0);

        match self.active_tab {
            SettingsTab::Players => self.render_players_tab(ui, p, shared),
            SettingsTab::Orgs => self.render_orgs_tab(ui, p, shared),
            SettingsTab::OrgAttribute => self.render_org_attribute_tab(ui, p, shared),
            SettingsTab::Waypoints => self.render_waypoints_tab(ui, p, shared),
            SettingsTab::Settings => self.render_settings_tab(ui, p, shared),
        }
    }

    fn render_header(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        ui.horizontal(|ui| {
            ui.label(quantico_text("SC OVERLAY", 21.0, p.accent));
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .size(12.0)
                    .color(p.muted),
            );

            let game = shared.last_game_window.read().clone();
            let state = GameWindowRules::decide(game.focused && !game.minimized, game.window_mode);
            let (status_txt, status_col) = if !game.present {
                ("● Waiting for Star Citizen", p.muted)
            } else if state == GameWindowState::NotBorderless {
                ("▲ Star Citizen Not Borderless", OverlayColor::Orange.rgb().into_color32())
            } else if game.focused {
                ("● Star Citizen Active (Borderless)", p.success)
            } else {
                ("● Star Citizen Running (Unfocused)", p.accent)
            };
            ui.add_space(8.0);
            ui.label(RichText::new(status_txt).size(12.5).color(status_col));
            let diag_ticks = shared.live_diagnostics.read().coordinator_ticks;
            let spin = match (diag_ticks / 2) % 4 {
                0 => "◐",
                1 => "◓",
                2 => "◑",
                _ => "◒",
            };
            ui.label(
                RichText::new(format!("{} #{:04}", spin, diag_ticks % 10_000))
                    .monospace()
                    .size(11.5)
                    .color(p.muted),
            )
            .on_hover_text("Live background coordinator heartbeat (increments continuously while running)");

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button(RichText::new("Quit").color(p.danger))
                    .on_hover_text("Exit SC Overlay completely (closing the window [X] minimizes to tray)")
                    .clicked()
                {
                    shared.quit_requested.store(true, Ordering::Relaxed);
                    shared.request_repaint();
                }

                if ui
                    .button("Interactive Dossier")
                    .on_hover_text("Open full-screen Citizen Dossier & Notes overlay")
                    .clicked()
                {
                    let _ = shared
                        .coordinator_tx
                        .send(CoordinatorCommand::OpenInteractive);
                }

                let mut debug_hud = shared.settings.read().debug_overlay_enabled;
                if ui
                    .checkbox(&mut debug_hud, "Debug HUD")
                    .on_hover_text(
                        "Show live heartbeat, capture FPS, scan latency, and detector counters on the overlay",
                    )
                    .changed()
                {
                    let _ = shared
                        .coordinator_tx
                        .send(CoordinatorCommand::ToggleDebugOverlay);
                }

                let mut overlay_enabled = !shared.settings.read().overlay_disabled;
                if ui.checkbox(&mut overlay_enabled, "Overlay Enabled").changed() {
                    let _ = shared
                        .coordinator_tx
                        .send(CoordinatorCommand::ToggleOverlay);
                }
            });
        });
    }

    fn render_banners(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        let update_opt = shared.update_available.read().clone();
        if let Some((tag, url)) = update_opt {
            egui::Frame::none()
                .fill(p.accent.gamma_multiply(0.15))
                .stroke(Stroke::new(1.0, p.accent))
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::symmetric(10.0, 5.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("New release {} is available!", tag))
                                .strong()
                                .color(p.accent),
                        );
                        if ui.small_button("View Release ↗").clicked() {
                            let _ = open_https_url(&url);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                *shared.update_available.write() = None;
                            }
                        });
                    });
                });
            ui.add_space(4.0);
        }

        let banner_opt = shared.status_banner.read().clone();
        if let Some(banner) = banner_opt {
            let col = if banner.is_error { p.danger } else { p.accent };
            egui::Frame::none()
                .fill(col.gamma_multiply(0.14))
                .stroke(Stroke::new(1.0, col))
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::symmetric(10.0, 5.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&banner.text).size(12.5).color(p.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                shared.clear_status();
                            }
                        });
                    });
                });
            ui.add_space(4.0);
        }
    }

    fn render_tab_bar(&mut self, ui: &mut egui::Ui, p: &ThemePalette) {
        egui::Frame::none()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let tabs = [
                        (SettingsTab::Players, "{ } Players"),
                        (SettingsTab::Orgs, "[ ] Orgs"),
                        (SettingsTab::OrgAttribute, "< > Org-Attribute"),
                        (SettingsTab::Waypoints, "⌖ POIs / Beacons"),
                        (SettingsTab::Settings, "⚙ Settings"),
                    ];
                    for (tab, label) in tabs {
                        let selected = self.active_tab == tab;
                        let text = RichText::new(label)
                            .size(13.5)
                            .strong()
                            .color(if selected { p.accent } else { p.text });
                        if ui.selectable_label(selected, text).clicked() {
                            self.active_tab = tab;
                        }
                    }
                });
            });
    }

    // ========================================================================
    // Tab 1: Players
    // ========================================================================
    fn render_players_tab(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        egui::Frame::none()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::same(10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Handle:").color(p.muted));
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.new_player_handle)
                            .desired_width(220.0)
                            .hint_text("Star Citizen Handle…"),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));

                    color_combo(ui, "new_player_color", &mut self.new_player_color, p.is_dark);

                    if ui.button("＋ Add Player").clicked() || enter {
                        let raw = self.new_player_handle.trim().to_string();
                        if !RsiNames::is_valid_handle(&raw) {
                            shared.set_status(
                                format!(
                                    "\"{}\" is not a valid handle (4-64 chars: letters, digits, - and _).",
                                    raw
                                ),
                                true,
                            );
                        } else {
                            shared
                                .social
                                .write()
                                .upsert_player(&raw, self.new_player_color);
                            shared.save_social();
                            shared.spawn_verify_and_cache_player(raw, true);
                            self.new_player_handle.clear();
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.player_filter)
                                .desired_width(180.0)
                                .hint_text("Filter players…"),
                        );
                    });
                });
            });

        ui.add_space(8.0);

        let players_snap = shared.social.read().players.clone();
        let filter_q = self.player_filter.trim().to_ascii_lowercase();
        let filtered: Vec<_> = players_snap
            .iter()
            .filter(|pl| filter_q.is_empty() || pl.handle.to_ascii_lowercase().contains(&filter_q))
            .cloned()
            .collect();

        ui.label(
            RichText::new(format!(
                "Showing {} of {} listed players (Tier 1 highest priority — renders {{ }} curly brackets)",
                filtered.len(),
                players_snap.len()
            ))
            .size(12.0)
            .color(p.muted),
        );
        ui.add_space(4.0);

        let mut color_updates: Vec<(String, OverlayColor)> = Vec::new();
        let mut removals: Vec<String> = Vec::new();

        ScrollArea::vertical()
            .id_salt("players_tab_scroll")
            .show(ui, |ui| {
                for entry in filtered {
                    let cached = shared.cache.get_player(&entry.handle);
                    let main_org_txt = cached
                        .as_ref()
                        .and_then(|c| c.orgs.iter().find(|o| o.is_main))
                        .map(|o| {
                            if o.is_redacted() {
                                "Hidden Main Org".to_string()
                            } else {
                                format!(
                                    "[{}] {}",
                                    o.sid.as_deref().unwrap_or(""),
                                    o.name.as_deref().unwrap_or("")
                                )
                            }
                        })
                        .unwrap_or_default();

                    egui::Frame::none()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0, p.border))
                        .rounding(Rounding::same(6.0))
                        .inner_margin(Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let c32 = overlay_color32(entry.color);
                                ui.label(
                                    RichText::new(format!("{{{}}}", entry.handle))
                                        .strong()
                                        .size(14.0)
                                        .color(c32),
                                );
                                if !main_org_txt.is_empty() {
                                    ui.label(
                                        RichText::new(main_org_txt).size(12.0).color(p.muted),
                                    );
                                }
                                if shared.notes.read().has_note(&entry.handle) {
                                    ui.label(RichText::new("[Note]").size(11.5).color(p.accent));
                                }

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .small_button(RichText::new("Remove").color(p.danger))
                                        .clicked()
                                    {
                                        removals.push(entry.handle.clone());
                                    }
                                    if ui.small_button("RSI ↗").clicked() {
                                        let url = format!(
                                            "https://robertsspaceindustries.com/en/citizens/{}",
                                            entry.handle
                                        );
                                        let _ = open_https_url(&url);
                                    }
                                    let mut col = entry.color;
                                    if color_combo(
                                        ui,
                                        ("pl_col", &entry.handle),
                                        &mut col,
                                        p.is_dark,
                                    ) {
                                        color_updates.push((entry.handle.clone(), col));
                                    }
                                });
                            });
                        });
                    ui.add_space(4.0);
                }
            });

        if !color_updates.is_empty() || !removals.is_empty() {
            let mut social = shared.social.write();
            for (h, c) in color_updates {
                social.upsert_player(&h, c);
            }
            for h in removals {
                social.remove_player(&h);
            }
            drop(social);
            shared.save_social();
        }
    }

    // ========================================================================
    // Tab 2: Orgs
    // ========================================================================
    fn render_orgs_tab(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        let active_colors = {
            let social = shared.social.read();
            ColorRanks::active_org_colors(&social.orgs)
        };
        let mut rank_moved: Option<(OverlayColor, isize)> = None;
        color_rank_strip(ui, p, &active_colors, |col, delta| {
            rank_moved = Some((col, delta));
        });
        if let Some((col, delta)) = rank_moved {
            {
                let mut social = shared.social.write();
                ColorRanks::move_org_color(&mut social.orgs, col, delta);
            }
            shared.save_social();
        }

        ui.add_space(6.0);

        egui::Frame::none()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::same(10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Org SID:").color(p.muted));
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.new_org_sid)
                            .desired_width(150.0)
                            .hint_text("e.g. XPLOR…"),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));

                    color_combo(ui, "new_org_color", &mut self.new_org_color, p.is_dark);

                    if ui.button("＋ Add Org").clicked() || enter {
                        let sid = self.new_org_sid.trim().to_ascii_uppercase();
                        if !RsiNames::is_valid_sid(&sid) {
                            shared.set_status(
                                format!(
                                    "\"{}\" is not a valid Org SID (2-10 letters/digits).",
                                    sid
                                ),
                                true,
                            );
                        } else {
                            shared.social.write().upsert_org(&sid, self.new_org_color);
                            shared.save_social();
                            shared.spawn_verify_and_cache_org(sid);
                            self.new_org_sid.clear();
                        }
                    }

                    let mut main_only = shared.social.read().orgs_main_org_only;
                    if ui
                        .checkbox(&mut main_only, "Match main org only")
                        .changed()
                    {
                        shared.social.write().orgs_main_org_only = main_only;
                        shared.save_social();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.org_filter)
                                .desired_width(160.0)
                                .hint_text("Filter orgs…"),
                        );
                    });
                });
            });

        ui.add_space(8.0);

        let mut orgs_snap = shared.social.read().orgs.clone();
        orgs_snap.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.sid.cmp(&b.sid)));
        let filter_q = self.org_filter.trim().to_ascii_lowercase();
        let filtered: Vec<_> = orgs_snap
            .iter()
            .filter(|o| filter_q.is_empty() || o.sid.to_ascii_lowercase().contains(&filter_q))
            .cloned()
            .collect();

        ui.label(
            RichText::new(format!(
                "Showing {} of {} listed organizations (Tier 2 priority — renders [ ] square brackets)",
                filtered.len(),
                orgs_snap.len()
            ))
            .size(12.0)
            .color(p.muted),
        );
        ui.add_space(4.0);

        let mut color_updates: Vec<(String, OverlayColor)> = Vec::new();
        let mut removals: Vec<String> = Vec::new();

        ScrollArea::vertical()
            .id_salt("orgs_tab_scroll")
            .show(ui, |ui| {
                for org in filtered {
                    let cached = shared.cache.get_org(&org.sid);
                    let detail = cached
                        .as_ref()
                        .map(|c| {
                            let members = c
                                .info
                                .member_count
                                .map(|m| format!(" • {} members", m))
                                .unwrap_or_default();
                            let arch = c
                                .info
                                .archetype
                                .as_deref()
                                .map(|a| format!(" • {}", a))
                                .unwrap_or_default();
                            format!("{}{}{}", c.info.long_name, members, arch)
                        })
                        .unwrap_or_default();

                    egui::Frame::none()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0, p.border))
                        .rounding(Rounding::same(6.0))
                        .inner_margin(Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let c32 = overlay_color32(org.color);
                                ui.label(
                                    RichText::new(format!("#{}", org.priority))
                                        .monospace()
                                        .size(12.0)
                                        .color(p.muted),
                                );
                                ui.label(
                                    RichText::new(format!("[{}]", org.sid))
                                        .strong()
                                        .size(14.0)
                                        .color(c32),
                                );
                                if !detail.is_empty() {
                                    ui.label(RichText::new(detail).size(12.0).color(p.muted));
                                }

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .small_button(RichText::new("Remove").color(p.danger))
                                        .clicked()
                                    {
                                        removals.push(org.sid.clone());
                                    }
                                    if ui.small_button("RSI ↗").clicked() {
                                        let url = format!(
                                            "https://robertsspaceindustries.com/en/orgs/{}",
                                            org.sid
                                        );
                                        let _ = open_https_url(&url);
                                    }
                                    let mut col = org.color;
                                    if color_combo(ui, ("org_col", &org.sid), &mut col, p.is_dark) {
                                        color_updates.push((org.sid.clone(), col));
                                    }
                                });
                            });
                        });
                    ui.add_space(4.0);
                }
            });

        if !color_updates.is_empty() || !removals.is_empty() {
            let mut social = shared.social.write();
            for (sid, col) in color_updates {
                social.upsert_org(&sid, col);
            }
            for sid in removals {
                social.remove_org(&sid);
            }
            drop(social);
            shared.save_social();
        }
    }

    // ========================================================================
    // Tab 3: Org-Attribute
    // ========================================================================
    fn render_org_attribute_tab(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        let active_colors = {
            let social = shared.social.read();
            ColorRanks::active_rule_colors(&social.org_rules)
        };
        let mut rank_moved: Option<(OverlayColor, isize)> = None;
        color_rank_strip(ui, p, &active_colors, |col, delta| {
            rank_moved = Some((col, delta));
        });
        if let Some((col, delta)) = rank_moved {
            {
                let mut social = shared.social.write();
                ColorRanks::move_rule_color(&mut social.org_rules, col, delta);
            }
            shared.save_social();
        }

        ui.add_space(6.0);

        // Top controls: Main Org Only + No Match Fallback Color
        egui::Frame::none()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut main_only = shared.social.read().org_rules_main_org_only;
                    if ui
                        .checkbox(&mut main_only, "Match main org only for attribute rules")
                        .changed()
                    {
                        shared.social.write().org_rules_main_org_only = main_only;
                        shared.save_social();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut no_match_col = shared.social.read().no_match_color();
                        if color_combo(ui, "no_match_fallback_combo", &mut no_match_col, p.is_dark)
                        {
                            shared.social.write().set_no_match_color(no_match_col);
                            shared.save_social();
                        }
                        ui.label(
                            RichText::new("No Match Fallback Color (Tier 4):")
                                .size(12.5)
                                .color(p.muted),
                        );
                    });
                });
            });

        ui.add_space(8.0);

        let rules_snap = shared.social.read().org_rules.clone();
        let find_rule = |kind: AttributeKind, val: Option<&str>| -> Option<(OverlayColor, i32)> {
            rules_snap
                .iter()
                .find(|r| {
                    r.kind == kind
                        && r.value
                            .as_deref()
                            .unwrap_or_default()
                            .eq_ignore_ascii_case(val.unwrap_or_default())
                })
                .map(|r| (r.color, r.priority))
        };

        let mut rule_changes: Vec<(AttributeTarget, Option<OverlayColor>)> = Vec::new();

        ScrollArea::vertical()
            .id_salt("org_attr_scroll")
            .show(ui, |ui| {
                // Section 1: Special Membership Rules
                ui.label(quantico_text("SPECIAL MEMBERSHIP RULES", 13.5, p.accent));
                ui.add_space(4.0);
                egui::Frame::none()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.border))
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::same(10.0))
                    .show(ui, |ui| {
                        let specials = [
                            (AttributeKind::Redacted, "Redacted Affiliate / Org"),
                            (AttributeKind::HiddenMainOrg, "Hidden Main Org"),
                            (AttributeKind::NoOrg, "No Organization (Unaffiliated)"),
                        ];
                        egui::Grid::new("special_rules_grid")
                            .num_columns(3)
                            .spacing([20.0, 8.0])
                            .show(ui, |ui| {
                                for (kind, label) in specials {
                                    let cur = find_rule(kind, None);
                                    let mut opt_col = cur.map(|(c, _)| c);
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(label).color(p.text));
                                        if let Some((_, prio)) = cur {
                                            ui.label(
                                                RichText::new(format!("#{}", prio))
                                                    .monospace()
                                                    .size(11.5)
                                                    .color(p.muted),
                                            );
                                        }
                                        if optional_color_combo(
                                            ui,
                                            ("spec_rule", kind.to_toml_str()),
                                            &mut opt_col,
                                            p.muted,
                                            p.is_dark,
                                        ) {
                                            rule_changes
                                                .push((AttributeTarget::new(kind, None), opt_col));
                                        }
                                    });
                                }
                                ui.end_row();
                            });
                    });

                ui.add_space(12.0);

                // Section 2: 6 Archetypes
                ui.label(quantico_text("ORGANIZATION ARCHETYPES (6)", 13.5, p.accent));
                ui.add_space(4.0);
                egui::Frame::none()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.border))
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::same(10.0))
                    .show(ui, |ui| {
                        egui::Grid::new("archetypes_rules_grid")
                            .num_columns(3)
                            .spacing([24.0, 8.0])
                            .show(ui, |ui| {
                                for (idx, &arch) in OrgVocabulary::ARCHETYPES.iter().enumerate() {
                                    let cur = find_rule(AttributeKind::Archetype, Some(arch));
                                    let mut opt_col = cur.map(|(c, _)| c);
                                    ui.horizontal(|ui| {
                                        ui.set_min_width(245.0);
                                        ui.label(RichText::new(arch).color(p.text));
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if optional_color_combo(
                                                    ui,
                                                    ("arch_rule", arch),
                                                    &mut opt_col,
                                                    p.muted,
                                                    p.is_dark,
                                                ) {
                                                    rule_changes.push((
                                                        AttributeTarget::new(
                                                            AttributeKind::Archetype,
                                                            Some(arch.to_string()),
                                                        ),
                                                        opt_col,
                                                    ));
                                                }
                                                if let Some((_, prio)) = cur {
                                                    ui.label(
                                                        RichText::new(format!("#{}", prio))
                                                            .monospace()
                                                            .size(11.5)
                                                            .color(p.muted),
                                                    );
                                                }
                                            },
                                        );
                                    });
                                    if idx % 3 == 2 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });

                ui.add_space(12.0);

                // Section 3: 15 Activities
                ui.label(quantico_text("ORGANIZATION ACTIVITIES (15)", 13.5, p.accent));
                ui.add_space(4.0);
                egui::Frame::none()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.border))
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::same(10.0))
                    .show(ui, |ui| {
                        egui::Grid::new("activities_rules_grid")
                            .num_columns(3)
                            .spacing([24.0, 8.0])
                            .show(ui, |ui| {
                                for (idx, &act) in OrgVocabulary::ACTIVITIES.iter().enumerate() {
                                    let cur = find_rule(AttributeKind::Activity, Some(act));
                                    let mut opt_col = cur.map(|(c, _)| c);
                                    ui.horizontal(|ui| {
                                        ui.set_min_width(245.0);
                                        ui.label(RichText::new(act).color(p.text));
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if optional_color_combo(
                                                    ui,
                                                    ("act_rule", act),
                                                    &mut opt_col,
                                                    p.muted,
                                                    p.is_dark,
                                                ) {
                                                    rule_changes.push((
                                                        AttributeTarget::new(
                                                            AttributeKind::Activity,
                                                            Some(act.to_string()),
                                                        ),
                                                        opt_col,
                                                    ));
                                                }
                                                if let Some((_, prio)) = cur {
                                                    ui.label(
                                                        RichText::new(format!("#{}", prio))
                                                            .monospace()
                                                            .size(11.5)
                                                            .color(p.muted),
                                                    );
                                                }
                                            },
                                        );
                                    });
                                    if idx % 3 == 2 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });
            });

        if !rule_changes.is_empty() {
            let mut social = shared.social.write();
            for (target, maybe_col) in rule_changes {
                match maybe_col {
                    Some(col) => social.upsert_attribute_rule(&target, col),
                    None => {
                        social.remove_attribute_rule(&target);
                    }
                }
            }
            drop(social);
            shared.save_social();
        }
    }

    // ========================================================================
    // Tab 4: POIs / Waypoint Beacons
    // ========================================================================
    fn render_waypoints_tab(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        let save_hk = shared.settings.read().hotkey_save_waypoint.clone();
        let live_fix = shared.last_position_fix.read().clone();

        // Top Bar: Current Position + Quick Save + Paste /showlocation + Import/Export
        egui::Frame::none()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::same(10.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let (pos_txt, pos_col) = if let Some((disp, _key, c)) = &live_fix.body {
                        (
                            format!(
                                "● Zone Lock [{}]: {:.1} km, {:.1} km, {:.1} km (zero rotation drift)",
                                disp,
                                c.x / 1000.0,
                                c.y / 1000.0,
                                c.z / 1000.0
                            ),
                            p.success,
                        )
                    } else if let Some(c) = live_fix.root {
                        let z_part = live_fix
                            .display_zone
                            .as_deref()
                            .map(|z| format!(" [{}]", z))
                            .unwrap_or_else(|| " [Deep Space]".to_string());
                        (
                            format!(
                                "● Position Lock{}: {:.1} km, {:.1} km, {:.1} km",
                                z_part,
                                c.x / 1000.0,
                                c.y / 1000.0,
                                c.z / 1000.0
                            ),
                            p.success,
                        )
                    } else {
                        (
                            "○ No Position Lock (enable r_DisplayInfo 2/3 or run /showlocation)"
                                .to_string(),
                            p.muted,
                        )
                    };
                    ui.label(RichText::new(pos_txt).size(12.5).color(pos_col));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("In-game hotkey: {}", save_hk))
                                .monospace()
                                .size(12.0)
                                .color(p.accent),
                        );
                    });
                });

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("POI Name:").color(p.muted));
                    let name_resp = ui.add(
                        TextEdit::singleline(&mut self.new_poi_name)
                            .desired_width(210.0)
                            .hint_text("Optional name (defaults to POI #N)…"),
                    );
                    let enter = name_resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));

                    color_combo(ui, "new_poi_color", &mut self.new_poi_color, p.is_dark);

                    if ui
                        .button("＋ Save Current Position")
                        .on_hover_text("Save your current Star Citizen coordinates as a POI beacon")
                        .clicked()
                        || enter
                    {
                        let trimmed = self.new_poi_name.trim().to_string();
                        let opt_name = if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        };
                        let _ = shared.coordinator_tx.send(CoordinatorCommand::SaveWaypoint {
                            name: opt_name,
                            color: Some(self.new_poi_color),
                        });
                        self.new_poi_name.clear();
                    }

                    if ui
                        .button("👁 Preview HUD Marker (15s)")
                        .on_hover_text("Preview the directional waypoint compass & ring marker on your desktop")
                        .clicked()
                    {
                        let _ = shared
                            .coordinator_tx
                            .send(CoordinatorCommand::ShowTestOverlay(TestOverlayPart::Waypoint));
                    }
                });

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Paste Coords:").color(p.muted));
                    let paste_resp = ui.add(
                        TextEdit::singleline(&mut self.paste_poi_coords)
                            .desired_width(300.0)
                            .hint_text("Coordinates: x:... y:... z:... (from /showlocation)"),
                    );
                    let enter = paste_resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if ui
                        .button("＋ Add from Coords")
                        .on_hover_text("Create a POI directly from pasted /showlocation coordinates")
                        .clicked()
                        || enter
                    {
                        if let Some(coord) = SpaceCoord::parse_showlocation(&self.paste_poi_coords)
                            .or_else(|| {
                                SpaceCoord::parse_display_info(&self.paste_poi_coords)
                                    .map(|(c, _)| c)
                            })
                        {
                            let trimmed = self.new_poi_name.trim().to_string();
                            let opt_name = if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed)
                            };
                            let wp = shared.waypoints.write().add_point(
                                coord,
                                opt_name,
                                None,
                                None,
                                Some(self.new_poi_color),
                                chrono::Utc::now(),
                            );
                            shared.save_waypoints();
                            shared.set_status(format!("Added POI \"{}\".", wp.name), false);
                            self.paste_poi_coords.clear();
                            self.new_poi_name.clear();
                        } else {
                            shared.set_status(
                                "Could not parse coordinates. Paste a /showlocation line (x:... y:... z:...).",
                                true,
                            );
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("📤 Export JSON…").clicked() {
                            self.export_waypoints_dialog(shared);
                        }
                        if ui.small_button("📥 Import JSON…").clicked() {
                            self.import_waypoints_dialog(shared);
                        }
                        ui.add(
                            TextEdit::singleline(&mut self.poi_filter)
                                .desired_width(145.0)
                                .hint_text("Filter POIs…"),
                        );
                    });
                });
            });

        ui.add_space(8.0);

        let store_snap = shared.waypoints.read().clone();
        let active_id = store_snap.active_waypoint_id.clone();

        // Active Beacon highlight bar
        if let Some(active_wp) = store_snap.active() {
            let c32 = overlay_color32(active_wp.color);
            let dist_txt = live_fix
                .distance_to_waypoint_m(active_wp)
                .map(|(d, locked)| {
                    if locked {
                        format!(
                            " • {} away (Zone Locked: {})",
                            format_distance_m(d),
                            active_wp.local_zone.as_deref().unwrap_or("")
                        )
                    } else {
                        format!(" • {} away", format_distance_m(d))
                    }
                })
                .unwrap_or_else(|| " • Waiting for live position".to_string());
            egui::Frame::none()
                .fill(c32.gamma_multiply(0.14))
                .stroke(Stroke::new(1.2, c32))
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::symmetric(10.0, 7.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(quantico_text("ACTIVE BEACON:", 13.0, c32));
                        ui.label(RichText::new(&active_wp.name).strong().size(14.0).color(p.text));
                        ui.label(RichText::new(dist_txt).size(12.5).color(p.muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✕ Unload Beacon").clicked() {
                                shared.waypoints.write().set_active(None);
                                shared.save_waypoints();
                            }
                        });
                    });
                });
            ui.add_space(6.0);
        }

        let filter_q = self.poi_filter.trim().to_ascii_lowercase();
        let filtered: Vec<_> = store_snap
            .waypoints
            .iter()
            .rev()
            .filter(|w| {
                filter_q.is_empty()
                    || w.name.to_ascii_lowercase().contains(&filter_q)
                    || w.zone
                        .as_deref()
                        .is_some_and(|z| z.to_ascii_lowercase().contains(&filter_q))
                    || w.notes
                        .as_deref()
                        .is_some_and(|n| n.to_ascii_lowercase().contains(&filter_q))
            })
            .cloned()
            .collect();

        ui.label(
            RichText::new(format!(
                "Showing {} of {} saved POIs (stored in Config/waypoints.json — press {} in-game to drop a point)",
                filtered.len(),
                store_snap.waypoints.len(),
                save_hk
            ))
            .size(12.0)
            .color(p.muted),
        );
        ui.add_space(4.0);

        let mut load_id: Option<Option<String>> = None;
        let mut color_updates: Vec<(String, OverlayColor)> = Vec::new();
        let mut rename_commit: Option<(String, String, String)> = None;
        let mut removals: Vec<String> = Vec::new();

        ScrollArea::vertical()
            .id_salt("waypoints_tab_scroll")
            .show(ui, |ui| {
                if filtered.is_empty() {
                    egui::Frame::none()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0, p.border))
                        .rounding(Rounding::same(6.0))
                        .inner_margin(Margin::same(16.0))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(
                                    "No POIs saved yet. Press Alt+Shift+P while flying in Star Citizen (with r_DisplayInfo 2/3 or /showlocation) to drop a custom beacon, then load it here for live directional HUD guidance.",
                                )
                                .color(p.muted),
                            );
                        });
                }

                for wp in filtered {
                    let is_active = active_id.as_deref() == Some(wp.id.as_str());
                    let is_editing = self.editing_poi_id.as_deref() == Some(wp.id.as_str());
                    let c32 = overlay_color32(wp.color);

                    egui::Frame::none()
                        .fill(p.surface)
                        .stroke(Stroke::new(if is_active { 1.4 } else { 1.0 }, if is_active { c32 } else { p.border }))
                        .rounding(Rounding::same(6.0))
                        .inner_margin(Margin::symmetric(10.0, 7.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if is_editing {
                                    let r = ui.add(
                                        TextEdit::singleline(&mut self.editing_poi_name)
                                            .desired_width(180.0)
                                            .hint_text("POI Name"),
                                    );
                                    ui.add(
                                        TextEdit::singleline(&mut self.editing_poi_notes)
                                            .desired_width(210.0)
                                            .hint_text("Optional notes…"),
                                    );
                                    let enter =
                                        r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                                    if ui.small_button("Save").clicked() || enter {
                                        rename_commit = Some((
                                            wp.id.clone(),
                                            self.editing_poi_name.clone(),
                                            self.editing_poi_notes.clone(),
                                        ));
                                        self.editing_poi_id = None;
                                    }
                                    if ui.small_button("Cancel").clicked() {
                                        self.editing_poi_id = None;
                                    }
                                } else {
                                    ui.label(
                                        RichText::new(format!("◆ {}", wp.name))
                                            .strong()
                                            .size(14.0)
                                            .color(c32),
                                    );
                                    if let Some((d_m, locked)) = live_fix.distance_to_waypoint_m(&wp)
                                    {
                                        let d = format_distance_m(d_m);
                                        ui.label(
                                            RichText::new(d)
                                                .monospace()
                                                .strong()
                                                .size(12.5)
                                                .color(if locked { p.success } else { p.text }),
                                        );
                                    }
                                    if let Some(lz) = &wp.local_zone {
                                        ui.label(
                                            RichText::new(format!("[ZONE: {}]", lz))
                                                .size(11.5)
                                                .color(p.accent),
                                        );
                                    } else if let Some(zone) = &wp.zone {
                                        ui.label(
                                            RichText::new(format!("[{}]", zone))
                                                .size(11.5)
                                                .color(p.muted),
                                        );
                                    }
                                    let display_c = wp.local_coord.unwrap_or(wp.coord);
                                    ui.label(
                                        RichText::new(format!(
                                            "({:.1}, {:.1}, {:.1} km)",
                                            display_c.x / 1000.0,
                                            display_c.y / 1000.0,
                                            display_c.z / 1000.0
                                        ))
                                        .monospace()
                                        .size(11.0)
                                        .color(p.muted),
                                    );
                                }

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .small_button(RichText::new("Delete").color(p.danger))
                                        .clicked()
                                    {
                                        removals.push(wp.id.clone());
                                    }
                                    if !is_editing && ui.small_button("✎ Rename").clicked() {
                                        self.editing_poi_id = Some(wp.id.clone());
                                        self.editing_poi_name = wp.name.clone();
                                        self.editing_poi_notes =
                                            wp.notes.clone().unwrap_or_default();
                                    }
                                    let mut col = wp.color;
                                    if color_combo(ui, ("wp_col", &wp.id), &mut col, p.is_dark) {
                                        color_updates.push((wp.id.clone(), col));
                                    }
                                    if is_active {
                                        if ui
                                            .small_button(RichText::new("✓ Active").color(p.success))
                                            .on_hover_text("Click to unload this beacon from the HUD")
                                            .clicked()
                                        {
                                            load_id = Some(None);
                                        }
                                    } else if ui
                                        .small_button(RichText::new("🎯 Load").color(p.accent))
                                        .on_hover_text("Load this POI onto your HUD with live directional marker")
                                        .clicked()
                                    {
                                        load_id = Some(Some(wp.id.clone()));
                                    }
                                });
                            });
                            if !is_editing {
                                if let Some(notes) = &wp.notes {
                                    ui.label(RichText::new(notes).size(11.5).color(p.muted));
                                }
                            }
                        });
                    ui.add_space(4.0);
                }
            });

        let mut dirty = false;
        if let Some(target_opt) = load_id {
            shared.waypoints.write().set_active(target_opt.as_deref());
            dirty = true;
        }
        if let Some((id, new_name, new_notes)) = rename_commit {
            let mut store = shared.waypoints.write();
            if store.rename(&id, &new_name) {
                dirty = true;
            }
            if store.set_notes(&id, Some(new_notes)) {
                dirty = true;
            }
        }
        if !color_updates.is_empty() || !removals.is_empty() {
            let mut store = shared.waypoints.write();
            for (id, c) in color_updates {
                if store.set_color(&id, c) {
                    dirty = true;
                }
            }
            for id in removals {
                if store.remove(&id) {
                    dirty = true;
                }
            }
        }
        if dirty {
            shared.save_waypoints();
        }
    }

    fn import_waypoints_dialog(&self, shared: &Arc<AppSharedState>) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import POIs / Waypoints JSON")
            .add_filter("Waypoints JSON (*.json)", &["json"])
            .pick_file()
        else {
            return;
        };
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                shared.set_status(format!("Failed reading {}: {}", path.display(), e), true);
                return;
            }
        };
        match shared.waypoints.write().import_json(&text) {
            Ok(added) => {
                shared.save_waypoints();
                shared.set_status(format!("Imported {} new POI(s).", added), false);
            }
            Err(err) => shared.set_status(err, true),
        }
    }

    fn export_waypoints_dialog(&self, shared: &Arc<AppSharedState>) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export POIs / Waypoints JSON")
            .set_file_name("waypoints.json")
            .add_filter("Waypoints JSON (*.json)", &["json"])
            .save_file()
        else {
            return;
        };
        let json = match shared.waypoints.read().export_json() {
            Ok(j) => j,
            Err(err) => {
                shared.set_status(err, true);
                return;
            }
        };
        match iris_core::config::AtomicFile::write_text(&path, &json, false) {
            Ok(()) => shared.set_status(format!("Exported POIs to {}.", path.display()), false),
            Err(e) => shared.set_status(format!("Export failed: {}", e), true),
        }
    }

    // ========================================================================
    // Tab 5: Settings
    // ========================================================================
    fn render_settings_tab(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        let mut s = shared.settings.read().clone();
        let mut changed = false;
        let mut reset_capture = false;

        ScrollArea::vertical()
            .id_salt("settings_tab_scroll")
            .show(ui, |ui| {
                // 1. Hotkeys & Timed Peek
                Self::section_card(ui, p, "HOTKEYS & TIMED OVERLAY PEEK", |ui| {
                    egui::Grid::new("hotkeys_grid")
                        .num_columns(3)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            self.hotkey_row(
                                ui,
                                p,
                                "Toggle Overlay On/Off:",
                                &s.hotkey_toggle_overlay,
                                RecordingHotkey::Toggle,
                            );
                            if ui.small_button("Reset (Shift+F12)").clicked() {
                                s.hotkey_toggle_overlay = "Shift+F12".to_string();
                                changed = true;
                            }
                            ui.end_row();

                            self.hotkey_row(
                                ui,
                                p,
                                "Timed Peek (when disabled):",
                                &s.hotkey_timed_overlay,
                                RecordingHotkey::Timed,
                            );
                            if ui.small_button("Reset (Tab)").clicked() {
                                s.hotkey_timed_overlay = "Tab".to_string();
                                changed = true;
                            }
                            ui.end_row();

                            self.hotkey_row(
                                ui,
                                p,
                                "Interactive Citizen Dossier:",
                                &s.hotkey_interactive_overlay,
                                RecordingHotkey::Interactive,
                            );
                            if ui.small_button("Reset (Shift+0)").clicked() {
                                s.hotkey_interactive_overlay = "Shift+0".to_string();
                                changed = true;
                            }
                            ui.end_row();

                            self.hotkey_row(
                                ui,
                                p,
                                "Save POI / Waypoint Beacon:",
                                &s.hotkey_save_waypoint,
                                RecordingHotkey::SaveWaypoint,
                            );
                            if ui.small_button("Reset (Alt+Shift+P)").clicked() {
                                s.hotkey_save_waypoint = "Alt+Shift+P".to_string();
                                changed = true;
                            }
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Timed Peek Duration:");
                        if ui
                            .add(
                                Slider::new(
                                    &mut s.timed_overlay_seconds,
                                    AppSettings::TIMED_SECONDS_MIN..=AppSettings::TIMED_SECONDS_MAX,
                                )
                                .suffix(" sec"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .small_button("⏱ Trigger Peek")
                            .on_hover_text("Activate timed overlay peek while overlay is disabled")
                            .clicked()
                        {
                            let _ = shared
                                .coordinator_tx
                                .send(CoordinatorCommand::TimedOverlay);
                        }
                    });
                });

                ui.add_space(10.0);

                // 2. Social Config Import / Export & Hosted URL Sync
                Self::section_card(ui, p, "SHAREABLE SOCIAL CONFIG & CLOUD SYNC", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("📥 Import social-config…").clicked() {
                            self.import_social_config_dialog(s.import_mode, shared);
                        }
                        if ui.button("📤 Export social-config…").clicked() {
                            self.export_social_config_dialog(shared);
                        }

                        ui.add_space(8.0);
                        ui.label("Import / Sync Mode:");
                        egui::ComboBox::from_id_salt("import_mode_combo")
                            .selected_text(match s.import_mode {
                                ImportMode::Merge => "Merge (Keep Local Colors)",
                                ImportMode::MergeImportedWins => "Merge (Imported Colors Win)",
                                ImportMode::BackupExisting => "Replace (Backup Existing)",
                            })
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut s.import_mode,
                                        ImportMode::Merge,
                                        "Merge (Keep Local Colors)",
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut s.import_mode,
                                        ImportMode::MergeImportedWins,
                                        "Merge (Imported Colors Win)",
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut s.import_mode,
                                        ImportMode::BackupExisting,
                                        "Replace (Backup Existing)",
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                            });
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("Hosted Config URL:");
                        ui.add(
                            TextEdit::singleline(&mut self.hosted_url_draft)
                                .desired_width(340.0)
                                .hint_text("https://github.com/... or https://pastebin.com/..."),
                        );
                        if ui.button("Save URL").clicked() {
                            let trimmed = self.hosted_url_draft.trim().to_string();
                            if trimmed.is_empty() {
                                s.hosted_config_url.clear();
                                s.hosted_config_auto_check = false;
                                changed = true;
                            } else {
                                match HostedConfigUrl::check(&trimmed) {
                                    Ok(_) => {
                                        s.hosted_config_url = trimmed;
                                        changed = true;
                                        shared.set_status("Saved Hosted Config URL.", false);
                                    }
                                    Err(err) => {
                                        shared.set_status(err, true);
                                    }
                                }
                            }
                        }

                        let syncing = shared.sync_in_progress.load(Ordering::Relaxed);
                        if ui
                            .add_enabled(!syncing, egui::Button::new(if syncing { "Syncing…" } else { "↻ Sync Now" }))
                            .clicked()
                        {
                            let trimmed = self.hosted_url_draft.trim().to_string();
                            if !trimmed.is_empty() && trimmed != s.hosted_config_url {
                                if HostedConfigUrl::check(&trimmed).is_ok() {
                                    shared.settings.write().hosted_config_url = trimmed.clone();
                                    s.hosted_config_url = trimmed;
                                    shared.save_settings(false);
                                }
                            }
                            shared.spawn_hosted_config_sync(true);
                        }
                    });

                    if ui
                        .checkbox(
                            &mut s.hosted_config_auto_check,
                            "Automatically check Hosted Config URL once every 24 hours",
                        )
                        .changed()
                    {
                        if s.hosted_config_auto_check && s.hosted_config_url.is_empty() {
                            s.hosted_config_auto_check = false;
                            shared.set_status("Save a valid https:// Hosted Config URL first.", true);
                        } else {
                            changed = true;
                        }
                    }
                });

                ui.add_space(10.0);

                // 3. Screen Capture & Platform
                Self::section_card(ui, p, "SCREEN CAPTURE & TRACKING PERFORMANCE", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Capture Backend:");
                        egui::ComboBox::from_id_salt("capture_method_combo")
                            .selected_text(match s.capture_method {
                                CaptureMethod::DesktopDuplication => {
                                    if cfg!(target_os = "linux") {
                                        "PipeWire Monitor / X11 Root"
                                    } else {
                                        "DXGI Desktop Duplication (Recommended)"
                                    }
                                }
                                CaptureMethod::WindowCapture => {
                                    if cfg!(target_os = "linux") {
                                        "XDG Portal Window Capture"
                                    } else {
                                        "Windows Graphics Capture (WGC)"
                                    }
                                }
                            })
                            .show_ui(ui, |ui| {
                                let dd_lbl = if cfg!(target_os = "linux") {
                                    "PipeWire Monitor / X11 Root"
                                } else {
                                    "DXGI Desktop Duplication (Recommended)"
                                };
                                let wgc_lbl = if cfg!(target_os = "linux") {
                                    "XDG Portal Window Capture"
                                } else {
                                    "Windows Graphics Capture (WGC)"
                                };
                                if ui
                                    .selectable_value(
                                        &mut s.capture_method,
                                        CaptureMethod::DesktopDuplication,
                                        dd_lbl,
                                    )
                                    .changed()
                                {
                                    changed = true;
                                    reset_capture = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut s.capture_method,
                                        CaptureMethod::WindowCapture,
                                        wgc_lbl,
                                    )
                                    .changed()
                                {
                                    changed = true;
                                    reset_capture = true;
                                }
                            });

                        if ui
                            .checkbox(
                                &mut s.overlay_visible_in_capture,
                                "Show overlay in OBS / Window Capture",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        if cfg!(windows)
                            && ui
                                .checkbox(
                                    &mut s.hide_capture_border,
                                    "Hide yellow WGC capture border",
                                )
                                .changed()
                        {
                            changed = true;
                            reset_capture = true;
                        }
                    });

                    if cfg!(target_os = "linux") {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let has_tok = !s.linux_portal_restore_token.is_empty();
                            let tok_status = if has_tok {
                                "Wayland XDG ScreenCast Token: Saved (no repeat prompt)"
                            } else {
                                "Wayland XDG ScreenCast Token: Not yet granted"
                            };
                            ui.label(RichText::new(tok_status).size(12.0).color(p.muted));
                            if has_tok && ui.small_button("Reset Portal Token").clicked() {
                                s.linux_portal_restore_token.clear();
                                changed = true;
                                reset_capture = true;
                            }
                        });
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Tracking FPS Cap:");
                        let fps_text = if s.nameplate_tracking_fps > AppSettings::TRACKING_FPS_HIGHEST
                        {
                            "Max".to_string()
                        } else {
                            format!("{} FPS", s.nameplate_tracking_fps)
                        };
                        if ui
                            .add(
                                Slider::new(
                                    &mut s.nameplate_tracking_fps,
                                    AppSettings::TRACKING_FPS_MIN..=AppSettings::TRACKING_FPS_MAX,
                                )
                                .step_by(AppSettings::TRACKING_FPS_STEP as f64)
                                .text(fps_text),
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(16.0);
                        ui.label("Tracking Loss Display:");
                        if ui
                            .add(
                                Slider::new(
                                    &mut s.tracking_loss_display_seconds,
                                    AppSettings::LOSS_SECONDS_MIN..=AppSettings::LOSS_SECONDS_MAX,
                                )
                                .step_by(AppSettings::LOSS_SECONDS_STEP as f64)
                                .suffix(" sec"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    });
                });

                ui.add_space(10.0);

                // 3b. Overlay Debug HUD & Live Pipeline Diagnostics
                Self::section_card(ui, p, "OVERLAY DEBUG HUD & LIVE DIAGNOSTICS", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .checkbox(
                                &mut s.debug_overlay_enabled,
                                "Show Debug / Heartbeat HUD on Overlay",
                            )
                            .on_hover_text(
                                "Renders a compact top-left status card with a live pulsing heartbeat, capture FPS, scan time, and plate/mining counters even when 0 targets are on screen",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(8.0);
                        if ui
                            .add_enabled(
                                s.debug_overlay_enabled,
                                egui::Checkbox::new(
                                    &mut s.debug_show_candidates,
                                    "Outline raw candidate boxes & scan boundary",
                                ),
                            )
                            .on_hover_text(
                                "Draws cyan boxes around raw text candidates, yellow around paired name+distance candidates, and magenta around tentative tracks",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(8.0);
                        if ui
                            .button("👁 Preview Debug HUD (15s)")
                            .on_hover_text(
                                "Preview the live ticking Debug HUD on your desktop right now (even if Star Citizen is closed)",
                            )
                            .clicked()
                        {
                            let _ = shared
                                .coordinator_tx
                                .send(CoordinatorCommand::ShowTestOverlay(TestOverlayPart::DebugHud));
                        }
                    });

                    ui.add_space(6.0);
                    let diag = shared.live_diagnostics.read().clone();
                    egui::Frame::none()
                        .fill(p.bg)
                        .stroke(Stroke::new(1.0, p.border))
                        .rounding(Rounding::same(5.0))
                        .inner_margin(Margin::symmetric(10.0, 7.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let status_txt = if diag.active_status.is_empty() {
                                "Initializing…"
                            } else {
                                diag.active_status.as_str()
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("Pipeline Status:").size(12.0).color(p.muted));
                                ui.label(
                                    RichText::new(status_txt)
                                        .strong()
                                        .size(12.0)
                                        .color(if diag.blank_frame || diag.capture_errors > 0 {
                                            OverlayColor::Orange.rgb().into_color32()
                                        } else {
                                            p.accent
                                        }),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "•  Overlay: {}  •  Loop: {:.0} Hz (tick #{})  •  Capture: {:.0} FPS ({} frames, {} idle, {} err)  •  Scan: {:.1} ms",
                                        if diag.overlay_visible { "Shown" } else { "Hidden" },
                                        diag.loop_hz,
                                        diag.coordinator_ticks,
                                        diag.capture_fps,
                                        diag.frames_captured,
                                        diag.capture_timeouts,
                                        diag.capture_errors,
                                        diag.last_scan_ms,
                                    ))
                                    .monospace()
                                    .size(11.5)
                                    .color(p.text),
                                );
                            });
                            ui.horizontal_wrapped(|ui| {
                                let last_ocr = diag.last_ocr_sample.as_deref().unwrap_or("none");
                                ui.label(
                                    RichText::new(format!(
                                        "Detector: {} boxes, {} pairs, {} OCR  •  Tracks: {} confirmed / {} tentative  •  RSI Queue: {}  •  Mining: {}  •  Last OCR: {}",
                                        diag.candidate_boxes,
                                        diag.candidate_pairs,
                                        diag.ocr_reads,
                                        diag.confirmed_tracks,
                                        diag.tentative_tracks,
                                        diag.in_flight_lookups,
                                        if diag.mining_status.is_empty() {
                                            "—"
                                        } else {
                                            diag.mining_status.as_str()
                                        },
                                        last_ocr,
                                    ))
                                    .monospace()
                                    .size(11.5)
                                    .color(p.muted),
                                );
                            });
                            if let Some(err) = &diag.last_capture_error {
                                ui.label(
                                    RichText::new(format!("Last Capture Error: {}", err))
                                        .monospace()
                                        .size(11.5)
                                        .color(p.danger),
                                );
                            }
                        });
                });

                ui.add_space(10.0);

                // 4. Tracking Boundary
                Self::section_card(ui, p, "TRACKING BOUNDARY (% INSET FROM EDGES)", |ui| {
                    egui::Grid::new("boundary_sliders_grid")
                        .num_columns(4)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Top %:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.boundary_top,
                                        AppSettings::BOUNDARY_MIN..=AppSettings::BOUNDARY_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.label("Bottom %:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.boundary_bottom,
                                        AppSettings::BOUNDARY_MIN..=AppSettings::BOUNDARY_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.end_row();

                            ui.label("Left %:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.boundary_left,
                                        AppSettings::BOUNDARY_MIN..=AppSettings::BOUNDARY_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.label("Right %:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.boundary_right,
                                        AppSettings::BOUNDARY_MIN..=AppSettings::BOUNDARY_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    if ui.button("👁 Show Boundary Preview (15s)").clicked() {
                        let _ = shared
                            .coordinator_tx
                            .send(CoordinatorCommand::ShowTestOverlay(TestOverlayPart::Boundary));
                    }
                });

                ui.add_space(10.0);

                // 5. Side Panel & Dossier Layout
                Self::section_card(ui, p, "RIGHT-SIDE PANEL & DOSSIER LAYOUT", |ui| {
                    egui::Grid::new("panel_layout_grid")
                        .num_columns(4)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Panel Width:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.panel_width_percent,
                                        AppSettings::PANEL_WIDTH_MIN..=AppSettings::PANEL_WIDTH_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }

                            ui.label("Text Size:");
                            if ui
                                .add(Slider::new(
                                    &mut s.panel_text_size,
                                    AppSettings::PANEL_TEXT_MIN..=AppSettings::PANEL_TEXT_MAX,
                                ))
                                .changed()
                            {
                                changed = true;
                            }
                            ui.end_row();

                            ui.label("Right Gap:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.panel_right_gap_percent,
                                        AppSettings::PANEL_GAP_MIN..=AppSettings::PANEL_GAP_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }

                            ui.label("Top Gap:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.panel_top_gap_percent,
                                        AppSettings::PANEL_GAP_MIN..=AppSettings::PANEL_GAP_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.end_row();

                            ui.label("Dossier Width:");
                            if ui
                                .add(
                                    Slider::new(
                                        &mut s.dossier_width_percent,
                                        AppSettings::DOSSIER_WIDTH_MIN
                                            ..=AppSettings::DOSSIER_WIDTH_MAX,
                                    )
                                    .suffix("%"),
                                )
                                .changed()
                            {
                                changed = true;
                            }

                            ui.label("Org Display:");
                            egui::ComboBox::from_id_salt("panel_org_display_combo")
                                .selected_text(match s.panel_org_display {
                                    OrgDisplay::FullName => "Full Organization Name",
                                    OrgDisplay::Sid => "SID Tag Only",
                                })
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_value(
                                            &mut s.panel_org_display,
                                            OrgDisplay::FullName,
                                            "Full Organization Name",
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                    if ui
                                        .selectable_value(
                                            &mut s.panel_org_display,
                                            OrgDisplay::Sid,
                                            "SID Tag Only",
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                });
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut s.panel_main_org_only, "Show Main Org only on side panel")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.add_space(12.0);
                        if ui.button("👁 Show Panel & Dossier Preview (15s)").clicked() {
                            let _ = shared
                                .coordinator_tx
                                .send(CoordinatorCommand::ShowTestOverlay(TestOverlayPart::Panel));
                        }
                    });
                });

                ui.add_space(10.0);

                // 6. Mining HUD Signal Scanner
                Self::section_card(ui, p, "MINING HUD SIGNAL OVERLAY", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let mut mining_enabled = !s.mining_disabled;
                        if ui
                            .checkbox(
                                &mut mining_enabled,
                                "Enable Mining Signal Decoder (Quantainium, Bexalite, cluster counts)",
                            )
                            .changed()
                        {
                            s.mining_disabled = !mining_enabled;
                            changed = true;
                        }

                        ui.add_space(12.0);
                        ui.label("Label Zoom:");
                        if ui
                            .add(
                                Slider::new(
                                    &mut s.mining_label_zoom_percent,
                                    AppSettings::MINING_ZOOM_MIN..=AppSettings::MINING_ZOOM_MAX,
                                )
                                .step_by(AppSettings::MINING_ZOOM_STEP as f64)
                                .suffix("%"),
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        if ui.button("Edit mining.txt").clicked() {
                            let _ = open_folder_or_file(&shared.paths.mining_file());
                        }
                    });
                });

                ui.add_space(10.0);

                // 6b. Mining Breadcrumb Cloud (/showlocation)
                Self::section_card(ui, p, "MINING BREADCRUMB CLOUD (/showlocation)", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .checkbox(
                                &mut s.breadcrumbs_enabled,
                                "Enable /showlocation Breadcrumb Cloud",
                            )
                            .on_hover_text(
                                "Watches your clipboard for Star Citizen /showlocation coordinates, tracks where you have mined, and warns when entering a visited area",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(10.0);
                        if ui
                            .add_enabled(
                                s.breadcrumbs_enabled,
                                egui::Checkbox::new(
                                    &mut s.breadcrumbs_persist,
                                    "Persist trail across restarts",
                                ),
                            )
                            .on_hover_text(
                                "Saves breadcrumbs to Data/breadcrumbs.json so your trail survives restarts (use Clear Trail when hopping servers)",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(10.0);
                        if ui
                            .add_enabled(
                                s.breadcrumbs_enabled,
                                egui::Checkbox::new(
                                    &mut s.breadcrumbs_screen_cloud,
                                    "Show screen color cloud when entering visited area",
                                ),
                            )
                            .on_hover_text(
                                "Displays a soft amber/red color cloud glow and alert banner at the top of the screen when you enter a previously visited zone",
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    });

                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                s.breadcrumbs_enabled,
                                egui::Checkbox::new(
                                    &mut s.breadcrumbs_auto_displayinfo,
                                    "Auto-track live position from r_DisplayInfo 2/3",
                                ),
                            )
                            .on_hover_text(
                                "Passively reads Zone: Root Pos: from Star Citizen's top-right r_DisplayInfo telemetry every few seconds (no chat spam, 100% EAC-safe)",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(12.0);
                        ui.label("Auto-Drop Interval:");
                        if ui
                            .add_enabled(
                                s.breadcrumbs_enabled && s.breadcrumbs_auto_displayinfo,
                                Slider::new(
                                    &mut s.breadcrumbs_auto_interval_secs,
                                    AppSettings::BREADCRUMB_INTERVAL_MIN_SECS
                                        ..=AppSettings::BREADCRUMB_INTERVAL_MAX_SECS,
                                )
                                .suffix(" sec"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    });

                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Visited Cloud Radius:");
                        if ui
                            .add_enabled(
                                s.breadcrumbs_enabled,
                                Slider::new(
                                    &mut s.breadcrumbs_visited_radius_km,
                                    AppSettings::BREADCRUMB_RADIUS_MIN_KM
                                        ..=AppSettings::BREADCRUMB_RADIUS_MAX_KM,
                                )
                                .suffix(" km"),
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(12.0);
                        if ui
                            .button("👁 Preview Cloud (15s)")
                            .on_hover_text(
                                "Preview the breadcrumb color cloud and 2D radar widget on your desktop right now",
                            )
                            .clicked()
                        {
                            let _ = shared.coordinator_tx.send(
                                CoordinatorCommand::ShowTestOverlay(TestOverlayPart::Breadcrumbs),
                            );
                        }

                        let (bc_count, bc_status) = shared.breadcrumb_summary.read().clone();
                        if ui
                            .add_enabled(
                                bc_count > 0,
                                egui::Button::new(RichText::new("🗑 Clear Trail").color(p.danger)),
                            )
                            .on_hover_text("Clear all recorded /showlocation breadcrumbs")
                            .clicked()
                        {
                            let _ = shared
                                .coordinator_tx
                                .send(CoordinatorCommand::ClearBreadcrumbs);
                        }

                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!("{} pts • {}", bc_count, bc_status))
                                .size(12.0)
                                .color(p.muted),
                        );
                    });

                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Tip: Open Star Citizen console (~) and run 'r_DisplayInfo 2' (or 3) for hands-free auto-tracking, or type '/showlocation' in chat anytime to drop a manual pin.",
                        )
                        .size(11.5)
                        .color(p.muted),
                    );
                });

                ui.add_space(10.0);

                // 6c. Waypoint Beacons (Custom POIs)
                Self::section_card(ui, p, "WAYPOINT BEACONS (CUSTOM POIs)", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .checkbox(&mut s.waypoints_enabled, "Enable Waypoint HUD Beacons")
                            .on_hover_text(
                                "Displays a top-center directional compass card and distance countdown for your loaded POI",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(10.0);
                        if ui
                            .add_enabled(
                                s.waypoints_enabled,
                                egui::Checkbox::new(
                                    &mut s.waypoints_show_ring_marker,
                                    "Show floating directional HUD ring marker",
                                ),
                            )
                            .on_hover_text(
                                "Places a glowing directional chevron + POI name + distance around your HUD pointing toward the active POI",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(10.0);
                        if ui
                            .button("👁 Preview Beacon HUD (15s)")
                            .on_hover_text(
                                "Preview the live directional waypoint HUD card and ring marker on your desktop",
                            )
                            .clicked()
                        {
                            let _ = shared.coordinator_tx.send(
                                CoordinatorCommand::ShowTestOverlay(TestOverlayPart::Waypoint),
                            );
                        }
                    });
                });

                ui.add_space(10.0);

                // 7. Cache, Theme & Diagnostics
                Self::section_card(ui, p, "CACHE, APPEARANCE & FILES", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("RSI Cache TTL:");
                        egui::ComboBox::from_id_salt("cache_ttl_combo")
                            .selected_text(format!("{} days", s.cache_ttl_days))
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(&mut s.cache_ttl_days, 30, "30 days")
                                    .changed()
                                {
                                    changed = true;
                                }
                                if ui
                                    .selectable_value(&mut s.cache_ttl_days, 7, "7 days")
                                    .changed()
                                {
                                    changed = true;
                                }
                            });

                        let refreshing = shared.refresh_in_progress.load(Ordering::Relaxed);
                        if ui
                            .add_enabled(
                                !refreshing,
                                egui::Button::new(if refreshing {
                                    "Refreshing RSI…"
                                } else {
                                    "↻ Refresh Cached Orgs & Players"
                                }),
                            )
                            .clicked()
                        {
                            shared.spawn_cache_refresh();
                        }

                        ui.add_space(12.0);
                        ui.label("Theme:");
                        egui::ComboBox::from_id_salt("dark_mode_combo")
                            .selected_text(match s.settings_dark_mode {
                                DarkMode::System => "System",
                                DarkMode::On => "Dark",
                                DarkMode::Off => "Light",
                            })
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut s.settings_dark_mode,
                                        DarkMode::System,
                                        "System",
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut s.settings_dark_mode,
                                        DarkMode::On,
                                        "Dark",
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut s.settings_dark_mode,
                                        DarkMode::Off,
                                        "Light",
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                            });

                        ui.add_space(12.0);
                        ui.label("UI Scale:");
                        egui::ComboBox::from_id_salt("window_scale_combo")
                            .selected_text(format!("{}%", s.settings_window_scale))
                            .show_ui(ui, |ui| {
                                for scale in AppSettings::WINDOW_SCALES {
                                    if ui
                                        .selectable_value(
                                            &mut s.settings_window_scale,
                                            scale,
                                            format!("{}%", scale),
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                }
                            });
                    });

                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .checkbox(&mut s.session_log_enabled, "Enable session log file")
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .checkbox(&mut s.check_for_updates, "Check for updates on startup")
                            .changed()
                        {
                            changed = true;
                        }
                    });

                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("📂 Open Config Folder").clicked() {
                            let _ = open_folder_or_file(&shared.paths.config_dir);
                        }
                        if ui.button("📂 Open Log Folder").clicked() {
                            let _ = open_folder_or_file(&shared.paths.logs_dir);
                        }
                        if ui.button("Edit exclusions.txt").clicked() {
                            let _ = open_folder_or_file(&shared.paths.exclusions_file());
                        }
                        if ui.button("↻ Reload Exclusions").clicked() {
                            let _ = shared
                                .coordinator_tx
                                .send(CoordinatorCommand::ReloadExclusions);
                            shared.set_status("Reloaded exclusions.txt.", false);
                        }
                    });
                });
            });

        if changed {
            *shared.settings.write() = s;
            shared.save_settings(reset_capture);
        }
    }

    fn section_card<F>(ui: &mut egui::Ui, p: &ThemePalette, title: &str, body: F)
    where
        F: FnOnce(&mut egui::Ui),
    {
        egui::Frame::none()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(quantico_text(title, 13.5, p.accent));
                ui.add_space(6.0);
                body(ui);
            });
    }

    fn hotkey_row(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        label: &str,
        current_value: &str,
        which: RecordingHotkey,
    ) {
        ui.label(label);
        let is_rec = self.recording_hotkey == Some(which);
        let btn_txt = if is_rec {
            RichText::new("Press keys… (Esc cancels)").color(p.accent)
        } else {
            RichText::new(current_value).monospace().color(p.text)
        };
        if ui.button(btn_txt).clicked() {
            self.recording_hotkey = if is_rec { None } else { Some(which) };
        }
    }

    fn capture_hotkey_input(
        &mut self,
        ctx: &egui::Context,
        which: RecordingHotkey,
        shared: &Arc<AppSharedState>,
    ) {
        let recorded = ctx.input(|i| {
            if i.key_pressed(Key::Escape) {
                return Some(None);
            }
            for ev in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } = ev
                {
                    if let Some(kname) = egui_key_to_hotkey_name(*key) {
                        let chord = HotkeyChord {
                            ctrl: modifiers.ctrl,
                            alt: modifiers.alt,
                            shift: modifiers.shift,
                            key: kname,
                        };
                        return Some(Some(chord.format()));
                    }
                }
            }
            None
        });

        if let Some(outcome) = recorded {
            self.recording_hotkey = None;
            if let Some(formatted) = outcome {
                if HotkeyText::parse(&formatted).is_ok() {
                    {
                        let mut s = shared.settings.write();
                        match which {
                            RecordingHotkey::Toggle => s.hotkey_toggle_overlay = formatted,
                            RecordingHotkey::Timed => s.hotkey_timed_overlay = formatted,
                            RecordingHotkey::Interactive => {
                                s.hotkey_interactive_overlay = formatted
                            }
                            RecordingHotkey::SaveWaypoint => s.hotkey_save_waypoint = formatted,
                        }
                    }
                    shared.save_settings(false);
                }
            }
        }
    }

    fn import_social_config_dialog(
        &self,
        mode: ImportMode,
        shared: &Arc<AppSharedState>,
    ) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import IRIS / SC Overlay Social Config")
            .add_filter("Social Config (*.txt, *.toml)", &["txt", "toml"])
            .pick_file()
        else {
            return;
        };

        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                shared.set_status(format!("Failed reading {}: {}", path.display(), e), true);
                return;
            }
        };

        let loaded = SocialConfigToml::read(&text);
        let Some(imported_cfg) = loaded.config else {
            shared.set_status(
                format!("Invalid social-config: {}", loaded.errors.join("; ")),
                true,
            );
            return;
        };

        if mode == ImportMode::BackupExisting {
            let _ = SocialConfigMerger::backup_existing(&shared.paths.social_config_file());
        }
        let merged = {
            let local = shared.social.read();
            SocialConfigMerger::apply(&local, &imported_cfg, mode)
        };
        let p_cnt = merged.players.len();
        let o_cnt = merged.orgs.len();
        *shared.social.write() = merged;
        shared.save_social();
        shared.set_status(
            format!(
                "Imported {} ({} players, {} orgs active).",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("social-config.txt"),
                p_cnt,
                o_cnt
            ),
            false,
        );
    }

    fn export_social_config_dialog(&self, shared: &Arc<AppSharedState>) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export Shareable Social Config")
            .set_file_name("social-config.txt")
            .add_filter("Social Config (*.txt)", &["txt", "toml"])
            .save_file()
        else {
            return;
        };

        let text = {
            let social = shared.social.read();
            SocialConfigToml::write(&social)
        };
        match iris_core::config::AtomicFile::write_text(&path, &text, false) {
            Ok(()) => shared.set_status(
                format!("Exported shareable social config to {}.", path.display()),
                false,
            ),
            Err(e) => shared.set_status(format!("Export failed: {}", e), true),
        }
    }
}

trait TupleToColor32 {
    fn into_color32(self) -> egui::Color32;
}

impl TupleToColor32 for (u8, u8, u8) {
    fn into_color32(self) -> egui::Color32 {
        egui::Color32::from_rgb(self.0, self.1, self.2)
    }
}

fn egui_key_to_hotkey_name(key: Key) -> Option<String> {
    let s = match key {
        Key::Tab => "Tab",
        Key::Space => "Space",
        Key::Enter => "Enter",
        Key::Backspace => "Backspace",
        Key::Insert => "Insert",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::F13 => "F13",
        Key::F14 => "F14",
        Key::F15 => "F15",
        Key::F16 => "F16",
        Key::F17 => "F17",
        Key::F18 => "F18",
        Key::F19 => "F19",
        Key::F20 => "F20",
        Key::Minus => "-",
        Key::Equals => "=",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Backslash => "\\",
        Key::Semicolon => ";",
        Key::Quote => "'",
        Key::Backtick => "`",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        _ => return None,
    };
    Some(s.to_string())
}
