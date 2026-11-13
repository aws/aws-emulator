// SPDX-License-Identifier: CC0-1.0

//! Where a metric's tumbling windows begin and end.

use super::*;

/// How many whole nanoseconds one day is.
const NANOS_PER_DAY: i128 = 24 * 60 * 60 * NANOS_PER_SECOND;

/// How many whole nanoseconds one week is.
const NANOS_PER_WEEK: i128 = 7 * NANOS_PER_DAY;

/// How far past the Unix epoch the first Sunday midnight (UTC) falls: the epoch
/// itself is a Thursday, and a weekly window is computed "the end of Sunday at
/// midnight each week (UTC)" (`TumblingWindow.interval`).
const EPOCH_TO_SUNDAY: i128 = 3 * NANOS_PER_DAY;

/// The grid of tumbling windows a metric is computed over: how long one window is
/// and where the boundaries fall.
///
/// The boundaries are NOT relative to when anything happened — "IoT SiteWise
/// computes the `1w` interval the end of Sunday at midnight each week (UTC), the
/// `1d` interval at the end of each day at midnight (UTC), the `1h` interval at
/// the end of each hour" — so they sit at whole multiples of the interval from the
/// Unix epoch, which is the midnight-and-hour grid that sentence describes, shifted
/// by [`EPOCH_TO_SUNDAY`] for a weekly interval and by the window's own `offset`.
#[derive(Debug, Clone, Copy)]
pub(super) struct WindowGrid {
    /// How long one window is, in whole nanoseconds. Always positive.
    interval_nanos: i128,
    /// Where the boundaries sit, as nanoseconds past an epoch-aligned grid.
    anchor_nanos: i128,
}

impl WindowGrid {
    /// The grid `window` names, or `None` when its interval names no span at all.
    pub(super) fn of_window(window: &TumblingWindow) -> Option<Self> {
        let interval_nanos = i128::try_from(window_span(&window.interval)?.as_nanos()).ok()?;
        if interval_nanos <= 0 {
            return None;
        }
        // A weekly grid ends on Sunday midnight (UTC) rather than on the
        // epoch-aligned Thursday; every shorter interval the published bounds
        // allow divides a day, so an epoch-aligned grid already falls at midnight
        // and on the hour.
        let calendar = if interval_nanos % NANOS_PER_WEEK == 0 {
            EPOCH_TO_SUNDAY
        } else {
            0
        };
        // An offset the documented spellings do not cover shifts the grid by
        // nothing, rather than taking the whole metric out of computation.
        let offset = window.offset.as_deref().and_then(window_offset).unwrap_or(0);
        Some(Self {
            interval_nanos,
            anchor_nanos: (calendar + offset).rem_euclid(interval_nanos),
        })
    }

    /// Which window the instant `at` falls in. "The start of each interval is
    /// exclusive and the end of each interval is inclusive", so an instant exactly
    /// on a boundary belongs to the window that ENDS there.
    pub(super) fn window_of(&self, at: i128) -> i128 {
        (at - self.anchor_nanos - 1).div_euclid(self.interval_nanos)
    }

    /// The instant window `window` ends at — where its computed data point sits —
    /// or `None` when that instant is past the range of an instant.
    pub(super) fn end_of(&self, window: i128) -> Option<i128> {
        window
            .checked_add(1)?
            .checked_mul(self.interval_nanos)?
            .checked_add(self.anchor_nanos)
    }
}

/// The span a tumbling window's `offset` shifts the grid by, in whole nanoseconds.
///
/// All three spellings `TumblingWindow.offset` documents: "the offset time" (`18h`)
/// and "the ISO 8601 format" (`PT18H`), both read by the one reading of a window
/// span this service shares ([`window_span`]), and "the 24-hour clock"
/// (`00:03:00`) — the time of day the grid is anchored at, which is that many
/// seconds past midnight.
fn window_offset(value: &str) -> Option<i128> {
    if let Some(span) = window_span(value) {
        return i128::try_from(span.as_nanos()).ok();
    }
    let mut fields = value.split(':');
    let hours: i128 = fields.next()?.parse().ok()?;
    let minutes: i128 = fields.next()?.parse().ok()?;
    let seconds: i128 = fields.next()?.parse().ok()?;
    if fields.next().is_some()
        || !(0..24).contains(&hours)
        || !(0..60).contains(&minutes)
        || !(0..60).contains(&seconds)
    {
        return None;
    }
    Some(((hours * 60 + minutes) * 60 + seconds) * NANOS_PER_SECOND)
}
