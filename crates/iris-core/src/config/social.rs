use crate::config::settings::quote;
use crate::config::{AtomicFile, ImportMode};
use crate::lookup::RsiNames;
use crate::models::{ColorMatch, MatchKind, OrgMembership, OverlayColor};
use chrono::Local;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerEntry {
    pub handle: String,
    pub color: OverlayColor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgEntry {
    pub sid: String,
    pub priority: i32,
    pub color: OverlayColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttributeKind {
    Archetype,
    Activity,
    Redacted,
    NoOrg,
    HiddenMainOrg,
    NoMatch,
}

impl AttributeKind {
    pub const fn to_toml_str(&self) -> &'static str {
        match self {
            AttributeKind::Archetype => "archetype",
            AttributeKind::Activity => "activity",
            AttributeKind::Redacted => "redacted",
            AttributeKind::NoOrg => "no_org",
            AttributeKind::HiddenMainOrg => "hidden_main_org",
            AttributeKind::NoMatch => "no_match",
        }
    }

    pub fn from_toml_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "archetype" => Some(AttributeKind::Archetype),
            "activity" => Some(AttributeKind::Activity),
            "redacted" => Some(AttributeKind::Redacted),
            "no_org" => Some(AttributeKind::NoOrg),
            "hidden_main_org" => Some(AttributeKind::HiddenMainOrg),
            "no_match" => Some(AttributeKind::NoMatch),
            _ => None,
        }
    }

    pub const fn display_name(&self) -> &'static str {
        match self {
            AttributeKind::Archetype => "Org-Archetype",
            AttributeKind::Activity => "Org-Activity",
            AttributeKind::Redacted => "Redacted",
            AttributeKind::NoOrg => "No Org",
            AttributeKind::HiddenMainOrg => "Hidden Main Org",
            AttributeKind::NoMatch => "No Match",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttributeTarget {
    pub kind: AttributeKind,
    pub value: Option<String>,
}

impl AttributeTarget {
    pub fn new(kind: AttributeKind, value: Option<String>) -> Self {
        Self { kind, value }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgAttributeRule {
    pub kind: AttributeKind,
    pub value: Option<String>,
    pub priority: i32,
    pub color: OverlayColor,
}

impl OrgAttributeRule {
    pub const NO_MATCH_PRIORITY: i32 = 10;

    pub fn default_no_match() -> Self {
        Self {
            kind: AttributeKind::NoMatch,
            value: None,
            priority: Self::NO_MATCH_PRIORITY,
            color: OverlayColor::Grey,
        }
    }

    pub fn rule_key(&self) -> String {
        match self.kind {
            AttributeKind::Archetype | AttributeKind::Activity => format!(
                "{}:{}",
                self.kind.to_toml_str(),
                self.value
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
            ),
            other => other.to_toml_str().to_string(),
        }
    }
}

pub struct OrgVocabulary;

impl OrgVocabulary {
    pub const ARCHETYPES: [&'static str; 6] = [
        "Club",
        "Corporation",
        "Faith",
        "Organization",
        "PMC",
        "Syndicate",
    ];

    pub const ACTIVITIES: [&'static str; 15] = [
        "Bounty Hunting",
        "Data Running",
        "Engineering",
        "Exploration",
        "Freelancing",
        "Infiltration",
        "Medical",
        "Piracy",
        "Resources",
        "Scouting",
        "Security",
        "Smuggling",
        "Social",
        "Trading",
        "Transport",
    ];

    pub const COMMITMENTS: [&'static str; 3] = ["Casual", "Regular", "Hardcore"];

    pub fn match_archetype(text: &str) -> Option<&'static str> {
        let t = text.trim();
        Self::ARCHETYPES
            .iter()
            .copied()
            .find(|a| a.eq_ignore_ascii_case(t))
    }

    pub fn match_activity(text: &str) -> Option<&'static str> {
        let t = text.trim();
        Self::ACTIVITIES
            .iter()
            .copied()
            .find(|a| a.eq_ignore_ascii_case(t))
    }

    pub fn match_commitment(text: &str) -> Option<&'static str> {
        let t = text.trim();
        Self::COMMITMENTS
            .iter()
            .copied()
            .find(|c| c.eq_ignore_ascii_case(t))
    }

    pub fn is_exclusive(text: &str) -> bool {
        let t = text.trim();
        t.eq_ignore_ascii_case("Exclusive") || t.eq_ignore_ascii_case("Roleplay")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialConfig {
    pub name: Option<String>,
    pub iris_version: Option<String>,
    pub orgs_main_org_only: bool,
    pub org_rules_main_org_only: bool,
    pub players: Vec<PlayerEntry>,
    pub orgs: Vec<OrgEntry>,
    pub org_rules: Vec<OrgAttributeRule>,
}

impl Default for SocialConfig {
    fn default() -> Self {
        Self {
            name: None,
            iris_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            orgs_main_org_only: false,
            org_rules_main_org_only: false,
            players: Vec::new(),
            orgs: Vec::new(),
            org_rules: vec![OrgAttributeRule::default_no_match()],
        }
    }
}

impl SocialConfig {
    pub fn no_match_color(&self) -> OverlayColor {
        self.org_rules
            .iter()
            .find(|r| r.kind == AttributeKind::NoMatch)
            .map(|r| r.color)
            .unwrap_or(OverlayColor::Grey)
    }

    pub fn set_no_match_color(&mut self, color: OverlayColor) {
        if let Some(r) = self
            .org_rules
            .iter_mut()
            .find(|r| r.kind == AttributeKind::NoMatch)
        {
            r.color = color;
            r.priority = OrgAttributeRule::NO_MATCH_PRIORITY;
        } else {
            self.org_rules.push(OrgAttributeRule {
                kind: AttributeKind::NoMatch,
                value: None,
                priority: OrgAttributeRule::NO_MATCH_PRIORITY,
                color,
            });
        }
    }

    pub fn ensure_normalized(&mut self) {
        if !self
            .org_rules
            .iter()
            .any(|r| r.kind == AttributeKind::NoMatch)
        {
            self.org_rules.push(OrgAttributeRule::default_no_match());
        }
        ColorRanks::normalize_orgs(&mut self.orgs);
        ColorRanks::normalize_rules(&mut self.org_rules);
    }

    pub fn upsert_player(&mut self, handle: &str, color: OverlayColor) {
        if let Some(existing) = self
            .players
            .iter_mut()
            .find(|p| p.handle.eq_ignore_ascii_case(handle))
        {
            existing.color = color;
        } else {
            self.players.push(PlayerEntry {
                handle: handle.to_string(),
                color,
            });
        }
    }

    pub fn remove_player(&mut self, handle: &str) -> bool {
        let before = self.players.len();
        self.players
            .retain(|p| !p.handle.eq_ignore_ascii_case(handle));
        self.players.len() != before
    }

    pub fn upsert_org(&mut self, sid: &str, color: OverlayColor) {
        let rank = ColorRanks::rank_for_org_color(&self.orgs, color);
        if let Some(existing) = self
            .orgs
            .iter_mut()
            .find(|o| o.sid.eq_ignore_ascii_case(sid))
        {
            existing.color = color;
            existing.priority = rank;
        } else {
            self.orgs.push(OrgEntry {
                sid: sid.to_string(),
                priority: rank,
                color,
            });
        }
        ColorRanks::normalize_orgs(&mut self.orgs);
    }

    pub fn remove_org(&mut self, sid: &str) -> bool {
        let before = self.orgs.len();
        self.orgs.retain(|o| !o.sid.eq_ignore_ascii_case(sid));
        ColorRanks::normalize_orgs(&mut self.orgs);
        self.orgs.len() != before
    }

    pub fn upsert_attribute_rule(&mut self, target: &AttributeTarget, color: OverlayColor) {
        if target.kind == AttributeKind::NoMatch {
            self.set_no_match_color(color);
            return;
        }
        let rank = ColorRanks::rank_for_rule_color(&self.org_rules, color);
        if let Some(existing) = self.org_rules.iter_mut().find(|r| {
            r.kind == target.kind
                && r.value.as_deref().unwrap_or_default().eq_ignore_ascii_case(
                    target.value.as_deref().unwrap_or_default(),
                )
        }) {
            existing.color = color;
            existing.priority = rank;
        } else {
            let insert_pos = self
                .org_rules
                .iter()
                .position(|r| r.kind == AttributeKind::NoMatch)
                .unwrap_or(self.org_rules.len());
            self.org_rules.insert(
                insert_pos,
                OrgAttributeRule {
                    kind: target.kind,
                    value: target.value.clone(),
                    priority: rank,
                    color,
                },
            );
        }
        ColorRanks::normalize_rules(&mut self.org_rules);
    }

    pub fn remove_attribute_rule(&mut self, target: &AttributeTarget) -> bool {
        if target.kind == AttributeKind::NoMatch {
            return false;
        }
        let before = self.org_rules.len();
        self.org_rules.retain(|r| {
            !(r.kind == target.kind
                && r.value.as_deref().unwrap_or_default().eq_ignore_ascii_case(
                    target.value.as_deref().unwrap_or_default(),
                ))
        });
        ColorRanks::normalize_rules(&mut self.org_rules);
        self.org_rules.len() != before
    }
}

/// Maintains unique 1..=9 Color Ranks across Org entries and Org-Attribute rules.
pub struct ColorRanks;

impl ColorRanks {
    pub fn active_org_colors(orgs: &[OrgEntry]) -> Vec<OverlayColor> {
        let mut by_color: HashMap<OverlayColor, i32> = HashMap::new();
        for o in orgs {
            by_color
                .entry(o.color)
                .and_modify(|p| *p = (*p).min(o.priority))
                .or_insert(o.priority);
        }
        let mut list: Vec<(OverlayColor, i32)> = by_color.into_iter().collect();
        list.sort_by_key(|(c, p)| (*p, *c));
        list.into_iter().map(|(c, _)| c).collect()
    }

    pub fn active_rule_colors(rules: &[OrgAttributeRule]) -> Vec<OverlayColor> {
        let mut by_color: HashMap<OverlayColor, i32> = HashMap::new();
        for r in rules.iter().filter(|r| r.kind != AttributeKind::NoMatch) {
            by_color
                .entry(r.color)
                .and_modify(|p| *p = (*p).min(r.priority))
                .or_insert(r.priority);
        }
        let mut list: Vec<(OverlayColor, i32)> = by_color.into_iter().collect();
        list.sort_by_key(|(c, p)| (*p, *c));
        list.into_iter().map(|(c, _)| c).collect()
    }

    pub fn rank_for_org_color(orgs: &[OrgEntry], color: OverlayColor) -> i32 {
        let active = Self::active_org_colors(orgs);
        if let Some(pos) = active.iter().position(|&c| c == color) {
            (pos + 1) as i32
        } else {
            (active.len() + 1).clamp(1, 9) as i32
        }
    }

    pub fn rank_for_rule_color(rules: &[OrgAttributeRule], color: OverlayColor) -> i32 {
        let active = Self::active_rule_colors(rules);
        if let Some(pos) = active.iter().position(|&c| c == color) {
            (pos + 1) as i32
        } else {
            (active.len() + 1).clamp(1, 9) as i32
        }
    }

    pub fn normalize_orgs(orgs: &mut [OrgEntry]) {
        let active = Self::active_org_colors(orgs);
        for o in orgs.iter_mut() {
            if let Some(pos) = active.iter().position(|&c| c == o.color) {
                o.priority = (pos + 1) as i32;
            }
        }
    }

    pub fn normalize_rules(rules: &mut [OrgAttributeRule]) {
        let active = Self::active_rule_colors(rules);
        for r in rules.iter_mut() {
            if r.kind == AttributeKind::NoMatch {
                r.priority = OrgAttributeRule::NO_MATCH_PRIORITY;
            } else if let Some(pos) = active.iter().position(|&c| c == r.color) {
                r.priority = (pos + 1) as i32;
            }
        }
    }

    pub fn move_org_color(orgs: &mut [OrgEntry], color: OverlayColor, delta: isize) {
        let mut active = Self::active_org_colors(orgs);
        if let Some(idx) = active.iter().position(|&c| c == color) {
            let next = (idx as isize + delta).clamp(0, active.len() as isize - 1) as usize;
            active.swap(idx, next);
            for o in orgs.iter_mut() {
                if let Some(pos) = active.iter().position(|&c| c == o.color) {
                    o.priority = (pos + 1) as i32;
                }
            }
        }
    }

    pub fn move_rule_color(rules: &mut [OrgAttributeRule], color: OverlayColor, delta: isize) {
        let mut active = Self::active_rule_colors(rules);
        if let Some(idx) = active.iter().position(|&c| c == color) {
            let next = (idx as isize + delta).clamp(0, active.len() as isize - 1) as usize;
            active.swap(idx, next);
            for r in rules.iter_mut() {
                if r.kind != AttributeKind::NoMatch {
                    if let Some(pos) = active.iter().position(|&c| c == r.color) {
                        r.priority = (pos + 1) as i32;
                    }
                }
            }
        }
    }
}

/// Evaluates IRIS's 4-tier color resolution (`PlayerList -> OrgList -> OrgAttribute -> NoMatch`).
pub struct ColorResolver;

impl ColorResolver {
    pub fn resolve(config: &SocialConfig, handle: &str, orgs: &[OrgMembership]) -> ColorMatch {
        // Tier 1: PlayerList
        if let Some(p) = config
            .players
            .iter()
            .find(|p| p.handle.eq_ignore_ascii_case(handle))
        {
            return ColorMatch {
                color: p.color,
                kind: MatchKind::PlayerList,
                deciding_org_sid: None,
                deciding_attribute: None,
            };
        }

        // Tier 2: OrgList (lowest priority integer wins; Main org evaluated before affiliates on tie)
        let mut best_org: Option<(&OrgEntry, &OrgMembership)> = None;
        for member in orgs
            .iter()
            .filter(|o| !config.orgs_main_org_only || o.is_main)
        {
            let Some(sid) = member.sid.as_deref() else {
                continue;
            };
            if let Some(entry) = config.orgs.iter().find(|e| e.sid.eq_ignore_ascii_case(sid)) {
                let replace = match best_org {
                    None => true,
                    Some((prev_e, prev_m)) => {
                        entry.priority < prev_e.priority
                            || (entry.priority == prev_e.priority && member.is_main && !prev_m.is_main)
                    }
                };
                if replace {
                    best_org = Some((entry, member));
                }
            }
        }
        if let Some((entry, member)) = best_org {
            return ColorMatch {
                color: entry.color,
                kind: MatchKind::OrgList,
                deciding_org_sid: member.sid.clone(),
                deciding_attribute: None,
            };
        }

        // Tier 3: OrgAttribute
        let checked_orgs: Vec<&OrgMembership> = if config.org_rules_main_org_only {
            orgs.iter().filter(|o| o.is_main).collect()
        } else {
            let mut ordered: Vec<&OrgMembership> = orgs.iter().filter(|o| o.is_main).collect();
            ordered.extend(orgs.iter().filter(|o| !o.is_main));
            ordered
        };

        let mut best_rule: Option<(&OrgAttributeRule, Option<String>, String)> = None;
        for rule in config
            .org_rules
            .iter()
            .filter(|r| r.kind != AttributeKind::NoMatch)
        {
            let hit: Option<(Option<String>, String)> = match rule.kind {
                AttributeKind::NoOrg => {
                    if orgs.is_empty() {
                        Some((None, "No Org".to_string()))
                    } else {
                        None
                    }
                }
                AttributeKind::Redacted => checked_orgs
                    .iter()
                    .find(|o| o.is_redacted())
                    .map(|_| (None, "REDACTED".to_string())),
                AttributeKind::HiddenMainOrg => {
                    if !orgs.is_empty() && orgs.iter().any(|o| o.is_main && o.is_redacted()) {
                        Some((None, "Hidden Main Org".to_string()))
                    } else if !orgs.is_empty() && !orgs.iter().any(|o| o.is_main) {
                        Some((None, "Hidden Main Org".to_string()))
                    } else {
                        None
                    }
                }
                AttributeKind::Archetype => {
                    let target = rule.value.as_deref().unwrap_or_default().trim();
                    checked_orgs.iter().find_map(|o| {
                        let arch = o.archetype.as_deref()?;
                        if arch.eq_ignore_ascii_case(target) {
                            Some((o.sid.clone(), target.to_string()))
                        } else {
                            None
                        }
                    })
                }
                AttributeKind::Activity => {
                    let target = rule.value.as_deref().unwrap_or_default().trim();
                    checked_orgs.iter().find_map(|o| {
                        if o.activities.iter().any(|a| a.eq_ignore_ascii_case(target)) {
                            Some((o.sid.clone(), target.to_string()))
                        } else {
                            None
                        }
                    })
                }
                AttributeKind::NoMatch => None,
            };

            if let Some((sid, attr_label)) = hit {
                let replace = match &best_rule {
                    None => true,
                    Some((prev, _, _)) => rule.priority < prev.priority,
                };
                if replace {
                    best_rule = Some((rule, sid, attr_label));
                }
            }
        }

        if let Some((rule, sid, attr_label)) = best_rule {
            return ColorMatch {
                color: rule.color,
                kind: MatchKind::OrgAttribute,
                deciding_org_sid: sid,
                deciding_attribute: Some(attr_label),
            };
        }

        // Tier 4: NoMatch
        ColorMatch::no_match(config.no_match_color())
    }

    pub fn resolve_unknown(config: &SocialConfig, handle: &str) -> ColorMatch {
        if let Some(p) = config
            .players
            .iter()
            .find(|p| p.handle.eq_ignore_ascii_case(handle))
        {
            return ColorMatch {
                color: p.color,
                kind: MatchKind::PlayerList,
                deciding_org_sid: None,
                deciding_attribute: None,
            };
        }
        ColorMatch::no_match(config.no_match_color())
    }
}

#[derive(Debug, Clone)]
pub struct ConfigLoadResult {
    pub config: Option<SocialConfig>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ConfigLoadResult {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty() && self.config.is_some()
    }
}

pub struct SocialConfigToml;

impl SocialConfigToml {
    pub const FORMAT_ID: &'static str = "sc-social-overlay";
    pub const FORMAT_VERSION: i64 = 1;
    pub const MAX_FILE_BYTES: usize = 1024 * 1024;

    pub fn write(config: &SocialConfig) -> String {
        let mut sb = String::new();
        sb.push_str(
            "# IRIS social config (TOML format). Shareable: contains no personal settings.\n",
        );
        sb.push_str(&format!("format = {}\n", quote(Self::FORMAT_ID)));
        sb.push_str(&format!("format_version = {}\n", Self::FORMAT_VERSION));
        if let Some(ver) = config.iris_version.as_deref() {
            if !ver.trim().is_empty() {
                sb.push_str(&format!("iris_version = {}\n", quote(ver.trim())));
            }
        }
        if let Some(name) = config.name.as_deref() {
            if !name.trim().is_empty() {
                sb.push_str(&format!("name = {}\n", quote(name)));
            }
        }
        sb.push_str(&format!(
            "orgs_main_org_only = {}\n",
            if config.orgs_main_org_only {
                "true"
            } else {
                "false"
            }
        ));
        sb.push_str(&format!(
            "org_rules_main_org_only = {}\n",
            if config.org_rules_main_org_only {
                "true"
            } else {
                "false"
            }
        ));

        for p in &config.players {
            sb.push_str("\n[[players]]\n");
            sb.push_str(&format!("handle = {}\n", quote(&p.handle)));
            sb.push_str(&format!("color = {}\n", quote(p.color.as_str())));
        }

        for o in &config.orgs {
            sb.push_str("\n[[orgs]]\n");
            sb.push_str(&format!("sid = {}\n", quote(&o.sid)));
            sb.push_str(&format!("priority = {}\n", o.priority.clamp(1, 9)));
            sb.push_str(&format!("color = {}\n", quote(o.color.as_str())));
        }

        let no_match = config
            .org_rules
            .iter()
            .find(|r| r.kind == AttributeKind::NoMatch)
            .cloned()
            .unwrap_or_else(OrgAttributeRule::default_no_match);

        for r in config
            .org_rules
            .iter()
            .filter(|r| r.kind != AttributeKind::NoMatch)
            .chain(std::iter::once(&no_match))
        {
            sb.push_str("\n[[org_rules]]\n");
            sb.push_str(&format!("kind = {}\n", quote(r.kind.to_toml_str())));
            if let Some(val) = r.value.as_deref() {
                if r.kind != AttributeKind::NoMatch {
                    sb.push_str(&format!("value = {}\n", quote(val)));
                }
            }
            let prio = if r.kind == AttributeKind::NoMatch {
                OrgAttributeRule::NO_MATCH_PRIORITY
            } else {
                r.priority.clamp(1, 9)
            };
            sb.push_str(&format!("priority = {}\n", prio));
            sb.push_str(&format!("color = {}\n", quote(r.color.as_str())));
        }

        sb
    }

    pub fn read(text: &str) -> ConfigLoadResult {
        if text.len() > Self::MAX_FILE_BYTES {
            return ConfigLoadResult {
                config: None,
                errors: vec!["Config file exceeds 1 MB limit.".to_string()],
                warnings: Vec::new(),
            };
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let root: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(err) => {
                return ConfigLoadResult {
                    config: None,
                    errors: vec![format!("TOML syntax: {}", err)],
                    warnings: Vec::new(),
                };
            }
        };

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut config = SocialConfig {
            name: None,
            iris_version: None,
            orgs_main_org_only: false,
            org_rules_main_org_only: false,
            players: Vec::new(),
            orgs: Vec::new(),
            org_rules: Vec::new(),
        };

        match root.get("format").and_then(|v| v.as_str()) {
            Some(f) if f == Self::FORMAT_ID => {}
            _ => errors.push(format!(
                "format: must be \"{}\" (this is not a Social Overlay config file).",
                Self::FORMAT_ID
            )),
        }
        match root.get("format_version").and_then(|v| v.as_integer()) {
            Some(v) if (1..=Self::FORMAT_VERSION).contains(&v) => {}
            Some(v) => errors.push(format!("format_version: {} is not supported.", v)),
            None => errors.push("format_version: missing or not a whole number.".to_string()),
        }
        if let Some(name_val) = root.get("name") {
            if let Some(n) = name_val.as_str() {
                config.name = Some(n.to_string());
            } else {
                errors.push("name: must be text.".to_string());
            }
        }
        if let Some(iv) = root.get("iris_version").and_then(|v| v.as_str()) {
            config.iris_version = Some(iv.trim().to_string());
        }
        if let Some(b) = root.get("orgs_main_org_only") {
            match b.as_bool() {
                Some(val) => config.orgs_main_org_only = val,
                None => errors.push("orgs_main_org_only: must be true or false.".to_string()),
            }
        }
        if let Some(b) = root.get("org_rules_main_org_only") {
            match b.as_bool() {
                Some(val) => config.org_rules_main_org_only = val,
                None => errors.push("org_rules_main_org_only: must be true or false.".to_string()),
            }
        }

        let mut seen_handles = HashSet::new();
        if let Some(players_val) = root.get("players") {
            match players_val.as_array() {
                Some(arr) => {
                    for (idx, item) in arr.iter().enumerate() {
                        let path = format!("players[{}]", idx + 1);
                        let Some(t) = item.as_table() else {
                            errors.push(format!("{}: must be a table.", path));
                            continue;
                        };
                        let handle = t
                            .get("handle")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        let color = t
                            .get("color")
                            .and_then(|v| v.as_str())
                            .map(|s| s.parse::<OverlayColor>());
                        if handle.is_none() {
                            errors.push(format!("{}.handle: missing.", path));
                        }
                        if color.is_none() {
                            errors.push(format!("{}.color: missing.", path));
                        }
                        if let (Some(h), Some(c_res)) = (handle, color) {
                            match c_res {
                                Ok(c) => {
                                    if !RsiNames::is_valid_handle(h) {
                                        errors.push(format!(
                                            "{}.handle: \"{}\" is not a valid handle (4 to 64 characters: letters, digits, - and _ only).",
                                            path, h
                                        ));
                                    } else if !seen_handles.insert(h.to_ascii_lowercase()) {
                                        errors.push(format!(
                                            "{}.handle: \"{}\" is listed more than once.",
                                            path, h
                                        ));
                                    } else {
                                        config.players.push(PlayerEntry {
                                            handle: h.to_string(),
                                            color: c,
                                        });
                                    }
                                }
                                Err(msg) => errors.push(format!("{}.color: {}", path, msg)),
                            }
                        }
                    }
                }
                None => errors.push("players: must be a list of [[players]] entries.".to_string()),
            }
        }

        let mut seen_sids = HashSet::new();
        if let Some(orgs_val) = root.get("orgs") {
            match orgs_val.as_array() {
                Some(arr) => {
                    for (idx, item) in arr.iter().enumerate() {
                        let path = format!("orgs[{}]", idx + 1);
                        let Some(t) = item.as_table() else {
                            errors.push(format!("{}: must be a table.", path));
                            continue;
                        };
                        let sid = t
                            .get("sid")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        let prio = t.get("priority").and_then(|v| v.as_integer());
                        let color = t
                            .get("color")
                            .and_then(|v| v.as_str())
                            .map(|s| s.parse::<OverlayColor>());
                        if sid.is_none() {
                            errors.push(format!("{}.sid: missing.", path));
                        }
                        let valid_prio = match prio {
                            Some(p) if (1..=9).contains(&p) => Some(p as i32),
                            Some(p) => {
                                errors.push(format!(
                                    "{}.priority: {} must be between 1 and 9.",
                                    path, p
                                ));
                                None
                            }
                            None => {
                                errors.push(format!("{}.priority: missing.", path));
                                None
                            }
                        };
                        if color.is_none() {
                            errors.push(format!("{}.color: missing.", path));
                        }
                        if let (Some(s), Some(p), Some(c_res)) = (sid, valid_prio, color) {
                            match c_res {
                                Ok(c) => {
                                    if !RsiNames::is_valid_sid(s) {
                                        errors.push(format!(
                                            "{}.sid: \"{}\" is not a valid org SID.",
                                            path, s
                                        ));
                                    } else if !seen_sids.insert(s.to_ascii_lowercase()) {
                                        errors.push(format!(
                                            "{}.sid: \"{}\" is listed more than once.",
                                            path, s
                                        ));
                                    } else {
                                        config.orgs.push(OrgEntry {
                                            sid: s.to_string(),
                                            priority: p,
                                            color: c,
                                        });
                                    }
                                }
                                Err(msg) => errors.push(format!("{}.color: {}", path, msg)),
                            }
                        }
                    }
                }
                None => errors.push("orgs: must be a list of [[orgs]] entries.".to_string()),
            }
        }

        let mut no_match: Option<OrgAttributeRule> = None;
        let mut rule_at: HashMap<String, usize> = HashMap::new();
        if let Some(rules_val) = root.get("org_rules") {
            match rules_val.as_array() {
                Some(arr) => {
                    for (idx, item) in arr.iter().enumerate() {
                        let path = format!("org_rules[{}]", idx + 1);
                        let Some(t) = item.as_table() else {
                            errors.push(format!("{}: must be a table.", path));
                            continue;
                        };
                        let Some(kind_str) = t.get("kind").and_then(|v| v.as_str()) else {
                            errors.push(format!("{}.kind: missing.", path));
                            continue;
                        };
                        let Some(kind) = AttributeKind::from_toml_str(kind_str) else {
                            errors.push(format!(
                                "{}.kind: \"{}\" must be one of archetype, activity, redacted, no_org, hidden_main_org, no_match.",
                                path, kind_str
                            ));
                            continue;
                        };
                        let Some(c_str) = t.get("color").and_then(|v| v.as_str()) else {
                            errors.push(format!("{}.color: missing.", path));
                            continue;
                        };
                        let color = match c_str.parse::<OverlayColor>() {
                            Ok(c) => c,
                            Err(msg) => {
                                errors.push(format!("{}.color: {}", path, msg));
                                continue;
                            }
                        };

                        if kind == AttributeKind::NoMatch {
                            if no_match.is_some() {
                                warnings.push(format!("{}: duplicate no_match ignored.", path));
                            } else {
                                no_match = Some(OrgAttributeRule {
                                    kind: AttributeKind::NoMatch,
                                    value: None,
                                    priority: OrgAttributeRule::NO_MATCH_PRIORITY,
                                    color,
                                });
                            }
                            continue;
                        }

                        let prio = match t.get("priority").and_then(|v| v.as_integer()) {
                            Some(p) if (1..=9).contains(&p) => p as i32,
                            Some(p) => {
                                errors.push(format!(
                                    "{}.priority: {} must be between 1 and 9.",
                                    path, p
                                ));
                                continue;
                            }
                            None => {
                                errors.push(format!("{}.priority: missing.", path));
                                continue;
                            }
                        };

                        let needs_val =
                            matches!(kind, AttributeKind::Archetype | AttributeKind::Activity);
                        let value = t
                            .get("value")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        if needs_val && value.is_none() {
                            errors.push(format!("{}.value: required for kind \"{}\".", path, kind_str));
                            continue;
                        }
                        if let Some(ref v) = value {
                            if kind == AttributeKind::Archetype
                                && OrgVocabulary::match_archetype(v).is_none()
                            {
                                errors.push(format!("{}.value: unknown archetype \"{}\".", path, v));
                                continue;
                            }
                            if kind == AttributeKind::Activity
                                && OrgVocabulary::match_activity(v).is_none()
                            {
                                errors.push(format!("{}.value: unknown activity \"{}\".", path, v));
                                continue;
                            }
                        }

                        let canonical_val = match (kind, value.as_deref()) {
                            (AttributeKind::Archetype, Some(v)) => {
                                OrgVocabulary::match_archetype(v).map(|s| s.to_string())
                            }
                            (AttributeKind::Activity, Some(v)) => {
                                OrgVocabulary::match_activity(v).map(|s| s.to_string())
                            }
                            _ => None,
                        };
                        let rule = OrgAttributeRule {
                            kind,
                            value: canonical_val,
                            priority: prio,
                            color,
                        };
                        let key = rule.rule_key();
                        if let Some(&prev_idx) = rule_at.get(&key) {
                            if rule.priority < config.org_rules[prev_idx].priority {
                                config.org_rules[prev_idx] = rule;
                            }
                        } else {
                            rule_at.insert(key, config.org_rules.len());
                            config.org_rules.push(rule);
                        }
                    }
                }
                None => {
                    errors.push("org_rules: must be a list of [[org_rules]] entries.".to_string())
                }
            }
        }

        config
            .org_rules
            .push(no_match.unwrap_or_else(OrgAttributeRule::default_no_match));

        ConfigLoadResult {
            config: if errors.is_empty() {
                Some(config)
            } else {
                None
            },
            errors,
            warnings,
        }
    }

    pub fn load(path: &Path) -> ConfigLoadResult {
        if !path.exists() {
            return ConfigLoadResult {
                config: Some(SocialConfig::default()),
                errors: Vec::new(),
                warnings: Vec::new(),
            };
        }
        match fs::read_to_string(path) {
            Ok(text) => Self::read(&text),
            Err(err) => ConfigLoadResult {
                config: None,
                errors: vec![format!("Could not read {}: {}", path.display(), err)],
                warnings: Vec::new(),
            },
        }
    }

    pub fn save(path: &Path, config: &SocialConfig) -> Result<(), String> {
        let text = Self::write(config);
        AtomicFile::write_text(path, &text, true)
            .map_err(|e| format!("Could not save social-config.txt: {}", e))
    }
}

pub struct SocialConfigMerger;

impl SocialConfigMerger {
    pub fn apply(local: &SocialConfig, imported: &SocialConfig, mode: ImportMode) -> SocialConfig {
        match mode {
            ImportMode::BackupExisting => {
                let mut out = imported.clone();
                out.ensure_normalized();
                out
            }
            ImportMode::Merge => Self::merge(local, imported, false),
            ImportMode::MergeImportedWins => Self::merge(local, imported, true),
        }
    }

    pub fn merge(
        local: &SocialConfig,
        imported: &SocialConfig,
        imported_wins: bool,
    ) -> SocialConfig {
        let mut out = if imported_wins {
            imported.clone()
        } else {
            local.clone()
        };

        // Players
        let mut players = local.players.clone();
        for imp in &imported.players {
            if let Some(ex) = players
                .iter_mut()
                .find(|p| p.handle.eq_ignore_ascii_case(&imp.handle))
            {
                if imported_wins {
                    *ex = imp.clone();
                }
            } else {
                players.push(imp.clone());
            }
        }
        out.players = players;

        // Orgs
        let mut orgs = local.orgs.clone();
        for imp in &imported.orgs {
            if let Some(ex) = orgs
                .iter_mut()
                .find(|o| o.sid.eq_ignore_ascii_case(&imp.sid))
            {
                if imported_wins {
                    *ex = imp.clone();
                }
            } else {
                orgs.push(imp.clone());
            }
        }
        out.orgs = orgs;

        // Rules
        let mut rules: Vec<OrgAttributeRule> = local
            .org_rules
            .iter()
            .filter(|r| r.kind != AttributeKind::NoMatch)
            .cloned()
            .collect();
        for imp in imported
            .org_rules
            .iter()
            .filter(|r| r.kind != AttributeKind::NoMatch)
        {
            let key = imp.rule_key();
            if let Some(ex) = rules.iter_mut().find(|r| r.rule_key() == key) {
                if imported_wins {
                    *ex = imp.clone();
                }
            } else {
                rules.push(imp.clone());
            }
        }
        let no_match_color = if imported_wins {
            imported.no_match_color()
        } else {
            local.no_match_color()
        };
        rules.push(OrgAttributeRule {
            kind: AttributeKind::NoMatch,
            value: None,
            priority: OrgAttributeRule::NO_MATCH_PRIORITY,
            color: no_match_color,
        });
        out.org_rules = rules;
        out.ensure_normalized();
        out
    }

    pub fn backup_existing(path: &Path) -> std::io::Result<Option<std::path::PathBuf>> {
        if !path.exists() {
            return Ok(None);
        }
        let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut candidate = parent.join(format!("social-config.backup-{}.txt", stamp));
        let mut suffix = 2;
        while candidate.exists() {
            candidate = parent.join(format!("social-config.backup-{}-{}.txt", stamp, suffix));
            suffix += 1;
        }
        fs::copy(path, &candidate)?;
        Ok(Some(candidate))
    }
}

pub struct HostedConfigUrl;

impl HostedConfigUrl {
    pub const MAX_LEN: usize = 2048;

    pub fn check(raw: &str) -> Result<String, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("URL is empty.".to_string());
        }
        if trimmed.len() > Self::MAX_LEN {
            return Err("URL is longer than 2048 characters.".to_string());
        }
        let parsed = reqwest::Url::parse(trimmed)
            .map_err(|e| format!("Not a valid URL ({}).", e))?;
        if parsed.scheme() != "https" {
            return Err("Hosted config URL must use https://.".to_string());
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("URL must not embed credentials.".to_string());
        }
        Ok(Self::rewrite_to_raw(&parsed))
    }

    pub fn rewrite_to_raw(url: &reqwest::Url) -> String {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let path = url.path();
        if host == "github.com" {
            // https://github.com/{owner}/{repo}/blob/{branch}/{path}
            let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            if segments.len() >= 5 && segments[2] == "blob" {
                let owner = segments[0];
                let repo = segments[1];
                let branch = segments[3];
                let rest = segments[4..].join("/");
                return format!(
                    "https://raw.githubusercontent.com/{}/{}/{}/{}",
                    owner, repo, branch, rest
                );
            }
        } else if host == "pastebin.com" {
            let trimmed = path.trim_matches('/');
            if !trimmed.is_empty() && !trimmed.starts_with("raw/") && !trimmed.contains('/') {
                return format!("https://pastebin.com/raw/{}", trimmed);
            }
        }
        url.as_str().to_string()
    }
}

pub struct HostedConfigClient;

impl HostedConfigClient {
    pub fn fetch(raw_url: &str) -> Result<(String, String, SocialConfig), String> {
        let fetch_url = HostedConfigUrl::check(raw_url)?;
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(12))
            .user_agent(format!("sc-overlay/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("HTTP client error: {}", e))?;

        let resp = client
            .get(&fetch_url)
            .send()
            .map_err(|e| format!("Download failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("Server returned HTTP {}.", resp.status()));
        }
        let bytes = resp
            .bytes()
            .map_err(|e| format!("Failed reading response: {}", e))?;
        if bytes.len() > SocialConfigToml::MAX_FILE_BYTES {
            return Err("Remote config exceeds 1 MB limit.".to_string());
        }
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|_| "Remote config is not valid UTF-8.".to_string())?;
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        let hash = hex::encode(hasher.finalize());

        let parsed = SocialConfigToml::read(&text);
        let Some(cfg) = parsed.config else {
            return Err(format!(
                "Remote file is not a valid social-config.txt: {}",
                parsed.errors.join("; ")
            ));
        };
        Ok((text, hash, cfg))
    }
}
