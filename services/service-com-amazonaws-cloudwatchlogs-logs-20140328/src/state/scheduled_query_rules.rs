// SPDX-License-Identifier: CC0-1.0

//! The rules a scheduled query's configuration is held to, shared by the two operations that
//! STATE one — `CreateScheduledQuery` and `UpdateScheduledQuery` — so a configuration a create
//! accepts is one an update accepts and both refuse the same request in the same words.
//!
//! Four rules live here:
//!
//! * `executionRoleArn` ("The ARN of the IAM role that grants permissions to execute the query")
//!   must be an IAM ROLE ARN, which the service's own request validator holds as a regular
//!   expression ([`EXECUTION_ROLE_ARN_PATTERN`]) and states as the member-constraint refusal
//!   every other parameter outside its constraint is stated with;
//! * `scheduleExpression` ("A cron expression that defines when the scheduled query runs. The
//!   expression uses standard cron syntax and supports minute-level precision") must PARSE as a
//!   schedule — a six-field `cron(...)`, read field by field by the `cron` crate, or a
//!   `rate(<value> <unit>)` — and one that does not is refused before anything is stored
//!   ([`schedule_expression_refusal`]). The format is this service's own, down to the field
//!   count its refusal names, so the rule lives with the other rules of a scheduled query's
//!   configuration rather than with any other service's schedule;
//! * the log groups a query reads ("An array of log group names or ARNs to query") are resolved
//!   against the log groups the account HOLDS, so a write naming one it does not hold is refused
//!   about that log group rather than about the scheduled query;
//! * `endTimeOffset` has an effective value a reader reports whether or not the write stated
//!   one ([`DEFAULT_END_TIME_OFFSET_SECONDS`]).
//!
//! The same parse also answers which INSTANTS a stored schedule names ([`schedule_of`]), which
//! is what decides when a scheduled query's runs fall due
//! ([`super::scheduled_query_executions`]): an expression a write accepted is one whose
//! instants can be walked, and one it refused names none.

use super::ComAmazonawsCloudwatchlogsLogs20140328State as State;

/// The member both writes of a scheduled query name its executing role in.
pub const EXECUTION_ROLE_ARN_MEMBER: &str = "executionRoleArn";

/// The expression a scheduled query's `executionRoleArn` must satisfy, as the SERVICE publishes
/// it for that member: an IAM role ARN of one of the AWS partitions, with the empty Region an
/// IAM ARN carries, a 12-digit account id and a `role/` resource part naming the role.
///
/// Stated UNANCHORED, which is the form this service's pattern-rendering rule states for an
/// expression AWS publishes (see `requirement` in `crate::service`), and the ONE definition the
/// refusal's text and the check itself both read — so the constraint a request is held to cannot
/// drift from the constraint the refusal states.
pub const EXECUTION_ROLE_ARN_PATTERN: &str =
    r"arn:(?:aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::([0-9]{12}):role\/(.+)";

/// [`EXECUTION_ROLE_ARN_PATTERN`] compiled once. A `@pattern` is satisfied by a value the
/// expression MATCHES, which for an unanchored expression is a value it matches anywhere in,
/// exactly as the service's own validator applies it.
static EXECUTION_ROLE_ARN: std::sync::LazyLock<::regex::Regex> = std::sync::LazyLock::new(|| {
    ::regex::Regex::new(EXECUTION_ROLE_ARN_PATTERN)
        .expect("the published executionRoleArn expression compiles")
});

/// The refusal message for an `executionRoleArn` that is not an IAM role ARN, or `None` for one
/// that is.
///
/// The text is the service's one member-constraint sentence
/// ([`super::validation_message`]) — the member that failed and the expression it failed, and
/// nothing of the value submitted there.
pub fn execution_role_arn_refusal(role_arn: &str) -> Option<String> {
    (!EXECUTION_ROLE_ARN.is_match(role_arn)).then(|| {
        super::validation_message(
            EXECUTION_ROLE_ARN_MEMBER,
            &format!("Member must satisfy regular expression pattern: {EXECUTION_ROLE_ARN_PATTERN}"),
        )
    })
}

/// The number of whitespace-separated fields a `cron(...)` schedule carries — "minutes hours
/// day_of_month month day_of_week year" — which is the format the refusal below states and the
/// one definition the check itself reads, so the two cannot drift apart.
const CRON_FIELD_COUNT: usize = 6;

/// The units a `rate(...)` schedule states its value in — minutes, hours and days, the units
/// AWS publishes for a schedule expression — each with the milliseconds one of it holds, which
/// is what makes a rate a period between instants ([`Schedule::next_after`]). Named in the
/// SINGULAR: a value of one takes the singular unit and every larger one the plural, so the
/// plural form is the same name with its `s`.
const RATE_UNITS: [(&str, i64); 3] = [
    ("minute", 60_000),
    ("hour", 3_600_000),
    ("day", 86_400_000),
];

/// The clause a schedule expression that is neither form is refused with, naming the format
/// expected of it and NOTHING of the value submitted or of the resource being written.
const INVALID_SCHEDULE_EXPRESSION_CLAUSE: &str = "Invalid cron expression format. Expected \
     format: cron(minutes hours day_of_month month day_of_week year)";

/// The refusal a request whose schedule expression does not parse is answered with: the
/// service's violation envelope over [`INVALID_SCHEDULE_EXPRESSION_CLAUSE`], whose count is the
/// number of violations the request carries — ONE, the schedule this check judges.
fn invalid_schedule_expression_message() -> String {
    format!("1 validation errors detected: {INVALID_SCHEDULE_EXPRESSION_CLAUSE}")
}

/// The refusal message for a `scheduleExpression` that is not a schedule at all, or `None` for
/// one that is.
///
/// The member is "A cron expression that defines when the scheduled query runs", so a value that
/// does not PARSE as a schedule configures nothing and is refused rather than stored: the whole
/// value is either a `cron(...)` of [`CRON_FIELD_COUNT`] fields that PARSE, or a
/// `rate(<value> <unit>)` stating a positive count of one of the [`RATE_UNITS`] (singular for a
/// value of one, plural for any larger one, as AWS publishes the rate form).
///
/// The cron body is read field by field by the `cron` crate, so a field that is not a legal cron
/// field in the place it sits — a value past the field's range, a name no month or weekday has,
/// a token that is not cron at all — fails the parse rather than being stored for a schedule
/// nothing could run; an expression that parses is reported back byte for byte by every reader.
pub fn schedule_expression_refusal(expression: &str) -> Option<String> {
    (!is_schedule_expression(expression)).then(invalid_schedule_expression_message)
}

/// Whether `expression` parses as either form of schedule expression. The ONE parse both writes
/// of a scheduled query are held to, so a schedule a create accepts is one an update accepts.
fn is_schedule_expression(expression: &str) -> bool {
    if let Some(body) = schedule_body(expression, "cron") {
        return is_cron_body(body);
    }
    if let Some(body) = schedule_body(expression, "rate") {
        return is_rate_body(body);
    }
    false
}

/// The text inside `form(...)`, or `None` when `expression` is not that form at all.
fn schedule_body<'a>(expression: &'a str, form: &str) -> Option<&'a str> {
    expression
        .strip_prefix(form)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

/// Whether a `cron(...)` body carries its [`CRON_FIELD_COUNT`] fields and each of them parses in
/// the place it sits, so the whole expression names instants.
fn is_cron_body(body: &str) -> bool {
    let fields: Vec<&str> = body.split_whitespace().collect();
    fields.len() == CRON_FIELD_COUNT
        && fields
            .iter()
            .enumerate()
            .all(|(index, field)| cron_field_parses(index, field))
        && parsed_cron(&fields).is_some()
}

/// The index of the day-of-month and the day-of-week field in the six the format states, the two
/// fields `?` ("no specific value") and AWS's own day markers are written in.
const DAY_OF_MONTH_FIELD: usize = 2;
const DAY_OF_WEEK_FIELD: usize = 4;

/// Whether `field` is a legal cron field in the place `index` names, judged by parsing it THERE
/// with every other field held at `*` — the token every field accepts — so the field that cannot
/// be read is the one that fails rather than the whole expression.
fn cron_field_parses(index: usize, field: &str) -> bool {
    let mut expression = ["0", "*", "*", "*", "*", "*", "*"];
    expression[index + 1] = normalised_cron_field(index, field);
    expression.join(" ").parse::<::cron::Schedule>().is_ok()
}

/// The whole expression as the parser reads it, or `None` for a combination of fields its grammar
/// does not admit even though each field parses in its own place.
fn parsed_cron(fields: &[&str]) -> Option<::cron::Schedule> {
    let normalised: Vec<&str> = ::std::iter::once("0")
        .chain(
            fields
                .iter()
                .enumerate()
                .map(|(index, field)| normalised_cron_field(index, field)),
        )
        .collect();
    normalised.join(" ").parse::<::cron::Schedule>().ok()
}

/// What the parser reads a field as, for the tokens AWS's cron syntax carries and the standard
/// grammar has no spelling of:
///
/// * `?` is "no specific value", which in the day-of-month and day-of-week fields — the only two
///   it may be written in — states the same set of days as `*`;
/// * `L` (the last day), `LW` (the last weekday) and `<day>W` (the weekday nearest a day) in the
///   day-of-month field, and `<day>L` (the last such weekday) and `<day>#<nth>` (the nth such
///   weekday) in the day-of-week field, each name a day of the month the grammar cannot spell,
///   so they stand for their field's whole range once the expression is handed to the parser.
///
/// Everything else is handed over as written, which is what makes the parser — rather than this
/// module — the judge of a field's values, names, ranges, lists and steps.
fn normalised_cron_field(index: usize, field: &str) -> &str {
    let stands_for_the_whole_field = match index {
        DAY_OF_MONTH_FIELD => field == "?" || is_day_of_month_marker(field),
        DAY_OF_WEEK_FIELD => field == "?" || is_day_of_week_marker(field),
        _ => false,
    };
    if stands_for_the_whole_field {
        "*"
    } else {
        field
    }
}

/// Whether a day-of-month field is one of AWS's day markers: the last day of the month, the last
/// weekday of the month, or the weekday nearest a day of the month.
fn is_day_of_month_marker(field: &str) -> bool {
    field == "L"
        || field == "LW"
        || field
            .strip_suffix('W')
            .is_some_and(|day| day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day)))
}

/// Whether a day-of-week field is one of AWS's day markers: the last given weekday of the month,
/// or the nth given weekday of the month (AWS admits the first through the fifth).
fn is_day_of_week_marker(field: &str) -> bool {
    if let Some(day) = field.strip_suffix('L') {
        return is_weekday(day);
    }
    match field.split_once('#') {
        Some((day, nth)) => {
            is_weekday(day) && nth.parse::<u8>().is_ok_and(|nth| (1..=5).contains(&nth))
        }
        None => false,
    }
}

/// Whether a token names ONE day of the week, in either spelling the field takes: the day's
/// number (1 through 7, SUN through SAT), or its three-letter name — AWS's `<day>L` and
/// `<day>#<nth>` markers each name a single day, so a wildcard, a list or a range in that place
/// is not one of them.
fn is_weekday(day: &str) -> bool {
    /// The three-letter name of each day, in the order the field numbers them.
    const WEEKDAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

    day.parse::<u8>().is_ok_and(|number| (1..=7).contains(&number))
        || WEEKDAY_NAMES
            .iter()
            .any(|name| day.eq_ignore_ascii_case(name))
}

/// Whether a `rate(...)` body states a positive value and a unit that agrees with it in number.
fn is_rate_body(body: &str) -> bool {
    rate_interval_ms(body).is_some()
}

/// The milliseconds between two instants a `rate(...)` body names, or `None` for a body that
/// names no period: the value and the unit are the ONE reading both the parse above and the
/// schedule's own instants are taken from, so a rate a write accepts is one the clock can run.
///
/// A rate runs the query every `value` units, so a value of none is no schedule; and a value of
/// one takes the singular unit, every larger one the plural, as AWS publishes the rate form.
fn rate_interval_ms(body: &str) -> Option<i64> {
    let fields: Vec<&str> = body.split_whitespace().collect();
    let [value, unit] = fields[..] else {
        return None;
    };
    let value = value.parse::<i64>().ok().filter(|value| *value > 0)?;
    if unit.ends_with('s') != (value > 1) {
        return None;
    }
    let singular = unit.strip_suffix('s').unwrap_or(unit);
    RATE_UNITS
        .iter()
        .find(|(name, _)| *name == singular)
        .map(|(_, millis)| value.saturating_mul(*millis))
}

/// The INSTANTS a schedule expression names — what turns a stored `scheduleExpression` into
/// the runs a scheduled query owes ([`super::scheduled_query_executions`]).
///
/// One value per form the expression takes, each read by the SAME parse the writes of a
/// scheduled query are held to ([`schedule_expression_refusal`]), so an expression a write
/// accepted is one whose instants can be walked and one it refused names none.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// A `cron(...)` expression, as the `cron` crate reads it: the instants it names are its
    /// own, field by field. Boxed, because a parsed cron schedule is far larger than the
    /// period beside it.
    Cron(Box<::cron::Schedule>),
    /// A `rate(<value> <unit>)` expression: one instant every so many milliseconds.
    Every(i64),
}

/// The schedule `expression` names, or `None` for a value that is no schedule at all.
pub fn schedule_of(expression: &str) -> Option<Schedule> {
    if let Some(body) = schedule_body(expression, "cron") {
        let fields: Vec<&str> = body.split_whitespace().collect();
        if fields.len() != CRON_FIELD_COUNT {
            return None;
        }
        return parsed_cron(&fields).map(|schedule| Schedule::Cron(Box::new(schedule)));
    }
    schedule_body(expression, "rate")
        .and_then(rate_interval_ms)
        .map(Schedule::Every)
}

impl Schedule {
    /// The first instant this schedule names strictly AFTER `instant_ms`, in the epoch
    /// milliseconds a scheduled query keeps its instants in, or `None` when it names no
    /// further instant.
    ///
    /// `timezone` is the query's own `ScheduleTimezone` — "The timezone for evaluating the
    /// schedule expression. This determines when the scheduled query executes relative to the
    /// specified timezone" — so a cron expression naming a time of day names it THERE; a query
    /// that states no timezone, or one no zone database knows, is evaluated in UTC.
    pub fn next_after(&self, instant_ms: i64, timezone: Option<&str>) -> Option<i64> {
        match self {
            Self::Cron(schedule) => {
                let after = ::chrono::DateTime::<::chrono::Utc>::from_timestamp_millis(instant_ms)?;
                schedule
                    .after(&after.with_timezone(&schedule_timezone(timezone)))
                    .next()
                    .map(|instant| instant.timestamp_millis())
            }
            Self::Every(interval_ms) => Some(instant_ms.saturating_add(*interval_ms)),
        }
    }

    /// The PERIOD of this schedule around `instant_ms`: the gap between the first instant it
    /// names after that one and the instant it names after THAT — which for a schedule naming
    /// instants at regular intervals is the interval itself, whatever instant it is asked
    /// about. `None` when the schedule names no two further instants.
    ///
    /// It is the lookback a query stating none of its own reads back over, so it has to be the
    /// schedule's period rather than the time still to run until the next instant: a run
    /// triggered at an instant the schedule does NOT name — the first run of a query, which
    /// falls a short span after the create — would otherwise read back only as far as the next
    /// named instant is away, a window the time of day decides rather than the schedule.
    pub fn interval_after(&self, instant_ms: i64, timezone: Option<&str>) -> Option<i64> {
        let next = self.next_after(instant_ms, timezone)?;
        self.next_after(next, timezone)
            .map(|following| following.saturating_sub(next))
    }
}

/// The timezone a cron expression is evaluated in: the zone the query names, or UTC for a
/// query that names none and for a name no zone database knows.
fn schedule_timezone(timezone: Option<&str>) -> ::chrono_tz::Tz {
    timezone
        .and_then(|zone| zone.parse::<::chrono_tz::Tz>().ok())
        .unwrap_or(::chrono_tz::UTC)
}

/// The refusal message for the FIRST of `identifiers` naming a log group this account does not
/// hold, in the order the request named them, or `None` when every one of them names a group it
/// holds.
///
/// A `ScheduledQueryLogGroupIdentifiers` entry is a log group's name or its ARN, so each is
/// resolved to the name the store is keyed by ([`super::log_group_name_from_identifier`]), the
/// way every other member that names a log group by either form is. The sentence is the one this
/// service states for a log group the account does not hold
/// ([`super::query_admission::log_group_not_found_refusal`]), so a scheduled query's missing log
/// group reads exactly like a started query's.
pub fn unheld_log_group_refusal(store: &State, identifiers: &[String]) -> Option<String> {
    identifiers
        .iter()
        .map(|identifier| super::log_group_name_from_identifier(identifier))
        .find(|name| !store.log_groups.contains_key(*name))
        .map(super::query_admission::log_group_not_found_refusal)
}

/// The end of the lookback window of a scheduled query whose write stated no `endTimeOffset`:
/// the offset "in seconds that defines the end of the lookback period for the query", which for
/// a query that states none ends the window AT the execution instant — no seconds before it.
pub const DEFAULT_END_TIME_OFFSET_SECONDS: i64 = 0;

/// The `endTimeOffset` a write RESOLVES to: the offset it stated, or
/// [`DEFAULT_END_TIME_OFFSET_SECONDS`] when it stated none.
///
/// A scheduled query always runs over a window with an end, so the stored record carries the
/// effective offset rather than the absence of one and every reader of the query reports it.
pub fn effective_end_time_offset(stated: Option<i64>) -> Option<i64> {
    Some(stated.unwrap_or(DEFAULT_END_TIME_OFFSET_SECONDS))
}

#[cfg(test)]
mod tests {
    use super::{
        effective_end_time_offset, execution_role_arn_refusal, schedule_expression_refusal,
        schedule_of, DEFAULT_END_TIME_OFFSET_SECONDS, EXECUTION_ROLE_ARN_PATTERN,
    };

    /// The PERIOD of a schedule is the gap between the instants it names, whatever instant it
    /// is read at — which is the lookback a scheduled query stating none of its own reads back
    /// over, so two consecutive runs of it tile the time line.
    #[test]
    fn a_schedules_period_is_the_gap_between_the_instants_it_names() {
        let hour = 60 * 60 * 1_000i64;
        let day = 24 * hour;
        // 2026-01-01T00:00:00Z, so every row's instants are stated against one known day.
        let midnight = 1_767_225_600_000i64;
        // (case, the expression, the instant it is read at, the period in milliseconds)
        let cases: [(&str, &str, i64, i64); 8] = [
            ("a daily cron read at midnight", "cron(0 3 * * ? *)", midnight, day),
            (
                "the same cron read an hour before the instant it names",
                "cron(0 3 * * ? *)",
                midnight + 2 * hour,
                day,
            ),
            (
                "the same cron read a minute after that instant",
                "cron(0 3 * * ? *)",
                midnight + 3 * hour + 60_000,
                day,
            ),
            (
                "an hourly cron read on the hour",
                "cron(0 * * * ? *)",
                midnight,
                hour,
            ),
            (
                "an hourly cron read mid-hour",
                "cron(0 * * * ? *)",
                midnight + hour / 2,
                hour,
            ),
            ("a rate of five minutes", "rate(5 minutes)", midnight, 5 * 60 * 1_000),
            ("a rate of twelve hours", "rate(12 hours)", midnight + 7 * hour, 12 * hour),
            ("a rate of one day", "rate(1 day)", midnight + 7 * hour, day),
        ];
        for (case, expression, read_at, expected) in cases {
            let schedule = schedule_of(expression)
                .unwrap_or_else(|| panic!("case {case:?}: {expression} is a schedule"));
            let actual = schedule.interval_after(read_at, None);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected a period of {expected} ms, got {actual:?}"
            );
        }
    }

    /// A schedule expression parses as a six-field `cron(...)` or a well-formed `rate(...)`, and
    /// every value that is neither is refused with the one sentence this class is stated with.
    #[test]
    fn a_schedule_expression_parses_as_a_six_field_cron_or_a_rate() {
        // (case, the value, whether it parses as a schedule)
        let cases: [(&str, &str, bool); 32] = [
            ("every hour, on the hour", "cron(0 * * * ? *)", true),
            ("every five minutes", "cron(*/5 * * * ? *)", true),
            ("a list, a range and a step", "cron(0,30 9-17/2 ? * MON-FRI *)", true),
            ("the last day of the month", "cron(15 10 L * ? *)", true),
            ("the last weekday of the month", "cron(15 10 LW * ? *)", true),
            ("the weekday nearest the 15th", "cron(15 10 15W * ? *)", true),
            ("the third Friday", "cron(0 12 ? * 6#3 *)", true),
            ("the last Friday", "cron(0 12 ? * 6L *)", true),
            ("a year stated as a range", "cron(0 12 1 * ? 2026-2030)", true),
            ("a rate of minutes", "rate(5 minutes)", true),
            ("a rate of one minute", "rate(1 minute)", true),
            ("a rate of hours", "rate(12 hours)", true),
            ("a rate of one day", "rate(1 day)", true),
            ("four cron fields", "cron(0 12 * *)", false),
            ("five cron fields", "cron(0 12 * * ?)", false),
            ("seven cron fields", "cron(0 12 * * ? * *)", false),
            ("an empty cron body", "cron()", false),
            ("a cron field outside cron's alphabet", "cron(0 12 * * ? ev€ry)", false),
            ("a minute spelled as a word", "cron(sixty 10 * * ? *)", false),
            ("a minute past its range", "cron(99 10 * * ? *)", false),
            ("an hour past its range", "cron(0 99 * * ? *)", false),
            ("a day of the month past its range", "cron(0 12 32 * ? *)", false),
            ("a month no calendar has", "cron(0 12 * SMARCH ? *)", false),
            ("a weekday no week has", "cron(0 10 ? * FUNDAY *)", false),
            ("markers in place of every field", "cron(sixty 99 L L L L)", false),
            ("a last-weekday marker over a wildcard", "cron(0 12 ? * *L *)", false),
            ("a weekday marker outside the field's days", "cron(0 12 ? * 0L *)", false),
            ("a nth-weekday marker over a list", "cron(0 12 ? * 1,2#3 *)", false),
            ("a rate of no units at all", "rate(5)", false),
            ("a rate of zero", "rate(0 minutes)", false),
            ("a rate whose unit disagrees in number", "rate(1 minutes)", false),
            ("a rate of an unpublished unit", "rate(5 fortnights)", false),
        ];
        for (case, value, parses) in cases {
            let refusal = schedule_expression_refusal(value);
            assert_eq!(
                refusal.is_none(),
                parses,
                "case {case:?}: expected parses={parses}, got {refusal:?}"
            );
            if let Some(said) = refusal {
                assert_eq!(
                    said,
                    "1 validation errors detected: Invalid cron expression format. Expected \
                     format: cron(minutes hours day_of_month month day_of_week year)",
                    "case {case:?}: the refusal states the expected format"
                );
            }
        }
    }

    /// The published expression admits an IAM role ARN of each partition it names and refuses
    /// every other shape of value, and the refusal states that expression.
    #[test]
    fn the_execution_role_expression_admits_an_iam_role_arn_of_each_partition() {
        // (case, the value, whether it satisfies the expression)
        let cases: [(&str, &str, bool); 12] = [
            ("the standard partition", "arn:aws:iam::000000000000:role/svc", true),
            ("the China partition", "arn:aws-cn:iam::123456789012:role/svc", true),
            (
                "the GovCloud partition",
                "arn:aws-us-gov:iam::123456789012:role/svc",
                true,
            ),
            ("an ISO partition", "arn:aws-iso:iam::123456789012:role/svc", true),
            ("an ISO-B partition", "arn:aws-iso-b:iam::123456789012:role/svc", true),
            (
                "a role under a path",
                "arn:aws:iam::123456789012:role/service-role/svc",
                true,
            ),
            ("not an ARN at all", "scheduled-query-execution", false),
            ("an ARN of another service", "arn:aws:logs:us-east-1:123456789012:log-group:g", false),
            ("an IAM user rather than a role", "arn:aws:iam::123456789012:user/someone", false),
            ("an account id that is too short", "arn:aws:iam::12345:role/svc", false),
            ("a role with no name", "arn:aws:iam::123456789012:role/", false),
            ("an IAM ARN naming a Region", "arn:aws:iam:us-east-1:123456789012:role/svc", false),
        ];
        for (case, value, admitted) in cases {
            let refusal = execution_role_arn_refusal(value);
            assert_eq!(
                refusal.is_none(),
                admitted,
                "case {case:?}: expected admitted={admitted}, got {refusal:?}"
            );
            if let Some(said) = refusal {
                assert_eq!(
                    said,
                    format!(
                        "1 validation error detected: Value at 'executionRoleArn' failed to \
                         satisfy constraint: Member must satisfy regular expression pattern: \
                         {EXECUTION_ROLE_ARN_PATTERN}"
                    ),
                    "case {case:?}: the refusal states the member and the expression"
                );
            }
        }
    }

    /// A write that states no end offset resolves to the documented one; a write that states
    /// one resolves to exactly it, including the boundary values.
    #[test]
    fn the_end_offset_a_write_states_none_for_resolves_to_the_default() {
        // (case, what the write stated, the offset it resolves to)
        let cases: [(&str, Option<i64>, i64); 4] = [
            ("stated none", None, DEFAULT_END_TIME_OFFSET_SECONDS),
            ("stated the default itself", Some(0), 0),
            ("stated a window of one hour", Some(3_600), 3_600),
            ("stated a window of a day", Some(86_400), 86_400),
        ];
        for (case, stated, expected) in cases {
            let actual = effective_end_time_offset(stated);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {expected}, got {actual:?}"
            );
        }
    }

    /// A schedule's period — what a query stating no lookback of its own reads back over — is
    /// the span between two consecutive instants the schedule names, and it does not depend on
    /// the instant it is asked about: two runs of one query read windows of the same length
    /// however long is left until the next instant when each was triggered.
    #[test]
    fn a_schedules_period_is_the_span_between_two_of_its_instants() {
        const MINUTE_MS: i64 = 60 * 1_000;
        const HOUR_MS: i64 = 60 * MINUTE_MS;
        const DAY_MS: i64 = 24 * HOUR_MS;

        // (case, the expression, the timezone it is evaluated in, its period)
        let schedules: [(&str, &str, Option<&str>, i64); 6] = [
            ("a daily cron", "cron(0 3 * * ? *)", None, DAY_MS),
            (
                "a daily cron in a named timezone",
                "cron(0 3 * * ? *)",
                Some("America/New_York"),
                DAY_MS,
            ),
            ("an hourly cron", "cron(0 * * * ? *)", None, HOUR_MS),
            ("a five-minute cron", "cron(*/5 * * * ? *)", None, 5 * MINUTE_MS),
            ("a rate of one day", "rate(1 day)", None, DAY_MS),
            ("a rate of five minutes", "rate(5 minutes)", None, 5 * MINUTE_MS),
        ];
        // The instants span what the period must NOT depend on: a moment after an instant the
        // daily cron names, a moment before the next one, the middle of the gap between two,
        // and a date in another month.
        let instants = [
            "2026-06-01T03:00:01Z",
            "2026-06-01T02:59:59Z",
            "2026-06-01T14:17:23Z",
            "2026-11-09T00:00:00Z",
        ];
        for (case, expression, timezone, period) in schedules {
            let schedule = schedule_of(expression)
                .unwrap_or_else(|| panic!("case {case:?}: {expression:?} names a schedule"));
            for instant in instants {
                let at = ::chrono::DateTime::parse_from_rfc3339(instant)
                    .expect("a fixed instant")
                    .timestamp_millis();
                let actual = schedule.interval_after(at, timezone);
                assert_eq!(
                    actual,
                    Some(period),
                    "case {case:?} at {instant}: expected a period of {period} ms, got {actual:?}"
                );
            }
        }
    }
}
