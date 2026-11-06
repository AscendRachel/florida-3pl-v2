//! The app's clock: US Eastern time (the Florida warehouse), whatever time zone the server is set to.
//! Every date and time the app records, and every hour on the Dashboard, uses this clock, so the people in the
//! Philippines and in the warehouse read the same times. Daylight saving follows the US rule (chrono-tz knows it).

use chrono::{NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::America::New_York;

/// The current US Eastern date and time, without a time zone attached.
pub fn now() -> NaiveDateTime {
    Utc::now().with_timezone(&New_York).naive_local()
}

/// Minutes to add to US Eastern time to get UTC: 300 in winter, 240 in summer time (what the pages expect as `tz_offset_min`).
pub fn offset_minutes() -> i32 {
    let utc = Utc::now();
    let east = New_York.offset_from_utc_datetime(&utc.naive_utc()).fix().local_minus_utc();
    -east / 60
}

/// A Unix time (seconds) as US Eastern date and time.
pub fn from_unix(secs: f64) -> NaiveDateTime {
    let utc = chrono::DateTime::from_timestamp(secs as i64, 0).unwrap_or_default();
    utc.with_timezone(&New_York).naive_local()
}
