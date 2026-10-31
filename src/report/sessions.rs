//! The session section: how much context each main session's first call sent, and how
//! many calls each session ran, spread as percentiles over the sessions.

use super::range::Thread;
use super::tally::count_one;
use super::transcript::Transcript;

const P10_PER_MILLE: u64 = 100;
const MEDIAN_PER_MILLE: u64 = 500;
const P90_PER_MILLE: u64 = 900;
const PER_MILLE: u64 = 1_000;

/// The session section, over the main sessions holding a call in the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSection {
    /// The sessions the spreads cover.
    pub sessions: u64,
    /// Sessions left out of the spreads because a call of theirs began before the range.
    pub begun_before_range: u64,
    /// The context of each session's first call, in tokens.
    pub starting_context: Option<Spread>,
    /// The calls each session ran.
    pub calls: Option<Spread>,
}

/// Percentiles of a set of values, a percentile p of n values being the value at sorted
/// position floor(p n), counting from one, and the first where that position is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spread {
    pub p10: u64,
    pub median: u64,
    pub p90: u64,
    pub max: u64,
}

/// The session section of the main threads in `transcripts`.
///
/// # Panics
///
/// Where a session's start and length were not gathered together.
#[must_use]
pub fn session_section(transcripts: &[Transcript]) -> SessionSection {
    let mut begun_before_range: u64 = 0;
    let mut starts = Vec::new();
    let mut lengths = Vec::new();
    let main = transcripts
        .iter()
        .filter(|transcript| transcript.thread == Thread::Main);
    for transcript in main {
        let Some(first) = transcript.calls.first() else {
            continue;
        };
        if transcript.before_range.is_some() {
            begun_before_range = count_one(begun_before_range);
            continue;
        }
        let Ok(length) = u64::try_from(transcript.calls.len()) else {
            unreachable!("a count of calls fits a u64")
        };
        starts.push(first.usage.context());
        lengths.push(length);
    }
    assert_eq!(
        starts.len(),
        lengths.len(),
        "each session has a start and a length"
    );
    let Ok(sessions) = u64::try_from(starts.len()) else {
        unreachable!("a count of sessions fits a u64")
    };
    SessionSection {
        sessions,
        begun_before_range,
        starting_context: spread(&starts),
        calls: spread(&lengths),
    }
}

/// The spread of `values`, where there are any.
#[must_use]
pub fn spread(values: &[u64]) -> Option<Spread> {
    // A copy is sorted so the caller's values keep their order.
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let max = *sorted.last()?;
    Some(Spread {
        p10: percentile(&sorted, P10_PER_MILLE),
        median: percentile(&sorted, MEDIAN_PER_MILLE),
        p90: percentile(&sorted, P90_PER_MILLE),
        max,
    })
}

fn percentile(sorted: &[u64], per_mille: u64) -> u64 {
    assert!(!sorted.is_empty(), "a percentile is taken of some values");
    assert!(
        per_mille <= PER_MILLE,
        "a percentile is at most the whole: {per_mille}"
    );
    let Ok(count) = u64::try_from(sorted.len()) else {
        unreachable!("a count of values fits a u64")
    };
    let Some(scaled) = count.checked_mul(per_mille) else {
        unreachable!("a count of sessions times a thousand fits a u64")
    };
    let Some(position) = scaled.checked_div(PER_MILLE) else {
        unreachable!("the divisor is a nonzero constant")
    };
    let index = position.saturating_sub(1);
    let Ok(index) = usize::try_from(index) else {
        unreachable!("an index below the count fits a usize")
    };
    let Some(value) = sorted.get(index) else {
        unreachable!("the floored position is at most the count: {index}")
    };
    *value
}
