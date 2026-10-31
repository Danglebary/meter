//! The day's spend under a spend limit: the snapshot holding the limit's spend at the day's
//! first reading, which reading starts the day, and the pace that spreads what remains of
//! the limit over the business days left.

use jiff::civil::Date;
use jiff::{Timestamp, Zoned};
use serde_json::{Value, json};

/// The spend limit's spend at the day's first reading, kept between runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub date: Date,
    /// The period's `resets_at` at that reading, which changes when the period rolls over.
    pub resets_at: i64,
    pub used_usd: f64,
}

impl Snapshot {
    /// The snapshot as the JSON text its file holds.
    ///
    /// # Panics
    ///
    /// Where the snapshot holds a spend that is negative or not finite, or a reset that is
    /// not a positive count of seconds.
    #[must_use]
    pub fn to_json(&self) -> String {
        assert!(self.used_usd.is_finite(), "a kept spend is a finite amount");
        assert!(self.used_usd >= 0.0, "a kept spend is not negative");
        assert!(
            self.resets_at > 0,
            "a kept reset is a positive count of seconds"
        );
        let value = json!({
            "date": self.date.to_string(),
            "resets_at": self.resets_at,
            "used_usd": self.used_usd,
        });
        value.to_string()
    }

    /// Reads back a snapshot `to_json` wrote.
    ///
    /// # Panics
    ///
    /// Where `text` is not a snapshot `to_json` wrote: the file is meter's own, so a
    /// mismatch is meter's bug rather than an outcome to handle.
    #[must_use]
    pub fn from_json(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            unreachable!("the day's spend snapshot is not JSON: {text}")
        };
        let date = value
            .get("date")
            .and_then(Value::as_str)
            .and_then(|date| date.parse::<Date>().ok());
        let Some(date) = date else {
            unreachable!("the day's spend snapshot holds no date: {text}")
        };
        let Some(resets_at) = value.get("resets_at").and_then(Value::as_i64) else {
            unreachable!("the day's spend snapshot holds no reset: {text}")
        };
        let Some(used_usd) = value.get("used_usd").and_then(Value::as_f64) else {
            unreachable!("the day's spend snapshot holds no spend: {text}")
        };
        let snapshot = Self {
            date,
            resets_at,
            used_usd,
        };
        assert!(
            snapshot.resets_at > 0,
            "a kept reset is a positive count of seconds"
        );
        assert!(snapshot.used_usd >= 0.0, "a kept spend is not negative");
        snapshot
    }
}

/// A spend reading, and the snapshot kept from an earlier one.
pub struct DayStartInput<'a> {
    pub previous: Option<&'a Snapshot>,
    pub now: &'a Zoned,
    pub resets_at: i64,
    pub used_usd: f64,
}

/// The limit's spend at the day's start, and the snapshot to keep where it changed.
#[derive(Debug, Clone, PartialEq)]
pub struct DayStart {
    pub used_usd: f64,
    pub write: Option<Snapshot>,
}

/// Which reading starts the day.
///
/// Today's snapshot of the same period stands; a period that rolled over today starts the day
/// at nothing spent; any other reading starts the day itself, as does one whose spend fell
/// below the kept start.
///
/// # Panics
///
/// Where the reading's spend is negative or not finite, which the input's check excludes.
#[must_use]
pub fn day_start(input: &DayStartInput) -> DayStart {
    assert!(
        input.used_usd.is_finite(),
        "a spend reading is a finite amount"
    );
    assert!(input.used_usd >= 0.0, "a spend reading is not negative");
    let today = input.now.date();
    let Some(previous) = input.previous else {
        return starting(today, input.resets_at, input.used_usd);
    };
    if rolled_over_today(previous, input) {
        return starting(today, input.resets_at, 0.0);
    }
    if previous.date == today {
        if previous.resets_at == input.resets_at {
            if previous.used_usd <= input.used_usd {
                return DayStart {
                    used_usd: previous.used_usd,
                    write: None,
                };
            }
        }
    }
    starting(today, input.resets_at, input.used_usd)
}

const fn starting(date: Date, resets_at: i64, used_usd: f64) -> DayStart {
    DayStart {
        used_usd,
        write: Some(Snapshot {
            date,
            resets_at,
            used_usd,
        }),
    }
}

/// The kept period ended today: its reset changed, and the old reset falls on today.
fn rolled_over_today(previous: &Snapshot, input: &DayStartInput) -> bool {
    if previous.resets_at == input.resets_at {
        return false;
    }
    match Timestamp::from_second(previous.resets_at) {
        Ok(reset) => reset.to_zoned(input.now.time_zone().clone()).date() == input.now.date(),
        Err(_) => false,
    }
}

/// What remained of a limit at the day's start, spread over the business days left.
pub struct Pace {
    pub limit_usd: f64,
    pub day_start_usd: f64,
    /// Business days left in the period, today included.
    pub business_days_left: u32,
}

impl Pace {
    /// What can be spent today and still land on the limit: what remained at the day's
    /// start divided by the business days left, or all of it where none is left.
    ///
    /// # Panics
    ///
    /// Where the limit is not positive, which the input's check excludes.
    #[must_use]
    pub fn per_day(&self) -> f64 {
        assert!(self.limit_usd > 0.0, "a spend limit is positive");
        let remaining = (self.limit_usd - self.day_start_usd).max(0.0);
        let per_day = if self.business_days_left == 0 {
            remaining
        } else {
            remaining / f64::from(self.business_days_left)
        };
        assert!(
            per_day <= remaining,
            "a day's share holds no more than what remains"
        );
        per_day
    }
}
