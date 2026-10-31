//! The status line's input, read field by field from the JSON Claude Code sends. Each field
//! is checked on its own, so one missing or malformed field leaves every other readable.

use serde_json::Value;

/// One field of the input: its checked value, or why it holds none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field<T> {
    Present(T),
    Absent,
    /// The field is there and fails its check; the JSON pointer names it.
    Malformed(&'static str),
}

impl<T> Field<T> {
    /// The checked value, where the field holds one.
    pub const fn value(&self) -> Option<&T> {
        match self {
            Self::Present(value) => Some(value),
            Self::Absent | Self::Malformed(_) => None,
        }
    }
}

/// Every field the status line shows, each checked on its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub current_dir: Field<String>,
    pub project_dir: Field<String>,
    pub model_id: Field<String>,
    pub model_display_name: Field<String>,
    pub effort: Field<String>,
    pub context_tokens: Field<u64>,
    pub cache: Field<Cache>,
    pub seven_day: Field<Window>,
    pub spend_limit: Field<SpendLimit>,
    pub session_cost_usd: Field<f64>,
    pub pull_request: Field<PullRequest>,
}

/// The main conversation's prompt cache, as of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cache {
    pub observed: bool,
    pub warm: bool,
    /// Epoch seconds at which the cached prefix goes cold, where the last response cached.
    pub expires_at: Option<i64>,
}

/// A usage window: the share of its limit used, and when it resets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub used_percentage: f64,
    pub resets_at: i64,
}

/// A gateway's spend limit. The dollar figures and the period arrive after the percentage,
/// so either can be absent while the limit is present.
#[derive(Debug, Clone, PartialEq)]
pub struct SpendLimit {
    pub used_percentage: f64,
    pub resets_at: i64,
    pub dollars: Field<Dollars>,
    pub period: Field<Period>,
}

/// The period's spend and its limit, in US dollars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dollars {
    pub used: f64,
    pub limit: f64,
}

/// The span of time a spend limit covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Daily,
    Weekly,
    Monthly,
}

/// The open pull request, or GitLab merge request, for the current branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub merge_request: bool,
}

impl Input {
    /// Reads every field the status line shows from Claude Code's input. A value that is
    /// not an object reads as every field absent.
    #[must_use]
    pub fn read(input: &Value) -> Self {
        let workspace_dir = text(input, "/workspace/current_dir");
        let current_dir = match workspace_dir {
            Field::Absent => text(input, "/cwd"),
            Field::Present(_) | Field::Malformed(_) => workspace_dir,
        };
        Self {
            current_dir,
            project_dir: text(input, "/workspace/project_dir"),
            model_id: text(input, "/model/id"),
            model_display_name: text(input, "/model/display_name"),
            effort: text(input, "/effort/level"),
            context_tokens: count(input, "/context_window/total_input_tokens"),
            cache: cache(input),
            seven_day: window(input),
            spend_limit: spend_limit(input),
            session_cost_usd: amount(input, "/cost/total_cost_usd"),
            pull_request: pull_request(input),
        }
    }
}

fn text(input: &Value, pointer: &'static str) -> Field<String> {
    match input.pointer(pointer) {
        None | Some(Value::Null) => Field::Absent,
        Some(Value::String(text)) if !text.is_empty() => Field::Present(text.clone()),
        Some(_) => Field::Malformed(pointer),
    }
}

fn flag(input: &Value, pointer: &'static str) -> Field<bool> {
    match input.pointer(pointer) {
        None | Some(Value::Null) => Field::Absent,
        Some(Value::Bool(flag)) => Field::Present(*flag),
        Some(_) => Field::Malformed(pointer),
    }
}

fn count(input: &Value, pointer: &'static str) -> Field<u64> {
    match input.pointer(pointer) {
        None | Some(Value::Null) => Field::Absent,
        Some(value) => value
            .as_u64()
            .map_or(Field::Malformed(pointer), Field::Present),
    }
}

/// A non-negative, finite number.
fn amount(input: &Value, pointer: &'static str) -> Field<f64> {
    match input.pointer(pointer) {
        None | Some(Value::Null) => Field::Absent,
        Some(value) => match value.as_f64() {
            Some(amount) if amount.is_finite() && amount >= 0.0 => Field::Present(amount),
            Some(_) | None => Field::Malformed(pointer),
        },
    }
}

/// A positive count of epoch seconds.
fn seconds(input: &Value, pointer: &'static str) -> Field<i64> {
    match input.pointer(pointer) {
        None | Some(Value::Null) => Field::Absent,
        Some(value) => match value.as_i64() {
            Some(seconds) if seconds > 0 => Field::Present(seconds),
            Some(_) | None => Field::Malformed(pointer),
        },
    }
}

fn cache(input: &Value) -> Field<Cache> {
    const POINTER: &str = "/prompt_cache";
    if input.pointer(POINTER).is_none() {
        return Field::Absent;
    }
    let observed = flag(input, "/prompt_cache/caching_observed");
    let warm = flag(input, "/prompt_cache/warm");
    let expires_at = seconds(input, "/prompt_cache/expires_at");
    match (observed, warm, expires_at) {
        (Field::Present(observed), Field::Present(warm), Field::Present(expires_at)) => {
            Field::Present(Cache {
                observed,
                warm,
                expires_at: Some(expires_at),
            })
        }
        (Field::Present(observed), Field::Present(warm), Field::Absent) => Field::Present(Cache {
            observed,
            warm,
            expires_at: None,
        }),
        _ => Field::Malformed(POINTER),
    }
}

fn window(input: &Value) -> Field<Window> {
    const POINTER: &str = "/rate_limits/seven_day";
    if input.pointer(POINTER).is_none() {
        return Field::Absent;
    }
    let used = amount(input, "/rate_limits/seven_day/used_percentage");
    let resets_at = seconds(input, "/rate_limits/seven_day/resets_at");
    match (used, resets_at) {
        (Field::Present(used_percentage), Field::Present(resets_at)) => Field::Present(Window {
            used_percentage,
            resets_at,
        }),
        _ => Field::Malformed(POINTER),
    }
}

fn spend_limit(input: &Value) -> Field<SpendLimit> {
    const POINTER: &str = "/rate_limits/spend_limit";
    if input.pointer(POINTER).is_none() {
        return Field::Absent;
    }
    let used = amount(input, "/rate_limits/spend_limit/used_percentage");
    let resets_at = seconds(input, "/rate_limits/spend_limit/resets_at");
    match (used, resets_at) {
        (Field::Present(used_percentage), Field::Present(resets_at)) => {
            Field::Present(SpendLimit {
                used_percentage,
                resets_at,
                dollars: dollars(input),
                period: period(input),
            })
        }
        _ => Field::Malformed(POINTER),
    }
}

fn dollars(input: &Value) -> Field<Dollars> {
    let used = amount(input, "/rate_limits/spend_limit/used_usd");
    let limit = amount(input, "/rate_limits/spend_limit/limit_usd");
    match (used, limit) {
        (Field::Absent, Field::Absent) => Field::Absent,
        (Field::Present(used), Field::Present(limit)) if limit > 0.0 => {
            Field::Present(Dollars { used, limit })
        }
        _ => Field::Malformed("/rate_limits/spend_limit/limit_usd"),
    }
}

fn period(input: &Value) -> Field<Period> {
    const POINTER: &str = "/rate_limits/spend_limit/period";
    match text(input, POINTER) {
        Field::Present(name) => match name.as_str() {
            "daily" => Field::Present(Period::Daily),
            "weekly" => Field::Present(Period::Weekly),
            "monthly" => Field::Present(Period::Monthly),
            _ => Field::Malformed(POINTER),
        },
        Field::Absent => Field::Absent,
        Field::Malformed(pointer) => Field::Malformed(pointer),
    }
}

fn pull_request(input: &Value) -> Field<PullRequest> {
    const POINTER: &str = "/pr/number";
    match count(input, POINTER) {
        Field::Present(number) if number > 0 => {
            let kind = text(input, "/pr/kind");
            let merge_request = kind.value().is_some_and(|kind| kind == "mr");
            Field::Present(PullRequest {
                number,
                merge_request,
            })
        }
        Field::Present(_) => Field::Malformed(POINTER),
        Field::Absent => Field::Absent,
        Field::Malformed(pointer) => Field::Malformed(pointer),
    }
}
