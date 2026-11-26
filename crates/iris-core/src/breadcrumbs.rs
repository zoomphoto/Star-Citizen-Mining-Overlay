use crate::config::AtomicFile;
use crate::models::{BreadcrumbOverlayInfo, BreadcrumbRenderPoint, ProximityZone};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Parsed multi-frame telemetry from Star Citizen's top-right `r_DisplayInfo 2/3` HUD.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfoTelemetry {
    /// Solar-system global coordinates (`Zone: Root Pos:` / `Zone: solarsystem_... Pos:`)
    /// plus whether both lines agreed within 5 km.
    pub root: Option<(SpaceCoord, bool)>,
    /// Innermost rotating planet/moon container (`display_name`, `normalized_key`, `coord_m`),
    /// e.g. `("pyr06", "pyro6", SpaceCoord)` — stationary relative to that planet/moon & its orbital stations.
    pub body: Option<(String, String, SpaceCoord)>,
    /// Most specific human-readable zone label seen (for display/tagging).
    pub display_zone: Option<String>,
}

/// Normalizes a Star Citizen body zone name for resilient OCR matching (e.g. `"pyr06"` == `"pyro6"`,
/// `"Hurst0n"` == `"hurston"`). Keeps digits `1..9` intact while folding `'0'` -> `'o'`.
pub fn normalize_zone_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            let lower = ch.to_ascii_lowercase();
            if lower == '0' {
                out.push('o');
            } else {
                out.push(lower);
            }
        }
    }
    out
}

/// Returns `true` if two Star Citizen zone labels refer to the same astronomical body.
pub fn zones_match(a: &str, b: &str) -> bool {
    let na = normalize_zone_key(a);
    let nb = normalize_zone_key(b);
    !na.is_empty() && na == nb
}

fn is_ship_or_interior_zone(zone_name: &str) -> bool {
    let lower = zone_name.to_ascii_lowercase();
    if lower.starts_with("rs_")
        || lower.starts_with("rs")
            && (lower.contains("int") || lower.contains("refinery") || lower.contains("station"))
        || lower.starts_with("int_")
        || lower.starts_with("hangar")
        || lower.starts_with("hab_")
        || lower.starts_with("shop_")
        || lower.starts_with("lobby")
        || lower.starts_with("elevator")
        || lower.starts_with("transit")
        || lower.starts_with("dock")
        || lower.contains("ruinstation")
    {
        return true;
    }

    const SHIP_PREFIXES: &[&str] = &[
        "misc_", "drak_", "anvl_", "aegs_", "orig_", "rsi_", "crus_", "argo_", "cnou_", "gama_",
        "espr_", "mrai_", "banu_", "xian_", "vncl_", "tmbl_", "krig_", "grin_", "tngs_", "xnaa_",
        "aopo_", "gatc_", "krug_",
    ];
    if SHIP_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }

    // Dynamic vehicle/entity containers carry a long numeric instance ID (>= 6 consecutive digits)
    let mut run = 0usize;
    for ch in zone_name.chars() {
        if ch.is_ascii_digit() {
            run += 1;
            if run >= 6 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// 3D coordinates in meters as reported by Star Citizen's `/showlocation` chat command.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpaceCoord {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl SpaceCoord {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// 3D Euclidean distance in meters (`f64` retains sub-millimeter precision at 10^11 m).
    pub fn distance_m(&self, other: &Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    pub fn distance_km(&self, other: &Self) -> f64 {
        self.distance_m(other) / 1000.0
    }

    /// Parses Star Citizen's `/showlocation` clipboard output, e.g.:
    /// `Coordinates: x:-48118694104.272797 y:48503341985.968727 z:78519.184221`
    ///
    /// If multiple lines are present, returns the last valid coordinate line.
    pub fn parse_showlocation(text: &str) -> Option<Self> {
        let mut last = None;
        for line in text.lines() {
            if let Some(coord) = Self::parse_line(line) {
                last = Some(coord);
            }
        }
        last
    }

    /// Parses Star Citizen's `r_DisplayInfo 2`/`3` top-right HUD telemetry OCR text, e.g.:
    /// `Zone: solarsystem_4254651523815 Pos: -16111907.1877km 45541999.9767km -90.6115km`
    /// `Zone: Root Pos: -16111907.1877km 45541999.9767km -90.6115km`
    ///
    /// Returns `(coord, dual_line_confirmed)` where `dual_line_confirmed` is true when both
    /// `Root Pos:` and `solarsystem ... Pos:` parsed and agreed within 5 km.
    pub fn parse_display_info(text: &str) -> Option<(Self, bool)> {
        let mut root_coord: Option<Self> = None;
        let mut solar_coord: Option<Self> = None;
        let mut fallback_global: Option<Self> = None;

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            let has_pos = lower.contains("pos") || lower.contains("p0s");
            let is_root = (lower.contains("root") || lower.contains("r00t")) && has_pos;
            let is_solar = (lower.contains("solarsystem")
                || lower.contains("solar")
                || lower.contains("501arsystem")
                || lower.contains("larsys"))
                && has_pos;

            if is_root || is_solar {
                if let Some(coord) = Self::parse_display_info_pos_line(line) {
                    // Reject ship-interior / local-container coordinates (e.g. MISC_Prospector 0.00m 0.05m 1.74m):
                    // Solar-system Root Pos is always > 100 km from origin.
                    if coord.x.abs() > 100_000.0 || coord.y.abs() > 100_000.0 {
                        if is_root {
                            root_coord = Some(coord);
                        } else {
                            solar_coord = Some(coord);
                        }
                        continue;
                    }
                }
            }

            // Fallback: if OCR mangled "Zone: Root Pos:" prefix on a solar-system coordinate row,
            // look for 3 km-suffixed numbers where |x| or |y| > 100,000 km (unique to Root/solarsystem).
            if let Some(coord) = Self::parse_three_km_anywhere(line) {
                if coord.x.abs() > 100_000_000.0 || coord.y.abs() > 100_000_000.0 {
                    if let Some(prev) = fallback_global {
                        if prev.distance_km(&coord) <= 5.0 {
                            return Some((coord, true));
                        }
                    }
                    fallback_global = Some(coord);
                }
            }
        }

        match (root_coord, solar_coord) {
            (Some(r), Some(s)) => {
                if r.distance_km(&s) <= 5.0 {
                    Some((r, true))
                } else {
                    Some((r, false))
                }
            }
            (Some(r), None) => Some((r, false)),
            (None, Some(s)) => Some((s, false)),
            (None, None) => fallback_global.map(|c| (c, false)),
        }
    }

    /// Parses both solar-system `Root Pos` AND the innermost planetary/moon rotating Object Container
    /// (e.g., `Zone: pyr06 Pos: -357.1km 302.1km -100.0km`) from Star Citizen's `r_DisplayInfo 2/3` OCR.
    pub fn parse_display_info_telemetry(text: &str) -> DisplayInfoTelemetry {
        let root = Self::parse_display_info(text);
        let display_zone = Self::parse_display_info_zone(text);
        let mut body: Option<(String, String, SpaceCoord)> = None;

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            let has_pos = lower.contains("pos") || lower.contains("p0s");
            if !has_pos {
                continue;
            }
            let is_root = lower.contains("root") || lower.contains("r00t");
            let is_solar = lower.contains("solarsystem")
                || lower.contains("solar")
                || lower.contains("501arsystem")
                || lower.contains("larsys");
            if is_root || is_solar {
                continue;
            }

            // Extract zone token between "Zone:" and "Pos:"
            let Some(pos_idx) = lower.find("pos").or_else(|| lower.find("p0s")) else {
                continue;
            };
            let prefix = &line[..pos_idx];
            let zone_raw = if let Some(z_idx) = prefix.to_ascii_lowercase().find("zone") {
                prefix[z_idx + 4..].trim_matches(|c: char| {
                    c == ':' || c == ';' || c == '.' || c.is_whitespace()
                })
            } else {
                // Handle slightly clipped "one: pyr06" / "e: pyr06" OCR prefix
                prefix
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .trim_matches(|c: char| c == ':' || c == ';' || c == '.')
            };

            let zone_tok = zone_raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| c == ':' || c == ';' || c == ',' || c == '.');
            if zone_tok.len() < 3 {
                continue;
            }
            let z_low = zone_tok.to_ascii_lowercase();
            if z_low == "root" || z_low == "r00t" || z_low == "zone" || z_low == "one" {
                continue;
            }
            if is_ship_or_interior_zone(zone_raw) || is_ship_or_interior_zone(zone_tok) {
                continue;
            }

            if let Some(coord) = Self::parse_display_info_pos_line(line) {
                let dist_from_origin_m =
                    (coord.x * coord.x + coord.y * coord.y + coord.z * coord.z).sqrt();
                // Planetary/moon surface & orbital coordinates are always >= 10 km from body center
                // and < 5,000,000 km (rejecting ship/room interiors in meters & Root Pos in tens of Gm).
                if (10_000.0..=5_000_000_000.0).contains(&dist_from_origin_m) {
                    let norm_key = normalize_zone_key(zone_tok);
                    if norm_key.len() >= 3 && body.is_none() {
                        body = Some((zone_tok.to_string(), norm_key, coord));
                    }
                }
            }
        }

        let display_zone = body
            .as_ref()
            .map(|(z, _, _)| z.clone())
            .or(display_zone);
        DisplayInfoTelemetry {
            root,
            body,
            display_zone,
        }
    }

    /// Extracts the most specific Star Citizen `Zone: <name>` label from `r_DisplayInfo` OCR text
    /// (preferring local containers like `rs_refinery_pyro`, falling back to `solarsystem_...`).
    pub fn parse_display_info_zone(text: &str) -> Option<String> {
        let mut local_zone: Option<String> = None;
        let mut solar_zone: Option<String> = None;
        for raw_line in text.lines() {
            let line = raw_line.trim();
            let lower = line.to_ascii_lowercase();
            let Some(z_idx) = lower.find("zone") else {
                continue;
            };
            let after = line[z_idx + 4..].trim_start_matches(|c: char| c == ':' || c == ';' || c.is_whitespace());
            let Some(tok) = after.split_whitespace().next() else {
                continue;
            };
            let clean = tok.trim_matches(|c: char| c == ',' || c == ';' || c == ':');
            if clean.len() < 3 {
                continue;
            }
            let cl = clean.to_ascii_lowercase();
            if cl == "root" || cl == "r00t" || cl == "pos" || cl == "pos:" {
                continue;
            }
            if cl.starts_with("solarsystem") || cl.starts_with("solar") {
                solar_zone = Some(clean.to_string());
            } else if local_zone.is_none() {
                local_zone = Some(clean.to_string());
            }
        }
        local_zone.or(solar_zone)
    }

    fn parse_three_km_anywhere(line: &str) -> Option<Self> {
        let mut norm = String::with_capacity(line.len());
        let chars: Vec<char> = line.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_digit = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
            match c {
                '−' | '–' | '—' => norm.push('-'),
                ',' if prev_digit && next_digit => norm.push('.'),
                'O' | 'o' if prev_digit || next_digit => norm.push('0'),
                'l' | 'I' | '|' if prev_digit || next_digit => norm.push('1'),
                _ => norm.push(c),
            }
        }
        let mut vals_m: Vec<f64> = Vec::new();
        for raw_tok in norm.split_whitespace() {
            let t = raw_tok.trim_matches(|c: char| c == ',' || c == ';' || c == ':');
            if let Some(m) = Self::parse_distance_token(t) {
                vals_m.push(m);
            }
        }
        if vals_m.len() >= 3 {
            let n = vals_m.len();
            Some(Self::new(vals_m[n - 3], vals_m[n - 2], vals_m[n - 1]))
        } else {
            None
        }
    }

    fn parse_display_info_pos_line(line: &str) -> Option<Self> {
        let lower = line.to_ascii_lowercase();
        let pos_idx = lower.find("pos").or_else(|| lower.find("p0s"))?;
        let after = &line[pos_idx + 3..];
        let after = after.trim_start_matches(|c: char| c == ':' || c == ';' || c == '.' || c.is_whitespace());

        // Normalize common OCR confusions in numeric coordinate triplets
        let mut norm = String::with_capacity(after.len());
        let chars: Vec<char> = after.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_digit = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
            match c {
                '−' | '–' | '—' => norm.push('-'),
                ',' if prev_digit && next_digit => norm.push('.'),
                'O' | 'o' if prev_digit || next_digit => norm.push('0'),
                'l' | 'I' | '|' if prev_digit || next_digit => norm.push('1'),
                _ => norm.push(c),
            }
        }

        // Collapse spaces immediately after a leading minus sign (e.g. "- 16111.5km")
        let mut tokens: Vec<String> = Vec::new();
        let mut pending_minus = false;
        for raw_tok in norm.split_whitespace() {
            if raw_tok == "-" {
                pending_minus = true;
                continue;
            }
            let tok = if pending_minus {
                pending_minus = false;
                format!("-{}", raw_tok)
            } else {
                raw_tok.to_string()
            };
            tokens.push(tok);
        }

        let mut vals_m: Vec<f64> = Vec::with_capacity(3);
        let mut idx = 0usize;
        while idx < tokens.len() && vals_m.len() < 3 {
            let t = tokens[idx].trim_matches(|c: char| c == ',' || c == ';');
            // Check if unit is attached or in the next token
            if let Some(m) = Self::parse_distance_token(t) {
                vals_m.push(m);
                idx += 1;
            } else if idx + 1 < tokens.len() {
                let combined = format!("{}{}", t, tokens[idx + 1]);
                if let Some(m) = Self::parse_distance_token(&combined) {
                    vals_m.push(m);
                    idx += 2;
                    continue;
                }
                idx += 1;
            } else {
                idx += 1;
            }
        }

        if vals_m.len() == 3 {
            Some(Self::new(vals_m[0], vals_m[1], vals_m[2]))
        } else {
            None
        }
    }

    fn parse_distance_token(tok: &str) -> Option<f64> {
        let lower = tok.to_ascii_lowercase();
        let (num_str, scale) = if let Some(s) = lower
            .strip_suffix("km")
            .or_else(|| lower.strip_suffix("kn"))
            .or_else(|| lower.strip_suffix("krn"))
        {
            (s, 1_000.0)
        } else if let Some(s) = tok.strip_suffix("Mm") {
            (s, 1_000_000.0)
        } else if let Some(s) = lower.strip_suffix('m') {
            (s, 1.0)
        } else {
            return None;
        };
        let v = num_str.trim().parse::<f64>().ok()?;
        if !v.is_finite() {
            return None;
        }
        Some(v * scale)
    }

    fn parse_line(line: &str) -> Option<Self> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        let after_prefix = if let Some(idx) = lower.find("coordinates:") {
            &trimmed[idx + "coordinates:".len()..]
        } else if lower.contains("x:") && lower.contains("y:") && lower.contains("z:") {
            trimmed
        } else {
            return None;
        };

        let mut x = None;
        let mut y = None;
        let mut z = None;
        for token in after_prefix.split_whitespace() {
            let clean = token.trim_matches(|c: char| c == ',' || c == ';' || c == ')');
            if let Some(rest) = clean.strip_prefix("x:").or_else(|| clean.strip_prefix("X:")) {
                x = rest.parse::<f64>().ok().filter(|v| v.is_finite());
            } else if let Some(rest) = clean.strip_prefix("y:").or_else(|| clean.strip_prefix("Y:")) {
                y = rest.parse::<f64>().ok().filter(|v| v.is_finite());
            } else if let Some(rest) = clean.strip_prefix("z:").or_else(|| clean.strip_prefix("Z:")) {
                z = rest.parse::<f64>().ok().filter(|v| v.is_finite());
            }
        }
        Some(Self {
            x: x?,
            y: y?,
            z: z?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BreadcrumbPoint {
    pub id: u64,
    pub coord: SpaceCoord,
    pub recorded_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BreadcrumbProximity {
    pub zone: ProximityZone,
    pub nearest_id: Option<u64>,
    pub nearest_distance_km: Option<f64>,
    pub nearest_age_secs: Option<i64>,
    pub nearby_count: usize,
    pub heading_rad: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordOutcome {
    /// Identical to the most recent recorded location (< 1 m away); ignored.
    Duplicate,
    Recorded {
        point: BreadcrumbPoint,
        proximity: BreadcrumbProximity,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreadcrumbTrail {
    pub points: Vec<BreadcrumbPoint>,
    pub next_id: u64,
}

impl Default for BreadcrumbTrail {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            next_id: 1,
        }
    }
}

impl BreadcrumbTrail {
    pub const MAX_POINTS: usize = 500;
    pub const APPROACHING_MULTIPLIER: f64 = 1.6;
    const DUPLICATE_EPSILON_M: f64 = 1.0;

    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut trail) = serde_json::from_str::<Self>(&text) else {
            return Self::default();
        };
        let max_id = trail.points.iter().map(|p| p.id).max().unwrap_or(0);
        if trail.next_id <= max_id {
            trail.next_id = max_id + 1;
        }
        trail
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize breadcrumbs: {}", e))?;
        AtomicFile::write_text(path, &json, false)
            .map_err(|e| format!("Failed to save breadcrumbs: {}", e))
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.next_id = 1;
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Evaluates proximity of `coord` against existing points, optionally ignoring the immediate
    /// recent wake (points recorded within `ignore_recent_secs` of `now`) so flying forward never
    /// triggers "VISITED CLOUD - 0.0 km" on the breadcrumb you literally just dropped behind you.
    pub fn evaluate_proximity_excluding_recent(
        &self,
        coord: SpaceCoord,
        visited_radius_km: f64,
        now: DateTime<Utc>,
        ignore_recent_secs: i64,
    ) -> BreadcrumbProximity {
        let radius = visited_radius_km.max(0.5);
        let approach_radius = radius * Self::APPROACHING_MULTIPLIER;

        let mut nearest: Option<(&BreadcrumbPoint, f64)> = None;
        let mut nearby_count = 0usize;

        for pt in &self.points {
            let age_secs = (now - pt.recorded_at).num_seconds().max(0);
            if ignore_recent_secs > 0 && age_secs < ignore_recent_secs {
                continue;
            }
            let d_km = coord.distance_km(&pt.coord);
            if d_km <= radius {
                nearby_count += 1;
            }
            if nearest.map(|(_, best)| d_km < best).unwrap_or(true) {
                nearest = Some((pt, d_km));
            }
        }

        let heading_rad = self.points.last().and_then(|prev| {
            let dx = coord.x - prev.coord.x;
            let dy = coord.y - prev.coord.y;
            if dx * dx + dy * dy >= 100.0 {
                Some(dy.atan2(dx))
            } else {
                None
            }
        });

        match nearest {
            Some((pt, d_km)) => {
                let zone = if d_km <= radius {
                    ProximityZone::Visited
                } else if d_km <= approach_radius {
                    ProximityZone::Approaching
                } else {
                    ProximityZone::Clear
                };
                let age_secs = (now - pt.recorded_at).num_seconds().max(0);
                BreadcrumbProximity {
                    zone,
                    nearest_id: Some(pt.id),
                    nearest_distance_km: Some(d_km),
                    nearest_age_secs: Some(age_secs),
                    nearby_count,
                    heading_rad,
                }
            }
            None => BreadcrumbProximity {
                zone: ProximityZone::Clear,
                nearest_id: None,
                nearest_distance_km: None,
                nearest_age_secs: None,
                nearby_count: 0,
                heading_rad,
            },
        }
    }

    /// Evaluates proximity of `coord` against all existing points in the trail.
    pub fn evaluate_proximity(
        &self,
        coord: SpaceCoord,
        visited_radius_km: f64,
        now: DateTime<Utc>,
    ) -> BreadcrumbProximity {
        self.evaluate_proximity_excluding_recent(coord, visited_radius_km, now, 0)
    }

    /// Records a new `/showlocation` coordinate and returns its proximity to prior points.
    pub fn record(
        &mut self,
        coord: SpaceCoord,
        visited_radius_km: f64,
        label: Option<String>,
        now: DateTime<Utc>,
    ) -> RecordOutcome {
        if let Some(last) = self.points.last() {
            if last.coord.distance_m(&coord) < Self::DUPLICATE_EPSILON_M {
                return RecordOutcome::Duplicate;
            }
        }

        let proximity = self.evaluate_proximity(coord, visited_radius_km, now);
        let point = BreadcrumbPoint {
            id: self.next_id,
            coord,
            recorded_at: now,
            label,
        };
        self.next_id += 1;
        self.points.push(point.clone());
        if self.points.len() > Self::MAX_POINTS {
            let excess = self.points.len() - Self::MAX_POINTS;
            self.points.drain(0..excess);
        }
        RecordOutcome::Recorded { point, proximity }
    }

    /// Evaluates the latest recorded point against all earlier points in the trail.
    pub fn current_proximity(
        &self,
        visited_radius_km: f64,
        now: DateTime<Utc>,
    ) -> Option<BreadcrumbProximity> {
        let current = self.points.last()?;
        let prior = &self.points[..self.points.len() - 1];
        let radius = visited_radius_km.max(0.5);
        let approach_radius = radius * Self::APPROACHING_MULTIPLIER;

        let mut nearest: Option<(&BreadcrumbPoint, f64)> = None;
        let mut nearby_count = 0usize;
        for pt in prior {
            let d_km = current.coord.distance_km(&pt.coord);
            if d_km <= radius {
                nearby_count += 1;
            }
            if nearest.map(|(_, best)| d_km < best).unwrap_or(true) {
                nearest = Some((pt, d_km));
            }
        }

        let heading_rad = prior.last().and_then(|prev| {
            let dx = current.coord.x - prev.coord.x;
            let dy = current.coord.y - prev.coord.y;
            if dx * dx + dy * dy >= 100.0 {
                Some(dy.atan2(dx))
            } else {
                None
            }
        });

        Some(match nearest {
            Some((pt, d_km)) => {
                let zone = if d_km <= radius {
                    ProximityZone::Visited
                } else if d_km <= approach_radius {
                    ProximityZone::Approaching
                } else {
                    ProximityZone::Clear
                };
                BreadcrumbProximity {
                    zone,
                    nearest_id: Some(pt.id),
                    nearest_distance_km: Some(d_km),
                    nearest_age_secs: Some((now - pt.recorded_at).num_seconds().max(0)),
                    nearby_count,
                    heading_rad,
                }
            }
            None => BreadcrumbProximity {
                zone: ProximityZone::Clear,
                nearest_id: None,
                nearest_distance_km: None,
                nearest_age_secs: None,
                nearby_count: 0,
                heading_rad: None,
            },
        })
    }

    /// Builds the render-ready overlay state for the breadcrumb cloud & mini-map.
    ///
    /// If `live_pos` is provided (from `r_DisplayInfo` OCR), the radar centers on `live_pos`
    /// and evaluates proximity against the entire recorded trail so the color cloud glows the
    /// moment you enter a visited zone—even between 30-second breadcrumb drops.
    pub fn build_overlay_info(
        &self,
        visited_radius_km: f64,
        show_screen_cloud: bool,
        live_pos: Option<SpaceCoord>,
        now: DateTime<Utc>,
    ) -> Option<BreadcrumbOverlayInfo> {
        let radius = visited_radius_km.max(0.5);
        let (center_coord, prox, center_id, use_live) = if let Some(live) = live_pos {
            if self.points.is_empty() {
                (
                    live,
                    BreadcrumbProximity {
                        zone: ProximityZone::Clear,
                        nearest_id: None,
                        nearest_distance_km: None,
                        nearest_age_secs: None,
                        nearby_count: 0,
                        heading_rad: None,
                    },
                    0,
                    true,
                )
            } else {
                // Ignore breadcrumbs dropped within the last 90 seconds so your own immediate wake
                // doesn't trigger "VISITED CLOUD - 0.0 km" right after dropping a point.
                let p = self.evaluate_proximity_excluding_recent(live, radius, now, 90);
                (live, p, self.next_id, true)
            }
        } else {
            let current = self.points.last()?;
            let p = self.current_proximity(radius, now)?;
            (current.coord, p, current.id, false)
        };

        let mut render_points = Vec::with_capacity(self.points.len() + 1);
        let last_idx = self.points.len().saturating_sub(1);
        for (idx, pt) in self.points.iter().enumerate() {
            let dx_km = (pt.coord.x - center_coord.x) / 1000.0;
            let dy_km = (pt.coord.y - center_coord.y) / 1000.0;
            let dz_km = (pt.coord.z - center_coord.z) / 1000.0;
            // Keep points within a local 250 km radar window so jumping across the star system
            // doesn't squash the local asteroid-belt radar view.
            let planar_km = (dx_km * dx_km + dy_km * dy_km).sqrt();
            let is_current = !use_live && idx == last_idx;
            if is_current || planar_km <= 250.0 {
                let age_mins = ((now - pt.recorded_at).num_seconds().max(0) / 60) as u32;
                render_points.push(BreadcrumbRenderPoint {
                    id: pt.id,
                    dx_km,
                    dy_km,
                    dz_km,
                    age_mins,
                    is_current,
                });
            }
        }
        if use_live {
            render_points.push(BreadcrumbRenderPoint {
                id: center_id,
                dx_km: 0.0,
                dy_km: 0.0,
                dz_km: 0.0,
                age_mins: 0,
                is_current: true,
            });
        }

        let headline = match (prox.zone, prox.nearest_id, prox.nearest_distance_km) {
            (ProximityZone::Visited, Some(id), Some(dist)) => {
                let age = format_age(prox.nearest_age_secs.unwrap_or(0));
                format!("VISITED CLOUD - {:.1} km from #{} ({})", dist, id, age)
            }
            (ProximityZone::Approaching, Some(id), Some(dist)) => {
                let age = format_age(prox.nearest_age_secs.unwrap_or(0));
                format!("ENTERING VISITED - {:.1} km from #{} ({})", dist, id, age)
            }
            (_, Some(id), Some(dist)) => {
                if use_live {
                    format!("FRESH SPACE - LIVE ({:.1} km from #{})", dist, id)
                } else {
                    format!("FRESH SPACE - #{} ({:.1} km from #{})", center_id, dist, id)
                }
            }
            _ => {
                if use_live {
                    "LIVE TELEMETRY - Tracking".to_string()
                } else {
                    format!("TRAIL STARTED - Breadcrumb #{}", center_id)
                }
            }
        };

        let dz_txt = self
            .points
            .last()
            .and_then(|last| {
                if use_live {
                    Some((center_coord.z - last.coord.z) / 1000.0)
                } else {
                    self.points
                        .len()
                        .checked_sub(2)
                        .and_then(|i| self.points.get(i))
                        .map(|prev| (center_coord.z - prev.coord.z) / 1000.0)
                }
            })
            .map(|dz| format!("  |  dZ {:+.1} km", dz))
            .unwrap_or_default();

        let mode_tag = if use_live { "AUTO" } else { "MANUAL" };
        let subline = format!(
            "{} pts ({})  |  Cloud {:.0} km{}",
            self.points.len(),
            mode_tag,
            radius,
            dz_txt
        );

        Some(BreadcrumbOverlayInfo {
            zone: prox.zone,
            headline,
            subline,
            visited_radius_km: radius,
            heading_rad: prox.heading_rad,
            show_screen_cloud,
            points: render_points,
            active_waypoint: None,
        })
    }

    pub fn toast_text(point: &BreadcrumbPoint, prox: &BreadcrumbProximity) -> String {
        match (prox.zone, prox.nearest_id, prox.nearest_distance_km) {
            (ProximityZone::Visited, Some(id), Some(dist)) => {
                format!(
                    "ALREADY VISITED ({:.1} km from #{}, {})",
                    dist,
                    id,
                    format_age(prox.nearest_age_secs.unwrap_or(0))
                )
            }
            (ProximityZone::Approaching, Some(id), Some(dist)) => {
                format!("ENTERING VISITED CLOUD ({:.1} km from #{})", dist, id)
            }
            (_, Some(_), Some(dist)) => {
                format!("Breadcrumb #{} logged (nearest {:.1} km)", point.id, dist)
            }
            _ => format!("Breadcrumb #{} logged", point.id),
        }
    }
}

pub fn format_age(secs: i64) -> String {
    if secs < 60 {
        format!("{}s ago", secs.max(0))
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h {}m ago", secs / 3600, (secs % 3600) / 60)
    }
}

/// Filters noisy low-contrast `r_DisplayInfo` OCR reads so single-frame misreads never pollute
/// the breadcrumb trail, while allowing smooth sub-quantum flight and Quantum Travel jumps.
#[derive(Debug, Default)]
pub struct DisplayInfoStabilizer {
    last_accepted: Option<(SpaceCoord, DateTime<Utc>)>,
    pending: Option<(SpaceCoord, DateTime<Utc>)>,
    last_drop: Option<(SpaceCoord, DateTime<Utc>)>,
}

impl DisplayInfoStabilizer {
    /// Minimum movement in meters required before an automatic timer tick drops a new breadcrumb.
    pub const MIN_AUTO_DROP_DISTANCE_M: f64 = 250.0;

    pub fn reset(&mut self) {
        self.last_accepted = None;
        self.pending = None;
        self.last_drop = None;
    }

    pub fn note_manual_drop(&mut self, coord: SpaceCoord, now: DateTime<Utc>) {
        self.last_accepted = Some((coord, now));
        self.last_drop = Some((coord, now));
        self.pending = None;
    }

    /// Validates an OCR coordinate candidate. Returns `Some(accepted_coord)` if trustworthy.
    pub fn ingest(
        &mut self,
        coord: SpaceCoord,
        dual_line_confirmed: bool,
        now: DateTime<Utc>,
    ) -> Option<SpaceCoord> {
        if dual_line_confirmed {
            self.last_accepted = Some((coord, now));
            self.pending = None;
            return Some(coord);
        }

        if let Some((prev, prev_ts)) = self.last_accepted {
            let dt = (now - prev_ts).num_seconds().max(1) as f64;
            let max_plausible_km = (dt * 2.0).max(15.0);
            if coord.distance_km(&prev) <= max_plausible_km {
                self.last_accepted = Some((coord, now));
                self.pending = None;
                return Some(coord);
            }
        }

        // First reading or large jump (Quantum Travel vs OCR misread): require 2 consistent reads
        if let Some((pend, pend_ts)) = self.pending {
            let dt = (now - pend_ts).num_seconds().max(1) as f64;
            if dt <= 45.0 && coord.distance_km(&pend) <= (dt * 2.0).max(12.0) {
                self.last_accepted = Some((coord, now));
                self.pending = None;
                return Some(coord);
            }
        }

        self.pending = Some((coord, now));
        None
    }

    /// Returns `true` if enough time has elapsed (and ship moved >= 250 m) to drop an automatic breadcrumb.
    pub fn should_auto_drop(
        &mut self,
        coord: SpaceCoord,
        interval_secs: i64,
        now: DateTime<Utc>,
    ) -> bool {
        let interval = interval_secs.clamp(5, 300);
        match self.last_drop {
            None => {
                self.last_drop = Some((coord, now));
                true
            }
            Some((prev, prev_ts)) => {
                let elapsed = (now - prev_ts).num_seconds();
                if elapsed >= interval && coord.distance_m(&prev) >= Self::MIN_AUTO_DROP_DISTANCE_M
                {
                    self.last_drop = Some((coord, now));
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// Filters noisy single-frame OCR glitches on planetary/moon local body coordinates
/// (`Zone: pyr06 Pos: ...`) while accepting smooth sub-quantum movement and zone transitions.
#[derive(Debug, Default)]
pub struct LocalBodyStabilizer {
    last_accepted: Option<(String, String, SpaceCoord, DateTime<Utc>)>,
    pending: Option<(String, String, SpaceCoord, DateTime<Utc>)>,
}

impl LocalBodyStabilizer {
    pub fn reset(&mut self) {
        self.last_accepted = None;
        self.pending = None;
    }

    /// Validates a planetary/moon body OCR reading `(display_name, norm_key, coord)`.
    pub fn ingest(
        &mut self,
        display_name: String,
        norm_key: String,
        coord: SpaceCoord,
        now: DateTime<Utc>,
    ) -> Option<(String, String, SpaceCoord)> {
        if let Some((_, prev_key, prev_coord, prev_ts)) = &self.last_accepted {
            let dt = (now - *prev_ts).num_seconds().max(1) as f64;
            if *prev_key == norm_key && dt <= 60.0 {
                let max_plausible_km = (dt * 2.5).max(15.0);
                if coord.distance_km(prev_coord) <= max_plausible_km {
                    let out = (display_name, norm_key, coord);
                    self.last_accepted = Some((out.0.clone(), out.1.clone(), out.2, now));
                    self.pending = None;
                    return Some(out);
                }
            }
        }

        // First reading in a zone or large jump: confirm with 2 consistent frames
        if let Some((_, pend_key, pend_coord, pend_ts)) = &self.pending {
            let dt = (now - *pend_ts).num_seconds().max(1) as f64;
            if *pend_key == norm_key && dt <= 20.0 && coord.distance_km(pend_coord) <= (dt * 2.5).max(15.0)
            {
                let out = (display_name, norm_key, coord);
                self.last_accepted = Some((out.0.clone(), out.1.clone(), out.2, now));
                self.pending = None;
                return Some(out);
            }
        }

        self.pending = Some((display_name, norm_key, coord, now));
        None
    }
}

