//! The status line's functional core.
//!
//! Claude Code's input is read field by field, beside the git state the shell reports, the
//! calendar, and the day's spend snapshot, and the lines are rendered from them. Nothing here
//! reads a file, the clock, or the environment; the shell gathers those as `Facts` and hands
//! them in, so every rule is tested against values.

mod calendar;
mod git;
mod input;
mod render;
mod spend;
mod style;

pub use calendar::{RangeTooLong, business_days};
pub use git::{GitState, Head, PorcelainFault, parse_porcelain};
pub use input::{Cache, Dollars, Field, Input, Period, PullRequest, SpendLimit, Window};
pub use render::{Facts, render};
pub use spend::{DayStart, DayStartInput, Pace, Snapshot, day_start};
pub use style::{Color, Line, Span, to_ansi};

#[cfg(test)]
mod tests;
