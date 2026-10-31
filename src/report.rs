//! Every message meter prints: the help, and the refusal of a command line.

use crate::cli::UsageFault;

/// The help `--help` prints, ending in a newline.
pub const HELP: &str = "\
meter renders Claude Code's status line and measures Claude Code's token usage and cost.

Usage:
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
