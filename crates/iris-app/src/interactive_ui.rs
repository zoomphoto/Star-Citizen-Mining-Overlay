use crate::state::{AppSharedState, CoordinatorCommand};
use crate::theme::{color_combo, overlay_color32, quantico_text, ThemePalette};
use chrono::Utc;
use eframe::egui::{
    self, Align, Color32, Key, Layout, Margin, Pos2, RichText, Rounding, ScrollArea, Stroke,
    TextEdit, Vec2, ViewportBuilder, ViewportClass, ViewportId,
};
use iris_core::config::ColorResolver;
use iris_core::lookup::RsiNames;
use iris_core::models::{MatchKind, OrgMembership, OverlayColor};
use iris_core::overlay::DossierText;
use iris_core::waypoints::format_distance_m;
use iris_platform::open_https_url;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteractiveView {
    Dossier,
    Waypoints,
}

pub struct InteractiveOverlayState {
    active_view: InteractiveView,
    search_query: String,
    selected_handle: Option<String>,
    note_draft_handle: Option<String>,
    note_draft_text: String,
    fetched_profiles_this_open: HashSet<String>,
    was_open: bool,
    new_poi_name: String,
    new_poi_color: OverlayColor,
    editing_poi_id: Option<String>,
    editing_poi_name: String,
}

impl Default for InteractiveOverlayState {
    fn default() -> Self {
        Self::new()
    }
}

impl InteractiveOverlayState {
    pub fn new() -> Self {
        Self {
            active_view: InteractiveView::Dossier,
            search_query: String::new(),
            selected_handle: None,
            note_draft_handle: None,
            note_draft_text: String::new(),
            fetched_profiles_this_open: HashSet::new(),
            was_open: false,
            new_poi_name: String::new(),
            new_poi_color: OverlayColor::Cyan,
            editing_poi_id: None,
            editing_poi_name: String::new(),
        }
    }

    pub fn show_if_open(&mut self, ctx: &egui::Context, shared: &Arc<AppSharedState>) {
        let is_open = shared.interactive_open.load(Ordering::Relaxed);
        if !is_open {
            self.was_open = false;
            self.fetched_profiles_this_open.clear();
            return;
        }

        if !self.was_open {
            self.was_open = true;
            if self.selected_handle.is_none() {
                if let Some(first) = shared.live_roster.read().first() {
                    self.select_handle(&first.handle, shared);
                }
            } else if let Some(cur) = self.selected_handle.clone() {
                self.select_handle(&cur, shared);
            }
        }

        let game = shared.last_game_window.read().clone();
        let target_rect = if game.present && game.bounds.width >= 320 && game.bounds.height >= 240 {
            game.bounds
        } else if game.monitor_bounds.width >= 320 && game.monitor_bounds.height >= 240 {
            game.monitor_bounds
        } else {
            iris_core::models::IntRect::new(60, 60, 1440, 860)
        };

        let vp_id = ViewportId::from_hash_of("sc-interactive-overlay");
        let builder = ViewportBuilder::default()
            .with_title("SC Overlay — Interactive Dossier")
            .with_app_id("sc-overlay-interactive")
            .with_position(Pos2::new(target_rect.x as f32, target_rect.y as f32))
            .with_inner_size(Vec2::new(
                target_rect.width as f32,
                target_rect.height as f32,
            ))
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false);

        ctx.show_viewport_immediate(vp_id, builder, |vp_ctx, class| {
            if vp_ctx.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
                shared.interactive_open.store(false, Ordering::Relaxed);
                return;
            }

            if class == ViewportClass::Embedded {
                let mut open_flag = true;
                egui::Window::new("SC Interactive Dossier")
                    .open(&mut open_flag)
                    .default_size([1100.0, 680.0])
                    .show(vp_ctx, |ui| {
                        self.render_contents(ui, shared);
                    });
                if !open_flag {
                    shared.interactive_open.store(false, Ordering::Relaxed);
                }
            } else {
                let scrim = egui::Frame::none()
                    .fill(Color32::from_rgba_unmultiplied(8, 10, 15, 196))
                    .inner_margin(Margin::same(20.0));
                egui::CentralPanel::default().frame(scrim).show(vp_ctx, |ui| {
                    self.render_contents(ui, shared);
                });
            }
        });
    }

    fn select_handle(&mut self, handle: &str, shared: &Arc<AppSharedState>) {
        let trimmed = handle.trim();
        if trimmed.is_empty() {
            return;
        }
        self.selected_handle = Some(trimmed.to_string());

        let extra = shared.cache.get_player_extra(trimmed);
        let citizen_rec = extra.as_ref().and_then(|e| e.citizen_record.as_deref());
        let existing_note = shared
            .notes
            .read()
            .find(trimmed, citizen_rec)
            .map(|n| n.text)
            .unwrap_or_default();
        self.note_draft_handle = Some(trimmed.to_string());
        self.note_draft_text = existing_note;

        let key = trimmed.to_ascii_lowercase();
        let needs_fetch = shared.cache.get_player(trimmed).is_none()
            || extra.and_then(|e| e.profile_refreshed).is_none();
        if needs_fetch && self.fetched_profiles_this_open.insert(key) {
            shared.spawn_verify_and_cache_player(trimmed.to_string(), false);
        }
    }

    fn render_contents(&mut self, ui: &mut egui::Ui, shared: &Arc<AppSharedState>) {
        let p = ThemePalette::dark();
        let (hotkey_hint, save_poi_hk) = {
            let s = shared.settings.read();
            (
                s.hotkey_interactive_overlay.clone(),
                s.hotkey_save_waypoint.clone(),
            )
        };
        let wp_count = shared.waypoints.read().waypoints.len();

        // Top Header Bar
        ui.horizontal(|ui| {
            ui.label(quantico_text("SC INTERACTIVE OVERLAY", 19.0, p.accent));
            ui.add_space(10.0);

            if ui
                .selectable_label(
                    self.active_view == InteractiveView::Dossier,
                    RichText::new("Citizen Dossier").strong().size(13.5),
                )
                .clicked()
            {
                self.active_view = InteractiveView::Dossier;
            }
            if ui
                .selectable_label(
                    self.active_view == InteractiveView::Waypoints,
                    RichText::new(format!("⌖ POIs / Beacons ({})", wp_count))
                        .strong()
                        .size(13.5),
                )
                .clicked()
            {
                self.active_view = InteractiveView::Waypoints;
            }

            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "Esc / {} closes • {} drops POI",
                    hotkey_hint, save_poi_hk
                ))
                .size(12.0)
                .color(p.muted),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button(RichText::new("✕ Close (Esc)").color(p.text))
                    .clicked()
                {
                    shared.interactive_open.store(false, Ordering::Relaxed);
                }
            });
        });
        ui.add_space(10.0);

        if self.active_view == InteractiveView::Waypoints {
            self.render_waypoints_view(ui, &p, shared);
            return;
        }

        let avail = ui.available_size();
        let left_w = (avail.x * 0.22).clamp(220.0, 320.0);
        let right_w = (avail.x * 0.25).clamp(250.0, 360.0);
        let center_w = (avail.x - left_w - right_w - 28.0).max(320.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;

            // ================================================================
            // Column 1: Search & Citizen Lookup
            // ================================================================
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, avail.y),
                Layout::top_down(Align::Min),
                |ui| {
                    self.render_search_column(ui, &p, shared);
                },
            );

            // ================================================================
            // Column 2: Center Citizen Dossier & Notes Editor
            // ================================================================
            ui.allocate_ui_with_layout(
                Vec2::new(center_w, avail.y),
                Layout::top_down(Align::Min),
                |ui| {
                    self.render_dossier_column(ui, &p, shared);
                },
            );

            // ================================================================
            // Column 3: Live & Session Roster
            // ================================================================
            ui.allocate_ui_with_layout(
                Vec2::new(right_w, avail.y),
                Layout::top_down(Align::Min),
                |ui| {
                    self.render_roster_column(ui, &p, shared);
                },
            );
        });
    }

    fn render_waypoints_view(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        let live_fix = shared.last_position_fix.read().clone();
        let store_snap = shared.waypoints.read().clone();
        let active_id = store_snap.active_waypoint_id.clone();

        egui::Frame::none()
            .fill(p.surface.gamma_multiply(0.96))
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::same(16.0))
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                ui.horizontal_wrapped(|ui| {
                    ui.label(quantico_text("SAVED POIs & HUD BEACONS", 16.0, p.accent));
                    ui.add_space(12.0);
                    let pos_lbl = if let Some((disp, _key, c)) = &live_fix.body {
                        format!(
                            "● Zone Lock [{}]: {:.1}, {:.1}, {:.1} km",
                            disp,
                            c.x / 1000.0,
                            c.y / 1000.0,
                            c.z / 1000.0
                        )
                    } else if let Some(c) = live_fix.root {
                        format!(
                            "● Live Fix: {:.1}, {:.1}, {:.1} km",
                            c.x / 1000.0,
                            c.y / 1000.0,
                            c.z / 1000.0
                        )
                    } else {
                        "○ Waiting for r_DisplayInfo 2/3 or /showlocation".to_string()
                    };
                    ui.label(
                        RichText::new(pos_lbl)
                            .size(12.5)
                            .color(if live_fix.has_any() { p.success } else { p.muted }),
                    );
                });

                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.new_poi_name)
                            .desired_width(240.0)
                            .hint_text("New POI name (optional)…"),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    color_combo(ui, "int_new_poi_col", &mut self.new_poi_color, true);
                    if ui.button("＋ Save Current Position").clicked() || enter {
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

                    if active_id.is_some() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("✕ Unload Active Beacon").clicked() {
                                shared.waypoints.write().set_active(None);
                                shared.save_waypoints();
                            }
                        });
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                let mut load_id: Option<Option<String>> = None;
                let mut rename_commit: Option<(String, String)> = None;
                let mut color_updates: Vec<(String, OverlayColor)> = Vec::new();
                let mut removals: Vec<String> = Vec::new();

                ScrollArea::vertical()
                    .id_salt("interactive_waypoints_scroll")
                    .show(ui, |ui| {
                        if store_snap.waypoints.is_empty() {
                            ui.label(
                                RichText::new(
                                    "No POIs saved yet. Save your current location above or press Alt+Shift+P in flight.",
                                )
                                .color(p.muted),
                            );
                        }
                        for wp in store_snap.waypoints.iter().rev() {
                            let is_active = active_id.as_deref() == Some(wp.id.as_str());
                            let is_editing = self.editing_poi_id.as_deref() == Some(wp.id.as_str());
                            let c32 = overlay_color32(wp.color);

                            egui::Frame::none()
                                .fill(p.bg.gamma_multiply(0.85))
                                .stroke(Stroke::new(
                                    if is_active { 1.5 } else { 1.0 },
                                    if is_active { c32 } else { p.border },
                                ))
                                .rounding(Rounding::same(6.0))
                                .inner_margin(Margin::symmetric(12.0, 8.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if is_editing {
                                            let r = ui.add(
                                                TextEdit::singleline(&mut self.editing_poi_name)
                                                    .desired_width(220.0),
                                            );
                                            let enter = r.lost_focus()
                                                && ui.input(|i| i.key_pressed(Key::Enter));
                                            if ui.small_button("Save").clicked() || enter {
                                                rename_commit = Some((
                                                    wp.id.clone(),
                                                    self.editing_poi_name.clone(),
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
                                                    .size(15.0)
                                                    .color(c32),
                                            );
                                            if let Some((d_m, locked)) =
                                                live_fix.distance_to_waypoint_m(wp)
                                            {
                                                ui.label(
                                                    RichText::new(format_distance_m(d_m))
                                                        .monospace()
                                                        .strong()
                                                        .size(13.5)
                                                        .color(if locked { p.success } else { p.text }),
                                                );
                                            }
                                            if let Some(lz) = &wp.local_zone {
                                                ui.label(
                                                    RichText::new(format!("[ZONE: {}]", lz))
                                                        .size(12.0)
                                                        .color(p.accent),
                                                );
                                            } else if let Some(zone) = &wp.zone {
                                                ui.label(
                                                    RichText::new(format!("[{}]", zone))
                                                        .size(12.0)
                                                        .color(p.muted),
                                                );
                                            }
                                        }

                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if ui
                                                .small_button(RichText::new("Delete").color(p.danger))
                                                .clicked()
                                            {
                                                removals.push(wp.id.clone());
                                            }
                                            if !is_editing && ui.small_button("Rename").clicked() {
                                                self.editing_poi_id = Some(wp.id.clone());
                                                self.editing_poi_name = wp.name.clone();
                                            }
                                            let mut col = wp.color;
                                            if color_combo(ui, ("int_wp_col", &wp.id), &mut col, true)
                                            {
                                                color_updates.push((wp.id.clone(), col));
                                            }
                                            if is_active {
                                                if ui
                                                    .small_button(
                                                        RichText::new("✓ Active").color(p.success),
                                                    )
                                                    .clicked()
                                                {
                                                    load_id = Some(None);
                                                }
                                            } else if ui
                                                .small_button(
                                                    RichText::new("🎯 Load Beacon").color(p.accent),
                                                )
                                                .clicked()
                                            {
                                                load_id = Some(Some(wp.id.clone()));
                                            }
                                        });
                                    });
                                });
                            ui.add_space(5.0);
                        }
                    });

                let mut dirty = false;
                if let Some(target_opt) = load_id {
                    shared.waypoints.write().set_active(target_opt.as_deref());
                    dirty = true;
                }
                if let Some((id, new_name)) = rename_commit {
                    if shared.waypoints.write().rename(&id, &new_name) {
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
            });
    }

    fn render_search_column(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        egui::Frame::none()
            .fill(p.surface.gamma_multiply(0.95))
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                ui.label(quantico_text("SEARCH CITIZENS", 14.0, p.text));
                ui.add_space(6.0);

                let resp = ui.add(
                    TextEdit::singleline(&mut self.search_query)
                        .desired_width(f32::INFINITY)
                        .hint_text("Type handle (e.g. Niteb1rd)…"),
                );

                let trimmed_q = self.search_query.trim().to_string();
                let valid_handle = RsiNames::is_valid_handle(&trimmed_q);

                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && valid_handle {
                    self.select_handle(&trimmed_q, shared);
                    shared.spawn_verify_and_cache_player(trimmed_q.clone(), false);
                }

                if valid_handle {
                    ui.add_space(6.0);
                    if ui
                        .button(
                            RichText::new(format!("🔍 Lookup \"{}\" on RSI", trimmed_q))
                                .color(p.accent),
                        )
                        .clicked()
                    {
                        self.select_handle(&trimmed_q, shared);
                        shared.spawn_verify_and_cache_player(trimmed_q.clone(), false);
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                // Collect autocomplete suggestions from Roster + Notes + Seen Cache + Player List
                let mut suggestions: Vec<(String, &'static str)> = Vec::new();
                let mut seen_lower = HashSet::new();
                let q_lower = trimmed_q.to_ascii_lowercase();

                for r in shared.live_roster.read().iter() {
                    if q_lower.is_empty() || r.handle.to_ascii_lowercase().contains(&q_lower) {
                        if seen_lower.insert(r.handle.to_ascii_lowercase()) {
                            suggestions.push((r.handle.clone(), "Session"));
                        }
                    }
                }

                if !q_lower.is_empty() {
                    for h in shared.notes.read().handles_matching(&trimmed_q, 15) {
                        if seen_lower.insert(h.to_ascii_lowercase()) {
                            suggestions.push((h, "Note"));
                        }
                    }
                    for sp in shared.cache.search_seen_players(&trimmed_q, 15) {
                        if seen_lower.insert(sp.handle.to_ascii_lowercase()) {
                            suggestions.push((sp.handle, "Seen"));
                        }
                    }
                    for pl in &shared.social.read().players {
                        if pl.handle.to_ascii_lowercase().contains(&q_lower)
                            && seen_lower.insert(pl.handle.to_ascii_lowercase())
                        {
                            suggestions.push((pl.handle.clone(), "Listed"));
                        }
                    }
                } else {
                    for h in shared.notes.read().handles().into_iter().take(15) {
                        if seen_lower.insert(h.to_ascii_lowercase()) {
                            suggestions.push((h, "Note"));
                        }
                    }
                }

                ScrollArea::vertical()
                    .id_salt("interactive_search_scroll")
                    .show(ui, |ui| {
                        if suggestions.is_empty() {
                            ui.label(
                                RichText::new("Type any Star Citizen handle above to inspect or take notes.")
                                    .size(12.0)
                                    .color(p.muted),
                            );
                        }
                        let mut clicked_handle: Option<String> = None;
                        for (handle, source_badge) in suggestions.into_iter().take(30) {
                            let is_sel = self
                                .selected_handle
                                .as_deref()
                                .is_some_and(|s| s.eq_ignore_ascii_case(&handle));
                            let row_resp = ui.selectable_label(
                                is_sel,
                                format!("{}  [{}]", handle, source_badge),
                            );
                            if row_resp.clicked() {
                                clicked_handle = Some(handle);
                            }
                        }
                        if let Some(h) = clicked_handle {
                            self.select_handle(&h, shared);
                        }
                    });
            });
    }

    fn render_dossier_column(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        egui::Frame::none()
            .fill(p.surface.gamma_multiply(0.96))
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::same(16.0))
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());

                let Some(selected) = self.selected_handle.clone() else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.label(quantico_text("NO CITIZEN SELECTED", 18.0, p.muted));
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(
                                "Click a player in the Session Roster on the right, or search any handle on the left.",
                            )
                            .color(p.muted),
                        );
                    });
                    return;
                };

                let cached_player = shared.cache.get_player(&selected);
                let extra = shared.cache.get_player_extra(&selected);
                let display_handle = cached_player
                    .as_ref()
                    .map(|c| c.handle.clone())
                    .or_else(|| extra.as_ref().map(|e| e.handle.clone()))
                    .unwrap_or_else(|| selected.clone());

                let orgs: Vec<OrgMembership> = cached_player
                    .as_ref()
                    .map(|c| c.orgs.clone())
                    .or_else(|| {
                        shared
                            .live_roster
                            .read()
                            .iter()
                            .find(|r| r.handle.eq_ignore_ascii_case(&display_handle))
                            .map(|r| r.identity.orgs.clone())
                    })
                    .unwrap_or_default();

                let social_snap = shared.social.read().clone();
                let cm = if cached_player.is_some() || !orgs.is_empty() {
                    ColorResolver::resolve(&social_snap, &display_handle, &orgs)
                } else {
                    ColorResolver::resolve_unknown(&social_snap, &display_handle)
                };
                let c32 = overlay_color32(cm.color);

                let citizen_rec = extra.as_ref().and_then(|e| e.citizen_record.clone());
                let moniker = extra.as_ref().and_then(|e| e.moniker.clone());

                // Sync note draft if handle changed or rename linked
                if self
                    .note_draft_handle
                    .as_deref()
                    .map(|h| !h.eq_ignore_ascii_case(&display_handle))
                    .unwrap_or(true)
                {
                    self.note_draft_handle = Some(display_handle.clone());
                    self.note_draft_text = shared
                        .notes
                        .read()
                        .find(&display_handle, citizen_rec.as_deref())
                        .map(|n| n.text)
                        .unwrap_or_default();
                }

                // Header Row: Handle + Record + RSI link + Player Color Quick Menu
                ui.horizontal_wrapped(|ui| {
                    let bracket_str = match cm.kind {
                        MatchKind::PlayerList => format!("{{{}}}", display_handle),
                        MatchKind::OrgList => format!("[{}]", display_handle),
                        MatchKind::OrgAttribute | MatchKind::NoMatch => {
                            format!("<{}>", display_handle)
                        }
                    };
                    ui.label(quantico_text(bracket_str, 22.0, c32));

                    if let Some(m) = moniker.as_deref().filter(|s| !s.is_empty()) {
                        ui.label(RichText::new(format!("\"{}\"", m)).size(14.0).color(p.muted));
                    }

                    let rec_label = match citizen_rec.as_deref() {
                        Some(r) if !r.is_empty() => format!("UEE #{}", r),
                        _ => format!("UEE {}", DossierText::NO_CITIZEN_RECORD),
                    };
                    ui.label(
                        RichText::new(rec_label)
                            .monospace()
                            .size(12.5)
                            .color(p.muted),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("RSI Profile ↗").clicked() {
                            let url = format!(
                                "https://robertsspaceindustries.com/en/citizens/{}",
                                display_handle
                            );
                            let _ = open_https_url(&url);
                        }
                        if ui
                            .button("↻ Refresh")
                            .on_hover_text("Re-fetch RSI Citizen & Org profile")
                            .clicked()
                        {
                            shared.spawn_verify_and_cache_player(display_handle.clone(), true);
                        }

                        egui::ComboBox::from_id_salt("dossier_player_color_override")
                            .selected_text(RichText::new("Set Player Color").color(c32))
                            .show_ui(ui, |ui| {
                                for col in OverlayColor::ALL {
                                    if ui
                                        .selectable_label(
                                            cm.kind == MatchKind::PlayerList && cm.color == col,
                                            RichText::new(format!("● {}", col.as_str()))
                                                .color(overlay_color32(col)),
                                        )
                                        .clicked()
                                    {
                                        shared.social.write().upsert_player(&display_handle, col);
                                        shared.save_social();
                                    }
                                }
                                if cm.kind == MatchKind::PlayerList {
                                    ui.separator();
                                    if ui.button("Remove from Player List").clicked() {
                                        shared.social.write().remove_player(&display_handle);
                                        shared.save_social();
                                    }
                                }
                            });
                    });
                });

                let class_str =
                    DossierText::classification_from(cm.kind, Some(&cm), &social_snap);
                ui.label(
                    RichText::new(format!("Classification: {} • {}", class_str, cm.color))
                        .size(13.0)
                        .color(c32),
                );

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // Organizations 2-Column Grid
                ui.label(quantico_text(
                    "ORGANIZATIONS (RIGHT-CLICK TO ASSIGN COLOR)",
                    13.5,
                    p.muted,
                ));
                ui.add_space(6.0);

                let org_boxes = DossierText::org_boxes(&orgs);
                let col_w = ((ui.available_width() - 10.0) * 0.5).max(140.0);

                egui::Grid::new("dossier_orgs_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        for (idx, obox) in org_boxes.iter().enumerate() {
                            let is_deciding = obox.sid.as_deref().is_some_and(|s| {
                                cm.deciding_org_sid
                                    .as_deref()
                                    .is_some_and(|ds| ds.eq_ignore_ascii_case(s))
                            });
                            let border_col = if is_deciding { c32 } else { p.border };

                            let card = egui::Frame::none()
                                .fill(p.elevated)
                                .stroke(Stroke::new(if is_deciding { 1.5 } else { 1.0 }, border_col))
                                .rounding(Rounding::same(6.0))
                                .inner_margin(Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.set_width(col_w - 22.0);
                                    ui.label(
                                        RichText::new(&obox.title)
                                            .strong()
                                            .color(if is_deciding { c32 } else { p.text }),
                                    );
                                    if let Some(full) = obox.full_name.as_deref() {
                                        ui.label(RichText::new(full).size(12.5).color(p.text));
                                    }
                                    let mut meta_parts = Vec::new();
                                    if let Some(arch) = obox.archetype.as_deref() {
                                        meta_parts.push(arch.to_string());
                                    }
                                    if let Some(pri) = obox.primary.as_deref() {
                                        meta_parts.push(pri.to_string());
                                    }
                                    if let Some(sec) = obox.secondary.as_deref() {
                                        meta_parts.push(sec.to_string());
                                    }
                                    if !meta_parts.is_empty() {
                                        ui.label(
                                            RichText::new(meta_parts.join(" • "))
                                                .size(11.5)
                                                .color(p.muted),
                                        );
                                    }
                                });

                            card.response.context_menu(|ui| {
                                if let Some(sid) = obox.sid.as_deref() {
                                    ui.label(RichText::new(format!("Org [{}]", sid)).strong());
                                    ui.separator();
                                    for col in OverlayColor::ALL {
                                        if ui
                                            .button(
                                                RichText::new(format!("Set [{}] ● {}", sid, col))
                                                    .color(overlay_color32(col)),
                                            )
                                            .clicked()
                                        {
                                            shared.social.write().upsert_org(sid, col);
                                            shared.save_social();
                                            ui.close_menu();
                                        }
                                    }
                                    if social_snap
                                        .orgs
                                        .iter()
                                        .any(|o| o.sid.eq_ignore_ascii_case(sid))
                                    {
                                        ui.separator();
                                        if ui.button(format!("Remove [{}] from Org List", sid)).clicked()
                                        {
                                            shared.social.write().remove_org(sid);
                                            shared.save_social();
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("Open RSI Org Page ↗").clicked() {
                                        let url =
                                            format!("https://robertsspaceindustries.com/en/orgs/{}", sid);
                                        let _ = open_https_url(&url);
                                        ui.close_menu();
                                    }
                                } else if let Some(target) = &obox.target {
                                    ui.label(
                                        RichText::new(format!(
                                            "Rule: {}",
                                            target.kind.display_name()
                                        ))
                                        .strong(),
                                    );
                                    ui.separator();
                                    for col in OverlayColor::ALL {
                                        if ui
                                            .button(
                                                RichText::new(format!("Set ● {}", col))
                                                    .color(overlay_color32(col)),
                                            )
                                            .clicked()
                                        {
                                            shared
                                                .social
                                                .write()
                                                .upsert_attribute_rule(target, col);
                                            shared.save_social();
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("Disable Rule").clicked() {
                                        shared.social.write().remove_attribute_rule(target);
                                        shared.save_social();
                                        ui.close_menu();
                                    }
                                }
                            });

                            if idx % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // Dossier Notes Editor (auto-saves to Config/dossier-notes.txt)
                ui.horizontal(|ui| {
                    ui.label(quantico_text("DOSSIER NOTES", 14.0, p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let chars = self.note_draft_text.chars().count();
                        ui.label(
                            RichText::new(format!("{}/4000 • Auto-saved", chars))
                                .size(11.5)
                                .color(p.muted),
                        );
                    });
                });
                ui.add_space(4.0);

                let remaining_h = (ui.available_height() - 8.0).max(90.0);
                let edit_resp = ui.add_sized(
                    Vec2::new(ui.available_width(), remaining_h),
                    TextEdit::multiline(&mut self.note_draft_text)
                        .hint_text(
                            "Write persistent intelligence notes for this citizen (first line appears on the live HUD side panel; linked across handle renames via UEE Citizen Record #)…",
                        ),
                );
                if edit_resp.changed() {
                    if shared.notes.read().set(
                        &display_handle,
                        citizen_rec.as_deref(),
                        &self.note_draft_text,
                        Utc::now(),
                    ) {
                        shared.save_notes();
                    }
                }
            });
    }

    fn render_roster_column(
        &mut self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        shared: &Arc<AppSharedState>,
    ) {
        egui::Frame::none()
            .fill(p.surface.gamma_multiply(0.95))
            .stroke(Stroke::new(1.0, p.border))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                let roster = shared.live_roster.read().clone();
                let social_snap = shared.social.read().clone();

                ui.horizontal(|ui| {
                    ui.label(quantico_text("SESSION ROSTER", 14.0, p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} seen", roster.len()))
                                .size(12.0)
                                .color(p.muted),
                        );
                    });
                });
                ui.add_space(6.0);

                ScrollArea::vertical()
                    .id_salt("interactive_roster_scroll")
                    .show(ui, |ui| {
                        if roster.is_empty() {
                            ui.add_space(20.0);
                            ui.label(
                                RichText::new(
                                    "No confirmed nameplates in this session yet.\n\nLook at players in Star Citizen (Borderless mode) to populate live contacts here.",
                                )
                                .size(12.5)
                                .color(p.muted),
                            );
                            return;
                        }

                        let recent: Vec<_> = roster.iter().filter(|r| r.recent).collect();
                        let earlier: Vec<_> = roster.iter().filter(|r| !r.recent).collect();
                        let mut next_selected: Option<String> = None;

                        if !recent.is_empty() {
                            ui.label(
                                RichText::new("RECENT (LAST 5 MIN)")
                                    .size(11.5)
                                    .strong()
                                    .color(p.accent),
                            );
                            ui.add_space(4.0);
                            for entry in recent {
                                if self.render_roster_card(ui, p, entry, &social_snap, shared) {
                                    next_selected = Some(entry.handle.clone());
                                }
                            }
                            ui.add_space(8.0);
                        }

                        if !earlier.is_empty() {
                            ui.label(
                                RichText::new("EARLIER THIS SESSION")
                                    .size(11.5)
                                    .strong()
                                    .color(p.muted),
                            );
                            ui.add_space(4.0);
                            for entry in earlier {
                                if self.render_roster_card(ui, p, entry, &social_snap, shared) {
                                    next_selected = Some(entry.handle.clone());
                                }
                            }
                        }

                        if let Some(h) = next_selected {
                            self.select_handle(&h, shared);
                        }
                    });
            });
    }

    fn render_roster_card(
        &self,
        ui: &mut egui::Ui,
        p: &ThemePalette,
        entry: &iris_core::pipeline::RosterEntry,
        social_snap: &iris_core::config::SocialConfig,
        shared: &Arc<AppSharedState>,
    ) -> bool {
        let cm = ColorResolver::resolve(social_snap, &entry.handle, &entry.identity.orgs);
        let c32 = overlay_color32(cm.color);
        let is_selected = self
            .selected_handle
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(&entry.handle));
        let has_note = shared.notes.read().has_note(&entry.handle);

        let main_org_label = entry
            .identity
            .orgs
            .iter()
            .find(|o| o.is_main)
            .map(|o| {
                if o.is_redacted() {
                    "Hidden Main".to_string()
                } else {
                    o.sid.clone().unwrap_or_default()
                }
            })
            .unwrap_or_else(|| "No Org".to_string());

        let fill = if is_selected {
            c32.gamma_multiply(0.22)
        } else {
            p.elevated
        };

        let inner = egui::Frame::none()
            .fill(fill)
            .stroke(Stroke::new(
                if is_selected { 1.5 } else { 1.0 },
                if is_selected { c32 } else { p.border },
            ))
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let badge = match cm.kind {
                        MatchKind::PlayerList => format!("{{{}}}", entry.handle),
                        MatchKind::OrgList => format!("[{}]", entry.handle),
                        _ => format!("<{}>", entry.handle),
                    };
                    ui.label(RichText::new(badge).strong().color(c32));
                    if has_note {
                        ui.label(RichText::new("[Note]").size(11.0).color(p.accent));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(main_org_label).size(11.5).color(p.muted));
                    });
                });
            });

        let interact = ui.interact(
            inner.response.rect,
            ui.id().with(("roster_card", &entry.handle)),
            egui::Sense::click(),
        );
        ui.add_space(4.0);

        interact.context_menu(|ui| {
            ui.label(RichText::new(&entry.handle).strong());
            ui.separator();
            for col in OverlayColor::ALL {
                if ui
                    .button(
                        RichText::new(format!("Set Player ● {}", col))
                            .color(overlay_color32(col)),
                    )
                    .clicked()
                {
                    shared.social.write().upsert_player(&entry.handle, col);
                    shared.save_social();
                    ui.close_menu();
                }
            }
            if social_snap
                .players
                .iter()
                .any(|pl| pl.handle.eq_ignore_ascii_case(&entry.handle))
            {
                ui.separator();
                if ui.button("Remove from Player List").clicked() {
                    shared.social.write().remove_player(&entry.handle);
                    shared.save_social();
                    ui.close_menu();
                }
            }
            ui.separator();
            if ui.button("Open RSI Citizen Page ↗").clicked() {
                let url = format!(
                    "https://robertsspaceindustries.com/en/citizens/{}",
                    entry.handle
                );
                let _ = open_https_url(&url);
                ui.close_menu();
            }
        });

        interact.clicked()
    }
}
