//! The command line's grammar: the arguments in, the command to run or a usage fault out.

use jiff::Timestamp;
use jiff::civil::Date;

/// The form a date given to `--since` or `--until` takes, and nothing more.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// What one invocation does.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Report(ReportArguments),
    Statusline,
    Version,
}

/// What `meter report` was asked to measure and how to print it. A date is kept as the
/// day it names; the shell places it in the local time zone.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReportArguments {
    pub bounds: Bounds,
    pub sessions: Vec<String>,
    pub projects: Vec<String>,
    /// The context, in tokens, the context-cap simulation restarts a session before.
    pub cap: Option<u64>,
    pub format: Format,
}

/// The ends of a report's range, as `--since` and `--until` give them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub since: Option<Bound>,
    pub until: Option<Bound>,
}

/// One end of a report's range, as given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    Date(Date),
    Instant(Timestamp),
}

/// The form a report prints in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    #[default]
    Text,
    Json,
}

/// Every flag `meter report` takes a value for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Since,
    Until,
    Session,
    Project,
    Cap,
    Format,
}

impl Flag {
    /// The flag as it is typed.
    #[must_use]
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Since => "--since",
            Self::Until => "--until",
            Self::Session => "--session",
            Self::Project => "--project",
            Self::Cap => "--cap",
            Self::Format => "--format",
        }
    }
}

/// Every way the arguments break the grammar.
#[derive(Debug, PartialEq, Eq)]
pub enum UsageFault {
    MissingCommand,
    UnknownArgument(String),
    MissingValue(Flag),
    RepeatedFlag(Flag),
    InvalidValue(Flag, String),
}

/// Parses the arguments after the program name.
///
/// # Errors
///
/// Returns the fault in the first argument the grammar refuses, or the absence of any.
pub fn parse_arguments(arguments: &[String]) -> Result<Command, UsageFault> {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => Err(UsageFault::MissingCommand),
        ["--help" | "-h"] => Ok(Command::Help),
        ["--version" | "-V"] => Ok(Command::Version),
        ["statusline"] => Ok(Command::Statusline),
        ["report", flags @ ..] => parse_report(flags).map(Command::Report),
        [
            "--help" | "-h" | "--version" | "-V" | "statusline",
            surplus,
            ..,
        ] => Err(UsageFault::UnknownArgument((*surplus).to_owned())),
        [first, ..] => Err(UsageFault::UnknownArgument((*first).to_owned())),
    }
}

/// Parses the flags after `report`: `--since` and `--until` once each, `--session` and
/// `--project` as often as given, and `--cap` and `--format` once each.
fn parse_report(words: &[&str]) -> Result<ReportArguments, UsageFault> {
    let mut since = None;
    let mut until = None;
    let mut cap = None;
    let mut format = None;
    let mut sessions = Vec::new();
    let mut projects = Vec::new();
    let mut given = Vec::new();
    let mut remaining = words.iter();
    // Each pass consumes at least one word, so the loop ends within `words.len()` passes.
    while let Some(word) = remaining.next() {
        let flag = match *word {
            "--since" => Flag::Since,
            "--until" => Flag::Until,
            "--session" => Flag::Session,
            "--project" => Flag::Project,
            "--cap" => Flag::Cap,
            "--format" => Flag::Format,
            other => return Err(UsageFault::UnknownArgument(other.to_owned())),
        };
        let Some(value) = remaining.next() else {
            return Err(UsageFault::MissingValue(flag));
        };
        let single = matches!(flag, Flag::Since | Flag::Until | Flag::Cap | Flag::Format);
        if single {
            if given.contains(&flag) {
                return Err(UsageFault::RepeatedFlag(flag));
            }
        }
        given.push(flag);
        match flag {
            Flag::Since => since = Some(parse_bound(flag, value)?),
            Flag::Until => until = Some(parse_bound(flag, value)?),
            Flag::Session => sessions.push(parse_session(value)?),
            Flag::Project => projects.push(parse_project(value)?),
            Flag::Cap => cap = Some(parse_cap(value)?),
            Flag::Format => format = Some(parse_format(value)?),
        }
    }
    Ok(ReportArguments {
        bounds: Bounds { since, until },
        sessions,
        projects,
        cap,
        format: format.unwrap_or_default(),
    })
}

/// A calendar date, `2026-10-30`, or a timestamp carrying its offset,
/// `2026-10-30T22:35:23Z`.
fn parse_bound(flag: Flag, value: &str) -> Result<Bound, UsageFault> {
    if let Ok(date) = Date::strptime(DATE_FORMAT, value) {
        return Ok(Bound::Date(date));
    }
    match value.parse::<Timestamp>() {
        Ok(instant) => Ok(Bound::Instant(instant)),
        Err(_) => Err(UsageFault::InvalidValue(flag, value.to_owned())),
    }
}

/// A session id names a transcript file in the store, so it holds letters, digits, and
/// hyphens alone and cannot climb out of the store's directories.
fn parse_session(value: &str) -> Result<String, UsageFault> {
    let named = !value.is_empty();
    let plain = value
        .chars()
        .all(|letter| letter.is_ascii_alphanumeric() || letter == '-');
    if named {
        if plain {
            return Ok(value.to_owned());
        }
    }
    Err(UsageFault::InvalidValue(Flag::Session, value.to_owned()))
}

fn parse_project(value: &str) -> Result<String, UsageFault> {
    if value.is_empty() {
        Err(UsageFault::InvalidValue(Flag::Project, value.to_owned()))
    } else {
        Ok(value.to_owned())
    }
}

fn parse_format(value: &str) -> Result<Format, UsageFault> {
    match value {
        "text" => Ok(Format::Text),
        "json" => Ok(Format::Json),
        _ => Err(UsageFault::InvalidValue(Flag::Format, value.to_owned())),
    }
}

/// A whole number of tokens above zero, written in digits alone.
fn parse_cap(value: &str) -> Result<u64, UsageFault> {
    // An empty value is all digits, and the parse refuses it.
    let digits = value.chars().all(|letter| letter.is_ascii_digit());
    if digits {
        if let Ok(tokens) = value.parse::<u64>() {
            if tokens > 0 {
                return Ok(tokens);
            }
        }
    }
    Err(UsageFault::InvalidValue(Flag::Cap, value.to_owned()))
}

#[cfg(test)]
mod tests;
