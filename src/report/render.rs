//! A report printed as text for a person, or as JSON for an agent. Both forms list every
//! group most costly first.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Map, Number, Value, json};
use std::cmp::Reverse;

use super::context::{Band, ContextSection, LONG_CONTEXT_FLOOR_TOKENS};
use super::cost::{CostSection, Spend, Ttl, Writes};
use super::price::PRICES_AS_OF;
use super::range::{Range, Thread};
use super::rebuilds::{Gap, Kind, REBUILD_CONTEXT_FLOOR_TOKENS, RebuildSection};
use super::sessions::{SessionSection, Spread};
use super::{Measured, Report};

const NANODOLLARS_PER_CENT: u128 = 10_000_000;
const CENTS_PER_DOLLAR: u128 = 100;
const NANODOLLARS_PER_DOLLAR: u128 = 1_000_000_000;
const TOKENS_PER_TENTH_MILLION: u128 = 100_000;
const TENTHS_PER_WHOLE: u128 = 10;
const PER_MILLE: u128 = 1_000;
const TOKENS_PER_THOUSAND: u64 = 1_000;
const THOUSANDS_GROUP: usize = 3;
const COLUMN_GAP: &str = "   ";
const ROW_INDENT: &str = "  ";
const LOCAL_MINUTE: &str = "%Y-%m-%d %H:%M";
const LOCAL_SECOND: &str = "%Y-%m-%d %H:%M:%S";
const SESSION: Noun = Noun {
    one: "session",
    many: "sessions",
};
const SUBAGENT: Noun = Noun {
    one: "subagent",
    many: "subagents",
};
const CALL: Noun = Noun {
    one: "call",
    many: "calls",
};
const SYNTHETIC_RECORD: Noun = Noun {
    one: "synthetic record",
    many: "synthetic records",
};
const MALFORMED_RECORD: Noun = Noun {
    one: "malformed record",
    many: "malformed records",
};
const UNREADABLE_TRANSCRIPT: Noun = Noun {
    one: "unreadable transcript",
    many: "unreadable transcripts",
};
const REBUILD: Noun = Noun {
    one: "rebuild",
    many: "rebuilds",
};

/// The report as text, its times in `zone`, ending in a newline.
#[must_use]
pub fn to_text(report: &Report, zone: &TimeZone) -> String {
    let mut text = String::new();
    if let Some(range) = range_line(&report.range, zone) {
        text.push_str(&range);
    }
    text.push_str(&measured_line(&report.measured, zone));
    if let Some(left_out) = left_out_line(&report.measured) {
        text.push_str(&left_out);
    }
    if report.measured.calls > 0 {
        text.push_str(&cost_text(&report.cost));
        text.push_str(&context_text(&report.context));
        text.push_str(&sessions_text(&report.sessions));
        text.push_str(&rebuild_text(&report.rebuilds));
    }
    text
}

fn range_line(range: &Range, zone: &TimeZone) -> Option<String> {
    match (range.start, range.end) {
        (None, None) => None,
        (Some(start), None) => Some(format!("Range: from {}\n", second(start, zone))),
        (None, Some(end)) => Some(format!("Range: before {}\n", second(end, zone))),
        (Some(start), Some(end)) => Some(format!(
            "Range: from {}, before {}\n",
            second(start, zone),
            second(end, zone)
        )),
    }
}

fn measured_line(measured: &Measured, zone: &TimeZone) -> String {
    let (Some(first), Some(last)) = (measured.first_call, measured.last_call) else {
        assert_eq!(measured.calls, 0, "calls carry a first and last time");
        return "Measured no calls.\n".to_owned();
    };
    format!(
        "Measured {} and {}: {}, {} to {}.\n",
        counted(measured.sessions, &SESSION),
        counted(measured.subagents, &SUBAGENT),
        counted(measured.calls, &CALL),
        minute(first, zone),
        minute(last, zone),
    )
}

fn left_out_line(measured: &Measured) -> Option<String> {
    let parts = [
        (measured.synthetic, &SYNTHETIC_RECORD),
        (measured.malformed, &MALFORMED_RECORD),
        (measured.unread, &UNREADABLE_TRANSCRIPT),
    ];
    let named: Vec<String> = parts
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|&(count, noun)| counted(count, noun))
        .collect();
    if named.is_empty() {
        None
    } else {
        Some(format!("Left out: {}.\n", named.join(", ")))
    }
}

fn cost_text(cost: &CostSection) -> String {
    let total = dollars(cost.buckets.total());
    let tables = [
        ("By bucket", bucket_table(cost)),
        ("By thread and model", thread_table(cost)),
        ("By model and effort", effort_table(cost)),
        ("Cache writes by thread and TTL", write_table(cost)),
        ("Unpriced, left out of every amount", unpriced_table(cost)),
    ];
    let mut text = format!("\nCost at list price, as of {PRICES_AS_OF}: {total}\n");
    for (heading, rows) in &tables {
        text.push_str(&section(heading, rows));
    }
    text
}

fn bucket_table(cost: &CostSection) -> Vec<Vec<Cell>> {
    let total = cost.buckets.total();
    bucket_rows(cost)
        .into_iter()
        .map(|(name, amount)| {
            vec![
                Cell::left(name),
                Cell::right(dollars(amount)),
                Cell::right(share(Portion {
                    part: amount,
                    whole: total,
                })),
            ]
        })
        .collect()
}

fn thread_table(cost: &CostSection) -> Vec<Vec<Cell>> {
    spend_rows(&cost.by_thread_and_model)
        .into_iter()
        .map(|((thread, model), spend)| {
            vec![
                Cell::left(thread.name().to_owned()),
                Cell::left(model.clone()),
                Cell::right(calls(spend.calls)),
                Cell::right(dollars(spend.nanodollars)),
            ]
        })
        .collect()
}

fn effort_table(cost: &CostSection) -> Vec<Vec<Cell>> {
    spend_rows(&cost.by_model_and_effort)
        .into_iter()
        .map(|((model, effort), spend)| {
            let effort = effort.clone().unwrap_or_else(|| "-".to_owned());
            vec![
                Cell::left(model.clone()),
                Cell::left(effort),
                Cell::right(calls(spend.calls)),
                Cell::right(dollars(spend.nanodollars)),
            ]
        })
        .collect()
}

fn write_table(cost: &CostSection) -> Vec<Vec<Cell>> {
    write_rows(cost)
        .into_iter()
        .map(|((thread, ttl), writes)| {
            let tokens = millions(writes.tokens);
            vec![
                Cell::left(thread.name().to_owned()),
                Cell::left(ttl.name().to_owned()),
                Cell::right(format!("{tokens} tokens")),
                Cell::right(dollars(writes.nanodollars)),
            ]
        })
        .collect()
}

fn unpriced_table(cost: &CostSection) -> Vec<Vec<Cell>> {
    cost.unpriced
        .iter()
        .map(|(model, count)| vec![Cell::left(model.clone()), Cell::right(calls(*count))])
        .collect()
}

/// The buckets most costly first.
fn bucket_rows(cost: &CostSection) -> Vec<(String, u128)> {
    let mut rows = vec![
        ("cache read".to_owned(), cost.buckets.cache_read),
        ("cache write".to_owned(), cost.buckets.cache_write),
        ("output".to_owned(), cost.buckets.output),
        ("input".to_owned(), cost.buckets.input),
    ];
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows
}

/// The groups most costly first, ties in the map's order.
fn spend_rows<K: Ord>(groups: &std::collections::BTreeMap<K, Spend>) -> Vec<(&K, Spend)> {
    let mut rows: Vec<(&K, Spend)> = groups.iter().map(|(key, spend)| (key, *spend)).collect();
    rows.sort_by_key(|row| Reverse(row.1.nanodollars));
    rows
}

fn write_rows(cost: &CostSection) -> Vec<((Thread, Ttl), Writes)> {
    let mut rows: Vec<((Thread, Ttl), Writes)> = cost
        .cache_writes
        .iter()
        .map(|(key, writes)| (*key, *writes))
        .collect();
    rows.sort_by_key(|row| Reverse(row.1.nanodollars));
    rows
}

/// One cell of a table, aligned to its column's left or right edge.
struct Cell {
    text: String,
    right: bool,
}

impl Cell {
    const fn left(text: String) -> Self {
        Self { text, right: false }
    }

    const fn right(text: String) -> Self {
        Self { text, right: true }
    }
}

/// A heading and its rows, each column as wide as its widest cell; nothing where there
/// are no rows.
fn section(heading: &str, rows: &[Vec<Cell>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.text.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut text = format!("\n{heading}\n");
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, &width)| {
                if cell.right {
                    format!("{:>width$}", cell.text)
                } else {
                    format!("{:<width$}", cell.text)
                }
            })
            .collect();
        let line = cells.join(COLUMN_GAP);
        text.push_str(ROW_INDENT);
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text
}

fn minute(at: Timestamp, zone: &TimeZone) -> String {
    at.to_zoned(zone.clone()).strftime(LOCAL_MINUTE).to_string()
}

fn second(at: Timestamp, zone: &TimeZone) -> String {
    at.to_zoned(zone.clone()).strftime(LOCAL_SECOND).to_string()
}

/// A noun's singular and plural.
struct Noun {
    one: &'static str,
    many: &'static str,
}

/// A count, thousands grouped, followed by its noun in the number it takes.
fn counted(count: u64, noun: &Noun) -> String {
    let word = if count == 1 { noun.one } else { noun.many };
    format!("{} {word}", grouped(u128::from(count)))
}

fn calls(count: u64) -> String {
    counted(count, &CALL)
}

/// Nanodollars as dollars and cents, thousands grouped, rounded half up to the cent.
#[must_use]
pub fn dollars(nanodollars: u128) -> String {
    let half_cent = NANODOLLARS_PER_CENT / 2;
    let rounded = checked(nanodollars.checked_add(half_cent));
    let cents = checked(rounded.checked_div(NANODOLLARS_PER_CENT));
    let whole = checked(cents.checked_div(CENTS_PER_DOLLAR));
    let part = checked(cents.checked_rem(CENTS_PER_DOLLAR));
    format!("${}.{part:02}", grouped(whole))
}

/// Tokens in millions to a tenth, rounded half up.
#[must_use]
pub fn millions(tokens: u128) -> String {
    let half_tenth = TOKENS_PER_TENTH_MILLION / 2;
    let rounded = checked(tokens.checked_add(half_tenth));
    let tenths = checked(rounded.checked_div(TOKENS_PER_TENTH_MILLION));
    let whole = checked(tenths.checked_div(TENTHS_PER_WHOLE));
    let part = checked(tenths.checked_rem(TENTHS_PER_WHOLE));
    format!("{}.{part}M", grouped(whole))
}

/// A part of a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Portion {
    pub part: u128,
    pub whole: u128,
}

/// The portion as a percentage to a tenth, rounded half up; nothing of nothing is none
/// of it.
///
/// # Panics
///
/// Where the part exceeds its whole.
#[must_use]
pub fn share(portion: Portion) -> String {
    let Portion { part, whole } = portion;
    assert!(
        part <= whole,
        "a part is at most its whole: {part} of {whole}"
    );
    if whole == 0 {
        return "0.0%".to_owned();
    }
    let scaled = checked(part.checked_mul(PER_MILLE));
    let half = checked(whole.checked_div(2));
    let rounded = checked(scaled.checked_add(half));
    let tenths = checked(rounded.checked_div(whole));
    let whole_percent = checked(tenths.checked_div(TENTHS_PER_WHOLE));
    let tenth = checked(tenths.checked_rem(TENTHS_PER_WHOLE));
    format!("{whole_percent}.{tenth}%")
}

fn grouped(number: u128) -> String {
    let digits = number.to_string();
    let mut grouped = String::with_capacity(digits.len().saturating_mul(2));
    for (index, digit) in digits.chars().enumerate() {
        let remaining = digits.len().saturating_sub(index);
        if index > 0 {
            if remaining.checked_rem(THOUSANDS_GROUP) == Some(0) {
                grouped.push(',');
            }
        }
        grouped.push(digit);
    }
    grouped
}

fn checked(value: Option<u128>) -> u128 {
    let Some(value) = value else {
        unreachable!("report figures stay far inside a u128, and every divisor is nonzero")
    };
    value
}

/// The report as JSON, ending in a newline.
#[must_use]
pub fn to_json(report: &Report) -> String {
    let value = json!({
        "range": {
            "start": report.range.start.map(|start| start.to_string()),
            "end": report.range.end.map(|end| end.to_string()),
        },
        "selection": {
            "sessions": report.selection.sessions,
            "projects": report.selection.projects,
        },
        "measured": measured_json(&report.measured),
        "cost": cost_json(&report.cost),
        "context": context_json(&report.context),
        "sessions": sessions_json(&report.sessions),
        "rebuilds": rebuild_json(&report.rebuilds),
    });
    let Ok(text) = serde_json::to_string_pretty(&value) else {
        unreachable!("a JSON value built from strings and numbers serializes")
    };
    format!("{text}\n")
}

fn measured_json(measured: &Measured) -> Value {
    json!({
        "sessions": measured.sessions,
        "subagents": measured.subagents,
        "calls": measured.calls,
        "synthetic_records": measured.synthetic,
        "malformed_records": measured.malformed,
        "unreadable_transcripts": measured.unread,
        "first_call": measured.first_call.map(|first| first.to_string()),
        "last_call": measured.last_call.map(|last| last.to_string()),
    })
}

fn cost_json(cost: &CostSection) -> Value {
    let threads: Vec<Value> = spend_rows(&cost.by_thread_and_model)
        .into_iter()
        .map(|((thread, model), spend)| {
            json!({ "thread": thread.name(), "model": model, "calls": spend.calls, "usd": usd(spend.nanodollars) })
        })
        .collect();
    let efforts: Vec<Value> = spend_rows(&cost.by_model_and_effort)
        .into_iter()
        .map(|((model, effort), spend)| {
            json!({ "model": model, "effort": effort, "calls": spend.calls, "usd": usd(spend.nanodollars) })
        })
        .collect();
    let writes: Vec<Value> = write_rows(cost)
        .into_iter()
        .map(|((thread, ttl), writes)| {
            json!({ "thread": thread.name(), "ttl": ttl.name(), "tokens": integer(writes.tokens), "usd": usd(writes.nanodollars) })
        })
        .collect();
    let unpriced: Vec<Value> = cost
        .unpriced
        .iter()
        .map(|(model, count)| json!({ "model": model, "calls": count }))
        .collect();
    let mut buckets = Map::new();
    buckets.insert("input".to_owned(), usd(cost.buckets.input));
    buckets.insert("cache_read".to_owned(), usd(cost.buckets.cache_read));
    buckets.insert("cache_write".to_owned(), usd(cost.buckets.cache_write));
    buckets.insert("output".to_owned(), usd(cost.buckets.output));
    json!({
        "prices_as_of": PRICES_AS_OF,
        "total_usd": usd(cost.buckets.total()),
        "buckets_usd": buckets,
        "by_thread_and_model": threads,
        "by_model_and_effort": efforts,
        "cache_writes": writes,
        "unpriced": unpriced,
    })
}

/// Nanodollars as a JSON number of dollars, every digit kept.
fn usd(nanodollars: u128) -> Value {
    let whole = checked(nanodollars.checked_div(NANODOLLARS_PER_DOLLAR));
    let part = checked(nanodollars.checked_rem(NANODOLLARS_PER_DOLLAR));
    let decimal = format!("{whole}.{part:09}");
    let Ok(number) = decimal.parse::<Number>() else {
        unreachable!("a decimal of digits parses as a JSON number: {decimal}")
    };
    Value::Number(number)
}

fn integer(count: u128) -> Value {
    let Ok(count) = u64::try_from(count) else {
        unreachable!("a sum of bounded token counts fits a u64: {count}")
    };
    Value::from(count)
}

fn context_text(context: &ContextSection) -> String {
    let rows: Vec<Vec<Cell>> = context
        .bands
        .iter()
        .map(|band| {
            vec![
                Cell::left(band_label(band)),
                Cell::right(calls(band.calls)),
                Cell::right(dollars(band.input_side)),
            ]
        })
        .collect();
    let long = share(Portion {
        part: context.long_context_cache_read,
        whole: context.cache_read,
    });
    let floor = thousands(LONG_CONTEXT_FLOOR_TOKENS);
    let mut text = section(
        "Main-thread calls by context, and their input-side cost",
        &rows,
    );
    let long_line = format!(
        "{ROW_INDENT}Calls over {floor} carry {long} of the main thread's cache-read cost.\n"
    );
    text.push_str(&long_line);
    text
}

fn band_label(band: &Band) -> String {
    let from = thousands(band.from_tokens);
    match band.to_tokens {
        None => format!("{from} and over"),
        Some(to) if band.from_tokens == 0 => format!("under {}", thousands(to)),
        Some(to) => format!("{from} to {}", thousands(to)),
    }
}

/// Tokens in whole thousands, as a band edge reads.
fn thousands(tokens: u64) -> String {
    assert_eq!(
        tokens.checked_rem(TOKENS_PER_THOUSAND),
        Some(0),
        "a band edge is a whole thousand: {tokens}"
    );
    let Some(whole) = tokens.checked_div(TOKENS_PER_THOUSAND) else {
        unreachable!("the divisor is a nonzero constant")
    };
    format!("{whole}k")
}

fn sessions_text(sessions: &SessionSection) -> String {
    let heading = format!(
        "Per main session, over {}",
        counted(sessions.sessions, &SESSION)
    );
    let mut text = match (sessions.starting_context, sessions.calls) {
        (Some(starting), Some(calls)) => {
            let rows = [
                spread_header(),
                spread_row("starting context", starting),
                spread_row("calls", calls),
            ];
            section(&heading, &rows)
        }
        (None, None) => format!("\n{heading}\n"),
        (Some(_), None) | (None, Some(_)) => {
            unreachable!("every session has both a start and a length")
        }
    };
    if sessions.begun_before_range > 0 {
        let left_out = counted(sessions.begun_before_range, &SESSION);
        let left_out_line =
            format!("{ROW_INDENT}Left out of these figures: {left_out} begun before the range.\n");
        text.push_str(&left_out_line);
    }
    text
}

fn spread_header() -> Vec<Cell> {
    let mut row = vec![Cell::left(String::new())];
    for name in ["p10", "median", "p90", "max"] {
        row.push(Cell::right(name.to_owned()));
    }
    row
}

fn spread_row(name: &str, spread: Spread) -> Vec<Cell> {
    let values = [spread.p10, spread.median, spread.p90, spread.max];
    let mut row = vec![Cell::left(name.to_owned())];
    for value in values {
        row.push(Cell::right(grouped(u128::from(value))));
    }
    row
}

fn context_json(context: &ContextSection) -> Value {
    let bands: Vec<Value> = context
        .bands
        .iter()
        .map(|band| {
            json!({
                "from_tokens": band.from_tokens,
                "to_tokens": band.to_tokens,
                "calls": band.calls,
                "input_side_usd": usd(band.input_side),
            })
        })
        .collect();
    json!({
        "thread": Thread::Main.name(),
        "bands": bands,
        "cache_read_usd": usd(context.cache_read),
        "long_context_floor_tokens": LONG_CONTEXT_FLOOR_TOKENS,
        "long_context_cache_read_usd": usd(context.long_context_cache_read),
    })
}

fn sessions_json(sessions: &SessionSection) -> Value {
    json!({
        "sessions": sessions.sessions,
        "begun_before_range": sessions.begun_before_range,
        "starting_context_tokens": sessions.starting_context.map(spread_json),
        "calls": sessions.calls.map(spread_json),
    })
}

fn spread_json(spread: Spread) -> Value {
    json!({
        "p10": spread.p10,
        "median": spread.median,
        "p90": spread.p90,
        "max": spread.max,
    })
}

fn rebuild_text(rebuilds: &RebuildSection) -> String {
    let floor = thousands(REBUILD_CONTEXT_FLOOR_TOKENS);
    let heading = format!(
        "Cache rebuilds: calls after their transcript's first writing over half of a context above {floor}"
    );
    let rows: Vec<Vec<Cell>> = rebuilds
        .rebuilds
        .iter()
        .map(|((thread, gap), band)| {
            let switches = grouped(u128::from(band.after_model_switch));
            vec![
                Cell::left(thread.name().to_owned()),
                Cell::left(format!("gap {}", gap_label(*gap))),
                Cell::left(idle_label(*gap).to_owned()),
                Cell::right(counted(band.count, &REBUILD)),
                Cell::right(dollars(band.nanodollars)),
                Cell::right(format!("{switches} after a model switch")),
            ]
        })
        .collect();
    let writes: Vec<Vec<Cell>> = rebuilds
        .writes_by_kind
        .iter()
        .map(|((thread, kind), nanodollars)| {
            vec![
                Cell::left(thread.name().to_owned()),
                Cell::left(kind_label(*kind).to_owned()),
                Cell::right(dollars(*nanodollars)),
            ]
        })
        .collect();
    let mut text = if rows.is_empty() {
        format!("\n{heading}\n{ROW_INDENT}No call rebuilt the cache.\n")
    } else {
        section(&heading, &rows)
    };
    text.push_str(&section("Cache writes by thread and kind", &writes));
    text
}

const fn gap_label(gap: Gap) -> &'static str {
    match gap {
        Gap::UnderFiveMinutes => "under 5m",
        Gap::FiveMinutesToAnHour => "5m to 1h",
        Gap::OverAnHour => "over 1h",
    }
}

const fn idle_label(gap: Gap) -> &'static str {
    match gap {
        Gap::UnderFiveMinutes => "within both TTLs",
        Gap::FiveMinutesToAnHour => "idle past the 5m TTL",
        Gap::OverAnHour => "idle past both TTLs",
    }
}

const fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::First => "first call",
        Kind::Rebuild => "rebuild",
        Kind::Incremental => "incremental",
    }
}

/// The TTLs a gap is past, by the name the cost section's cache writes give each.
fn idle_past(gap: Gap) -> Vec<&'static str> {
    match gap {
        Gap::UnderFiveMinutes => Vec::new(),
        Gap::FiveMinutesToAnHour => vec![Ttl::FiveMinutes.name()],
        Gap::OverAnHour => vec![Ttl::FiveMinutes.name(), Ttl::OneHour.name()],
    }
}

fn rebuild_json(rebuilds: &RebuildSection) -> Value {
    let bands: Vec<Value> = rebuilds
        .rebuilds
        .iter()
        .map(|((thread, gap), band)| {
            json!({
                "thread": thread.name(),
                "gap": gap_label(*gap),
                "idle_past_ttls": idle_past(*gap),
                "rebuilds": band.count,
                "after_model_switch": band.after_model_switch,
                "cache_write_usd": usd(band.nanodollars),
            })
        })
        .collect();
    let writes: Vec<Value> = rebuilds
        .writes_by_kind
        .iter()
        .map(|((thread, kind), nanodollars)| {
            json!({
                "thread": thread.name(),
                "kind": kind_label(*kind),
                "cache_write_usd": usd(*nanodollars),
            })
        })
        .collect();
    json!({
        "context_floor_tokens": REBUILD_CONTEXT_FLOOR_TOKENS,
        "rebuilds": bands,
        "cache_writes_by_kind": writes,
    })
}
