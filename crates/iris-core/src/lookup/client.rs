use crate::lookup::{
    OrgInfo, OrgMembersPage, OrgMembersParser, OrgPageParser, PlayerOrgs, PlayerOrgsParser,
    PlayerProfile, ProfilePageParser, RsiNames, RsiResult,
};
use parking_lot::Mutex;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use reqwest::StatusCode;
use std::collections::HashSet;
use std::time::{Duration, Instant};

const RSI_HOST: &str = "robertsspaceindustries.com";
const RSI_BASE: &str = "https://robertsspaceindustries.com";

/// Adaptive rate limiter matching IRIS (`1.0s` base gap, doubling up to `30s` on 429/403/5xx/timeout,
/// honoring `Retry-After` up to `120s`, halving back to `1.0s` on 2xx/404).
pub struct RsiRateLimiter {
    state: Mutex<RateState>,
}

struct RateState {
    gap: Duration,
    next_allowed: Instant,
}

impl RsiRateLimiter {
    pub const BASE_GAP: Duration = Duration::from_secs(1);
    pub const MAX_GAP: Duration = Duration::from_secs(30);
    pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(120);

    pub fn new() -> Self {
        Self {
            state: Mutex::new(RateState {
                gap: Self::BASE_GAP,
                next_allowed: Instant::now(),
            }),
        }
    }

    pub fn wait_turn(&self) {
        let sleep_dur = {
            let mut st = self.state.lock();
            let now = Instant::now();
            if st.next_allowed > now {
                let wait = st.next_allowed - now;
                let gap = st.gap;
                st.next_allowed += gap;
                Some(wait)
            } else {
                st.next_allowed = now + st.gap;
                None
            }
        };
        if let Some(d) = sleep_dur {
            std::thread::sleep(d);
        }
    }

    pub fn on_success(&self) {
        let mut st = self.state.lock();
        let half = st.gap / 2;
        st.gap = half.max(Self::BASE_GAP);
    }

    pub fn on_backoff(&self, retry_after_secs: Option<u64>) {
        let mut st = self.state.lock();
        st.gap = (st.gap * 2).min(Self::MAX_GAP);
        if let Some(secs) = retry_after_secs {
            let forced = Duration::from_secs(secs).min(Self::MAX_RETRY_AFTER);
            let target = Instant::now() + forced;
            if target > st.next_allowed {
                st.next_allowed = target;
            }
        }
    }
}

impl Default for RsiRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Circuit breaker: 5 consecutive 403 or unparseable-200 responses pauses RSI queries for 5 minutes.
pub struct RsiCircuitBreaker {
    state: Mutex<BreakerState>,
}

struct BreakerState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

impl RsiCircuitBreaker {
    pub const TRIP_THRESHOLD: u32 = 5;
    pub const PAUSE_DURATION: Duration = Duration::from_secs(300);

    pub fn new() -> Self {
        Self {
            state: Mutex::new(BreakerState {
                consecutive_failures: 0,
                open_until: None,
            }),
        }
    }

    pub fn is_open(&self) -> bool {
        let mut st = self.state.lock();
        if let Some(until) = st.open_until {
            if Instant::now() < until {
                return true;
            }
            st.open_until = None;
            st.consecutive_failures = 0;
        }
        false
    }

    pub fn record_ok(&self) {
        let mut st = self.state.lock();
        st.consecutive_failures = 0;
        st.open_until = None;
    }

    pub fn record_trip_candidate(&self) {
        let mut st = self.state.lock();
        st.consecutive_failures += 1;
        if st.consecutive_failures >= Self::TRIP_THRESHOLD {
            st.open_until = Some(Instant::now() + Self::PAUSE_DURATION);
        }
    }
}

impl Default for RsiCircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

/// Deduplicates in-flight org attribute fetches across concurrent player lookups.
#[derive(Default)]
pub struct OrgFetchGate {
    inflight: Mutex<HashSet<String>>,
}

impl OrgFetchGate {
    pub fn try_enter(&self, sid: &str) -> bool {
        self.inflight.lock().insert(sid.to_ascii_lowercase())
    }

    pub fn leave(&self, sid: &str) {
        self.inflight.lock().remove(&sid.to_ascii_lowercase());
    }
}

/// Strict origin-pinned HTTPS client for `https://robertsspaceindustries.com`.
pub struct RsiClient {
    http: Client,
    limiter: RsiRateLimiter,
    breaker: RsiCircuitBreaker,
}

impl RsiClient {
    pub fn new() -> anyhow::Result<Self> {
        let http = Client::builder()
            .redirect(Policy::custom(|attempt| {
                let url = attempt.url();
                let valid_host = url
                    .host_str()
                    .map(|h| {
                        h.eq_ignore_ascii_case(RSI_HOST)
                            || h.eq_ignore_ascii_case("www.robertsspaceindustries.com")
                    })
                    .unwrap_or(false);
                if url.scheme() == "https" && valid_host && attempt.previous().len() < 4 {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .user_agent(format!(
                "Mozilla/5.0 (compatible; sc-overlay/{})",
                env!("CARGO_PKG_VERSION")
            ))
            .build()?;

        Ok(Self {
            http,
            limiter: RsiRateLimiter::new(),
            breaker: RsiCircuitBreaker::new(),
        })
    }

    pub fn fetch_player_orgs(&self, handle: &str) -> RsiResult<PlayerOrgs> {
        if !RsiNames::is_valid_handle(handle) {
            return RsiResult::NotFound;
        }
        let url = format!("{}/en/citizens/{}/organizations", RSI_BASE, handle);
        match self.get_text(&url, Duration::from_secs(7)) {
            HttpOutcome::NotFound => RsiResult::NotFound,
            HttpOutcome::Error(e) => RsiResult::Failed(e),
            HttpOutcome::Ok(html) => {
                let parsed = PlayerOrgsParser::parse(&html, handle);
                if matches!(parsed, RsiResult::Failed(_)) {
                    self.breaker.record_trip_candidate();
                } else {
                    self.breaker.record_ok();
                }
                parsed
            }
        }
    }

    pub fn fetch_player_profile(&self, handle: &str) -> RsiResult<PlayerProfile> {
        if !RsiNames::is_valid_handle(handle) {
            return RsiResult::NotFound;
        }
        let url = format!("{}/en/citizens/{}", RSI_BASE, handle);
        match self.get_text(&url, Duration::from_secs(7)) {
            HttpOutcome::NotFound => RsiResult::NotFound,
            HttpOutcome::Error(e) => RsiResult::Failed(e),
            HttpOutcome::Ok(html) => {
                let parsed = ProfilePageParser::parse(&html, handle);
                if matches!(parsed, RsiResult::Failed(_)) {
                    self.breaker.record_trip_candidate();
                } else {
                    self.breaker.record_ok();
                }
                parsed
            }
        }
    }

    pub fn fetch_org(&self, sid: &str) -> RsiResult<OrgInfo> {
        if !RsiNames::is_valid_sid(sid) {
            return RsiResult::NotFound;
        }
        let url = format!("{}/en/orgs/{}", RSI_BASE, sid);
        match self.get_text(&url, Duration::from_secs(10)) {
            HttpOutcome::NotFound => RsiResult::NotFound,
            HttpOutcome::Error(e) => RsiResult::Failed(e),
            HttpOutcome::Ok(html) => {
                let parsed = OrgPageParser::parse(&html, sid);
                if matches!(parsed, RsiResult::Failed(_)) {
                    self.breaker.record_trip_candidate();
                } else {
                    self.breaker.record_ok();
                }
                parsed
            }
        }
    }

    pub fn fetch_org_members(&self, sid: &str, page: u32, page_size: u32) -> RsiResult<OrgMembersPage> {
        if !RsiNames::is_valid_sid(sid) {
            return RsiResult::NotFound;
        }
        if self.breaker.is_open() {
            return RsiResult::Failed("RSI circuit breaker open".to_string());
        }
        self.limiter.wait_turn();
        let url = format!("{}/api/orgs/getOrgMembers", RSI_BASE);
        let body = serde_json::json!({
            "symbol": sid,
            "search": "",
            "pagesize": page_size,
            "page": page,
        });
        let resp = self
            .http
            .post(&url)
            .timeout(Duration::from_secs(15))
            .header("X-Requested-With", "XMLHttpRequest")
            .json(&body)
            .send();

        match resp {
            Err(e) => {
                self.limiter.on_backoff(None);
                RsiResult::Failed(format!("network error: {}", e))
            }
            Ok(r) => {
                let status = r.status();
                if status == StatusCode::NOT_FOUND {
                    self.limiter.on_success();
                    self.breaker.record_ok();
                    return RsiResult::NotFound;
                }
                if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    let retry_after = r
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok());
                    self.limiter.on_backoff(retry_after);
                    if status == StatusCode::FORBIDDEN {
                        self.breaker.record_trip_candidate();
                    }
                    return RsiResult::Failed(format!("HTTP {}", status));
                }
                if !status.is_success() {
                    return RsiResult::Failed(format!("HTTP {}", status));
                }
                match r.text() {
                    Ok(txt) => {
                        self.limiter.on_success();
                        let parsed = OrgMembersParser::parse_response(&txt);
                        if matches!(parsed, RsiResult::Failed(_)) {
                            self.breaker.record_trip_candidate();
                        } else {
                            self.breaker.record_ok();
                        }
                        parsed
                    }
                    Err(e) => RsiResult::Failed(format!("body read error: {}", e)),
                }
            }
        }
    }

    fn get_text(&self, url: &str, timeout: Duration) -> HttpOutcome {
        if self.breaker.is_open() {
            return HttpOutcome::Error("RSI circuit breaker open (cooldown)".to_string());
        }
        self.limiter.wait_turn();
        match self.http.get(url).timeout(timeout).send() {
            Err(e) => {
                self.limiter.on_backoff(None);
                HttpOutcome::Error(format!("request failed: {}", e))
            }
            Ok(r) => {
                let status = r.status();
                if status == StatusCode::NOT_FOUND {
                    self.limiter.on_success();
                    self.breaker.record_ok();
                    return HttpOutcome::NotFound;
                }
                if status == StatusCode::FORBIDDEN
                    || status == StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
                {
                    let retry_after = r
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok());
                    self.limiter.on_backoff(retry_after);
                    if status == StatusCode::FORBIDDEN {
                        self.breaker.record_trip_candidate();
                    }
                    return HttpOutcome::Error(format!("HTTP {}", status));
                }
                if !status.is_success() {
                    return HttpOutcome::Error(format!("HTTP {}", status));
                }
                match r.text() {
                    Ok(t) => {
                        self.limiter.on_success();
                        HttpOutcome::Ok(t)
                    }
                    Err(e) => HttpOutcome::Error(format!("failed reading body: {}", e)),
                }
            }
        }
    }
}

enum HttpOutcome {
    Ok(String),
    NotFound,
    Error(String),
}
