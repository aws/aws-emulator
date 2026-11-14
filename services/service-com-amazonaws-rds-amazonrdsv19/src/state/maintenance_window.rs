// SPDX-License-Identifier: CC0-1.0

//! The weekly maintenance window a DB instance or DB cluster keeps: the form the service
//! STORES it in, and when the next occurrence of one begins.
//!
//! One module for both, because the window is one concept with two uses: it is the value
//! every operation that takes a `PreferredMaintenanceWindow` stores and reports back, and
//! it is the schedule a modification the caller did not ask to apply immediately waits for
//! ([`super::instance_modification`]).

use super::*;

/// The days a maintenance window may name, in the `ddd` form
/// `PreferredMaintenanceWindow` documents: "Valid Days: Mon | Tue | Wed | Thu | Fri | Sat
/// | Sun", which the service stores and reports folded to lower case. The order is the
/// week's, so a day's position in it is how far into the week that day begins — the same
/// Monday-first numbering the calendar crate's own `num_days_from_monday` gives.
const WINDOW_DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;

/// The form the service STORES a preferred maintenance window in: the window as the
/// request wrote it, with its day names folded to lower case
/// (`mon:00:00-mon:03:00`), which is what every read of the resource reports back
/// whatever case the request used.
///
/// Folding the whole value is folding the day names: the rest of the
/// `ddd:hh24:mi-ddd:hh24:mi` form the member documents is digits and separators, which
/// have no case to fold.
pub fn stored_maintenance_window(requested: &str) -> String {
    requested.to_ascii_lowercase()
}

/// Whether `at` falls INSIDE an occurrence of `window` — whether the window is OPEN.
///
/// What a change "applied during the next maintenance window" waits for: the window is a
/// weekly span, so the change is owed from the moment the clock enters one of its
/// occurrences — and the window a resource keeps NOW is what decides it, so a resource whose
/// window moves has its pending changes applied in the window it has. That is what makes the
/// window CONTROL when a pending change is applied.
///
/// A span whose end is earlier in the week than its start wraps the week's end
/// (`sun:23:00-mon:01:00`) and is open on either side of it. `false` for a value that is not
/// a window at all, which schedules nothing: the caller leaves such a change pending rather
/// than applying it at a time no window names.
pub fn window_is_open(window: &str, at: SystemTime) -> bool {
    let Some((start, end)) = window_span_in_week(window) else {
        return false;
    };
    let Some(at_in_week) = seconds_into_week(at) else {
        return false;
    };
    if start <= end {
        (start..=end).contains(&at_in_week)
    } else {
        at_in_week >= start || at_in_week <= end
    }
}

/// How far into the week each end of `window` falls, in seconds from the start of Monday,
/// or `None` where the value is not of the documented `ddd:hh24:mi-ddd:hh24:mi` form.
fn window_span_in_week(window: &str) -> Option<(u64, u64)> {
    let (start, end) = window.split_once('-')?;
    Some((point_in_week(start)?, point_in_week(end)?))
}

/// How far into the week one `ddd:hh24:mi` point falls, in seconds from the start of Monday.
fn point_in_week(point: &str) -> Option<u64> {
    let mut parts = point.split(':');
    let day = parts.next()?.to_ascii_lowercase();
    let hour: u64 = parts.next()?.parse().ok()?;
    let minute: u64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    let day_index = WINDOW_DAYS.iter().position(|known| *known == day)? as u64;
    Some(day_index * SECONDS_PER_DAY + hour * SECONDS_PER_HOUR + minute * SECONDS_PER_MINUTE)
}

/// How far into the week `at` falls, in seconds from the start of Monday UTC, read through
/// the calendar of the `chrono` crate rather than by hand.
///
/// `None` for an instant that calendar cannot place, which names no point in any week.
fn seconds_into_week(at: SystemTime) -> Option<u64> {
    use ::chrono::{DateTime, Timelike, Utc};

    let epoch_seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let at = DateTime::<Utc>::from_timestamp(i64::try_from(epoch_seconds).ok()?, 0)?;
    let weekday = u64::from(
        ::chrono::Datelike::weekday(&at).num_days_from_monday(),
    );
    Some(
        weekday * SECONDS_PER_DAY
            + u64::from(at.hour()) * SECONDS_PER_HOUR
            + u64::from(at.minute()) * SECONDS_PER_MINUTE
            + u64::from(at.second()),
    )
}
