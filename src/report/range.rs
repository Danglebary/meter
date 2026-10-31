//! What a report selects: the span of time its records fall in, and the transcripts it
//! reads.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

use crate::cli::{Bound, Bounds};

/// The span a report measures: records at or after `start`, and before `end`. An absent
/// end leaves its side open.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
}

/// Why `--since` and `--until` make no range.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeFault {
    /// `--since` falls at or after the end `--until` sets.
    Empty,
    /// A bound lies past the last instant a timestamp holds.
    Unplaceable,
}

impl Range {
    /// Places `--since` and `--until` in time. A date given to `--since` starts at its
    /// midnight in `zone`, a date given to `--until` runs through its last instant there,
    /// and a timestamp given to `--until` is inside the range.
    ///
    /// # Errors
    ///
    /// Where the range holds no instant, or a bound cannot be placed in time.
    pub fn resolve(bounds: Bounds, zone: &TimeZone) -> Result<Self, RangeFault> {
        let start = match bounds.since {
            None => None,
            Some(Bound::Instant(instant)) => Some(instant),
            Some(Bound::Date(date)) => Some(midnight(date, zone)?),
        };
        let end = match bounds.until {
            None => None,
            Some(Bound::Instant(instant)) => Some(
                instant
                    .checked_add(1.nanosecond())
                    .map_err(|_| RangeFault::Unplaceable)?,
            ),
            Some(Bound::Date(date)) => {
                let next = date.tomorrow().map_err(|_| RangeFault::Unplaceable)?;
                Some(midnight(next, zone)?)
            }
        };
        if let (Some(start), Some(end)) = (start, end) {
            if start >= end {
                return Err(RangeFault::Empty);
            }
        }
        Ok(Self { start, end })
    }

    /// Whether the instant falls in the range.
    #[must_use]
    pub fn holds(&self, at: Timestamp) -> bool {
        let started = self.start.is_none_or(|start| at >= start);
        let before_end = self.end.is_none_or(|end| at < end);
        if started { before_end } else { false }
    }

    /// Whether a transcript last written at `modified` can hold a record in the range.
    /// Claude Code appends each record as it happens, so a file holds no record later than
    /// its last write.
    #[must_use]
    pub fn may_hold_records_written_until(&self, modified: Timestamp) -> bool {
        self.start.is_none_or(|start| modified >= start)
    }

    /// Whether the instant falls before the range starts.
    #[must_use]
    pub fn precedes(&self, at: Timestamp) -> bool {
        self.start.is_some_and(|start| at < start)
    }
}

fn midnight(date: Date, zone: &TimeZone) -> Result<Timestamp, RangeFault> {
    let zoned = date
        .to_zoned(zone.clone())
        .map_err(|_| RangeFault::Unplaceable)?;
    Ok(zoned.timestamp())
}

/// Whose calls a transcript holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Thread {
    Main,
    Subagent,
}

impl Thread {
    /// The thread as a report names it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Subagent => "subagent",
        }
    }
}

/// Where a transcript sits in the store: `<project>/<session>.jsonl` holds a session's main
/// thread, and the files under `<project>/<session>/subagents/` its subagents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub project: String,
    pub session: String,
    pub thread: Thread,
}

/// Which transcripts a report reads: those of the sessions named and the projects named,
/// each list selecting every transcript where it is empty.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    pub sessions: Vec<String>,
    /// Each project as the directory name the store keeps it under.
    pub projects: Vec<String>,
}

impl Selection {
    /// Whether the transcript at `location` is read.
    #[must_use]
    pub fn selects(&self, location: &Location) -> bool {
        let session = self.sessions.is_empty() || self.sessions.contains(&location.session);
        let project = self.projects.is_empty() || self.projects.contains(&location.project);
        if session { project } else { false }
    }
}
