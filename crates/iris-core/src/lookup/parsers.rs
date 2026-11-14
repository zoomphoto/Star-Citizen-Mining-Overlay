use crate::config::OrgVocabulary;
use crate::lookup::{
    OrgInfo, OrgMembersPage, PlayerOrgs, PlayerProfile, RsiNames, RsiResult,
};
use crate::models::OrgMembership;
use scraper::{ElementRef, Html, Selector};

pub fn clean_html_text(raw: &str) -> String {
    let decoded = html_escape::decode_html_entities(raw);
    let mut out = String::with_capacity(decoded.len());
    let mut prev_space = true;
    for ch in decoded.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn element_text(el: ElementRef<'_>) -> String {
    let raw: String = el.text().collect();
    clean_html_text(&raw)
}

pub struct PlayerOrgsParser;

impl PlayerOrgsParser {
    pub fn parse(html: &str, requested_handle: &str) -> RsiResult<PlayerOrgs> {
        let doc = Html::parse_document(html);
        let org_block_sel = Selector::parse(".box-content.org").unwrap();
        let heading_sel = Selector::parse(".title, .heading, h3").unwrap();
        let profile_sel = Selector::parse(".profile, .profile-content").unwrap();
        let entry_sel = Selector::parse(".entry").unwrap();
        let label_sel = Selector::parse(".label").unwrap();
        let value_sel = Selector::parse(".value").unwrap();
        let org_link_sel = Selector::parse("a[href*='/orgs/']").unwrap();

        let mut orgs = Vec::new();
        let mut blocks_seen = 0usize;

        for block in doc.select(&org_block_sel) {
            let classes: Vec<&str> = block.value().classes().collect();
            let heading = block
                .select(&heading_sel)
                .next()
                .map(element_text)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let is_main = classes.contains(&"main") || heading.contains("main organization");
            let is_aff = !is_main
                && (classes.contains(&"affiliation") || heading.contains("affiliation"));
            if !is_main && !is_aff {
                continue;
            }
            blocks_seen += 1;
            if let Some(m) = Self::read_block(
                block,
                is_main,
                &entry_sel,
                &label_sel,
                &value_sel,
                &org_link_sel,
            ) {
                orgs.push(m);
            }
        }

        if blocks_seen == 0 {
            let body_text = clean_html_text(&doc.root_element().text().collect::<String>())
                .to_ascii_lowercase();
            let mentions_headings =
                body_text.contains("main organization") || body_text.contains("affiliation");
            let looks_like_profile = doc.select(&profile_sel).next().is_some();
            if mentions_headings || !looks_like_profile {
                return RsiResult::Failed(
                    "player page: no organization blocks recognized (page layout may have changed)"
                        .to_string(),
                );
            }
        }

        let shown = value_for_label(
            doc.root_element(),
            "Handle name",
            &entry_sel,
            &label_sel,
            &value_sel,
        );
        let handle = match shown {
            Some(h) if RsiNames::is_valid_handle(&h) => h,
            _ => requested_handle.to_string(),
        };

        RsiResult::Found(PlayerOrgs { handle, orgs })
    }

    fn read_block(
        block: ElementRef<'_>,
        is_main: bool,
        entry_sel: &Selector,
        label_sel: &Selector,
        value_sel: &Selector,
        org_link_sel: &Selector,
    ) -> Option<OrgMembership> {
        let classes: Vec<&str> = block.value().classes().collect();
        if classes.contains(&"visibility-R") || classes.contains(&"visibility-H") {
            return Some(OrgMembership::new(None, is_main));
        }

        let sid = value_for_label(
            block,
            "Spectrum Identification",
            entry_sel,
            label_sel,
            value_sel,
        )
        .filter(|s| RsiNames::is_valid_sid(s))
        .or_else(|| sid_from_link(block, org_link_sel));

        let name = block
            .select(org_link_sel)
            .map(element_text)
            .find(|t| !t.is_empty());

        let Some(sid) = sid else {
            let text = element_text(block).to_ascii_lowercase();
            let redacted = text.contains("redacted");
            let no_org = ["no main org", "no affiliation", "public records"]
                .iter()
                .any(|m| text.contains(m));
            if !redacted && no_org {
                return None;
            }
            return Some(OrgMembership::new(None, is_main));
        };

        let labeled_arch = value_for_label(block, "Archetype", entry_sel, label_sel, value_sel);
        let labeled_acts = values_for_label(block, "Activity", entry_sel, label_sel, value_sel);

        if !is_main {
            let archetype = labeled_arch.map(|a| {
                OrgVocabulary::match_archetype(&a)
                    .map(|s| s.to_string())
                    .unwrap_or(a)
            });
            return Some(OrgMembership {
                sid: Some(sid),
                is_main: false,
                archetype,
                activities: labeled_acts,
                name,
            });
        }

        let scanned = scan_attributes(block);
        let archetype = match labeled_arch {
            Some(a) => Some(
                OrgVocabulary::match_archetype(&a)
                    .map(|s| s.to_string())
                    .unwrap_or(a),
            ),
            None => scanned.0,
        };
        let activities = if !labeled_acts.is_empty() {
            labeled_acts
        } else {
            scanned.3
        };

        Some(OrgMembership {
            sid: Some(sid),
            is_main: true,
            archetype,
            activities,
            name,
        })
    }
}

pub struct ProfilePageParser;

impl ProfilePageParser {
    pub const NO_CITIZEN_RECORD: &'static str = "n/a";

    pub fn parse(html: &str, requested_handle: &str) -> RsiResult<PlayerProfile> {
        let doc = Html::parse_document(html);
        let rec_sel = Selector::parse(".citizen-record").unwrap();
        let profile_sel = Selector::parse(".profile").unwrap();
        let info_entry_sel = Selector::parse(".profile .info .entry").unwrap();
        let label_sel = Selector::parse(".label").unwrap();
        let value_sel = Selector::parse(".value").unwrap();

        let rec_el = doc.select(&rec_sel).next();
        let has_label = rec_el.is_some()
            || doc.select(&label_sel).any(|l| {
                element_text(l)
                    .to_ascii_lowercase()
                    .contains("uee citizen record")
            });
        if doc.select(&profile_sel).next().is_none() && !has_label {
            return RsiResult::Failed(
                "profile page: no profile block (page layout may have changed)".to_string(),
            );
        }

        let mut citizen_record = None;
        if let Some(el) = rec_el {
            if let Some(val_el) = el.select(&value_sel).next() {
                let raw = element_text(val_el);
                if let Some(rest) = raw.strip_prefix('#') {
                    let digits = rest.trim();
                    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                        citizen_record = Some(digits.to_string());
                    }
                }
            }
        }

        let mut moniker = None;
        for entry in doc.select(&info_entry_sel) {
            if entry.select(&label_sel).next().is_some() {
                continue;
            }
            if let Some(val_el) = entry.select(&value_sel).next() {
                let text = element_text(val_el);
                if !text.is_empty() {
                    moniker = Some(text);
                    break;
                }
            }
        }

        RsiResult::Found(PlayerProfile {
            handle: requested_handle.to_string(),
            moniker,
            citizen_record,
            has_label,
        })
    }

    pub fn shown(record: Option<&str>) -> &str {
        match record {
            Some(s) if !s.is_empty() => s,
            _ => Self::NO_CITIZEN_RECORD,
        }
    }
}

pub struct OrgPageParser;

impl OrgPageParser {
    pub fn parse(html: &str, requested_sid: &str) -> RsiResult<OrgInfo> {
        let doc = Html::parse_document(html);
        let h1_org_sel = Selector::parse("#organization h1").unwrap();
        let h1_sel = Selector::parse("h1").unwrap();
        let count_sel = Selector::parse(".count, .logo .count").unwrap();
        let tag_sel =
            Selector::parse(".tags li, .tags span, .tags a, .focus li, .focus img, .focus span")
                .unwrap();
        let org_area_sel = Selector::parse("#organization").unwrap();
        let focus_img_sel = Selector::parse(".focus img").unwrap();

        let mut heading = None;
        for sel in [&h1_org_sel, &h1_sel] {
            for h in doc.select(sel) {
                let t = element_text(h);
                if t.contains(" / ") {
                    heading = Some(t);
                    break;
                }
            }
            if heading.is_some() {
                break;
            }
        }

        let Some(heading) = heading else {
            return RsiResult::Failed(
                "org page: heading 'Name / SID' not found (page layout may have changed)"
                    .to_string(),
            );
        };
        let Some(cut) = heading.rfind(" / ") else {
            return RsiResult::Failed("org page: heading separator missing".to_string());
        };
        let long_name = heading[..cut].trim().to_string();
        let symbol = heading[cut + 3..].trim();
        if long_name.is_empty() {
            return RsiResult::Failed("org page: heading has no org name".to_string());
        }
        let sid = if symbol.eq_ignore_ascii_case(requested_sid) {
            symbol.to_string()
        } else {
            requested_sid.to_string()
        };

        let mut member_count = None;
        for el in doc.select(&count_sel) {
            member_count = parse_member_count(&element_text(el));
            if member_count.is_some() {
                break;
            }
        }
        if member_count.is_none() {
            let body_text = clean_html_text(&doc.root_element().text().collect::<String>());
            member_count = parse_member_count(&body_text);
        }

        let mut archetype: Option<String> = None;
        let mut commitment: Option<String> = None;
        let mut is_exclusive = false;
        let mut activities: Vec<String> = Vec::new();
        let mut any_tag = false;

        for el in doc.select(&tag_sel) {
            let mut candidates = Vec::new();
            let inner = element_text(el);
            if !inner.is_empty() && inner.len() <= 40 {
                candidates.push(inner);
            }
            for attr in ["alt", "title"] {
                if let Some(v) = el.value().attr(attr) {
                    let clean = clean_html_text(v);
                    if !clean.is_empty() && clean.len() <= 40 {
                        candidates.push(clean);
                    }
                }
            }
            for text in candidates {
                any_tag |= classify_tag(
                    &text,
                    &mut archetype,
                    &mut commitment,
                    &mut is_exclusive,
                    &mut activities,
                );
            }
        }

        if !any_tag {
            let area = doc
                .select(&org_area_sel)
                .next()
                .unwrap_or_else(|| doc.root_element());
            let scanned = scan_attributes(area);
            archetype = scanned.0;
            commitment = scanned.1;
            is_exclusive = scanned.2;
            activities = scanned.3;
        }

        let mut unknown_activities = Vec::new();
        for icon in doc.select(&focus_img_sel) {
            for attr in ["alt", "title"] {
                if let Some(v) = icon.value().attr(attr) {
                    let clean = clean_html_text(v);
                    if !clean.is_empty()
                        && clean.len() <= 40
                        && OrgVocabulary::match_activity(&clean).is_none()
                        && OrgVocabulary::match_archetype(&clean).is_none()
                        && OrgVocabulary::match_commitment(&clean).is_none()
                        && !OrgVocabulary::is_exclusive(&clean)
                        && !unknown_activities
                            .iter()
                            .any(|u: &String| u.eq_ignore_ascii_case(&clean))
                    {
                        unknown_activities.push(clean);
                    }
                }
            }
        }

        RsiResult::Found(OrgInfo {
            sid,
            long_name,
            member_count,
            archetype,
            commitment,
            is_exclusive,
            activities,
            unknown_activities,
        })
    }
}

pub struct OrgMembersParser;

impl OrgMembersParser {
    pub fn parse_response(json_text: &str) -> RsiResult<OrgMembersPage> {
        let root: serde_json::Value = match serde_json::from_str(json_text) {
            Ok(v) => v,
            Err(e) => return RsiResult::Failed(format!("members response invalid JSON: {}", e)),
        };
        let success = root.get("success").and_then(json_int);
        if success != Some(1) {
            let msg = root
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("no message");
            return RsiResult::Failed(format!("members API reported failure ({})", msg));
        }
        let Some(data) = root.get("data").and_then(|v| v.as_object()) else {
            return RsiResult::Failed("members response has no data".to_string());
        };
        let Some(html) = data.get("html").and_then(|v| v.as_str()) else {
            return RsiResult::Failed("members response has no html".to_string());
        };
        let total = data.get("totalrows").and_then(json_int);
        RsiResult::Found(Self::parse_fragment(html, total))
    }

    pub fn parse_fragment(html: &str, total_rows: Option<i32>) -> OrgMembersPage {
        let doc = Html::parse_fragment(html);
        let item_sel = Selector::parse(".member-item").unwrap();
        let link_sel = Selector::parse("a[href*='/citizens/']").unwrap();
        let nick_sel = Selector::parse(".nick").unwrap();

        let mut handles = Vec::new();
        let mut entries_seen = 0usize;

        for entry in doc.select(&item_sel) {
            entries_seen += 1;
            if entry.value().classes().any(|c| c == "visibility-R") {
                continue;
            }
            if let Some(href) = entry
                .select(&link_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
            {
                let path = href
                    .split(['?', '#'])
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('/');
                if let Some(seg) = path.rsplit('/').next() {
                    if RsiNames::is_valid_handle(seg) {
                        handles.push(seg.to_string());
                        continue;
                    }
                }
            }
            if let Some(nick_el) = entry.select(&nick_sel).next() {
                let nick = element_text(nick_el);
                if !nick.is_empty()
                    && !nick.eq_ignore_ascii_case("REDACTED")
                    && RsiNames::is_valid_handle(&nick)
                {
                    handles.push(nick);
                }
            }
        }

        OrgMembersPage {
            handles,
            entries_seen,
            total_rows,
        }
    }
}

fn json_int(v: &serde_json::Value) -> Option<i32> {
    if let Some(n) = v.as_i64() {
        return i32::try_from(n).ok();
    }
    if let Some(s) = v.as_str() {
        return s.trim().parse::<i32>().ok();
    }
    if let Some(b) = v.as_bool() {
        return Some(if b { 1 } else { 0 });
    }
    None
}

fn sid_from_link(scope: ElementRef<'_>, org_link_sel: &Selector) -> Option<String> {
    let href = scope
        .select(org_link_sel)
        .next()
        .and_then(|a| a.value().attr("href"))?;
    let path = href
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let seg = path.rsplit('/').next()?;
    if RsiNames::is_valid_sid(seg) {
        Some(seg.to_string())
    } else {
        None
    }
}

fn value_for_label(
    scope: ElementRef<'_>,
    label: &str,
    entry_sel: &Selector,
    label_sel: &Selector,
    value_sel: &Selector,
) -> Option<String> {
    let needle = label.to_ascii_lowercase();
    for entry in scope.select(entry_sel) {
        let Some(lbl_el) = entry.select(label_sel).next() else {
            continue;
        };
        if !element_text(lbl_el).to_ascii_lowercase().contains(&needle) {
            continue;
        }
        if let Some(val_el) = entry.select(value_sel).next() {
            let v = element_text(val_el);
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn values_for_label(
    scope: ElementRef<'_>,
    label: &str,
    entry_sel: &Selector,
    label_sel: &Selector,
    value_sel: &Selector,
) -> Vec<String> {
    let needle = label.to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    for entry in scope.select(entry_sel) {
        let Some(lbl_el) = entry.select(label_sel).next() else {
            continue;
        };
        if !element_text(lbl_el).to_ascii_lowercase().contains(&needle) {
            continue;
        }
        if let Some(val_el) = entry.select(value_sel).next() {
            let v = element_text(val_el);
            if !v.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&v)) {
                out.push(v);
            }
        }
    }
    out
}

fn classify_tag(
    text: &str,
    archetype: &mut Option<String>,
    commitment: &mut Option<String>,
    exclusive: &mut bool,
    activities: &mut Vec<String>,
) -> bool {
    if let Some(a) = OrgVocabulary::match_archetype(text) {
        if archetype.is_none() {
            *archetype = Some(a.to_string());
        }
        return true;
    }
    if let Some(c) = OrgVocabulary::match_commitment(text) {
        if commitment.is_none() {
            *commitment = Some(c.to_string());
        }
        return true;
    }
    if OrgVocabulary::is_exclusive(text) {
        *exclusive = true;
        return true;
    }
    if let Some(act) = OrgVocabulary::match_activity(text) {
        if !activities.iter().any(|x| x.eq_ignore_ascii_case(act)) {
            activities.push(act.to_string());
        }
        return true;
    }
    false
}

fn scan_attributes(
    area: ElementRef<'_>,
) -> (Option<String>, Option<String>, bool, Vec<String>) {
    let mut archetype = None;
    let mut commitment = None;
    let mut exclusive = false;
    let mut activities = Vec::new();

    for text_node in area.text() {
        let clean = clean_html_text(text_node);
        if clean.is_empty() || clean.len() > 40 {
            continue;
        }
        classify_tag(
            &clean,
            &mut archetype,
            &mut commitment,
            &mut exclusive,
            &mut activities,
        );
    }
    (archetype, commitment, exclusive, activities)
}

fn parse_member_count(text: &str) -> Option<i32> {
    // Matches `(\d[\d,]*)\s+members?\b`
    let lower = text.to_ascii_lowercase();
    for (idx, _) in lower.match_indices("member") {
        let after = &lower[idx + "member".len()..];
        let valid_end = after.is_empty()
            || !after.chars().next().unwrap().is_ascii_alphabetic()
            || (after.starts_with('s')
                && after[1..]
                    .chars()
                    .next()
                    .map(|c| !c.is_ascii_alphabetic())
                    .unwrap_or(true));
        if !valid_end {
            continue;
        }
        let before = text[..idx].trim_end();
        let num_part: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit() || *c == ',')
            .collect::<String>()
            .chars()
            .rev()
            .filter(|c| *c != ',')
            .collect();
        if !num_part.is_empty() {
            if let Ok(n) = num_part.parse::<i32>() {
                return Some(n);
            }
        }
    }
    None
}
