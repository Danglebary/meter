//! The cost section: what the calls cost at list price, by what the tokens bought, by
//! thread and model, by cache-write TTL, and by model and effort, with the models the
//! price table lacks named apart.

use std::collections::BTreeMap;

use super::price::{Cost, cost};
use super::range::Thread;
use super::transcript::Transcript;

/// The cost section. Every amount is in nanodollars, summed in a u128 so no count of calls
/// the store can hold overflows it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CostSection {
    pub buckets: Buckets,
    pub by_thread_and_model: BTreeMap<(Thread, String), Spend>,
    pub cache_writes: BTreeMap<(Thread, Ttl), Writes>,
    pub by_model_and_effort: BTreeMap<(String, Option<String>), Spend>,
    /// The calls of each model the price table lacks, left out of every amount.
    pub unpriced: BTreeMap<String, u64>,
}

/// The cost by what the tokens bought.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Buckets {
    pub input: u128,
    pub cache_read: u128,
    pub cache_write: u128,
    pub output: u128,
}

impl Buckets {
    #[must_use]
    pub fn total(&self) -> u128 {
        let mut total: u128 = 0;
        for part in [self.input, self.cache_read, self.cache_write, self.output] {
            total = add(total, part);
        }
        total
    }

    /// These buckets with one call's cost added.
    #[must_use]
    fn plus(self, priced: &Cost) -> Self {
        Self {
            input: add(self.input, u128::from(priced.input)),
            cache_read: add(self.cache_read, u128::from(priced.cache_read)),
            cache_write: add(self.cache_write, u128::from(priced.cache_write())),
            output: add(self.output, u128::from(priced.output)),
        }
    }
}

/// The calls of one group and what they cost.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Spend {
    pub calls: u64,
    pub nanodollars: u128,
}

impl Spend {
    /// This spend with one call added.
    #[must_use]
    fn plus(self, priced: &Cost) -> Self {
        Self {
            calls: count_one(self.calls),
            nanodollars: add(self.nanodollars, u128::from(priced.total())),
        }
    }
}

/// The tokens written to the cache at one TTL and what they cost.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Writes {
    pub tokens: u128,
    pub nanodollars: u128,
}

impl Writes {
    /// These writes with `more` added.
    #[must_use]
    fn plus(self, more: Self) -> Self {
        Self {
            tokens: add(self.tokens, more.tokens),
            nanodollars: add(self.nanodollars, more.nanodollars),
        }
    }
}

/// How long a cache write is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ttl {
    FiveMinutes,
    OneHour,
}

impl Ttl {
    /// The TTL as a report names it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
        }
    }
}

/// The cost section of the calls in `transcripts`.
#[must_use]
pub fn cost_section(transcripts: &[Transcript]) -> CostSection {
    let mut section = CostSection::default();
    for transcript in transcripts {
        for call in &transcript.calls {
            let Some(priced) = cost(&call.model, &call.usage) else {
                let calls = section.unpriced.entry(call.model.clone()).or_default();
                *calls = count_one(*calls);
                continue;
            };
            section.buckets = section.buckets.plus(&priced);
            let by_thread = section
                .by_thread_and_model
                .entry((transcript.thread, call.model.clone()))
                .or_default();
            *by_thread = by_thread.plus(&priced);
            let by_effort = section
                .by_model_and_effort
                .entry((call.model.clone(), call.effort.clone()))
                .or_default();
            *by_effort = by_effort.plus(&priced);
            let five = section
                .cache_writes
                .entry((transcript.thread, Ttl::FiveMinutes))
                .or_default();
            let five_written = Writes {
                tokens: u128::from(call.usage.cache_write_5m),
                nanodollars: u128::from(priced.cache_write_5m),
            };
            *five = five.plus(five_written);
            let hour = section
                .cache_writes
                .entry((transcript.thread, Ttl::OneHour))
                .or_default();
            let hour_written = Writes {
                tokens: u128::from(call.usage.cache_write_1h),
                nanodollars: u128::from(priced.cache_write_1h),
            };
            *hour = hour.plus(hour_written);
        }
    }
    section.cache_writes.retain(|_, writes| writes.tokens > 0);
    section
}

fn add(sum: u128, part: u128) -> u128 {
    let Some(total) = sum.checked_add(part) else {
        unreachable!("bounded calls of bounded cost sum within a u128")
    };
    total
}

fn count_one(count: u64) -> u64 {
    let Some(next) = count.checked_add(1) else {
        unreachable!("a count of calls fits a u64")
    };
    next
}
