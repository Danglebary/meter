//! List prices, and what a call costs at them.
//!
//! The table holds the rates of the pricing page,
//! <https://platform.claude.com/docs/en/about-claude/pricing> — checked 2026-10-30, with
//! cache writes at 1.25x input for the 5-minute TTL and 2x for the 1-hour TTL. A rate the
//! page has changed keeps the rate before it beside the day the change took effect, which
//! the API release notes give,
//! <https://platform.claude.com/docs/en/release-notes/overview> — checked 2026-10-30. The
//! notes date a change by its day alone, so a change takes effect from the start of that
//! day in UTC. A price is held in nanodollars per token, a thousand times the dollars per
//! million tokens the table lists, so every price is a whole number.

use super::transcript::{Call, Usage};
use jiff::civil::Date;
use jiff::tz::Offset;

/// The date the price table was last checked against the pricing page.
pub const PRICES_AS_OF: &str = "2026-10-30";
/// The day Claude Sonnet 5.5's cache reads fell from 0.1x input to 0.05x.
const SONNET_5_5_CACHE_READ_CUT: Date = Date::constant(2026, 10, 7);
/// The context Claude Haiku 5.5 prices at its lower rates; a longer one takes the higher.
const HAIKU_5_5_LOWER_RATE_CONTEXT_MAX: u64 = 100_000;
/// A 5-minute cache write costs 1.25x input.
const CACHE_WRITE_5M: Multiple = Multiple {
    numerator: 5,
    denominator: 4,
};
/// A 1-hour cache write costs 2x input.
const CACHE_WRITE_1H: Multiple = Multiple {
    numerator: 2,
    denominator: 1,
};

/// A rational multiple of a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Multiple {
    numerator: u64,
    denominator: u64,
}

/// A model's prices, in nanodollars per token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rates {
    input: u64,
    output: u64,
    cache_read: u64,
}

/// The prices standing for `call`'s model on its day in UTC, for a call sending the context
/// `usage` reports, where the table holds the model.
fn rates(call: &Call, usage: &Usage) -> Option<Rates> {
    let context = usage.context();
    let rates = match call.model.as_bytes() {
        b"claude-opus-5-5" => Rates {
            input: 4_000,
            output: 20_000,
            cache_read: 200,
        },
        b"claude-fable-5-1" => Rates {
            input: 10_000,
            output: 50_000,
            cache_read: 250,
        },
        b"claude-sonnet-5-5" => {
            let day = Offset::UTC.to_datetime(call.timestamp).date();
            if day < SONNET_5_5_CACHE_READ_CUT {
                Rates {
                    input: 2_000,
                    output: 10_000,
                    cache_read: 200,
                }
            } else {
                Rates {
                    input: 2_000,
                    output: 10_000,
                    cache_read: 100,
                }
            }
        }
        b"claude-haiku-5-5" => {
            if context <= HAIKU_5_5_LOWER_RATE_CONTEXT_MAX {
                Rates {
                    input: 100,
                    output: 500,
                    cache_read: 10,
                }
            } else {
                Rates {
                    input: 500,
                    output: 2_500,
                    cache_read: 50,
                }
            }
        }
        b"claude-opus-5" => Rates {
            input: 5_000,
            output: 25_000,
            cache_read: 500,
        },
        b"claude-haiku-4-5-20251001" => Rates {
            input: 1_000,
            output: 5_000,
            cache_read: 100,
        },
        _ => return None,
    };
    assert!(
        rates.cache_read < rates.input,
        "a cache read costs less than input: {rates:?}"
    );
    assert!(
        rates.input < rates.output,
        "input costs less than output: {rates:?}"
    );
    Some(rates)
}

/// What one call cost, in nanodollars, by what its tokens bought.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
}

impl Cost {
    /// The cache writes at both TTLs.
    #[must_use]
    pub fn cache_write(&self) -> u64 {
        let Some(sum) = self.cache_write_5m.checked_add(self.cache_write_1h) else {
            unreachable!("a call's costs are bounded by its token bound times a price")
        };
        sum
    }

    /// The whole cost of the call.
    #[must_use]
    pub fn total(&self) -> u64 {
        let parts = [self.input, self.cache_read, self.cache_write(), self.output];
        let mut total: u64 = 0;
        for part in parts {
            let Some(sum) = total.checked_add(part) else {
                unreachable!("a call's costs are bounded by its token bound times a price")
            };
            total = sum;
        }
        total
    }
}

/// The cache-read rate standing for `call`, in nanodollars per token, where the table
/// prices its model.
#[must_use]
pub fn cache_read_rate(call: &Call) -> Option<u64> {
    let rates = rates(call, &call.usage)?;
    Some(rates.cache_read)
}

/// What `call` costs when it reports `usage`, at the rates standing for its model on its
/// day, where the table prices the model.
#[must_use]
pub fn cost(call: &Call, usage: &Usage) -> Option<Cost> {
    let rates = rates(call, usage)?;
    Some(Cost {
        input: priced(usage.input, rates.input),
        cache_read: priced(usage.cache_read, rates.cache_read),
        cache_write_5m: priced(usage.cache_write_5m, scaled(rates.input, CACHE_WRITE_5M)),
        cache_write_1h: priced(usage.cache_write_1h, scaled(rates.input, CACHE_WRITE_1H)),
        output: priced(usage.output, rates.output),
    })
}

/// A rate times a multiple that leaves it whole, as it leaves every rate in the table.
fn scaled(rate: u64, multiple: Multiple) -> u64 {
    let Multiple {
        numerator,
        denominator,
    } = multiple;
    let Some(product) = rate.checked_mul(numerator) else {
        unreachable!("a rate times a small multiple fits a u64: {rate}")
    };
    assert_eq!(
        product.checked_rem(denominator),
        Some(0),
        "the rate {rate} scales by {numerator}/{denominator} to a whole number"
    );
    let Some(quotient) = product.checked_div(denominator) else {
        unreachable!("the denominator is a nonzero constant")
    };
    quotient
}

fn priced(tokens: u64, rate: u64) -> u64 {
    let Some(cost) = tokens.checked_mul(rate) else {
        unreachable!("a token count of at most ten million times a rate fits a u64: {tokens}")
    };
    cost
}
