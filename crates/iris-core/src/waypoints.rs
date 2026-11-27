use crate::breadcrumbs::{normalize_zone_key, planetary_enu_basis, zones_match, SpaceCoord};
use crate::config::AtomicFile;
use crate::models::{OverlayColor, WaypointOverlayInfo};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;

fn default_waypoint_color() -> OverlayColor {
    OverlayColor::Cyan
}

/// A saved Point of Interest (POI) beacon in 3D Star Citizen coordinates.
///
/// Stores solar-system `Root Pos` (`coord`) AND, when dropped near/on a rotating planet, moon,
/// or planetary orbital station, its local planetary Object Container coordinates (`local_zone`,
/// `local_coord`) so the beacon has zero rotation drift.
///
/// The JSON schema is intentionally clean and self-contained so waypoints can later be
/// POSTed/GETed from a sharing API without breaking local storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Waypoint {
    pub id: String,
    pub name: String,
    pub coord: SpaceCoord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    /// Local coordinates inside a rotating planet/moon container (e.g. `pyro6`, `daymar`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_coord: Option<SpaceCoord>,
    /// Canonical/display planet/moon container name corresponding to `local_coord`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default = "default_waypoint_color")]
    pub color: OverlayColor,
    pub created_at: DateTime<Utc>,
}

/// Current live ship position across both solar-system Root space and (when near/on a planet or moon)
/// the local rotating planetary Object Container.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LivePositionFix {
    pub root: Option<SpaceCoord>,
    pub body: Option<(String, String, SpaceCoord)>,
    pub display_zone: Option<String>,
}

impl LivePositionFix {
    pub fn has_any(&self) -> bool {
        self.root.is_some() || self.body.is_some()
    }

    /// Computes live 3D distance in meters to `wp`, preferring zero-drift planetary local frame
    /// whenever both the ship and `wp` are inside the same planet/moon container.
    pub fn distance_to_waypoint_m(&self, wp: &Waypoint) -> Option<(f64, bool)> {
        if let (Some((_disp, cur_key, cur_local)), Some(wp_zone), Some(wp_local)) =
            (&self.body, wp.local_zone.as_deref(), wp.local_coord)
        {
            if zones_match(cur_key, wp_zone) {
                return Some((cur_local.distance_m(&wp_local), true));
            }
        }
        self.root.map(|r| (r.distance_m(&wp.coord), false))
    }
}

/// Persistent collection of saved POIs plus the currently loaded/active beacon id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaypointStore {
    #[serde(default = "WaypointStore::default_schema_version")]
    pub schema_version: u32,
    #[serde(default = "WaypointStore::default_next_seq")]
    pub next_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_waypoint_id: Option<String>,
    #[serde(default)]
    pub waypoints: Vec<Waypoint>,
}

impl Default for WaypointStore {
    fn default() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            next_seq: 1,
            active_waypoint_id: None,
            waypoints: Vec::new(),
        }
    }
}

impl WaypointStore {
    pub const SCHEMA_VERSION: u32 = 1;

    fn default_schema_version() -> u32 {
        Self::SCHEMA_VERSION
    }

    fn default_next_seq() -> u64 {
        1
    }

    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut store) = serde_json::from_str::<Self>(&text) else {
            return Self::default();
        };
        if store.next_seq == 0 {
            store.next_seq = (store.waypoints.len() as u64) + 1;
        }
        // Clear dangling active id if it was manually removed from JSON
        if let Some(active_id) = store.active_waypoint_id.as_deref() {
            if !store.waypoints.iter().any(|w| w.id == active_id) {
                store.active_waypoint_id = None;
            }
        }
        store
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize waypoints: {}", e))?;
        AtomicFile::write_text(path, &json, false)
            .map_err(|e| format!("Failed to save waypoints: {}", e))
    }

    /// Saves a new POI at `coord` (plus optional planetary `local_body` frame for zero rotation drift),
    /// auto-naming it `POI #N` when `name` is empty/None.
    #[allow(clippy::too_many_arguments)]
    pub fn add_point_with_local(
        &mut self,
        coord: SpaceCoord,
        local_body: Option<(String, SpaceCoord)>,
        name: Option<String>,
        zone: Option<String>,
        notes: Option<String>,
        color: Option<OverlayColor>,
        now: DateTime<Utc>,
    ) -> Waypoint {
        let seq = self.next_seq.max(1);
        self.next_seq = seq + 1;

        let trimmed_name = name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("POI #{}", seq));

        let (local_zone, local_coord) = match local_body {
            Some((z, c)) => {
                let norm = normalize_zone_key(&z);
                if norm.is_empty() {
                    (None, None)
                } else {
                    (Some(norm), Some(c))
                }
            }
            None => (None, None),
        };

        let trimmed_zone = zone
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| local_zone.clone());

        let trimmed_notes = notes
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        let id = format!("poi-{}-{}", now.timestamp_millis(), seq);
        let wp = Waypoint {
            id,
            name: trimmed_name,
            coord,
            zone: trimmed_zone,
            local_coord,
            local_zone,
            notes: trimmed_notes,
            color: color.unwrap_or_else(default_waypoint_color),
            created_at: now,
        };
        self.waypoints.push(wp.clone());
        wp
    }

    /// Saves a new POI at `coord`, auto-naming it `POI #N` when `name` is empty/None.
    pub fn add_point(
        &mut self,
        coord: SpaceCoord,
        name: Option<String>,
        zone: Option<String>,
        notes: Option<String>,
        color: Option<OverlayColor>,
        now: DateTime<Utc>,
    ) -> Waypoint {
        self.add_point_with_local(coord, None, name, zone, notes, color, now)
    }

    pub fn active(&self) -> Option<&Waypoint> {
        let id = self.active_waypoint_id.as_deref()?;
        self.waypoints.iter().find(|w| w.id == id)
    }

    pub fn set_active(&mut self, id: Option<&str>) -> bool {
        match id {
            None => {
                let changed = self.active_waypoint_id.is_some();
                self.active_waypoint_id = None;
                changed
            }
            Some(target_id) => {
                if self.waypoints.iter().any(|w| w.id == target_id) {
                    let changed = self.active_waypoint_id.as_deref() != Some(target_id);
                    self.active_waypoint_id = Some(target_id.to_string());
                    changed
                } else {
                    false
                }
            }
        }
    }

    pub fn rename(&mut self, id: &str, new_name: &str) -> bool {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some(wp) = self.waypoints.iter_mut().find(|w| w.id == id) {
            if wp.name != trimmed {
                wp.name = trimmed.to_string();
                return true;
            }
        }
        false
    }

    pub fn set_color(&mut self, id: &str, color: OverlayColor) -> bool {
        if let Some(wp) = self.waypoints.iter_mut().find(|w| w.id == id) {
            if wp.color != color {
                wp.color = color;
                return true;
            }
        }
        false
    }

    pub fn set_notes(&mut self, id: &str, notes: Option<String>) -> bool {
        let clean = notes
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        if let Some(wp) = self.waypoints.iter_mut().find(|w| w.id == id) {
            if wp.notes != clean {
                wp.notes = clean;
                return true;
            }
        }
        false
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.waypoints.len();
        self.waypoints.retain(|w| w.id != id);
        if self.active_waypoint_id.as_deref() == Some(id) {
            self.active_waypoint_id = None;
        }
        self.waypoints.len() != before
    }

    /// Exports all waypoints (or a single waypoint) as clean API-ready JSON.
    pub fn export_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to export waypoints JSON: {}", e))
    }

    /// Imports waypoints from either a full `WaypointStore` JSON object, a JSON array of
    /// `Waypoint`s, or a single `Waypoint` JSON object. Deduplicates by `id`.
    /// Returns the number of newly added waypoints.
    pub fn import_json(&mut self, text: &str) -> Result<usize, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("Waypoint JSON is empty.".to_string());
        }
        let incoming: Vec<Waypoint> = if let Ok(store) = serde_json::from_str::<WaypointStore>(trimmed) {
            store.waypoints
        } else if let Ok(list) = serde_json::from_str::<Vec<Waypoint>>(trimmed) {
            list
        } else if let Ok(single) = serde_json::from_str::<Waypoint>(trimmed) {
            vec![single]
        } else {
            return Err("Unrecognized waypoint JSON format.".to_string());
        };

        let mut added = 0usize;
        for mut wp in incoming {
            if wp.id.trim().is_empty() {
                let seq = self.next_seq.max(1);
                self.next_seq = seq + 1;
                wp.id = format!("poi-imp-{}-{}", Utc::now().timestamp_millis(), seq);
            }
            if wp.name.trim().is_empty() {
                let seq = self.next_seq.max(1);
                self.next_seq = seq + 1;
                wp.name = format!("POI #{}", seq);
            }
            if !self.waypoints.iter().any(|existing| existing.id == wp.id) {
                self.waypoints.push(wp);
                added += 1;
            }
        }
        Ok(added)
    }
}

/// Rolling short-window velocity & heading estimator from live Root coordinates.
///
/// Because Star Citizen's Root coordinate frame is consistent across space, comparing recent
/// positions (`P_now - P_prev`) yields exact horizontal & vertical flight heading without
/// depending on ship roll or local physics-grid camera Euler conventions.
#[derive(Debug, Default, Clone)]
pub struct HeadingTracker {
    samples: VecDeque<(SpaceCoord, f64)>,
    last_valid_yaw: Option<(f64, f64)>,
    last_valid_velocity: Option<([f64; 3], f64)>,
}

impl HeadingTracker {
    const MAX_SAMPLES: usize = 16;
    const WINDOW_MS: f64 = 2_500.0;
    const MIN_DISPLACEMENT_M: f64 = 10.0;
    const HEADING_HOLD_MS: f64 = 2_500.0;
    const VELOCITY_HOLD_MS: f64 = 2_000.0;

    pub fn reset(&mut self) {
        self.samples.clear();
        self.last_valid_yaw = None;
        self.last_valid_velocity = None;
    }

    pub fn push(&mut self, coord: SpaceCoord, now_ms: f64) {
        if let Some((last_c, last_t)) = self.samples.back() {
            // If we quantum-jumped (> 500 km in one step), reset the short velocity window
            if last_c.distance_km(&coord) > 500.0 || now_ms < *last_t {
                self.samples.clear();
                self.last_valid_velocity = None;
            } else if last_c.distance_m(&coord) < 0.25 && (now_ms - *last_t) < 120.0 {
                return;
            }
        }

        self.samples.push_back((coord, now_ms));
        while self.samples.len() > Self::MAX_SAMPLES {
            self.samples.pop_front();
        }
        while let Some((_, ts)) = self.samples.front() {
            if self.samples.len() > 2 && now_ms - *ts > Self::WINDOW_MS {
                self.samples.pop_front();
            } else {
                break;
            }
        }

        // Find the most recent prior sample with at least MIN_DISPLACEMENT_M of travel
        // (preferring a crisp ~350-800 ms baseline so turns update immediately without lag).
        let mut best_vel: Option<(f64, f64, f64, f64)> = None;
        for &(prev_c, prev_t) in self.samples.iter().rev().skip(1) {
            let dt_ms = now_ms - prev_t;
            if dt_ms < 80.0 {
                continue;
            }
            let dx = coord.x - prev_c.x;
            let dy = coord.y - prev_c.y;
            let dz = coord.z - prev_c.z;
            let dist_3d = (dx * dx + dy * dy + dz * dz).sqrt();
            if dist_3d >= Self::MIN_DISPLACEMENT_M {
                let dt_s = dt_ms / 1000.0;
                best_vel = Some((dx / dt_s, dy / dt_s, dz / dt_s, (dx * dx + dy * dy).sqrt()));
                if dt_ms >= 350.0 {
                    break;
                }
            }
        }

        if let Some((vx, vy, vz, horiz_disp)) = best_vel {
            self.last_valid_velocity = Some(([vx, vy, vz], now_ms));
            if horiz_disp >= Self::MIN_DISPLACEMENT_M {
                self.last_valid_yaw = Some((vy.atan2(vx), now_ms));
            }
        }
    }

    /// Returns the horizontal travel azimuth `atan2(dy, dx)` in radians if known recently (for deep space).
    pub fn heading_rad(&self, now_ms: f64) -> Option<f64> {
        self.last_valid_yaw
            .filter(|(_, ts)| now_ms - *ts <= Self::HEADING_HOLD_MS)
            .map(|(yaw, _)| yaw)
    }

    /// Returns the recent 3D velocity vector `(vx, vy, vz)` in m/s if actively moving.
    pub fn velocity_mps(&self, now_ms: f64) -> Option<[f64; 3]> {
        self.last_valid_velocity
            .filter(|(_, ts)| now_ms - *ts <= Self::VELOCITY_HOLD_MS)
            .map(|(v, _)| v)
    }

    /// Returns the most recent valid 3D velocity vector within the heading hold window,
    /// used on spherical planets to project travel heading onto the local ENU tangent plane.
    pub fn recent_velocity_for_heading(&self, now_ms: f64) -> Option<[f64; 3]> {
        self.last_valid_velocity
            .filter(|(_, ts)| now_ms - *ts <= Self::HEADING_HOLD_MS)
            .map(|(v, _)| v)
    }
}

#[inline]
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Computes live HUD guidance for an active `Waypoint` from the ship's current position & heading.
pub struct WaypointGuidance;

impl WaypointGuidance {
    pub const ARRIVED_RADIUS_M: f64 = 150.0;

    /// Builds HUD guidance selecting zero-drift planetary local-zone coordinates whenever both
    /// the ship and `wp` are inside the same planet/moon Object Container, and otherwise falling
    /// back seamlessly to solar-system Root coordinates.
    pub fn build_overlay_info_dual(
        wp: &Waypoint,
        fix: &LivePositionFix,
        root_heading: &HeadingTracker,
        body_heading: &HeadingTracker,
        show_ring_marker: bool,
        now_ms: f64,
    ) -> WaypointOverlayInfo {
        if let (Some((disp_zone, cur_key, cur_local)), Some(wp_zone), Some(wp_local)) =
            (&fix.body, wp.local_zone.as_deref(), wp.local_coord)
        {
            if zones_match(cur_key, wp_zone) {
                return Self::build_for_frame(
                    wp,
                    wp_local,
                    Some(*cur_local),
                    body_heading,
                    Some(disp_zone.clone()),
                    show_ring_marker,
                    now_ms,
                );
            }
        }

        Self::build_for_frame(
            wp,
            wp.coord,
            fix.root,
            root_heading,
            None,
            show_ring_marker,
            now_ms,
        )
    }

    pub fn build_overlay_info(
        wp: &Waypoint,
        current_pos: Option<SpaceCoord>,
        heading: &HeadingTracker,
        show_ring_marker: bool,
        now_ms: f64,
    ) -> WaypointOverlayInfo {
        Self::build_for_frame(wp, wp.coord, current_pos, heading, None, show_ring_marker, now_ms)
    }

    fn build_for_frame(
        wp: &Waypoint,
        target_coord: SpaceCoord,
        current_pos: Option<SpaceCoord>,
        heading: &HeadingTracker,
        zone_lock: Option<String>,
        show_ring_marker: bool,
        now_ms: f64,
    ) -> WaypointOverlayInfo {
        let Some(cur) = current_pos else {
            return WaypointOverlayInfo {
                waypoint_id: wp.id.clone(),
                name: wp.name.clone(),
                color: wp.color,
                has_position_fix: false,
                arrived: false,
                distance_m: 0.0,
                distance_text: "-- km".to_string(),
                steering_text: "WAITING FOR POSITION".to_string(),
                vertical_text: String::new(),
                subline: "Enable r_DisplayInfo 2/3 or type /showlocation".to_string(),
                rel_yaw_rad: None,
                dz_m: 0.0,
                world_bearing_deg: 0.0,
                closing_mps: None,
                zone_lock,
                show_ring_marker: false,
            };
        };

        let tx = target_coord.x - cur.x;
        let ty = target_coord.y - cur.y;
        let tz = target_coord.z - cur.z;
        let dist_m = (tx * tx + ty * ty + tz * tz).sqrt();
        let arrived = dist_m <= Self::ARRIVED_RADIUS_M;
        let distance_text = format_distance_m(dist_m);

        // When locked to a planetary/moon container (`zone_lock.is_some()`), project onto the
        // local spherical East-North-Up (ENU) surface tangent plane so:
        // - `world_bearing_deg` matches Star Citizen's real-time top compass bar (0°=N, 90°=E, 180°=S, 270°=W)
        // - `vertical_delta_m` is true radial altitude difference (||P_target|| - ||P_cur||), NOT polar Z!
        // - Left/Right steering works accurately at the equator, mid-latitudes, and southern hemisphere.
        let (horiz_m, vertical_delta_m, world_bearing_deg, rel_yaw_rad, is_planetary) =
            if zone_lock.is_some() {
                if let Some((east, north, _up)) = planetary_enu_basis(cur) {
                    let t_vec = [tx, ty, tz];
                    let t_e = dot3(t_vec, east);
                    let t_n = dot3(t_vec, north);
                    let horiz = (t_e * t_e + t_n * t_n).sqrt();
                    let r_target = (target_coord.x * target_coord.x
                        + target_coord.y * target_coord.y
                        + target_coord.z * target_coord.z)
                        .sqrt();
                    let r_cur = (cur.x * cur.x + cur.y * cur.y + cur.z * cur.z).sqrt();
                    let d_alt = r_target - r_cur;

                    // Compass heading: 0 = North, +pi/2 = East, pi = South, -pi/2 = West
                    let target_hdg_rad = t_e.atan2(t_n);
                    let bearing_deg = target_hdg_rad.to_degrees().rem_euclid(360.0);

                    let rel_yaw = if arrived || horiz < 10.0 {
                        None
                    } else {
                        heading.recent_velocity_for_heading(now_ms).and_then(|v| {
                            let v_e = dot3(v, east);
                            let v_n = dot3(v, north);
                            // Require deliberate forward travel (>= 6 m/s) so hovering / strafing /
                            // turning in place never freezes a stale reverse arrow.
                            if (v_e * v_e + v_n * v_n).sqrt() >= 6.0 {
                                let vel_hdg_rad = v_e.atan2(v_n);
                                // rel_yaw < 0 => RIGHT, rel_yaw > 0 => LEFT
                                Some(wrap_angle_pi(vel_hdg_rad - target_hdg_rad))
                            } else {
                                None
                            }
                        })
                    };
                    (horiz, d_alt, bearing_deg, rel_yaw, true)
                } else {
                    let horiz = (tx * tx + ty * ty).sqrt();
                    let target_az = ty.atan2(tx);
                    let bearing_deg = (90.0 - target_az.to_degrees()).rem_euclid(360.0);
                    let rel_yaw = if arrived || horiz < 10.0 {
                        None
                    } else {
                        heading
                            .heading_rad(now_ms)
                            .map(|vel_az| wrap_angle_pi(target_az - vel_az))
                    };
                    (horiz, tz, bearing_deg, rel_yaw, false)
                }
            } else {
                let horiz = (tx * tx + ty * ty).sqrt();
                let target_az = ty.atan2(tx);
                let bearing_deg = (90.0 - target_az.to_degrees()).rem_euclid(360.0);
                let rel_yaw = if arrived || horiz < 10.0 {
                    None
                } else {
                    heading
                        .heading_rad(now_ms)
                        .map(|vel_az| wrap_angle_pi(target_az - vel_az))
                };
                (horiz, tz, bearing_deg, rel_yaw, false)
            };

        // Closing rate along line of sight: dot(vel, unit_to_target)
        let closing_mps = if dist_m >= 1.0 {
            heading.velocity_mps(now_ms).map(|v| {
                let ux = tx / dist_m;
                let uy = ty / dist_m;
                let uz = tz / dist_m;
                v[0] * ux + v[1] * uy + v[2] * uz
            })
        } else {
            None
        };

        let steering_text = if arrived {
            "ARRIVED AT POI".to_string()
        } else if let Some(rel) = rel_yaw_rad {
            let deg = rel.to_degrees();
            let abs_deg = deg.abs().round() as i32;
            if abs_deg <= 10 {
                "ON COURSE".to_string()
            } else if abs_deg >= 155 {
                "TURN AROUND".to_string()
            } else if deg < 0.0 {
                format!("RIGHT {}°", abs_deg)
            } else {
                format!("LEFT {}°", abs_deg)
            }
        } else if horiz_m < 10.0 {
            if vertical_delta_m >= 0.0 {
                "DIRECTLY ABOVE".to_string()
            } else {
                "DIRECTLY BELOW".to_string()
            }
        } else if is_planetary {
            format!("FLY HDG {:03.0}°", world_bearing_deg)
        } else {
            format!("HDG {:03.0}° (FLY TO LOCK)", world_bearing_deg)
        };

        let vertical_text = if arrived {
            "ON TARGET".to_string()
        } else if vertical_delta_m.abs() < 35.0 {
            "LEVEL".to_string()
        } else if vertical_delta_m > 0.0 {
            format!("UP +{}", format_distance_m(vertical_delta_m))
        } else {
            format!("DOWN -{}", format_distance_m(vertical_delta_m.abs()))
        };

        let mut parts: Vec<String> = Vec::new();
        // On planets/moons, always show the zero-lag compass heading matching SC's top compass bar!
        if is_planetary && !arrived && horiz_m >= 10.0 && rel_yaw_rad.is_some() {
            parts.push(format!("HDG {:03.0}°", world_bearing_deg));
        }
        parts.push(steering_text.clone());
        if !vertical_text.is_empty() && !arrived {
            parts.push(vertical_text.clone());
        }
        if let Some(rate) = closing_mps {
            if rate >= 5.0 {
                let eta_s = (dist_m / rate).round() as u64;
                parts.push(format!("ETA {}", format_eta_secs(eta_s)));
            } else if rate <= -15.0 && !arrived {
                parts.push("MOVING AWAY".to_string());
            }
        }
        if let Some(ref zl) = zone_lock {
            parts.push(format!("ZONE {}", zl.to_ascii_uppercase()));
        }
        let subline = parts.join("  |  ");

        WaypointOverlayInfo {
            waypoint_id: wp.id.clone(),
            name: wp.name.clone(),
            color: wp.color,
            has_position_fix: true,
            arrived,
            distance_m: dist_m,
            distance_text,
            steering_text,
            vertical_text,
            subline,
            rel_yaw_rad,
            dz_m: vertical_delta_m,
            world_bearing_deg,
            closing_mps,
            zone_lock,
            show_ring_marker: show_ring_marker && !arrived && rel_yaw_rad.is_some(),
        }
    }
}

/// Formats distance cleanly for Star Citizen HUD display:
/// - `< 1 km`: `"420 m"`
/// - `1 km .. 100 km`: `"10.0 km"`, `"9.9 km"`
/// - `100 km .. 10,000 km`: `"342 km"`
/// - `>= 10,000 km`: `"14.2 Mm"`
pub fn format_distance_m(dist_m: f64) -> String {
    let m = dist_m.abs();
    if m < 1_000.0 {
        format!("{:.0} m", m)
    } else if m < 100_000.0 {
        format!("{:.1} km", m / 1_000.0)
    } else if m < 10_000_000.0 {
        format!("{:.0} km", m / 1_000.0)
    } else {
        format!("{:.1} Mm", m / 1_000_000.0)
    }
}

pub fn format_eta_secs(secs: u64) -> String {
    if secs < 60 {
        format!("0:{:02}", secs)
    } else if secs < 3600 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn wrap_angle_pi(rad: f64) -> f64 {
    let mut a = (rad + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
    if a <= -std::f64::consts::PI {
        a += std::f64::consts::TAU;
    }
    a
}
