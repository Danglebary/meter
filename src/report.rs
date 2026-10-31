//! Every message meter prints: the help, and the refusal of a command line.

use crate::cli::UsageFault;
use std::io;
use std::path::Path;

/// The help `--help` prints, ending in a newline.
pub const HELP: &str = "\
meter renders Claude Code's status line and measures Claude Code's token usage and cost.

Usage:
  meter statusline   Print Claude Code's status line from the JSON it sends on stdin.
  meter --help       Print this help.
  meter --version    Print the version.
";

/// The line a usage fault prints on stderr, ending in a newline.
#[must_use]
pub fn usage_fault(fault: &UsageFault) -> String {
    let detail = match fault {
        UsageFault::MissingCommand => "no command given".to_owned(),
        UsageFault::UnknownArgument(argument) => format!("unknown argument `{argument}`"),
    };
    format!("meter: {detail}; `meter --help` lists the commands\n")
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
