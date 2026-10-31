//! Business days: weekdays, Monday through Friday, with no holiday list.

use jiff::civil::{Date, Weekday};

/// The longest range counted; a longer one is refused rather than walked.
const DAYS_MAX: u32 = 400;

/// The range spans more days than the count walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeTooLong;

/// The weekdays from `from` through `through`, both included, and none where `from` comes
/// after `through`.
///
/// # Errors
///
/// Returns `RangeTooLong` where the range spans more than `DAYS_MAX` days.
///
/// # Panics
///
/// Where the count of weekdays outgrows the days walked, which the walk excludes.
pub fn business_days(from: Date, through: Date) -> Result<u32, RangeTooLong> {
    let mut count: u32 = 0;
    let mut walked: u32 = 0;
    let mut day = from;
    while day <= through {
        if walked < DAYS_MAX {
            walked = walked.saturating_add(1);
        } else {
            return Err(RangeTooLong);
        }
        if is_business_day(day) {
            count = count.saturating_add(1);
        }
        let Ok(next) = day.tomorrow() else {
            return Err(RangeTooLong);
        };
        day = next;
    }
    assert!(
        count <= walked,
        "a count of days walked holds no more weekdays than days"
    );
    Ok(count)
}

fn is_business_day(day: Date) -> bool {
    matches!(
        day.weekday(),
        Weekday::Monday
            | Weekday::Tuesday
            | Weekday::Wednesday
            | Weekday::Thursday
            | Weekday::Friday
    )
}
