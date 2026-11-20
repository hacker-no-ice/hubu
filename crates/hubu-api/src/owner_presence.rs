//! Owner presence (v0.3.0 spec, section 5).
//!
//! Owner actions need the owner's physical presence, confirmed by
//! `hubu-server` itself through a [`PresenceProvider`]. Each confirmation
//! authorizes exactly the one action (or approval) its dialog names; there is
//! no owner window (owner decision, 2026-11-19).
// Unit tests install fixture providers directly, so the environment-driven
// provider selection and the macOS provider are unused in the test build. On
// other platforms the macOS-only policy setting and methods are unused.
#![cfg_attr(any(test, not(target_os = "macos")), allow(dead_code))]
use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    path::Path,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

#[cfg(target_os = "macos")]
mod macos;

pub(crate) const NO_PROMPT_HEADER: &str = "x-hubu-no-prompt";
pub(crate) const CLIENT_HEADER: &str = "x-hubu-client";
pub(crate) const PROVIDER_ENV: &str = "HUBU_OWNER_PRESENCE_PROVIDER";
pub(crate) const FIXTURE_ENV: &str = "HUBU_OWNER_PRESENCE_FIXTURE";
pub(crate) const POLICY_ENV: &str = "HUBU_OWNER_PRESENCE_POLICY";
/// `on` keeps the prompt rate limit for the fixture provider (proof kits that
/// exercise it). Scripted runs leave it off: no person is being prompted.
pub(crate) const FIXTURE_RATE_LIMIT_ENV: &str = "HUBU_OWNER_PRESENCE_FIXTURE_RATE_LIMIT";

pub(crate) const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const RATE_LIMIT_PERIOD: Duration = Duration::from_secs(10 * 60);
/// At most this many refused or timed-out prompts per [`RATE_LIMIT_PERIOD`].
/// Confirmed prompts do not count: the limit stops an agent wearing the owner
/// down, not the owner working.
pub(crate) const RATE_LIMIT_MAX_PROMPTS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresenceMethod {
    Biometric,
    DevicePassword,
    Fixture,
}

impl PresenceMethod {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Biometric => "biometric",
            Self::DevicePassword => "device_password",
            Self::Fixture => "fixture",
        }
    }
}

pub(crate) struct PresenceRequest {
    /// A verb phrase built by Hubu from validated, stored data. Never caller
    /// free text.
    pub(crate) reason: String,
    pub(crate) timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PresenceOutcome {
    Confirmed {
        method: PresenceMethod,
    },
    /// Includes user, app and system cancellation and every dialog error.
    Denied,
    TimedOut,
    Unavailable(String),
}

pub(crate) trait PresenceProvider: Send + Sync {
    /// Blocks until the owner answers, the timeout passes, or the prompt fails.
    fn confirm(&self, request: &PresenceRequest) -> PresenceOutcome;
    /// The provider name reported by `GET /owner/presence`.
    fn name(&self) -> &'static str;
}

/// Answers from a script. Tests and proof kits only.
///
/// The script is a comma-separated list of `confirm`, `deny`, `timeout` and
/// `unavailable`. Answers are used in order and the last one repeats.
pub(crate) struct FixtureProvider {
    script: Mutex<VecDeque<PresenceOutcome>>,
    last: Mutex<PresenceOutcome>,
}

impl FixtureProvider {
    pub(crate) fn from_outcomes(outcomes: Vec<PresenceOutcome>) -> Self {
        let last = outcomes
            .last()
            .cloned()
            .unwrap_or(PresenceOutcome::Confirmed {
                method: PresenceMethod::Fixture,
            });
        Self {
            script: Mutex::new(outcomes.into()),
            last: Mutex::new(last),
        }
    }

    pub(crate) fn parse(script: &str) -> Result<Self> {
        let outcomes = script
            .split(',')
            .map(str::trim)
            .filter(|answer| !answer.is_empty())
            .map(|answer| match answer {
                "confirm" => Ok(PresenceOutcome::Confirmed {
                    method: PresenceMethod::Fixture,
                }),
                "deny" => Ok(PresenceOutcome::Denied),
                "timeout" => Ok(PresenceOutcome::TimedOut),
                "unavailable" => Ok(PresenceOutcome::Unavailable(
                    "fixture provider scripted unavailable".to_string(),
                )),
                other => Err(anyhow!(
                    "{FIXTURE_ENV} accepts confirm, deny, timeout or unavailable, not `{other}`"
                )),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::from_outcomes(outcomes))
    }
}

impl PresenceProvider for FixtureProvider {
    fn confirm(&self, _request: &PresenceRequest) -> PresenceOutcome {
        let next = self
            .script
            .lock()
            .ok()
            .and_then(|mut script| script.pop_front());
        match next {
            Some(outcome) => {
                if let Ok(mut last) = self.last.lock() {
                    *last = outcome.clone();
                }
                outcome
            }
            None => self
                .last
                .lock()
                .map(|last| last.clone())
                .unwrap_or(PresenceOutcome::Denied),
        }
    }

    fn name(&self) -> &'static str {
        "fixture"
    }
}

/// Used where no presence provider can run (no macOS GUI session support).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) struct UnavailableProvider(pub(crate) &'static str);

impl PresenceProvider for UnavailableProvider {
    fn confirm(&self, _request: &PresenceRequest) -> PresenceOutcome {
        PresenceOutcome::Unavailable(self.0.to_string())
    }

    fn name(&self) -> &'static str {
        "none"
    }
}

/// Picks the provider from the environment at server start.
///
/// `HUBU_OWNER_PRESENCE_PROVIDER=fixture` selects the scripted provider, and
/// is refused when the database lives inside a managed profile. Otherwise
/// macOS uses LocalAuthentication and other platforms have no provider.
pub(crate) fn provider_from_env(database_path: &Path) -> Result<Arc<dyn PresenceProvider>> {
    match std::env::var(PROVIDER_ENV).ok().as_deref().map(str::trim) {
        Some("fixture") => {
            if is_inside_managed_profile(database_path) {
                return Err(anyhow!(
                    "{PROVIDER_ENV}=fixture is for tests and proof kits; hubu-server refuses it for a database inside a managed profile (`{}`)",
                    database_path.display()
                ));
            }
            let script = std::env::var(FIXTURE_ENV).unwrap_or_else(|_| "confirm".to_string());
            Ok(Arc::new(FixtureProvider::parse(&script)?))
        }
        None | Some("") | Some("system") => system_provider(),
        Some(other) => Err(anyhow!(
            "{PROVIDER_ENV} accepts `system` or `fixture`, not `{other}`"
        )),
    }
}

#[cfg(target_os = "macos")]
fn system_provider() -> Result<Arc<dyn PresenceProvider>> {
    let policy = match std::env::var(POLICY_ENV).ok().as_deref().map(str::trim) {
        None | Some("") | Some("owner") => macos::LocalAuthPolicy::DeviceOwner,
        Some("biometrics") => macos::LocalAuthPolicy::Biometrics,
        Some(other) => {
            return Err(anyhow!(
                "{POLICY_ENV} accepts `owner` or `biometrics`, not `{other}`"
            ))
        }
    };
    Ok(Arc::new(macos::LocalAuthProvider::new(policy)))
}

#[cfg(not(target_os = "macos"))]
fn system_provider() -> Result<Arc<dyn PresenceProvider>> {
    Ok(Arc::new(UnavailableProvider(
        "owner presence needs macOS LocalAuthentication in the owner's GUI session",
    )))
}

/// A managed profile root holds both `stack.toml` and `credentials.toml`.
///
/// SQLite follows symlinks, so both the path as written and the path with
/// every symlink resolved are checked; either one inside a managed profile
/// refuses the fixture provider.
pub(crate) fn is_inside_managed_profile(database_path: &Path) -> bool {
    let absolute = if database_path.is_absolute() {
        database_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(database_path))
            .unwrap_or_else(|_| database_path.to_path_buf())
    };
    let inside = |path: &Path| {
        path.ancestors()
            .skip(1)
            .any(|dir| dir.join("stack.toml").is_file() && dir.join("credentials.toml").is_file())
    };
    inside(&absolute) || resolved_path(&absolute).is_some_and(|resolved| inside(&resolved))
}

/// The path with symlinks resolved. A database that does not exist yet is
/// resolved through its nearest existing ancestor.
fn resolved_path(path: &Path) -> Option<std::path::PathBuf> {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return Some(resolved);
    }
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        missing.push(current.file_name()?.to_os_string());
        current = current.parent()?;
        if let Ok(resolved) = std::fs::canonicalize(current) {
            return Some(
                missing
                    .iter()
                    .rev()
                    .fold(resolved, |path, part| path.join(part)),
            );
        }
    }
}

/// Which client presented an owner request (spec section 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OwnerClient {
    Portal,
    Cli,
    Agent,
    Unknown,
}

impl OwnerClient {
    pub(crate) fn from_header(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("portal") => Self::Portal,
            Some(value) if value.eq_ignore_ascii_case("cli") => Self::Cli,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Portal => "portal",
            Self::Cli => "cli",
            Self::Agent => "agent",
            Self::Unknown => "unknown",
        }
    }
}

/// Why a prompt did not confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptRefusal {
    InProgress,
    RateLimited { retry_after: Duration },
    Denied,
    TimedOut,
    Unavailable(String),
}

impl PromptRefusal {
    /// The governance `outcome` for this refusal.
    pub(crate) fn outcome(&self) -> &'static str {
        match self {
            Self::InProgress => "superseded",
            Self::RateLimited { .. } => "rate_limited",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Unavailable(_) => "unavailable",
        }
    }

    pub(crate) fn error_code(&self) -> &'static str {
        match self {
            Self::InProgress => "owner_prompt_in_progress",
            Self::RateLimited { .. } => "owner_prompt_rate_limited",
            Self::Denied => "owner_presence_denied",
            Self::TimedOut => "owner_presence_timeout",
            Self::Unavailable(_) => "owner_presence_unavailable",
        }
    }

    pub(crate) fn status(&self) -> u16 {
        match self {
            Self::InProgress => 409,
            Self::RateLimited { .. } => 429,
            _ => 403,
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::InProgress => {
                "another owner confirmation is already on screen; wait for it to finish".to_string()
            }
            Self::RateLimited { retry_after } => format!(
                "too many owner confirmations were requested recently; try again in {} seconds",
                retry_after.as_secs().max(1)
            ),
            Self::Denied => "the owner did not confirm this action".to_string(),
            Self::TimedOut => "the owner did not answer the confirmation in time".to_string(),
            Self::Unavailable(reason) => format!("owner confirmation is unavailable: {reason}"),
        }
    }

    pub(crate) fn retry_guidance(&self) -> Value {
        match self {
            Self::InProgress => json!({
                "action": "retry_later",
                "message": "retry after the owner answers the confirmation that is on screen"
            }),
            Self::RateLimited { retry_after } => json!({
                "action": "retry_later",
                "retry_after_seconds": retry_after.as_secs().max(1),
                "message": "do not keep prompting the owner; wait before asking again"
            }),
            Self::Denied => json!({
                "action": "ask_owner",
                "message": "the owner refused; do not retry unless the owner asks you to"
            }),
            Self::TimedOut => json!({
                "action": "ask_owner",
                "message": "ask the owner whether they are at the Mac before trying again"
            }),
            Self::Unavailable(_) => json!({
                "action": "ask_owner",
                "message": "run hubu-server in the owner's macOS GUI session so it can ask for Touch ID"
            }),
        }
    }
}

#[derive(Default)]
struct PresenceState {
    prompting: bool,
    /// When recent prompts were refused or timed out.
    recent_refusals: VecDeque<Instant>,
}

/// Server-memory owner presence state: the provider, whether a dialog is on
/// screen, and the refusal rate limit. It holds no authority.
pub(crate) struct OwnerPresence {
    provider: RwLock<Arc<dyn PresenceProvider>>,
    state: Mutex<PresenceState>,
    rate_limit: RwLock<Option<(usize, Duration)>>,
    clock_offset: Mutex<Duration>,
}

/// Clears `prompting` even if the provider panics.
struct PromptGuard<'a>(&'a OwnerPresence);

impl Drop for PromptGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.prompting = false;
        }
    }
}

impl OwnerPresence {
    pub(crate) fn new(provider: Arc<dyn PresenceProvider>) -> Self {
        Self {
            provider: RwLock::new(provider),
            state: Mutex::new(PresenceState::default()),
            rate_limit: RwLock::new(Some((RATE_LIMIT_MAX_PROMPTS, RATE_LIMIT_PERIOD))),
            clock_offset: Mutex::new(Duration::ZERO),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_provider(&self, provider: Arc<dyn PresenceProvider>) {
        *self.provider.write().expect("presence provider lock") = provider;
    }

    pub(crate) fn set_rate_limit(&self, limit: Option<(usize, Duration)>) {
        *self.rate_limit.write().expect("presence rate limit lock") = limit;
    }

    #[cfg(test)]
    pub(crate) fn advance_clock(&self, by: Duration) {
        *self.clock_offset.lock().expect("presence clock lock") += by;
    }

    fn now(&self) -> Instant {
        Instant::now() + self.clock_offset.lock().map(|o| *o).unwrap_or_default()
    }

    fn limit(&self) -> Option<(usize, Duration)> {
        self.rate_limit.read().ok().and_then(|limit| *limit)
    }

    pub(crate) fn provider_name(&self) -> &'static str {
        self.provider
            .read()
            .map(|provider| provider.name())
            .unwrap_or("unknown")
    }

    /// Raises one dialog. Holds no lock while the provider waits for the owner.
    pub(crate) fn prompt(&self, reason: &str) -> Result<PresenceMethod, PromptRefusal> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| PromptRefusal::Unavailable("presence state poisoned".to_string()))?;
            if state.prompting {
                return Err(PromptRefusal::InProgress);
            }
            if let Some((max, period)) = self.limit() {
                let now = self.now();
                while state
                    .recent_refusals
                    .front()
                    .is_some_and(|at| now.duration_since(*at) >= period)
                {
                    state.recent_refusals.pop_front();
                }
                if state.recent_refusals.len() >= max {
                    let oldest = *state.recent_refusals.front().expect("non-empty when full");
                    return Err(PromptRefusal::RateLimited {
                        retry_after: period.saturating_sub(now.duration_since(oldest)),
                    });
                }
            }
            state.prompting = true;
        }
        let _guard = PromptGuard(self);
        let provider = self
            .provider
            .read()
            .map(|provider| Arc::clone(&provider))
            .map_err(|_| PromptRefusal::Unavailable("presence provider poisoned".to_string()))?;
        let outcome = provider.confirm(&PresenceRequest {
            reason: reason.to_string(),
            timeout: PROMPT_TIMEOUT,
        });
        let refused = |state: &Self| {
            let now = state.now();
            if let Ok(mut inner) = state.state.lock() {
                inner.recent_refusals.push_back(now);
            }
        };
        match outcome {
            PresenceOutcome::Confirmed { method } => Ok(method),
            PresenceOutcome::Denied => {
                refused(self);
                Err(PromptRefusal::Denied)
            }
            PresenceOutcome::TimedOut => {
                refused(self);
                Err(PromptRefusal::TimedOut)
            }
            // No dialog was shown, so it does not count toward the limit.
            PresenceOutcome::Unavailable(reason) => Err(PromptRefusal::Unavailable(reason)),
        }
    }

    pub(crate) fn status(&self) -> Value {
        let now = self.now();
        let limit = self.limit();
        let (prompting, recent) = self.state.lock().map_or((false, 0), |state| {
            let period = limit.map_or(RATE_LIMIT_PERIOD, |(_, period)| period);
            (
                state.prompting,
                state
                    .recent_refusals
                    .iter()
                    .filter(|at| now.duration_since(**at) < period)
                    .count(),
            )
        });
        json!({
            "provider": self.provider_name(),
            "prompt_in_progress": prompting,
            "refusals_remaining": limit.map(|(max, _)| max.saturating_sub(recent)),
        })
    }
}

pub(crate) fn random_hex(bytes: usize) -> Result<String> {
    let mut buffer = vec![0_u8; bytes];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut buffer))
        .map_err(|error| anyhow!("read randomness for an owner presence record id: {error}"))?;
    Ok(super::hex_encode(&buffer))
}

/// Reduces an agent-controlled label to `[A-Za-z0-9._-]`, at most 32 chars.
pub(crate) fn sanitize_label(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(32)
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

/// Like [`sanitize_label`] for public ids, which can be longer.
pub(crate) fn sanitize_id(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(outcomes: Vec<PresenceOutcome>) -> OwnerPresence {
        OwnerPresence::new(Arc::new(FixtureProvider::from_outcomes(outcomes)))
    }

    fn confirm() -> PresenceOutcome {
        PresenceOutcome::Confirmed {
            method: PresenceMethod::Fixture,
        }
    }

    #[test]
    fn deny_timeout_and_unavailable_are_refusals() {
        let presence = presence(vec![
            PresenceOutcome::Denied,
            PresenceOutcome::TimedOut,
            PresenceOutcome::Unavailable("no gui".to_string()),
        ]);
        assert_eq!(presence.prompt("a"), Err(PromptRefusal::Denied));
        assert_eq!(presence.prompt("a"), Err(PromptRefusal::TimedOut));
        assert_eq!(
            presence.prompt("a"),
            Err(PromptRefusal::Unavailable("no gui".to_string()))
        );
    }

    #[test]
    fn rate_limit_allows_five_refusals_per_ten_minutes() {
        let presence = presence(vec![PresenceOutcome::Denied]);
        for _ in 0..5 {
            assert_eq!(presence.prompt("a"), Err(PromptRefusal::Denied));
        }
        let Err(PromptRefusal::RateLimited { retry_after }) = presence.prompt("a") else {
            panic!("sixth prompt should be rate limited");
        };
        assert!(retry_after <= RATE_LIMIT_PERIOD);
        presence.advance_clock(RATE_LIMIT_PERIOD);
        assert_eq!(presence.prompt("a"), Err(PromptRefusal::Denied));
    }

    #[test]
    fn confirmed_prompts_do_not_count_toward_the_limit() {
        let presence = presence(vec![confirm()]);
        for _ in 0..20 {
            assert_eq!(presence.prompt("a"), Ok(PresenceMethod::Fixture));
        }
        assert_eq!(presence.status()["refusals_remaining"], 5);
    }

    #[test]
    fn unavailable_prompts_do_not_count_toward_the_limit() {
        let presence = presence(vec![PresenceOutcome::Unavailable("x".to_string())]);
        for _ in 0..10 {
            assert!(matches!(
                presence.prompt("a"),
                Err(PromptRefusal::Unavailable(_))
            ));
        }
    }

    struct BlockingProvider {
        entered: std::sync::Barrier,
        release: std::sync::Barrier,
    }

    impl PresenceProvider for BlockingProvider {
        fn confirm(&self, _request: &PresenceRequest) -> PresenceOutcome {
            self.entered.wait();
            self.release.wait();
            PresenceOutcome::Confirmed {
                method: PresenceMethod::Fixture,
            }
        }
        fn name(&self) -> &'static str {
            "blocking"
        }
    }

    #[test]
    fn a_second_prompt_while_one_is_on_screen_is_rejected() {
        let provider = Arc::new(BlockingProvider {
            entered: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
        });
        let presence = Arc::new(OwnerPresence::new(provider.clone()));
        let first = {
            let presence = Arc::clone(&presence);
            std::thread::spawn(move || presence.prompt("first"))
        };
        provider.entered.wait();
        assert_eq!(presence.prompt("second"), Err(PromptRefusal::InProgress));
        provider.release.wait();
        assert_eq!(first.join().unwrap(), Ok(PresenceMethod::Fixture));
        assert!(!presence.state.lock().unwrap().prompting);
    }

    #[test]
    fn fixture_script_parses_and_repeats_its_last_answer() {
        let fixture = FixtureProvider::parse("deny, confirm").unwrap();
        let request = PresenceRequest {
            reason: "r".to_string(),
            timeout: PROMPT_TIMEOUT,
        };
        assert_eq!(fixture.confirm(&request), PresenceOutcome::Denied);
        assert_eq!(fixture.confirm(&request), confirm());
        assert_eq!(fixture.confirm(&request), confirm());
        assert!(FixtureProvider::parse("approve").is_err());
    }

    #[test]
    fn fixture_is_refused_inside_a_managed_profile() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("stack.toml"), "").unwrap();
        std::fs::write(root.path().join("credentials.toml"), "").unwrap();
        std::fs::create_dir_all(root.path().join("state/hubu")).unwrap();
        assert!(is_inside_managed_profile(
            &root.path().join("state/hubu/hubu.sqlite3")
        ));
        let other = tempfile::tempdir().unwrap();
        assert!(!is_inside_managed_profile(
            &other.path().join("hubu.sqlite3")
        ));

        // A symlink outside the profile to the managed database file.
        let database = root.path().join("state/hubu/hubu.sqlite3");
        std::fs::write(&database, "").unwrap();
        let file_link = other.path().join("linked.sqlite3");
        std::os::unix::fs::symlink(&database, &file_link).unwrap();
        assert!(is_inside_managed_profile(&file_link));
        // A symlinked parent directory, with a database not created yet.
        let dir_link = other.path().join("linked-state");
        std::os::unix::fs::symlink(root.path().join("state/hubu"), &dir_link).unwrap();
        assert!(is_inside_managed_profile(&dir_link.join("new.sqlite3")));
        assert!(is_inside_managed_profile(
            &dir_link.join("nested/new.sqlite3")
        ));
    }

    #[test]
    fn labels_are_reduced_to_a_safe_alphabet() {
        assert_eq!(sanitize_label("design-agent"), "design-agent");
        assert_eq!(
            sanitize_label("evil\"\n approve $9999 now"),
            "evilapprove9999now"
        );
        assert_eq!(sanitize_label(&"a".repeat(80)).len(), 32);
        assert_eq!(sanitize_label("dеsign"), "dsign"); // Cyrillic е removed
        assert_eq!(sanitize_label("\u{202e}"), "unnamed");
    }
}
