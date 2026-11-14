use crate::config::AtomicFile;
use chrono::{DateTime, Datelike, Local, SecondsFormat, Timelike, Utc};
use parking_lot::Mutex;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DossierNote {
    pub handle: String,
    pub citizen_record: Option<String>,
    pub updated: DateTime<Utc>,
    pub text: String,
}

/// Thread-safe Dossier Notes store (`Config/dossier-notes.txt`) 100% file-format compatible with IRIS.
/// Supports citizen-record linking across handle renames using Star Citizen in-game year (`+930`).
pub struct DossierNotes {
    notes: Mutex<Vec<DossierNote>>,
    max_file_bytes: usize,
    skipped_lines: Mutex<usize>,
}

impl DossierNotes {
    pub const FILE_NAME: &'static str = "dossier-notes.txt";
    pub const MAX_NOTE_CHARS: usize = 4000;
    pub const DEFAULT_MAX_FILE_BYTES: usize = 20 * 1024 * 1024;
    pub const FORMAT_VERSION: i32 = 1;
    pub const UNIVERSE_YEAR_OFFSET: i32 = 930;

    const HEADER: &'static str = "# IRIS dossier notes";
    const HANDLE_TAG: &'static str = "@handle ";
    const CITIZEN_TAG: &'static str = "@citizen ";
    const UPDATED_TAG: &'static str = "@updated ";

    pub fn new(max_file_bytes: usize) -> Self {
        Self {
            notes: Mutex::new(Vec::new()),
            max_file_bytes,
            skipped_lines: Mutex::new(0),
        }
    }

    pub fn load(path: &Path) -> Self {
        let store = Self::new(Self::DEFAULT_MAX_FILE_BYTES);
        if path.exists() {
            if let Ok(text) = fs::read_to_string(path) {
                store.read_from(&text);
            }
        }
        store
    }

    pub fn count(&self) -> usize {
        self.notes.lock().len()
    }

    pub fn skipped_lines(&self) -> usize {
        *self.skipped_lines.lock()
    }

    pub fn read_from(&self, text: &str) {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut guard = self.notes.lock();
        let mut skipped = self.skipped_lines.lock();
        guard.clear();
        *skipped = 0;

        let mut handle: Option<String> = None;
        let mut record: Option<String> = None;
        let mut updated: DateTime<Utc> = DateTime::<Utc>::MIN_UTC;
        let mut lines: Vec<String> = Vec::new();

        let finish = |guard: &mut Vec<DossierNote>,
                      handle: &mut Option<String>,
                      record: &mut Option<String>,
                      updated: &mut DateTime<Utc>,
                      lines: &mut Vec<String>| {
            if let Some(h) = handle.take() {
                if !lines.is_empty() {
                    guard.push(DossierNote {
                        handle: h,
                        citizen_record: record.take(),
                        updated: *updated,
                        text: lines.join("\n"),
                    });
                }
            }
            *record = None;
            *updated = DateTime::<Utc>::MIN_UTC;
            lines.clear();
        };

        for raw in text.replace("\r\n", "\n").split('\n') {
            let line = raw.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix(Self::HANDLE_TAG) {
                finish(&mut guard, &mut handle, &mut record, &mut updated, &mut lines);
                let name = rest.trim();
                if !name.is_empty() && !name.contains(' ') {
                    handle = Some(name.to_string());
                } else {
                    *skipped += 1;
                }
            } else if let Some(rest) = line.strip_prefix(Self::CITIZEN_TAG) {
                if handle.is_some() {
                    let digits = rest.trim();
                    if is_record(Some(digits)) {
                        record = Some(digits.to_string());
                    }
                } else {
                    *skipped += 1;
                }
            } else if let Some(rest) = line.strip_prefix(Self::UPDATED_TAG) {
                if handle.is_some() {
                    if let Ok(dt) = DateTime::parse_from_rfc3339(rest.trim()) {
                        updated = dt.with_timezone(&Utc);
                    }
                } else {
                    *skipped += 1;
                }
            } else if let Some(after) = line.strip_prefix('>') {
                if handle.is_some() {
                    let content = after.strip_prefix(' ').unwrap_or(after);
                    lines.push(content.to_string());
                } else {
                    *skipped += 1;
                }
            } else {
                *skipped += 1;
            }
        }

        finish(&mut guard, &mut handle, &mut record, &mut updated, &mut lines);
    }

    pub fn serialize(&self) -> String {
        let guard = self.notes.lock();
        Self::serialize_slice(&guard)
    }

    fn serialize_slice(notes: &[DossierNote]) -> String {
        let mut sb = String::new();
        sb.push_str(&format!("{} {}\r\n", Self::HEADER, Self::FORMAT_VERSION));
        for note in notes {
            if note.text.is_empty() {
                continue;
            }
            sb.push_str(&format!("\r\n{}{}\r\n", Self::HANDLE_TAG, note.handle));
            if let Some(rec) = &note.citizen_record {
                if !rec.is_empty() {
                    sb.push_str(&format!("{}{}\r\n", Self::CITIZEN_TAG, rec));
                }
            }
            sb.push_str(&format!(
                "{}{}\r\n",
                Self::UPDATED_TAG,
                note.updated.to_rfc3339_opts(SecondsFormat::Nanos, true)
            ));
            for line in note.text.split('\n') {
                if line.is_empty() {
                    sb.push_str(">\r\n");
                } else {
                    sb.push_str(&format!("> {}\r\n", line));
                }
            }
        }
        sb
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let text = self.serialize();
        AtomicFile::write_text(path, &text, true)
    }

    pub fn find(&self, handle: &str, citizen_record: Option<&str>) -> Option<DossierNote> {
        let guard = self.notes.lock();
        Self::index_of(&guard, handle, citizen_record).map(|idx| guard[idx].clone())
    }

    pub fn has_note(&self, handle: &str) -> bool {
        let guard = self.notes.lock();
        guard
            .iter()
            .any(|n| n.handle.eq_ignore_ascii_case(handle))
    }

    pub fn handles(&self) -> Vec<String> {
        self.notes.lock().iter().map(|n| n.handle.clone()).collect()
    }

    pub fn handles_matching(&self, query: &str, limit: usize) -> Vec<String> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        self.notes
            .lock()
            .iter()
            .filter(|n| n.handle.to_ascii_lowercase().contains(&q))
            .take(limit)
            .map(|n| n.handle.clone())
            .collect()
    }

    pub fn panel_text(&self, handle: &str) -> Option<String> {
        let guard = self.notes.lock();
        guard
            .iter()
            .find(|n| n.handle.eq_ignore_ascii_case(handle))
            .map(|n| n.text.replace('\n', " "))
    }

    pub fn link(
        &self,
        handle: &str,
        citizen_record: Option<&str>,
    ) -> (Option<DossierNote>, bool) {
        let mut guard = self.notes.lock();
        let rec = if is_record(citizen_record) {
            citizen_record
        } else {
            None
        };
        let Some(idx) = Self::index_of(&guard, handle, rec) else {
            return (None, false);
        };
        let mut note = guard[idx].clone();
        if let Some(r) = rec {
            if !note.handle.eq_ignore_ascii_case(handle) {
                let line = format!(
                    "Handle name changed from {} to {} seen on {}.",
                    note.handle,
                    handle,
                    Self::universe_time_now()
                );
                note.handle = handle.to_string();
                note.text = format!("{}\n{}", note.text, line);
                note.updated = Utc::now();
                guard[idx] = note.clone();
                return (Some(note), true);
            }
            if note.citizen_record.is_none() {
                note.citizen_record = Some(r.to_string());
                guard[idx] = note.clone();
                return (Some(note), true);
            }
        }
        (Some(note), false)
    }

    pub fn set(
        &self,
        handle: &str,
        citizen_record: Option<&str>,
        text: &str,
        utc_now: DateTime<Utc>,
    ) -> bool {
        let mut clean = Self::clean(text);
        if clean.chars().count() > Self::MAX_NOTE_CHARS {
            clean = clean.chars().take(Self::MAX_NOTE_CHARS).collect();
        }
        let mut guard = self.notes.lock();
        let rec = if is_record(citizen_record) {
            citizen_record.map(|s| s.to_string())
        } else {
            None
        };
        let idx = Self::index_of(&guard, handle, rec.as_deref());
        if clean.is_empty() {
            if let Some(i) = idx {
                guard.remove(i);
            }
            return true;
        }

        let mut candidate = guard.clone();
        let effective_rec = rec.or_else(|| idx.and_then(|i| guard[i].citizen_record.clone()));
        let note = DossierNote {
            handle: handle.to_string(),
            citizen_record: effective_rec,
            updated: utc_now,
            text: clean,
        };
        if let Some(i) = idx {
            candidate[i] = note;
        } else {
            candidate.push(note);
        }
        if Self::serialize_slice(&candidate).len() > self.max_file_bytes {
            return false;
        }
        *guard = candidate;
        true
    }

    pub fn clean(text: &str) -> String {
        let mut sb = String::with_capacity(text.len());
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for c in normalized.chars() {
            if c == '\n' {
                sb.push('\n');
            } else if c == '\t' {
                sb.push_str("    ");
            } else if !c.is_control() {
                sb.push(c);
            }
        }
        sb
    }

    pub fn universe_time_now() -> String {
        let now = Local::now();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            now.year() + Self::UNIVERSE_YEAR_OFFSET,
            now.month(),
            now.day(),
            now.hour(),
            now.minute()
        )
    }

    fn index_of(notes: &[DossierNote], handle: &str, record: Option<&str>) -> Option<usize> {
        if is_record(record) {
            if let Some(by_rec) = notes
                .iter()
                .position(|n| n.citizen_record.as_deref() == record)
            {
                return Some(by_rec);
            }
        }
        notes.iter().position(|n| {
            n.handle.eq_ignore_ascii_case(handle)
                && (n.citizen_record.is_none()
                    || !is_record(record)
                    || n.citizen_record.as_deref() == record)
        })
    }
}

fn is_record(text: Option<&str>) -> bool {
    match text {
        Some(s) if !s.is_empty() => s.bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
}

