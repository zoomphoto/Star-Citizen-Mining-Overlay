pub mod cache;
pub mod client;
pub mod parsers;

pub use cache::{CacheDatabase, LruCache, NotFoundEntry, NotFoundKind, PlayerExtra, SeenPlayer};
pub use client::{OrgFetchGate, RsiCircuitBreaker, RsiClient, RsiRateLimiter};
pub use parsers::{OrgMembersParser, OrgPageParser, PlayerOrgsParser, ProfilePageParser};

use crate::models::OrgMembership;

pub struct RsiNames;

impl RsiNames {
    pub const MIN_HANDLE_LENGTH: usize = 4;
    pub const MAX_HANDLE_LENGTH: usize = 64;
    pub const MIN_SID_LENGTH: usize = 1;
    pub const MAX_SID_LENGTH: usize = 20;

    pub fn is_valid_handle(handle: &str) -> bool {
        let len = handle.len();
        (Self::MIN_HANDLE_LENGTH..=Self::MAX_HANDLE_LENGTH).contains(&len)
            && handle
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }

    pub fn is_valid_sid(sid: &str) -> bool {
        let len = sid.len();
        (Self::MIN_SID_LENGTH..=Self::MAX_SID_LENGTH).contains(&len)
            && sid
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RsiResult<T> {
    Found(T),
    NotFound,
    Failed(String),
}

impl<T> RsiResult<T> {
    pub fn is_found(&self) -> bool {
        matches!(self, RsiResult::Found(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerOrgs {
    pub handle: String,
    pub orgs: Vec<OrgMembership>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerProfile {
    pub handle: String,
    pub moniker: Option<String>,
    pub citizen_record: Option<String>,
    pub has_label: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgInfo {
    pub sid: String,
    pub long_name: String,
    pub member_count: Option<i32>,
    pub archetype: Option<String>,
    pub commitment: Option<String>,
    pub is_exclusive: bool,
    pub activities: Vec<String>,
    pub unknown_activities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgMembersPage {
    pub handles: Vec<String>,
    pub entries_seen: usize,
    pub total_rows: Option<i32>,
}
