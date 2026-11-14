use crate::lookup::{OrgInfo, PlayerOrgs};
use crate::models::OrgMembership;
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

pub struct LruCache<V> {
    capacity: usize,
    idle_ttl: Duration,
    inner: Mutex<LruInner<V>>,
}

struct LruInner<V> {
    map: HashMap<String, (V, Instant)>,
    order: VecDeque<String>,
}

impl<V: Clone> LruCache<V> {
    pub fn new(capacity: usize, idle_ttl: Duration) -> Self {
        Self {
            capacity: capacity.max(16),
            idle_ttl,
            inner: Mutex::new(LruInner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let k = key.to_ascii_lowercase();
        let mut guard = self.inner.lock();
        let now = Instant::now();
        if let Some((_, ts)) = guard.map.get(&k) {
            if now.duration_since(*ts) > self.idle_ttl {
                guard.map.remove(&k);
                guard.order.retain(|x| x != &k);
                return None;
            }
        }
        if let Some((val, ts)) = guard.map.get_mut(&k) {
            *ts = now;
            let out = val.clone();
            guard.order.retain(|x| x != &k);
            guard.order.push_back(k);
            return Some(out);
        }
        None
    }

    pub fn insert(&self, key: &str, value: V) {
        let k = key.to_ascii_lowercase();
        let mut guard = self.inner.lock();
        if !guard.map.contains_key(&k) && guard.map.len() >= self.capacity {
            if let Some(oldest) = guard.order.pop_front() {
                guard.map.remove(&oldest);
            }
        } else {
            guard.order.retain(|x| x != &k);
        }
        guard.order.push_back(k.clone());
        guard.map.insert(k, (value, Instant::now()));
    }

    pub fn remove(&self, key: &str) {
        let k = key.to_ascii_lowercase();
        let mut guard = self.inner.lock();
        guard.map.remove(&k);
        guard.order.retain(|x| x != &k);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFoundKind {
    Player,
    Org,
}

impl NotFoundKind {
    fn as_str(&self) -> &'static str {
        match self {
            NotFoundKind::Player => "player",
            NotFoundKind::Org => "org",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotFoundEntry {
    pub name: String,
    pub last_refreshed: DateTime<Utc>,
    pub miss_count: i32,
}

impl NotFoundEntry {
    /// Escalating TTL matching IRIS schema v3: 1 day on 1st miss, 7 days on 2nd, 30 days on 3+ misses.
    pub fn is_fresh(&self, now: DateTime<Utc>) -> bool {
        let ttl_hours = match self.miss_count {
            0 | 1 => 24,
            2 => 24 * 7,
            _ => 24 * 30,
        };
        now - self.last_refreshed < ChronoDuration::hours(ttl_hours)
    }
}

#[derive(Debug, Clone)]
pub struct CachedPlayer {
    pub handle: String,
    pub orgs: Vec<OrgMembership>,
    pub last_refreshed: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CachedOrg {
    pub info: OrgInfo,
    pub last_refreshed: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PlayerExtra {
    pub handle: String,
    pub last_seen_plate: Option<DateTime<Utc>>,
    pub moniker: Option<String>,
    pub citizen_record: Option<String>,
    pub profile_refreshed: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct SeenPlayer {
    pub handle: String,
    pub last_seen_plate: DateTime<Utc>,
}

/// SQLite WAL cache (`Data/cache.db`) 100% compatible with IRIS `CurrentSchemaVersion = 3`.
pub struct CacheDatabase {
    conn: Mutex<Connection>,
    player_mem: LruCache<Option<CachedPlayer>>,
    org_mem: LruCache<Option<CachedOrg>>,
}

impl CacheDatabase {
    pub const CURRENT_SCHEMA_VERSION: i32 = 3;
    const ACTIVITY_SEP: &'static str = "|";

    pub fn open_file(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    pub fn from_connection(conn: Connection) -> anyhow::Result<Self> {
        conn.busy_timeout(Duration::from_millis(5000))?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");

        let db = Self {
            conn: Mutex::new(conn),
            player_mem: LruCache::new(2048, Duration::from_secs(1800)),
            org_mem: LruCache::new(1024, Duration::from_secs(1800)),
        };
        db.ensure_schema()?;
        Ok(db)
    }

    pub fn schema_version(&self) -> anyhow::Result<i32> {
        let guard = self.conn.lock();
        let ver: i32 = guard.pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(ver)
    }

    fn ensure_schema(&self) -> anyhow::Result<()> {
        let mut guard = self.conn.lock();
        let version: i32 = guard.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > Self::CURRENT_SCHEMA_VERSION {
            anyhow::bail!(
                "cache.db has schema version {}, but this version supports up to {}.",
                version,
                Self::CURRENT_SCHEMA_VERSION
            );
        }

        let tx = guard.transaction()?;
        if version == 0 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS players (
                    handle TEXT NOT NULL PRIMARY KEY COLLATE NOCASE,
                    source TEXT NOT NULL,
                    last_refreshed TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS player_orgs (
                    handle TEXT NOT NULL COLLATE NOCASE,
                    position INTEGER NOT NULL,
                    sid TEXT COLLATE NOCASE,
                    is_main INTEGER NOT NULL,
                    archetype TEXT,
                    activities TEXT NOT NULL,
                    last_refreshed TEXT NOT NULL,
                    name TEXT,
                    PRIMARY KEY (handle, position)
                );
                CREATE TABLE IF NOT EXISTS orgs (
                    sid TEXT NOT NULL PRIMARY KEY COLLATE NOCASE,
                    long_name TEXT,
                    member_count INTEGER,
                    archetype TEXT,
                    commitment TEXT,
                    is_exclusive INTEGER NOT NULL,
                    activities TEXT NOT NULL,
                    attrs_refreshed TEXT,
                    members_refreshed TEXT,
                    source TEXT NOT NULL,
                    last_refreshed TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS org_members (
                    sid TEXT NOT NULL COLLATE NOCASE,
                    handle TEXT NOT NULL COLLATE NOCASE,
                    last_refreshed TEXT NOT NULL,
                    PRIMARY KEY (sid, handle)
                );
                CREATE TABLE IF NOT EXISTS not_found (
                    kind TEXT NOT NULL,
                    name TEXT NOT NULL COLLATE NOCASE,
                    source TEXT NOT NULL,
                    last_refreshed TEXT NOT NULL,
                    miss_count INTEGER NOT NULL DEFAULT 1,
                    PRIMARY KEY (kind, name)
                );",
            )?;
        }
        if version < 2 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS duplicate_name_counts (
                    name TEXT NOT NULL PRIMARY KEY COLLATE NOCASE,
                    frames INTEGER NOT NULL,
                    last_seen TEXT NOT NULL
                );",
            )?;
        }
        if version < 3 && !has_column(&tx, "not_found", "miss_count")? {
            tx.execute(
                "ALTER TABLE not_found ADD COLUMN miss_count INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }
        if !has_column(&tx, "player_orgs", "name")? {
            tx.execute("ALTER TABLE player_orgs ADD COLUMN name TEXT", [])?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS player_extra (
                handle TEXT NOT NULL PRIMARY KEY COLLATE NOCASE,
                last_seen_plate TEXT,
                moniker TEXT,
                citizen_record TEXT,
                profile_refreshed TEXT
            );",
        )?;
        if version < Self::CURRENT_SCHEMA_VERSION {
            tx.pragma_update(None, "user_version", Self::CURRENT_SCHEMA_VERSION)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_player(&self, handle: &str) -> Option<CachedPlayer> {
        if let Some(cached) = self.player_mem.get(handle) {
            return cached;
        }
        let guard = self.conn.lock();
        let row: Option<(String, String)> = guard
            .query_row(
                "SELECT handle, last_refreshed FROM players WHERE handle = ?1",
                params![handle],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();

        let Some((canonical_handle, refreshed_str)) = row else {
            self.player_mem.insert(handle, None);
            return None;
        };
        let last_refreshed = parse_time(&refreshed_str);
        let mut orgs = Vec::new();
        if let Ok(mut stmt) = guard.prepare(
            "SELECT sid, is_main, archetype, activities, name FROM player_orgs WHERE handle = ?1 ORDER BY position ASC",
        ) {
            if let Ok(rows) = stmt.query_map(params![&canonical_handle], |r| {
                let sid: Option<String> = r.get(0)?;
                let is_main: i32 = r.get(1)?;
                let archetype: Option<String> = r.get(2)?;
                let acts: String = r.get(3)?;
                let name: Option<String> = r.get(4)?;
                Ok(OrgMembership {
                    sid,
                    is_main: is_main != 0,
                    archetype,
                    activities: split_activities(&acts),
                    name,
                })
            }) {
                for m in rows.flatten() {
                    orgs.push(m);
                }
            }
        }
        let player = CachedPlayer {
            handle: canonical_handle,
            orgs,
            last_refreshed,
        };
        self.player_mem.insert(handle, Some(player.clone()));
        Some(player)
    }

    pub fn save_player(&self, player: &PlayerOrgs, source: &str, now: DateTime<Utc>) -> anyhow::Result<()> {
        let now_str = format_time(now);
        {
            let mut guard = self.conn.lock();
            let tx = guard.transaction()?;
            tx.execute(
                "INSERT INTO players (handle, source, last_refreshed) VALUES (?1, ?2, ?3)
                 ON CONFLICT(handle) DO UPDATE SET handle = excluded.handle, source = excluded.source, last_refreshed = excluded.last_refreshed",
                params![player.handle, source, now_str],
            )?;
            tx.execute(
                "DELETE FROM player_orgs WHERE handle = ?1",
                params![player.handle],
            )?;
            for (pos, org) in player.orgs.iter().enumerate() {
                tx.execute(
                    "INSERT INTO player_orgs (handle, position, sid, is_main, archetype, activities, last_refreshed, name)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        player.handle,
                        pos as i32,
                        org.sid,
                        if org.is_main { 1 } else { 0 },
                        org.archetype,
                        join_activities(&org.activities),
                        now_str,
                        org.name,
                    ],
                )?;
            }
            tx.execute(
                "DELETE FROM not_found WHERE kind = 'player' AND name = ?1",
                params![player.handle],
            )?;
            tx.commit()?;
        }
        let cached = CachedPlayer {
            handle: player.handle.clone(),
            orgs: player.orgs.clone(),
            last_refreshed: now,
        };
        self.player_mem.insert(&player.handle, Some(cached));
        Ok(())
    }

    pub fn get_org(&self, sid: &str) -> Option<CachedOrg> {
        if let Some(cached) = self.org_mem.get(sid) {
            return cached;
        }
        let guard = self.conn.lock();
        let row = guard
            .query_row(
                "SELECT sid, long_name, member_count, archetype, commitment, is_exclusive, activities, last_refreshed
                 FROM orgs WHERE sid = ?1",
                params![sid],
                |r| {
                    let sid_val: String = r.get(0)?;
                    let long_name: Option<String> = r.get(1)?;
                    let member_count: Option<i32> = r.get(2)?;
                    let archetype: Option<String> = r.get(3)?;
                    let commitment: Option<String> = r.get(4)?;
                    let is_excl: i32 = r.get(5)?;
                    let acts: String = r.get(6)?;
                    let refreshed: String = r.get(7)?;
                    Ok(CachedOrg {
                        info: OrgInfo {
                            sid: sid_val.clone(),
                            long_name: long_name.unwrap_or(sid_val),
                            member_count,
                            archetype,
                            commitment,
                            is_exclusive: is_excl != 0,
                            activities: split_activities(&acts),
                            unknown_activities: Vec::new(),
                        },
                        last_refreshed: parse_time(&refreshed),
                    })
                },
            )
            .optional()
            .ok()
            .flatten();
        self.org_mem.insert(sid, row.clone());
        row
    }

    pub fn save_org(&self, org: &OrgInfo, source: &str, now: DateTime<Utc>) -> anyhow::Result<()> {
        let now_str = format_time(now);
        {
            let mut guard = self.conn.lock();
            let tx = guard.transaction()?;
            tx.execute(
                "INSERT INTO orgs (sid, long_name, member_count, archetype, commitment, is_exclusive, activities, attrs_refreshed, source, last_refreshed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(sid) DO UPDATE SET
                    long_name = excluded.long_name,
                    member_count = excluded.member_count,
                    archetype = excluded.archetype,
                    commitment = excluded.commitment,
                    is_exclusive = excluded.is_exclusive,
                    activities = excluded.activities,
                    attrs_refreshed = excluded.attrs_refreshed,
                    source = excluded.source,
                    last_refreshed = excluded.last_refreshed",
                params![
                    org.sid,
                    org.long_name,
                    org.member_count,
                    org.archetype,
                    org.commitment,
                    if org.is_exclusive { 1 } else { 0 },
                    join_activities(&org.activities),
                    now_str,
                    source,
                    now_str,
                ],
            )?;
            // Also backfill affiliate org rows in `player_orgs` that lacked archetype/activities/name!
            tx.execute(
                "UPDATE player_orgs SET
                    archetype = COALESCE(archetype, ?2),
                    activities = CASE WHEN activities = '' THEN ?3 ELSE activities END,
                    name = COALESCE(name, ?4)
                 WHERE sid = ?1",
                params![
                    org.sid,
                    org.archetype,
                    join_activities(&org.activities),
                    org.long_name,
                ],
            )?;
            tx.execute(
                "DELETE FROM not_found WHERE kind = 'org' AND name = ?1",
                params![org.sid],
            )?;
            tx.commit()?;
        }
        self.org_mem.insert(
            &org.sid,
            Some(CachedOrg {
                info: org.clone(),
                last_refreshed: now,
            }),
        );
        Ok(())
    }

    pub fn get_not_found(&self, kind: NotFoundKind, name: &str) -> Option<NotFoundEntry> {
        let guard = self.conn.lock();
        guard
            .query_row(
                "SELECT name, last_refreshed, miss_count FROM not_found WHERE kind = ?1 AND name = ?2",
                params![kind.as_str(), name],
                |r| {
                    let n: String = r.get(0)?;
                    let ts: String = r.get(1)?;
                    let count: i32 = r.get(2)?;
                    Ok(NotFoundEntry {
                        name: n,
                        last_refreshed: parse_time(&ts),
                        miss_count: count,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn record_not_found(
        &self,
        kind: NotFoundKind,
        name: &str,
        source: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let now_str = format_time(now);
        let guard = self.conn.lock();
        guard.execute(
            "INSERT INTO not_found (kind, name, source, last_refreshed, miss_count)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(kind, name) DO UPDATE SET
                source = excluded.source,
                last_refreshed = excluded.last_refreshed,
                miss_count = not_found.miss_count + 1",
            params![kind.as_str(), name, source, now_str],
        )?;
        Ok(())
    }

    pub fn note_seen_on_plate(&self, handle: &str, now: DateTime<Utc>) {
        let now_str = format_time(now);
        let guard = self.conn.lock();
        let _ = guard.execute(
            "INSERT INTO player_extra (handle, last_seen_plate) VALUES (?1, ?2)
             ON CONFLICT(handle) DO UPDATE SET last_seen_plate = excluded.last_seen_plate",
            params![handle, now_str],
        );
    }

    pub fn save_profile(
        &self,
        handle: &str,
        moniker: Option<&str>,
        citizen_record: Option<&str>,
        now: DateTime<Utc>,
    ) {
        let now_str = format_time(now);
        let guard = self.conn.lock();
        let _ = guard.execute(
            "INSERT INTO player_extra (handle, moniker, citizen_record, profile_refreshed)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(handle) DO UPDATE SET
                moniker = excluded.moniker,
                citizen_record = excluded.citizen_record,
                profile_refreshed = excluded.profile_refreshed",
            params![handle, moniker, citizen_record, now_str],
        );
    }

    pub fn get_player_extra(&self, handle: &str) -> Option<PlayerExtra> {
        let guard = self.conn.lock();
        guard
            .query_row(
                "SELECT handle, last_seen_plate, moniker, citizen_record, profile_refreshed
                 FROM player_extra WHERE handle = ?1",
                params![handle],
                |r| {
                    let h: String = r.get(0)?;
                    let seen: Option<String> = r.get(1)?;
                    let moniker: Option<String> = r.get(2)?;
                    let rec: Option<String> = r.get(3)?;
                    let prof_ref: Option<String> = r.get(4)?;
                    Ok(PlayerExtra {
                        handle: h,
                        last_seen_plate: seen.as_deref().map(parse_time),
                        moniker,
                        citizen_record: rec,
                        profile_refreshed: prof_ref.as_deref().map(parse_time),
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn search_seen_players(&self, query: &str, limit: usize) -> Vec<SeenPlayer> {
        let q = query.trim();
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        let guard = self.conn.lock();
        let mut out = Vec::new();
        if let Ok(mut stmt) = guard.prepare(
            "SELECT handle, last_seen_plate FROM player_extra
             WHERE last_seen_plate IS NOT NULL AND instr(lower(handle), lower(?1)) > 0
             ORDER BY last_seen_plate DESC LIMIT ?2",
        ) {
            if let Ok(rows) = stmt.query_map(params![q, limit as i64], |r| {
                let h: String = r.get(0)?;
                let ts: String = r.get(1)?;
                Ok(SeenPlayer {
                    handle: h,
                    last_seen_plate: parse_time(&ts),
                })
            }) {
                for item in rows.flatten() {
                    out.push(item);
                }
            }
        }
        out
    }

    pub fn all_cached_handles(&self) -> Vec<String> {
        let guard = self.conn.lock();
        let mut out = Vec::new();
        if let Ok(mut stmt) = guard.prepare("SELECT handle FROM players ORDER BY handle ASC") {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                for h in rows.flatten() {
                    out.push(h);
                }
            }
        }
        out
    }

    pub fn invalidate_player_mem(&self, handle: &str) {
        self.player_mem.remove(handle);
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let sql = format!(
        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?1",
        table
    );
    let count: i64 = conn.query_row(&sql, params![column], |r| r.get(0))?;
    Ok(count > 0)
}

fn format_time(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn parse_time(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

fn join_activities(acts: &[String]) -> String {
    acts.iter()
        .map(|a| a.replace(CacheDatabase::ACTIVITY_SEP, ""))
        .collect::<Vec<_>>()
        .join(CacheDatabase::ACTIVITY_SEP)
}

fn split_activities(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split(CacheDatabase::ACTIVITY_SEP)
            .map(|s| s.to_string())
            .collect()
    }
}
