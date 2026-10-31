//! Every message meter prints: the help, and the refusal of a command line.

use crate::cli::{Flag, UsageFault};
use crate::report::RangeFault;
use std::io;
use std::path::Path;

/// The help `--help` prints, ending in a newline.
pub const HELP: &str = "\
meter renders Claude Code's status line and measures Claude Code's token usage and cost.

Usage:
  meter statusline   Print Claude Code's status line from the JSON it sends on stdin.
  meter report       Measure the transcripts under ~/.claude/projects.
  meter --help       Print this help.
  meter --version    Print the version.

Report flags, each narrowing what is measured; with none, every transcript is:
  --since <date|timestamp>   Records at or after this. A date starts at its local midnight.
  --until <date|timestamp>   Records at or before this. A date runs through its local end.
  --session <id>             The session and its subagents. Repeatable.
  --project <path>           The sessions started in this directory. Repeatable.
  --cap <tokens>             Simulate restarting each main session before this context.
  --format <text|json>       The form printed; text when absent.

A timestamp carries its offset: 2026-10-30T22:35:23Z or 2026-10-30T15:35:23-07:00.
";

/// The line a usage fault prints on stderr, ending in a newline.
#[must_use]
pub fn usage_fault(fault: &UsageFault) -> String {
    let detail = match fault {
        UsageFault::MissingCommand => "no command given".to_owned(),
        UsageFault::UnknownArgument(argument) => format!("unknown argument `{argument}`"),
        UsageFault::MissingValue(flag) => {
            format!("`{}` needs {}", flag.spelling(), expected(*flag))
        }
        UsageFault::RepeatedFlag(flag) => format!("`{}` is given twice", flag.spelling()),
        UsageFault::InvalidValue(flag, value) => {
            format!("`{} {value}` is not {}", flag.spelling(), expected(*flag))
        }
    };
    format!("meter: {detail}; `meter --help` lists the commands\n")
}

/// What a flag's value has to be, as the refusal of a wrong one names it.
const fn expected(flag: Flag) -> &'static str {
    match flag {
        Flag::Since | Flag::Until => "a date such as 2026-10-30 or a timestamp with its offset",
        Flag::Session => "a session id of letters, digits, and hyphens",
        Flag::Project => "a directory",
        Flag::Cap => "a whole number of tokens above zero",
        Flag::Format => "`text` or `json`",
    }
}

/// The line printed on stderr where `--since` and `--until` make no range.
#[must_use]
pub fn range_fault(fault: &RangeFault) -> String {
    let detail = match fault {
        RangeFault::Empty => "`--since` falls at or after `--until`, so the range holds nothing",
        RangeFault::Unplaceable => {
            "a bound of the range lies past the last instant meter can place"
        }
    };
    format!("meter: {detail}; `meter --help` lists the commands\n")
}

/// The line printed on stderr where a `--project` directory cannot be made absolute or
/// is not UTF-8.
#[must_use]
pub fn project_unresolved(project: &str) -> String {
    format!(
        "meter: `--project {project}` names no directory meter can resolve; `meter --help` lists the commands\n"
    )
}

/// The line printed on stderr where `HOME` is unset, so the transcript store has no place.
#[must_use]
pub fn home_unset() -> String {
    "meter: HOME is unset, so the transcript store under ~/.claude/projects cannot be found\n"
        .to_owned()
}

/// The line printed on stderr where a folder of the transcript store could not be listed.
#[must_use]
pub fn store_unread(path: &Path, kind: io::ErrorKind) -> String {
    format!(
        "meter: the transcript store at {} could not be read: {kind}\n",
        path.display()
    )
}

/// The line printed on stderr where a folder of the transcript store holds more entries
/// than a report reads.
#[must_use]
pub fn store_too_large(path: &Path, entries_max: usize) -> String {
    format!(
        "meter: the transcript store at {} holds more than {entries_max} entries, more than a report reads\n",
        path.display()
    )
}

/// The line printed on stderr where the transcript store, or one folder of it, holds more
/// transcripts than a report reads.
#[must_use]
pub fn store_too_many_transcripts(path: &Path, transcripts_max: usize) -> String {
    format!(
        "meter: the transcript store at {} holds more than {transcripts_max} transcripts, more than a report reads\n",
        path.display()
    )
}

/// The line printed on stderr where one transcript could not be read; the report goes on
/// without it.
#[must_use]
pub fn transcript_unread(path: &Path, error: &io::Error) -> String {
    format!(
        "meter: the transcript at {} could not be read and is left out: {error}\n",
        path.display()
    )
}

/// The line printed on stderr where one transcript runs past the lines a report reads of
/// it; the report goes on with the lines read.
#[must_use]
pub fn transcript_truncated(path: &Path, lines_max: u64) -> String {
    format!(
        "meter: the transcript at {} runs past {lines_max} lines; the lines after are left out\n",
        path.display()
    )
}

/// The line printed on stderr where the meta file beside the subagent transcript at
/// `transcript` could not be read; the subagent's type reads as unknown.
#[must_use]
pub fn meta_unread(transcript: &Path, kind: io::ErrorKind) -> String {
    format!(
        "meter: the meta file beside the subagent transcript at {} could not be read, so its type is unknown: {kind}\n",
        transcript.display()
    )
}

/// The line printed on stderr where the status line's input is not JSON.
#[must_use]
pub fn unreadable_input(detail: &str) -> String {
    format!("meter: the status line's input is not JSON: {detail}\n")
}

/// The line printed on stderr where the day's spend snapshot could not be read.
#[must_use]
pub fn snapshot_unread(path: &Path, error: &io::Error) -> String {
    format!(
        "meter: the day's spend snapshot at {} could not be read: {error}\n",
        path.display()
    )
}

/// The line printed on stderr where the day's spend snapshot could not be written.
#[must_use]
pub fn snapshot_unwritten(path: &Path, error: &io::Error) -> String {
    format!(
        "meter: the day's spend snapshot at {} could not be written: {error}\n",
        path.display()
    )
}
