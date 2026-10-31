//! The status line's lines, rendered from the input and the facts the shell gathers: where
//! the session stands, the session itself, and the account's usage or spend.

use jiff::{Timestamp, ToSpan, Zoned};

use super::calendar::business_days;
use super::git::{GitState, Head};
use super::input::{Dollars, Field, Input, Period, SpendLimit, Window};
use super::spend::Pace;
use super::style::{Color, Line};

const CONTEXT_TOKENS_YELLOW_MIN: u64 = 100_000;
const CONTEXT_TOKENS_RED_MIN: u64 = 200_000;
const TOKENS_PER_THOUSAND: u64 = 1_000;
const TOKENS_HALF_THOUSAND: u64 = 500;
const CACHE_WARNING_SECONDS: i64 = 600;
const SECONDS_PER_MINUTE: i64 = 60;
const SEVEN_DAY_WINDOW_SECONDS: i64 = 604_800;
const PACE_POINTS_YELLOW_MAX: f64 = 10.0;
const USED_PERCENTAGE_RED_MIN: f64 = 90.0;
const DAY_SPEND_YELLOW_RATIO_MAX: f64 = 1.25;
const PERCENT: f64 = 100.0;
const NOON_HOUR: i8 = 12;
const THOUSANDS_GROUP: usize = 3;

/// What the shell gathers beside the input.
pub struct Facts<'a> {
    pub now: &'a Zoned,
    pub home: Option<&'a str>,
    pub git: Option<&'a GitState>,
    /// Whether the session's project holds a `.pirr/` directory.
    pub pirr_checkout: bool,
    /// The spend limit's spend at the day's first reading, where it is known.
    pub day_start_usd: Option<f64>,
}

/// The status line's lines, each left out where it has nothing to show.
#[must_use]
pub fn render(input: &Input, facts: &Facts) -> Vec<Line> {
    let mut lines = vec![
        location(input, facts),
        session(input, facts),
        account(input, facts),
    ];
    lines.retain(Line::has_text);
    lines
}

fn location(input: &Input, facts: &Facts) -> Line {
    let mut line = Line::default();
    if let Some(dir) = input.current_dir.value() {
        line.push(display_dir(dir, facts.home), Color::Plain);
    }
    if let Some(git) = facts.git {
        let head = match &git.head {
            Head::Branch(name) | Head::Detached(name) => name.clone(),
        };
        line.push_after(" on ", head, Color::Blue);
        let markers = [
            (git.staged, "+", Color::Green),
            (git.unstaged, "*", Color::Green),
            (git.untracked, "?", Color::Yellow),
        ];
        for (marked, marker, color) in markers {
            if marked {
                line.push(marker, color);
            }
        }
    }
    if let Some(pull_request) = input.pull_request.value() {
        let label = if pull_request.merge_request {
            format!("MR !{}", pull_request.number)
        } else {
            format!("PR #{}", pull_request.number)
        };
        line.push_after(" · ", label, Color::Plain);
    }
    line
}

/// The directory with the home directory written as `~`.
fn display_dir(dir: &str, home: Option<&str>) -> String {
    let Some(rest) = home.and_then(|home| dir.strip_prefix(home)) else {
        return dir.to_owned();
    };
    if rest.is_empty() {
        "~".to_owned()
    } else if rest.starts_with('/') {
        format!("~{rest}")
    } else {
        dir.to_owned()
    }
}

fn session(input: &Input, facts: &Facts) -> Line {
    let mut line = Line::default();
    let label = [model_label(input), input.effort.value().cloned()];
    for word in label.into_iter().flatten() {
        line.push_after(" ", word, Color::Plain);
    }
    let mut state = Line::default();
    if let Some(&tokens) = input.context_tokens.value() {
        if tokens > 0 {
            state.push(format_tokens(tokens), context_color(tokens));
        }
    }
    cache_state(&mut state, input, facts);
    line.append(" | ", state);
    line
}

/// The family word of a `claude-<family>-<version>` id, or the display name lowercased.
fn model_label(input: &Input) -> Option<String> {
    let family = input.model_id.value().and_then(|id| {
        let word = id.strip_prefix("claude-")?.split('-').next()?;
        word.chars()
            .all(|letter| letter.is_ascii_lowercase())
            .then(|| word.to_owned())
    });
    family.filter(|word| !word.is_empty()).or_else(|| {
        input
            .model_display_name
            .value()
            .map(|name| name.to_lowercase())
    })
}

const fn context_color(tokens: u64) -> Color {
    if tokens < CONTEXT_TOKENS_YELLOW_MIN {
        Color::Green
    } else if tokens < CONTEXT_TOKENS_RED_MIN {
        Color::Yellow
    } else {
        Color::Red
    }
}

/// Tokens in thousands, rounded to the nearest.
fn format_tokens(tokens: u64) -> String {
    let rounded = tokens.saturating_add(TOKENS_HALF_THOUSAND);
    let Some(thousands) = rounded.checked_div(TOKENS_PER_THOUSAND) else {
        unreachable!("the divisor is a nonzero constant")
    };
    format!("{thousands}k")
}

fn cache_state(line: &mut Line, input: &Input, facts: &Facts) {
    let Some(cache) = input.cache.value() else {
        return;
    };
    if !cache.observed {
        return;
    }
    let now = facts.now.timestamp().as_second();
    let warm = cache.warm && cache.expires_at.is_none_or(|expires_at| expires_at > now);
    if warm {
        line.push_after(" · ", "warm", Color::Green);
        if let Some(expires_at) = cache.expires_at {
            let left = expires_at.saturating_sub(now);
            let minutes = left.saturating_add(SECONDS_PER_MINUTE - 1) / SECONDS_PER_MINUTE;
            let color = if left <= CACHE_WARNING_SECONDS {
                Color::Yellow
            } else {
                Color::Plain
            };
            line.push_after(" · ", format!("{minutes}m"), color);
        }
    } else {
        line.push_after(" · ", "cold", Color::Red);
        let hint = if facts.pirr_checkout {
            "/clear → /pirr:pickup"
        } else {
            "/clear"
        };
        line.push_after(" · ", hint, Color::Plain);
    }
}

fn account(input: &Input, facts: &Facts) -> Line {
    match (&input.seven_day, &input.spend_limit) {
        (Field::Present(_) | Field::Absent | Field::Malformed(_), Field::Present(limit)) => {
            cost_line(input, limit, facts)
        }
        (Field::Present(window), Field::Absent) => usage_line(window, facts.now),
        (Field::Absent, Field::Absent) => session_cost_line(input),
        (Field::Malformed(_), Field::Absent) | (_, Field::Malformed(_)) => Line::default(),
    }
}

fn usage_line(window: &Window, now: &Zoned) -> Line {
    let mut line = Line::default();
    let start = window.resets_at.saturating_sub(SEVEN_DAY_WINDOW_SECONDS);
    let elapsed = share_elapsed(start, window.resets_at, now.timestamp().as_second());
    line.push("7d ", Color::Plain);
    line.push(
        format!("{:.0}%", window.used_percentage),
        pace_color(window.used_percentage, elapsed),
    );
    if let Ok(reset) = Timestamp::from_second(window.resets_at) {
        let local = reset.to_zoned(now.time_zone().clone());
        line.push(format!(" · resets {}", format_reset(&local)), Color::Plain);
    }
    line
}

/// The percentage of the span from `start` to `end` that has passed at `now`.
fn share_elapsed(start: i64, end: i64, now: i64) -> Option<f64> {
    let length = i32::try_from(end.checked_sub(start)?).ok()?;
    let elapsed = i32::try_from(now.checked_sub(start)?.clamp(0, i64::from(length))).ok()?;
    if length > 0 {
        Some(f64::from(elapsed) / f64::from(length) * PERCENT)
    } else {
        None
    }
}

fn pace_color(used_percentage: f64, elapsed_percentage: Option<f64>) -> Color {
    if used_percentage >= USED_PERCENTAGE_RED_MIN {
        return Color::Red;
    }
    let Some(elapsed) = elapsed_percentage else {
        return Color::Plain;
    };
    let ahead = used_percentage - elapsed;
    if ahead <= 0.0 {
        Color::Green
    } else if ahead <= PACE_POINTS_YELLOW_MAX {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn session_cost_line(input: &Input) -> Line {
    let mut line = Line::default();
    if let Some(&cost) = input.session_cost_usd.value() {
        if cost > 0.0 {
            line.push(format_dollars(cost), Color::Plain);
        }
    }
    line
}

fn cost_line(input: &Input, limit: &SpendLimit, facts: &Facts) -> Line {
    let mut line = Line::default();
    if let Some(&cost) = input.session_cost_usd.value() {
        line.push(format_dollars(cost), Color::Plain);
    }
    let Some(dollars) = limit.dollars.value() else {
        line.push_after(
            " | ",
            format!("spend {:.0}%", limit.used_percentage),
            Color::Plain,
        );
        return line;
    };
    let today = facts.now.date();
    let last_day = Timestamp::from_second(limit.resets_at.saturating_sub(1))
        .ok()
        .map(|moment| moment.to_zoned(facts.now.time_zone().clone()).date());
    let days_left = last_day.and_then(|last_day| business_days(today, last_day).ok());
    if let (Some(day_start), Some(days_left)) = (facts.day_start_usd, days_left) {
        line.append(" | ", day_segment(dollars, day_start, days_left));
    }
    let elapsed = period_elapsed(limit, facts.now);
    line.push_after(
        " | ",
        format_dollars(dollars.used),
        pace_color(limit.used_percentage, elapsed),
    );
    line.push(
        format!(" / {}", format_dollars(dollars.limit)),
        Color::Plain,
    );
    let tomorrow = today.tomorrow().ok();
    let days_after = tomorrow
        .zip(last_day)
        .and_then(|(from, through)| business_days(from, through).ok());
    if let Some(days) = days_after {
        let unit = if days == 1 { "day" } else { "days" };
        line.push_after(" | ", format!("{days} {unit}"), Color::Plain);
    }
    line
}

/// The day's spend over the pace, colored by their ratio.
fn day_segment(dollars: &Dollars, day_start_usd: f64, business_days_left: u32) -> Line {
    let pace = Pace {
        limit_usd: dollars.limit,
        day_start_usd,
        business_days_left,
    }
    .per_day();
    let spent = (dollars.used - day_start_usd).max(0.0);
    let color = if spent <= pace {
        Color::Green
    } else if spent <= pace * DAY_SPEND_YELLOW_RATIO_MAX {
        Color::Yellow
    } else {
        Color::Red
    };
    let mut line = Line::default();
    line.push(format_dollars(spent), color);
    line.push(format!(" / {}", format_dollars(pace)), Color::Plain);
    line
}

/// The percentage of the limit's period that has passed, where its period is known.
fn period_elapsed(limit: &SpendLimit, now: &Zoned) -> Option<f64> {
    let period = limit.period.value()?;
    let reset = Timestamp::from_second(limit.resets_at)
        .ok()?
        .to_zoned(now.time_zone().clone());
    let start = match period {
        Period::Daily => reset.checked_sub(1.day()),
        Period::Weekly => reset.checked_sub(1.week()),
        Period::Monthly => reset.checked_sub(1.month()),
    }
    .ok()?;
    share_elapsed(
        start.timestamp().as_second(),
        limit.resets_at,
        now.timestamp().as_second(),
    )
}

/// US dollars with thousands grouped, and the cents dropped from a whole amount.
///
/// # Panics
///
/// Where the amount is negative or not finite, which the input's check excludes.
#[must_use]
pub fn format_dollars(amount: f64) -> String {
    assert!(amount.is_finite(), "a dollar amount is finite");
    assert!(amount >= 0.0, "a dollar amount is not negative");
    let fixed = format!("{amount:.2}");
    let Some((whole, cents)) = fixed.split_once('.') else {
        unreachable!("a two-place format holds a decimal point: {fixed}")
    };
    let mut grouped = String::with_capacity(whole.len().saturating_mul(2));
    for (index, digit) in whole.chars().enumerate() {
        let remaining = whole.len().saturating_sub(index);
        if index > 0 {
            if remaining.checked_rem(THOUSANDS_GROUP) == Some(0) {
                grouped.push(',');
            }
        }
        grouped.push(digit);
    }
    if cents == "00" {
        format!("${grouped}")
    } else {
        format!("${grouped}.{cents}")
    }
}

/// A reset moment as month/day and twelve-hour time, the minutes shown where nonzero.
#[must_use]
pub fn format_reset(moment: &Zoned) -> String {
    let hour = moment.hour();
    let clock_hour = match hour {
        0 => NOON_HOUR,
        1..=NOON_HOUR => hour,
        _ => hour.saturating_sub(NOON_HOUR),
    };
    let suffix = if hour < NOON_HOUR { "am" } else { "pm" };
    let time = match moment.minute() {
        0 => format!("{clock_hour}{suffix}"),
        minute => format!("{clock_hour}:{minute:02}{suffix}"),
    };
    format!("{}/{} @ {time}", moment.month(), moment.day())
}
