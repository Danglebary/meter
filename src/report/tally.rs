//! Checked sums of the report's counts and amounts. The bounds on transcripts, lines, and
//! tokens per field keep every sum far inside its type, so an overflow is a programmer
//! error.

/// A count with one more added.
#[must_use]
pub fn count_one(count: u64) -> u64 {
    plus(count, 1)
}

/// The sum of two counts.
#[must_use]
pub fn plus(count: u64, more: u64) -> u64 {
    let Some(total) = count.checked_add(more) else {
        unreachable!("bounded counts sum within a u64: {count} + {more}")
    };
    total
}

/// The sum of two amounts.
#[must_use]
pub fn add(sum: u128, part: u128) -> u128 {
    let Some(total) = sum.checked_add(part) else {
        unreachable!("bounded calls of bounded cost sum within a u128")
    };
    total
}
