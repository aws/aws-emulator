// SPDX-License-Identifier: CC0-1.0

//! WHAT `DescribeSecret` and `ListSecrets` report as the next rotation of a secret, over the
//! wire, for every form a rotation schedule is stated in.
//!
//! THE RULE: a secret's next-rotation instant is the first instant its STORED schedule names
//! after the event the schedule is measured from. A `cron()` expression names those instants
//! on the UTC calendar — "Secrets Manager rotation schedules use UTC time zone", and the
//! rotation window "starts according to the ScheduleExpression"
//! (`RotationRulesType$ScheduleExpression` / `$Duration`, this service's Smithy model) — so
//! the reported instant is the occurrence that expression names, which is where the window
//! `Duration` measures opens. An interval schedule names one span past the event instead, and
//! "this value is calculated from the rotation schedule after every successful rotation"
//! (`RotationRulesType$AutomaticallyAfterDays`), where a manual write of the secret's value
//! counts as a rotation of it.
//!
//! Every test is a TABLE over the class the rule generalises: which calendar field a `cron()`
//! expression restricts (hours, minutes, day of month, day of week, month, year), whether it
//! is written in the wildcard dialect the service's own schedule-expression reference adds to
//! the ordinary cron fields, whether a rotation window `Duration` travels with it, and how a
//! schedule that names an interval is written. The expressions are the six-field form the
//! service documents (minutes, hours, day-of-month, month, day-of-week, year); the expected
//! calendar of each row is asserted from the reported instant's own UTC fields rather than
//! from a value one run produced.

use std::time::SystemTime;

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The harness this suite drives the emulator with is the rotation suite's own, beside it in
// the same test target: one emulator-per-test router, one `awsJson1_1` call, the rotation
// function's ARN, the clock slack a reported instant is compared within, and the create and
// describe every row starts from.
use crate::secret_rotation::{
    call, create_secret, describe, emulator, epoch_seconds, instant, ok, CLOCK_SLACK_SECONDS,
    DAY, HOUR, LAMBDA_ARN,
};

/// A reported timestamp member as a UTC calendar instant, which is the calendar a rotation
/// schedule is stated on.
fn utc(parsed: &Value, member: &str, case: &str) -> DateTime<Utc> {
    let seconds = instant(parsed, member, case);
    #[allow(clippy::cast_possible_truncation)]
    DateTime::from_timestamp(seconds as i64, 0)
        .unwrap_or_else(|| panic!("case {case:?}: {member} is not an instant: {seconds}"))
}

/// Configure rotation for one secret with `rules`, without starting a rotation.
async fn schedule(router: &MultiServiceRouter, name: &str, rules: &Value, case: &str) {
    ok(
        router,
        "RotateSecret",
        json!({
            "SecretId": name,
            "RotationLambdaARN": LAMBDA_ARN,
            "RotationRules": rules,
            // False so each row asserts the SCHEDULE alone: nothing is staged and no
            // rotation function runs.
            "RotateImmediately": false,
        }),
        case,
    )
    .await;
}

/// Which calendar fields one `cron()` row's occurrences fall on, as the test reads them back
/// off the reported instant.
struct Occurrence {
    /// The minute of the hour every occurrence falls on.
    minute: u32,
    /// The hours of the day they may fall on — a row restricting none lists them all.
    hours: &'static [u32],
    /// The days of the month they may fall on, or `None` where the expression restricts none.
    days_of_month: Option<&'static [u32]>,
    /// The day of the week they fall on, or `None` where the expression restricts none.
    weekday: Option<Weekday>,
    /// The month they fall in, or `None` where the expression restricts none.
    month: Option<u32>,
    /// The longest span this calendar can leave between one occurrence and the next, in
    /// seconds: what makes the reported instant the FIRST occurrence past the call rather
    /// than some later one.
    longest_gap: f64,
}

/// `DescribeSecret` reports the occurrence a `cron()` schedule next names, on the UTC
/// calendar, and `ListSecrets` reports the same instant.
///
/// The rows differ along which field the expression restricts: the hour, the minute, a day of
/// the month, a day of the week (with a rotation window `Duration` on it), a named month, a
/// list of hours, and a month and a weekday whose names carry one of the letters the service's
/// own wildcard dialect uses.
#[tokio::test]
async fn a_cron_schedule_reports_the_next_utc_occurrence_as_the_next_rotation() {
    // (case, the rules the call carries, the calendar its occurrences fall on)
    let rows: &[(&str, Value, Occurrence)] = &[
        (
            "every day at 08:00",
            json!({"ScheduleExpression": "cron(0 8 * * ? *)"}),
            Occurrence {
                minute: 0,
                hours: &[8],
                days_of_month: None,
                weekday: None,
                month: None,
                longest_gap: DAY,
            },
        ),
        (
            "every day at 22:30",
            json!({"ScheduleExpression": "cron(30 22 * * ? *)"}),
            Occurrence {
                minute: 30,
                hours: &[22],
                days_of_month: None,
                weekday: None,
                month: None,
                longest_gap: DAY,
            },
        ),
        (
            "twice a day, at a list of hours",
            json!({"ScheduleExpression": "cron(15 4,16 * * ? *)"}),
            Occurrence {
                minute: 15,
                hours: &[4, 16],
                days_of_month: None,
                weekday: None,
                month: None,
                longest_gap: DAY,
            },
        ),
        (
            "the first of every month at midnight",
            json!({"ScheduleExpression": "cron(0 0 1 * ? *)"}),
            Occurrence {
                minute: 0,
                hours: &[0],
                days_of_month: Some(&[1]),
                weekday: None,
                month: None,
                // The longest month, so the reported occurrence is the next one.
                longest_gap: 31.0 * DAY,
            },
        ),
        (
            "the fifteenth of every month at 06:45",
            json!({"ScheduleExpression": "cron(45 6 15 * ? *)"}),
            Occurrence {
                minute: 45,
                hours: &[6],
                days_of_month: Some(&[15]),
                weekday: None,
                month: None,
                longest_gap: 31.0 * DAY,
            },
        ),
        (
            "a three-hour rotation window every Saturday at 16:00",
            json!({"ScheduleExpression": "cron(0 16 ? * SAT *)", "Duration": "3h"}),
            Occurrence {
                minute: 0,
                hours: &[16],
                days_of_month: None,
                weekday: Some(Weekday::Sat),
                month: None,
                longest_gap: 7.0 * DAY,
            },
        ),
        (
            "once a year, in a month named in words",
            json!({"ScheduleExpression": "cron(0 9 1 JAN ? *)"}),
            Occurrence {
                minute: 0,
                hours: &[9],
                days_of_month: Some(&[1]),
                weekday: None,
                month: Some(1),
                longest_gap: 366.0 * DAY,
            },
        ),
        (
            // `JUL` carries an `L` and `WED` a `W`, the letters the service's own
            // wildcard dialect uses; they are ordinary names all the same.
            "once a year, in a month whose name carries a dialect letter",
            json!({"ScheduleExpression": "cron(0 7 1 JUL ? *)"}),
            Occurrence {
                minute: 0,
                hours: &[7],
                days_of_month: Some(&[1]),
                weekday: None,
                month: Some(7),
                longest_gap: 366.0 * DAY,
            },
        ),
        (
            "every week, on a weekday whose name carries a dialect letter",
            json!({"ScheduleExpression": "cron(30 5 ? * WED *)"}),
            Occurrence {
                minute: 30,
                hours: &[5],
                days_of_month: None,
                weekday: Some(Weekday::Wed),
                month: None,
                longest_gap: 7.0 * DAY,
            },
        ),
    ];

    let router = emulator();
    for (index, (case, rules, occurrence)) in rows.iter().enumerate() {
        let name = format!("cron-scheduled-{index}");
        create_secret(&router, &name, "the-current-value", case).await;

        let before = epoch_seconds(SystemTime::now());
        schedule(&router, &name, rules, case).await;
        let described = describe(&router, &name, case).await;

        assert_eq!(
            described.get("RotationRules"),
            Some(rules),
            "case {case:?}: expected the rules to round-trip, got {described}"
        );
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: expected rotation to be enabled, got {described}"
        );
        // Nothing rotated: no rotation function ran, so there is no completed rotation.
        assert_eq!(
            described.get("LastRotatedDate"),
            None,
            "case {case:?}: expected no LastRotatedDate before a rotation completes, got \
             {described}"
        );

        let next = instant(&described, "NextRotationDate", case);
        assert!(
            next > before,
            "case {case:?}: expected the next rotation ahead of the call at {before}, got {next}"
        );
        assert!(
            next - before <= occurrence.longest_gap + CLOCK_SLACK_SECONDS,
            "case {case:?}: expected the FIRST occurrence past the call, no more than \
             {} seconds out, got {}",
            occurrence.longest_gap,
            next - before
        );

        let at = utc(&described, "NextRotationDate", case);
        assert_eq!(
            at.minute(),
            occurrence.minute,
            "case {case:?}: expected an occurrence at minute {}, got {at}",
            occurrence.minute
        );
        assert!(
            occurrence.hours.contains(&at.hour()),
            "case {case:?}: expected an occurrence in hours {:?} UTC, got {at}",
            occurrence.hours
        );
        assert_eq!(
            at.second(),
            0,
            "case {case:?}: expected an occurrence on the minute, got {at}"
        );
        if let Some(days) = occurrence.days_of_month {
            assert!(
                days.contains(&at.day()),
                "case {case:?}: expected an occurrence on days {days:?} of the month, got {at}"
            );
        }
        if let Some(weekday) = occurrence.weekday {
            assert_eq!(
                at.weekday(),
                weekday,
                "case {case:?}: expected an occurrence on a {weekday:?}, got {at}"
            );
        }
        if let Some(month) = occurrence.month {
            assert_eq!(
                at.month(),
                month,
                "case {case:?}: expected an occurrence in month {month}, got {at}"
            );
        }

        // The same instant through ListSecrets, which projects the same secret.
        let listed = ok(
            &router,
            "ListSecrets",
            json!({"Filters": [{"Key": "name", "Values": [&name]}]}),
            case,
        )
        .await;
        let entry = listed
            .get("SecretList")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .unwrap_or_else(|| panic!("case {case:?}: no SecretList entry in {listed}"));
        assert_eq!(
            instant(entry, "NextRotationDate", case),
            next,
            "case {case:?}: expected ListSecrets to report the same next rotation, got {entry}"
        );
    }
}

/// A `cron()` schedule this service computes no occurrence from is still stored, reported back
/// and rotation-enabled — it just makes no rotation due — and a schedule whose expression no
/// grammar reads is refused before anything is stored.
///
/// The rows differ along why no occurrence is computed: a year already behind the clock, and
/// each wildcard of the dialect the service's own schedule-expression reference documents on
/// top of the ordinary cron fields (`L` last, `W` nearest weekday, `#` nth weekday of the
/// month) — which AWS accepts, so this service accepts them too. The refusing rows are fields
/// outside the range their place allows, which no cron dialect reads.
#[tokio::test]
async fn a_cron_expression_naming_no_computed_occurrence_schedules_nothing() {
    let router = emulator();

    // (case, the expression this service stores and computes no occurrence from)
    let no_occurrence: &[(&str, &str)] = &[
        // A year in the past names occurrences that have all happened.
        ("a year already behind the clock", "cron(0 10 1 1 ? 2000)"),
        ("the last day of the month", "cron(0 16 L * ? *)"),
        (
            "the nearest weekday to a day of the month",
            "cron(0 8 15W * ? *)",
        ),
        ("the last weekday of the month", "cron(0 8 LW * ? *)"),
        ("the third Friday of the month", "cron(0 10 ? * 6#3 *)"),
        ("the last Friday of the month", "cron(0 10 ? * 6L *)"),
    ];
    for (index, (case, expression)) in no_occurrence.iter().enumerate() {
        let name = format!("cron-without-occurrence-{index}");
        create_secret(&router, &name, "the-current-value", case).await;
        let rules = json!({"ScheduleExpression": expression});
        schedule(&router, &name, &rules, case).await;
        let described = describe(&router, &name, case).await;
        // "If the secret isn't configured for rotation or rotation has been disabled, Secrets
        // Manager returns null" is the only other way this member is absent, so rotation is
        // still reported as ON and the rules still round-trip.
        assert_eq!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: expected rotation to be enabled, got {described}"
        );
        assert_eq!(
            described.get("RotationRules"),
            Some(&rules),
            "case {case:?}: expected the rules to round-trip, got {described}"
        );
        assert_eq!(
            described.get("NextRotationDate"),
            None,
            "case {case:?}: expected no rotation due for a schedule this service computes no \
             occurrence from, got {described}"
        );
    }

    // (case, an expression stating no calendar this service can read)
    let unreadable: &[(&str, &str)] = &[
        ("an hour past the end of the day", "cron(0 24 * * ? *)"),
        ("a minute past the end of the hour", "cron(60 8 * * ? *)"),
        ("a day of the month no month has", "cron(0 8 32 * ? *)"),
        ("a month that is not one of the twelve", "cron(0 8 1 13 ? *)"),
        ("a day of the week that is not one of the seven", "cron(0 8 ? * FUNDAY *)"),
        (
            "an hour past the end of the day beside one of the service's own wildcards",
            "cron(0 24 L * ? *)",
        ),
        ("fields that are not a calendar at all", "cron(every day at 8 please now)"),
    ];
    for (index, (case, expression)) in unreadable.iter().enumerate() {
        let name = format!("unreadable-cron-{index}");
        create_secret(&router, &name, "the-current-value", case).await;
        let (status, parsed) = call(
            &router,
            "RotateSecret",
            &json!({
                "SecretId": &name,
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": {"ScheduleExpression": expression},
                "RotateImmediately": false,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {expression} to be refused, got {status} with {parsed}"
        );
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no __type in {parsed}"));
        assert!(
            code.ends_with("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException for {expression}, got {code}"
        );
        // Nothing was configured, so the refused call left the secret unrotated.
        let described = describe(&router, &name, case).await;
        assert_ne!(
            described.get("RotationEnabled"),
            Some(&json!(true)),
            "case {case:?}: a refused schedule enabled rotation, got {described}"
        );
    }
}

/// A write that becomes the secret's CURRENT value re-anchors the schedule to itself:
/// "Manually updating the secret value by calling PutSecretValue or UpdateSecret is considered
/// a valid rotation" (`RotationRulesType$AutomaticallyAfterDays`, this service's Smithy
/// model), so the next rotation is measured from that write.
///
/// The rows differ along how the schedule is stated — an interval in days, a `rate()`
/// interval, and a `cron()` calendar — and along which operation writes the value.
#[tokio::test]
async fn a_manual_write_of_the_value_re_anchors_the_schedule() {
    // (case, the rules, the span past the write the next rotation falls at, or None where the
    // schedule names a calendar rather than a span)
    let rows: &[(&str, Value, Option<f64>)] = &[
        (
            "an interval in days, rewritten by PutSecretValue",
            json!({"AutomaticallyAfterDays": 30}),
            Some(30.0 * DAY),
        ),
        (
            "a rate in hours, rewritten by PutSecretValue",
            json!({"ScheduleExpression": "rate(12 hours)"}),
            Some(12.0 * HOUR),
        ),
        (
            "a cron window, rewritten by PutSecretValue",
            json!({"ScheduleExpression": "cron(0 16 ? * SAT *)", "Duration": "3h"}),
            None,
        ),
    ];

    let router = emulator();
    for (index, (case, rules, span)) in rows.iter().enumerate() {
        let name = format!("re-anchored-{index}");
        create_secret(&router, &name, "the-first-value", case).await;
        schedule(&router, &name, rules, case).await;
        let scheduled = describe(&router, &name, case).await;
        let before_configure = instant(&scheduled, "NextRotationDate", case);

        // The write that replaces the current value.
        let written = epoch_seconds(SystemTime::now());
        ok(
            &router,
            "PutSecretValue",
            json!({"SecretId": &name, "SecretString": "the-rewritten-value"}),
            case,
        )
        .await;
        let described = describe(&router, &name, case).await;
        let next = instant(&described, "NextRotationDate", case);
        assert!(
            next >= before_configure,
            "case {case:?}: expected the write to leave the schedule no earlier than the \
             configured {before_configure}, got {next}"
        );
        match span {
            // An interval schedule measures the next rotation from the write.
            Some(span) => assert!(
                (next - (written + span)).abs() <= CLOCK_SLACK_SECONDS,
                "case {case:?}: expected the next rotation {span} seconds past the write at \
                 {written}, got {next}"
            ),
            // A calendar schedule names the occurrence itself, so the write leaves the
            // reported instant on that calendar.
            None => {
                let at = utc(&described, "NextRotationDate", case);
                assert!(
                    at.weekday() == Weekday::Sat && at.hour() == 16 && at.minute() == 0,
                    "case {case:?}: expected the occurrence the expression names, got {at}"
                );
                assert!(
                    next > written,
                    "case {case:?}: expected the occurrence ahead of the write at {written}, \
                     got {next}"
                );
            }
        }
        // The write is not a rotation this service performed, so nothing claims it rotated
        // the secret: "the last date and time that Secrets Manager rotated the secret"
        // (`DescribeSecretResponse$LastRotatedDate`, this service's Smithy model).
        assert_eq!(
            described.get("LastRotatedDate"),
            None,
            "case {case:?}: expected no LastRotatedDate from a manual write, got {described}"
        );

        // UpdateSecret writes the value the same way, so the schedule follows it too.
        let updated_at = epoch_seconds(SystemTime::now());
        ok(
            &router,
            "UpdateSecret",
            json!({"SecretId": &name, "SecretString": "the-updated-value"}),
            case,
        )
        .await;
        let described = describe(&router, &name, case).await;
        let next = instant(&described, "NextRotationDate", case);
        if let Some(span) = span {
            assert!(
                (next - (updated_at + span)).abs() <= CLOCK_SLACK_SECONDS,
                "case {case:?}: expected the next rotation {span} seconds past the update at \
                 {updated_at}, got {next}"
            );
        } else {
            let at = utc(&described, "NextRotationDate", case);
            assert!(
                at.weekday() == Weekday::Sat && at.hour() == 16 && at.minute() == 0,
                "case {case:?}: expected the occurrence the expression names, got {at}"
            );
        }
    }
}
