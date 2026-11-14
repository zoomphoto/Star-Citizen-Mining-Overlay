use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

const EMBEDDED_MINING_TXT: &str = include_str!("../../../../assets/mining.txt");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiningResource {
    pub base: i32,
    pub name: String,
    pub max_multiple: i32,
}

#[derive(Debug, Clone)]
pub struct MiningTable {
    pub resources: Vec<MiningResource>,
    pub errors: Vec<String>,
    pub version: Option<String>,
    labels: HashMap<i32, String>,
    pub max_signal: i32,
}

impl Default for MiningTable {
    fn default() -> Self {
        Self::empty()
    }
}

impl MiningTable {
    pub const VERSION_PREFIX: &'static str = "# IRIS mining data ";
    pub const MULTIPLE_LABEL: &'static str = "Multiple";
    pub const ORE_MULTIPLE_LABEL: &'static str = "Ore/Multiple";

    pub fn empty() -> Self {
        Self {
            resources: Vec::new(),
            errors: Vec::new(),
            version: None,
            labels: HashMap::new(),
            max_signal: 0,
        }
    }

    pub fn embedded_text() -> &'static str {
        EMBEDDED_MINING_TXT
    }

    pub fn embedded() -> Arc<MiningTable> {
        static INSTANCE: OnceLock<Arc<MiningTable>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(Self::parse(EMBEDDED_MINING_TXT)))
            .clone()
    }

    pub fn parse(text: &str) -> Self {
        let mut resources = Vec::new();
        let mut errors = Vec::new();
        let mut version: Option<String> = None;

        for (i, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if let Some(rest) = line.strip_prefix(Self::VERSION_PREFIX) {
                if version.is_none() {
                    version = Some(rest.trim().to_string());
                }
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('/').collect();
            if parts.len() != 3 {
                errors.push(format!(
                    "line {}: expected base signal/name/most multiples",
                    i + 1
                ));
                continue;
            }
            let num_str = parts[0].trim().replace(',', "");
            let name = parts[1].trim();
            let Ok(base) = num_str.parse::<i32>() else {
                errors.push(format!(
                    "line {}: \"{}\" is not a signal value",
                    i + 1,
                    parts[0].trim()
                ));
                continue;
            };
            if base <= 0 {
                errors.push(format!(
                    "line {}: \"{}\" is not a signal value",
                    i + 1,
                    parts[0].trim()
                ));
                continue;
            }
            if name.is_empty() {
                errors.push(format!("line {}: the name is empty", i + 1));
                continue;
            }
            let mult_str = parts[2].trim().trim_end_matches(['x', 'X']);
            let Ok(max_multiple) = mult_str.parse::<i32>() else {
                errors.push(format!(
                    "line {}: \"{}\" is not a number of multiples (1-99)",
                    i + 1,
                    parts[2].trim()
                ));
                continue;
            };
            if !(1..=99).contains(&max_multiple) {
                errors.push(format!(
                    "line {}: \"{}\" is not a number of multiples (1-99)",
                    i + 1,
                    parts[2].trim()
                ));
                continue;
            }

            resources.push(MiningResource {
                base,
                name: name.to_string(),
                max_multiple,
            });
        }

        let mut grouped: HashMap<i32, Vec<String>> = HashMap::new();
        for res in &resources {
            for n in 1..=res.max_multiple {
                let Some(val) = res.base.checked_mul(n) else {
                    break;
                };
                grouped
                    .entry(val)
                    .or_default()
                    .push(format!("{} {}x", res.name, n));
            }
        }

        let labels: HashMap<i32, String> = grouped
            .into_iter()
            .map(|(k, v)| (k, v.join(" / ")))
            .collect();
        let max_signal = labels.keys().copied().max().unwrap_or(0);

        Self {
            resources,
            errors,
            version,
            labels,
            max_signal,
        }
    }

    pub fn lookup(&self, value: i32) -> Option<&str> {
        self.labels.get(&value).map(|s| s.as_str())
    }

    pub fn contains(&self, value: i32) -> bool {
        self.labels.contains_key(&value)
    }

    pub fn above_limit_label(&self, value: i32) -> Option<&'static str> {
        if self.max_signal <= 0 || value <= self.max_signal {
            None
        } else if value % 1000 == 0 {
            Some(Self::MULTIPLE_LABEL)
        } else {
            Some(Self::ORE_MULTIPLE_LABEL)
        }
    }

    pub fn is_outdated(file_text: &str) -> bool {
        let parsed = Self::parse(file_text);
        let embedded = Self::embedded();
        match (parsed.version.as_deref(), embedded.version.as_deref()) {
            (Some(v), Some(shipped)) => v != shipped,
            _ => false,
        }
    }
}

struct TableSourceInner {
    table: Arc<MiningTable>,
    stamp: Option<SystemTime>,
    checked_ms: f64,
}

pub struct MiningTableSource {
    path: PathBuf,
    inner: Mutex<TableSourceInner>,
}

impl MiningTableSource {
    const CHECK_EVERY_MS: f64 = 1000.0;

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            inner: Mutex::new(TableSourceInner {
                table: MiningTable::embedded(),
                stamp: None,
                checked_ms: f64::NEG_INFINITY,
            }),
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.path
    }

    pub fn current(&self, now_ms: f64) -> Arc<MiningTable> {
        let mut g = self.inner.lock();
        if now_ms - g.checked_ms < Self::CHECK_EVERY_MS {
            return g.table.clone();
        }
        g.checked_ms = now_ms;

        let stamp = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok());
        if stamp == g.stamp {
            return g.table.clone();
        }
        g.stamp = stamp;
        if stamp.is_none() {
            g.table = MiningTable::embedded();
            return g.table.clone();
        }

        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                let table = Arc::new(MiningTable::parse(&text));
                g.table = table.clone();
                table
            }
            Err(_) => {
                g.stamp = None;
                g.table.clone()
            }
        }
    }

    pub fn ensure_file(&self) -> anyhow::Result<()> {
        if self.path.exists() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, MiningTable::embedded_text())?;
        Ok(())
    }
}
