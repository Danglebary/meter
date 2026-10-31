//! Every exit code meter ends with, each with one owner.

pub const SUCCESS: u8 = 0;
/// The arguments break the grammar.
pub const USAGE: u8 = 2;
/// The status line's input is not JSON; nothing was printed.
pub const UNREADABLE_INPUT: u8 = 3;
/// Writing the status line to stdout failed.
pub const OUTPUT: u8 = 4;
/// The transcript store could not be found or listed; nothing was printed.
pub const UNREADABLE_STORE: u8 = 5;
