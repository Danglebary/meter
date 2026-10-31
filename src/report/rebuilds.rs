//! The cache-rebuild section: which calls rebuilt the prompt cache, by the gap since their
//! transcript's previous call, and what each thread's cache writes cost on first calls, on
//! rebuilds, and incrementally.

use std::collections::BTreeMap;

use jiff::Timestamp;

use super::price::cost;
use super::range::Thread;
use super::tally::{add, count_one};
use super::transcript::{Transcript, Usage};

/// A call can rebuild the cache only where its context is above this; below it, writing
/// most of the context is a short conversation's growth.
pub const REBUILD_CONTEXT_FLOOR_TOKENS: u64 = 30_000;
/// How long a cache written at the 5-minute TTL stays warm unread.
pub const TTL_5M_MILLISECONDS: i128 = 5 * 60 * 1000;
/// How long a cache written at the 1-hour TTL stays warm unread.
pub const TTL_1H_MILLISECONDS: i128 = 60 * 60 * 1000;

/// The rebuild section. Every amount is in nanodollars of cache writes; a call the price
/// table lacks counts and costs nothing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RebuildSection {
    pub rebuilds: BTreeMap<(Thread, Gap), Rebuilds>,
    pub writes_by_kind: BTreeMap<(Thread, Kind), u128>,
}

/// The rebuilds in one band of gap, and what their cache writes cost.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rebuilds {
    pub count: u64,
    /// Rebuilds whose model differs from the previous call's.
    pub after_model_switch: u64,
    pub nanodollars: u128,
}

/// How long a call followed its transcript's previous call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gap {
    /// Within both TTLs, so the cache was warm at either.
    UnderFiveMinutes,
    /// Past the 5-minute TTL and within the 1-hour TTL.
    FiveMinutesToAnHour,
    /// Past both TTLs.
    OverAnHour,
}

impl Gap {
    /// The band a gap of `milliseconds` falls in, each band holding its upper edge.
    #[must_use]
    pub const fn of(milliseconds: i128) -> Self {
        if milliseconds > TTL_1H_MILLISECONDS {
            Self::OverAnHour
        } else if milliseconds > TTL_5M_MILLISECONDS {
            Self::FiveMinutesToAnHour
        } else {
            Self::UnderFiveMinutes
        }
    }
}

/// What a call's cache writes did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// The transcript's first call, which writes the cache it starts from.
    First,
    /// A later call writing over half of a context above `REBUILD_CONTEXT_FLOOR_TOKENS`.
    Rebuild,
    /// Any other later call, adding to a cache already written.
    Incremental,
}

/// The call before one, as the rebuild rule reads it.
#[derive(Debug, Clone, Copy)]
struct Previous<'a> {
    timestamp: Timestamp,
    model: &'a str,
}

/// The rebuild section of the calls in `transcripts`. A transcript's first call in the
/// range follows the last call before the range, where there was one.
///
/// # Panics
///
/// Where a call began before the call it follows, which the order of a transcript's calls
/// excludes.
#[must_use]
pub fn rebuild_section(transcripts: &[Transcript]) -> RebuildSection {
    let mut section = RebuildSection::default();
    for transcript in transcripts {
        let mut previous = transcript.before_range.as_ref().map(|prior| Previous {
            timestamp: prior.timestamp,
            model: prior.model.as_str(),
        });
        for call in &transcript.calls {
            let written = cost(&call.model, &call.usage)
                .unwrap_or_default()
                .cache_write();
            let kind = previous.map_or(Kind::First, |_| kind(&call.usage));
            let kind_key = (transcript.thread, kind);
            let by_kind = section.writes_by_kind.entry(kind_key).or_default();
            *by_kind = add(*by_kind, u128::from(written));
            if kind == Kind::Rebuild {
                let Some(before) = previous else {
                    unreachable!("a rebuild follows a call")
                };
                let gap = call.timestamp.duration_since(before.timestamp).as_millis();
                assert!(gap >= 0, "a call follows the call before it: {gap} ms");
                let switched = before.model != call.model;
                let band = (transcript.thread, Gap::of(gap));
                let rebuilds = section.rebuilds.entry(band).or_default();
                *rebuilds = Rebuilds {
                    count: count_one(rebuilds.count),
                    after_model_switch: if switched {
                        count_one(rebuilds.after_model_switch)
                    } else {
                        rebuilds.after_model_switch
                    },
                    nanodollars: add(rebuilds.nanodollars, u128::from(written)),
                };
            }
            previous = Some(Previous {
                timestamp: call.timestamp,
                model: call.model.as_str(),
            });
        }
    }
    section
}

/// What a call after its transcript's first did with the cache.
fn kind(usage: &Usage) -> Kind {
    let context = usage.context();
    let Some(written) = usage.cache_write_5m.checked_add(usage.cache_write_1h) else {
        unreachable!("two token fields of at most ten million fit a u64")
    };
    let Some(doubled) = written.checked_mul(2) else {
        unreachable!("a doubled token count of at most twenty million fits a u64")
    };
    let long = context > REBUILD_CONTEXT_FLOOR_TOKENS;
    let most = doubled > context;
    if long {
        if most {
            return Kind::Rebuild;
        }
    }
    Kind::Incremental
}
