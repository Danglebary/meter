//! The report's functional core.
//!
//! The shell finds the transcripts the selection names and hands this module their lines;
//! each line is read field by field, the records of one call fold into that call, and the
//! calls are measured section by section and rendered as text or JSON. Nothing here reads
//! a file, the clock, or the environment, so every rule is tested against values.

mod context;
mod cost;
mod price;
mod range;
mod rebuilds;
mod render;
mod sessions;
mod subagents;
mod tally;
mod transcript;

pub use context::{
    Band, CONTEXT_BAND_EDGES_TOKENS, ContextSection, LONG_CONTEXT_FLOOR_TOKENS, context_section,
};
pub use cost::{Buckets, CostSection, Spend, Ttl, Writes, cost_section};
pub use price::{Cost, PRICES_AS_OF, cost};
pub use range::{Location, Range, RangeFault, Selection, Thread, project_directory};
pub use rebuilds::{
    Gap, Kind, REBUILD_CONTEXT_FLOOR_TOKENS, RebuildSection, Rebuilds, TTL_1H_MILLISECONDS,
    TTL_5M_MILLISECONDS, rebuild_section,
};
pub use render::{to_json, to_text};
pub use sessions::{SessionSection, Spread, session_section, spread};
pub use subagents::{Agents, SubagentSection, agent_type, subagent_section};
pub use transcript::{
    Call, Fold, Line, Prior, Record, SYNTHETIC_MODEL, TOKENS_PER_FIELD_MAX, Transcript, Usage,
    read_line,
};

use jiff::Timestamp;
use tally::plus;

/// The most transcripts a report reads; the store holds about a hundredth of this.
pub const TRANSCRIPTS_MAX: usize = 100_000;
/// The most lines a report reads of one transcript; the longest in the store holds a small
/// fraction of this.
pub const LINES_PER_TRANSCRIPT_MAX: usize = 1_000_000;
/// The longest line a report reads; a longer one counts as malformed.
pub const LINE_BYTES_MAX: usize = 64 * 1024 * 1024;
/// The most of a subagent's meta file a report reads; one holds a few hundred bytes.
pub const META_BYTES_MAX: u64 = 64 * 1024;

/// What a report measured, and each section of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub range: Range,
    pub selection: Selection,
    pub measured: Measured,
    pub cost: CostSection,
    pub context: ContextSection,
    pub sessions: SessionSection,
    pub rebuilds: RebuildSection,
    pub subagents: SubagentSection,
}

impl Report {
    /// The report over what the shell gathered.
    #[must_use]
    pub fn of(gathered: Gathered<'_>) -> Self {
        let Gathered {
            range,
            selection,
            transcripts,
            unread,
        } = gathered;
        Self {
            range,
            selection: selection.clone(),
            measured: Measured::of(transcripts, unread),
            cost: cost_section(transcripts),
            context: context_section(transcripts),
            sessions: session_section(transcripts),
            rebuilds: rebuild_section(transcripts),
            subagents: subagent_section(transcripts),
        }
    }
}

/// What the shell gathered for a report: the range and selection it read under, the
/// transcripts it read, and how many selected transcripts it could not read.
#[derive(Debug, Clone, Copy)]
pub struct Gathered<'a> {
    pub range: Range,
    pub selection: &'a Selection,
    pub transcripts: &'a [Transcript],
    pub unread: u64,
}

/// How many sessions, subagents, and calls a report measured, what it left out, and when
/// its first and last calls began.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Measured {
    /// Main threads holding at least one call.
    pub sessions: u64,
    /// Subagent transcripts holding at least one call.
    pub subagents: u64,
    pub calls: u64,
    pub synthetic: u64,
    pub malformed: u64,
    pub unread: u64,
    pub first_call: Option<Timestamp>,
    pub last_call: Option<Timestamp>,
}

impl Measured {
    /// What `transcripts` hold, with `unread` counting the transcripts not read.
    ///
    /// # Panics
    ///
    /// Where a transcript's calls do not run in the order they began.
    #[must_use]
    pub fn of(transcripts: &[Transcript], unread: u64) -> Self {
        let mut measured = Self {
            unread,
            ..Self::default()
        };
        for transcript in transcripts {
            measured.synthetic = plus(measured.synthetic, transcript.synthetic);
            measured.malformed = plus(measured.malformed, transcript.malformed);
            let (Some(first), Some(last)) = (transcript.calls.first(), transcript.calls.last())
            else {
                continue;
            };
            match transcript.thread {
                Thread::Main => measured.sessions = plus(measured.sessions, 1),
                Thread::Subagent => measured.subagents = plus(measured.subagents, 1),
            }
            let Ok(calls) = u64::try_from(transcript.calls.len()) else {
                unreachable!("a count of calls fits a u64")
            };
            measured.calls = plus(measured.calls, calls);
            assert!(
                first.timestamp <= last.timestamp,
                "a transcript's calls run in the order they began"
            );
            let earliest = measured
                .first_call
                .is_none_or(|known| first.timestamp < known);
            let latest = measured
                .last_call
                .is_none_or(|known| last.timestamp > known);
            if earliest {
                measured.first_call = Some(first.timestamp);
            }
            if latest {
                measured.last_call = Some(last.timestamp);
            }
        }
        measured
    }
}

#[cfg(test)]
mod tests;
