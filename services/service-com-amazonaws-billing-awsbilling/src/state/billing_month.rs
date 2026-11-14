// SPDX-License-Identifier: CC0-1.0

//! The `YYYY-MM` billing-month arithmetic this service's monthly charge
//! reports share.
//!
//! Both `ListBusinessSupportAccountCharges` and
//! `GetEnterpriseSupportChargeSummary` take a modeled billing month
//! (`^\d{4}-(0[1-9]|1[0-2])$`), compare it with the month the clock is in and
//! report the UTC period its charges accrued in, so that reading of a month
//! lives here rather than beside either handler.

/// How many ASCII digits the modeled `BillingMonth` states a year in
/// (`^\d{4}-(0[1-9]|1[0-2])$`), which is also the width the modeled `Timestamp`
/// renders one at.
const YEAR_DIGITS: usize = 4;

/// How many ASCII digits the same expression states a month in.
const MONTH_DIGITS: usize = 2;

/// The months one calendar year holds — the modulus of the month arithmetic
/// below, and the highest month a modeled `YYYY-MM` value can name.
const MONTHS_PER_YEAR: i32 = 12;

/// The year and month of a modeled `YYYY-MM` billing month, or `None` for a
/// value outside that form.
///
/// The whole expression is enforced here rather than only its separator: four
/// digits of year, then two digits naming a month of the calendar
/// (`0[1-9]|1[0-2]`). Every reader of a month takes its parts from here, so no
/// value a caller can state reaches the arithmetic below outside the range the
/// model admits.
pub fn parse_month(billing_month: &str) -> Option<(i32, i32)> {
    let (year, month) = billing_month.split_once('-')?;
    if year.len() != YEAR_DIGITS || month.len() != MONTH_DIGITS {
        return None;
    }
    if !year
        .bytes()
        .chain(month.bytes())
        .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let month = month.parse().ok()?;
    if !(1..=MONTHS_PER_YEAR).contains(&month) {
        return None;
    }
    Some((year.parse().ok()?, month))
}

/// A month as the count of whole calendar months before it, so two months
/// compare and subtract.
///
/// Saturating, so no pair of values can make the count wrap or panic: a month
/// comes from [`parse_month`] or from the clock, both inside the four-digit
/// range the model states, and an index that saturates still orders the same
/// way.
pub fn month_index((year, month): (i32, i32)) -> i32 {
    year.saturating_mul(MONTHS_PER_YEAR)
        .saturating_add(month - 1)
}

/// The first instant of a month, read off the UTC civil calendar.
///
/// `None` for a year the modeled `Timestamp` cannot render at four digits,
/// which only the month after the last `YYYY-MM` month is.
pub fn first_instant_of(year: i32, month: i32) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(
        &format!("{year:04}-{month:02}-01T00:00:00Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()
}

/// The month after `(year, month)` on the UTC civil calendar.
///
/// Saturating on the year for the same reason [`month_index`] is: the last year
/// the modeled `Timestamp` renders has no month after it anyway, and
/// [`first_instant_of`] answers `None` for one it cannot render.
pub fn next_month((year, month): (i32, i32)) -> (i32, i32) {
    if month >= MONTHS_PER_YEAR {
        (year.saturating_add(1), 1)
    } else {
        (year, month + 1)
    }
}

/// The half-open UTC period of one calendar month: its first instant, and the
/// first instant of the month after it.
///
/// AWS bills on UTC calendar months, so both bounds come from the month's own
/// position on the civil calendar — never from a fixed month length. Shared by
/// the reports that take a modeled `YYYY-MM` month ([`month_period`]) and by
/// `ListBillingViewSegments`, whose default period is "the current billing
/// period, which is the calendar month in UTC".
pub fn calendar_month_period(
    year: i32,
    month: i32,
) -> Option<(::aws_smithy_types::DateTime, ::aws_smithy_types::DateTime)> {
    let (next_year, next_month) = next_month((year, month));
    Some((
        first_instant_of(year, month)?,
        first_instant_of(next_year, next_month)?,
    ))
}

/// The half-open UTC period a billing month's charges accrue in: the first
/// instant of the month, and the first instant of the month after it.
pub fn month_period(
    billing_month: &str,
) -> Option<(::aws_smithy_types::DateTime, ::aws_smithy_types::DateTime)> {
    let (year, month) = parse_month(billing_month)?;
    calendar_month_period(year, month)
}
