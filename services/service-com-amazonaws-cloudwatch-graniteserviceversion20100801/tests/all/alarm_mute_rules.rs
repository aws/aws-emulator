// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CloudWatch alarm mute rule loop: `PutAlarmMuteRule` creating and
//! updating a rule, and `GetAlarmMuteRule` reading back what it persisted — including the
//! `Status` the clock makes of the rule's own schedule and the not-found a name no rule carries
//! is refused with.
//!
//! Every table row is a distinct input along what the rule under test generalises over (which
//! expression form the schedule states its instants in, how long its window stays open, which
//! time zone it is read in, which optional members the configuration carries, how many alarms it
//! targets, which Region it was written in, and how a requested name misses), so the recorded
//! call is one row among many rather than the specification.

use crate::harness::{build_router, call, call_in_region, error_code, error_message, ok};
use ::chrono::{DateTime, TimeDelta, Utc};
use ::chrono_tz::Tz;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the harness signs its requests for, and so the one a rule it writes lives in.
const REGION: &str = "us-east-1";

/// Another Region, for the rows that assert a mute rule belongs to the Region it was written in.
const OTHER_REGION: &str = "us-west-2";

/// A `PutAlarmMuteRule` request with only the members the model makes required.
fn put_request(name: &str, expression: &str, duration: &str) -> Value {
    json!({
        "Name": name,
        "Rule": {"Schedule": {"Expression": expression, "Duration": duration}},
    })
}

/// The same request with `extra` merged into it, so a row states only what it varies.
fn with_members(mut request: Value, extra: Value) -> Value {
    let (Some(target), Some(extra)) = (request.as_object_mut(), extra.as_object()) else {
        return request;
    };
    for (member, value) in extra {
        target.insert(member.clone(), value.clone());
    }
    request
}

/// The schedule of a request, for a row that varies one of its members.
fn schedule(expression: &str, duration: &str, timezone: Option<&str>) -> Value {
    match timezone {
        None => json!({"Expression": expression, "Duration": duration}),
        Some(timezone) => {
            json!({"Expression": expression, "Duration": duration, "Timezone": timezone})
        }
    }
}

/// An `at` expression naming the wall-clock instant `at` falls on in `timezone`: "At expressions
/// … Format: `at(yyyy-MM-ddThh:mm)`" (`Schedule$Expression`), a LOCAL instant carrying no zone of
/// its own.
fn at_expression(at: DateTime<Utc>, timezone: Tz) -> String {
    format!("at({})", at.with_timezone(&timezone).format("%Y-%m-%dT%H:%M"))
}

/// A string member of a body.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// A timestamp member, which the awsJson protocols carry as epoch seconds.
fn stamp(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// The rule `name` names in `REGION`, read back through the implemented reader.
async fn stored_rule(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetAlarmMuteRule", &json!({"AlarmMuteRuleName": name})).await
}

#[tokio::test]
async fn every_mute_rule_configuration_round_trips_through_get_alarm_mute_rule() {
    let router = build_router();
    let now = Utc::now();

    // Rows differ along the expression form (and so the `MuteType` derived from it), the length
    // of the window, whether a time zone is named, which optional members the configuration
    // carries, how many alarms the rule targets, and whether the rule already existed.
    //
    // The expressions and durations are the published examples of `Schedule$Expression` and
    // `Schedule$Duration`, so the catalog they come from is the model's own.
    let cases: Vec<(&str, Value)> = vec![
        (
            "required members only, recurring daily",
            put_request("daily-maintenance", "cron(0 2 * * *)", "PT4H"),
        ),
        (
            "a weekly window with a description and one targeted alarm",
            with_members(
                put_request("weekly-maintenance", "cron(0 2 * * SUN)", "PT6H"),
                json!({
                    "Description": "Sunday system maintenance",
                    "MuteTargets": {"AlarmNames": ["WebServerCPUAlarm"]},
                }),
            ),
        ),
        (
            "a monthly window in a named time zone targeting three alarms",
            with_members(
                put_request("monthly-database", "cron(0 1 1 * *)", "P2DT12H"),
                json!({
                    "Rule": {"Schedule": schedule(
                        "cron(0 1 1 * *)",
                        "P2DT12H",
                        Some("America/New_York"),
                    )},
                    "MuteTargets": {"AlarmNames": [
                        "WebServerCPUAlarm",
                        "DatabaseConnectionAlarm",
                        "QueueDepthAlarm",
                    ]},
                }),
            ),
        ),
        (
            "a window bounded by a start and an expire date, with tags",
            with_members(
                put_request("bounded-window", "cron(0 18 * * FRI)", "P2DT12H"),
                json!({
                    "StartDate": (now + TimeDelta::hours(1)).timestamp(),
                    "ExpireDate": (now + TimeDelta::days(30)).timestamp(),
                    "Tags": [{"Key": "Team", "Value": "TeamA"}],
                }),
            ),
        ),
        (
            "a one-time window, the shortest a schedule may state",
            put_request("incident-response", "at(2024-05-10T14:00)", "PT1M"),
        ),
        (
            "a one-time window, the longest a schedule may state, in a named zone",
            with_members(
                put_request("annual-shutdown", "at(2024-12-23T00:00)", "P15D"),
                json!({
                    "Rule": {"Schedule": schedule(
                        "at(2024-12-23T00:00)",
                        "P15D",
                        Some("Asia/Tokyo"),
                    )},
                    "Description": "",
                    "MuteTargets": {"AlarmNames": []},
                }),
            ),
        ),
        (
            "a rule that already exists, written again with a new configuration",
            with_members(
                put_request("daily-maintenance", "cron(0 23 * * *)", "PT2H"),
                json!({"Description": "nightly backup operations"}),
            ),
        ),
    ];

    for (case, request) in cases {
        let answered = ok(&router, "PutAlarmMuteRule", &request).await;
        assert_eq!(
            answered,
            json!({}),
            "case {case:?}: PutAlarmMuteRule answers an empty body, got {answered}"
        );

        let name = text(&request, "Name").unwrap_or_default();
        let read = stored_rule(&router, &name).await;

        // Every member the request carried reads back as it was written.
        assert_eq!(
            text(&read, "Name"),
            Some(name.clone()),
            "case {case:?}: expected the rule's own name, got {read}"
        );
        assert_eq!(
            read.get("Rule"),
            request.get("Rule"),
            "case {case:?}: expected the stored schedule {:?}, got {read}",
            request.get("Rule")
        );
        assert_eq!(
            read.get("Description"),
            request.get("Description"),
            "case {case:?}: expected the stored description {:?}, got {read}",
            request.get("Description")
        );
        assert_eq!(
            read.get("MuteTargets"),
            request.get("MuteTargets"),
            "case {case:?}: expected the stored targets {:?}, got {read}",
            request.get("MuteTargets")
        );
        for member in ["StartDate", "ExpireDate"] {
            assert_eq!(
                stamp(&read, member),
                request.get(member).and_then(Value::as_f64),
                "case {case:?}: expected the stored {member} {:?}, got {read}",
                request.get(member)
            );
        }
        // The members the SERVICE fills in: the ARN of the rule in the Region it was written in,
        // the instant of the write, the kind of window its expression opens, and the status the
        // clock makes of that window.
        assert_eq!(
            text(&read, "AlarmMuteRuleArn"),
            Some(format!(
                "arn:aws:cloudwatch:{REGION}:{}:alarm-mute-rule:{name}",
                ::engine::EMULATOR_ACCOUNT_ID
            )),
            "case {case:?}: expected the rule's own ARN, got {read}"
        );
        let last_updated = stamp(&read, "LastUpdatedTimestamp");
        assert!(
            last_updated.is_some_and(|at| at >= now.timestamp() as f64),
            "case {case:?}: expected the instant of the write, got {last_updated:?} in {read}"
        );
        let expression = request["Rule"]["Schedule"]["Expression"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_mute_type = if expression.starts_with("at(") {
            "ONE_TIME"
        } else {
            "RECURRING"
        };
        assert_eq!(
            text(&read, "MuteType").as_deref(),
            Some(expected_mute_type),
            "case {case:?}: expected the mute type of {expression:?}, got {read}"
        );
        let status = text(&read, "Status").unwrap_or_default();
        assert!(
            ["SCHEDULED", "ACTIVE", "EXPIRED"].contains(&status.as_str()),
            "case {case:?}: expected one of the modelled statuses, got {status:?} in {read}"
        );

        // A read changes nothing a later caller sees: the second read answers the first's body.
        let read_again = stored_rule(&router, &name).await;
        assert_eq!(
            read_again, read,
            "case {case:?}: a read must not change the rule, got {read_again} after {read}"
        );
    }

    // The update row replaced the configuration of the rule it named rather than adding a second
    // rule under that name.
    let updated = stored_rule(&router, "daily-maintenance").await;
    assert_eq!(
        (
            updated["Rule"]["Schedule"]["Expression"].as_str(),
            updated["Rule"]["Schedule"]["Duration"].as_str(),
            text(&updated, "Description").as_deref(),
        ),
        (Some("cron(0 23 * * *)"), Some("PT2H"), Some("nightly backup operations")),
        "case \"after the update\": expected the rule the second write configured, got {updated}"
    );
}

#[tokio::test]
async fn a_mute_rules_status_is_what_the_clock_makes_of_its_window() {
    let router = build_router();
    let now = Utc::now();

    // Rows differ along where the clock stands relative to the windows the schedule opens: inside
    // one, before the first, after the last, before the rule takes effect, and past the date it is
    // no longer evaluated. The two zoned rows carry the SAME local instant as their UTC
    // counterparts, so what differs between them is only the zone the expression is read in.
    //
    // Each row is stated relative to the clock the suite runs on rather than to a recorded
    // instant: the windows are hours wide, so no row depends on when in the minute it runs.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a one-time window that opened this minute",
            json!({"Rule": {"Schedule": schedule(&at_expression(now, Tz::UTC), "PT1H", None)}}),
            "ACTIVE",
        ),
        (
            "a one-time window still to open",
            json!({"Rule": {"Schedule": schedule(
                &at_expression(now + TimeDelta::hours(2), Tz::UTC),
                "PT1H",
                None,
            )}}),
            "SCHEDULED",
        ),
        (
            "a one-time window that has closed",
            json!({"Rule": {"Schedule": schedule(
                &at_expression(now - TimeDelta::hours(3), Tz::UTC),
                "PT1H",
                None,
            )}}),
            "EXPIRED",
        ),
        (
            "a recurring window that opens every minute",
            json!({"Rule": {"Schedule": schedule("cron(* * * * *)", "PT1H", None)}}),
            "ACTIVE",
        ),
        (
            "a recurring schedule before the rule takes effect",
            json!({
                "Rule": {"Schedule": schedule("cron(* * * * *)", "PT1H", None)},
                "StartDate": (now + TimeDelta::hours(2)).timestamp(),
            }),
            "SCHEDULED",
        ),
        (
            "a recurring schedule past the date it is no longer evaluated",
            json!({
                "Rule": {"Schedule": schedule("cron(* * * * *)", "PT1H", None)},
                "ExpireDate": (now - TimeDelta::minutes(1)).timestamp(),
            }),
            "EXPIRED",
        ),
        (
            "a one-time window open now, read in a zone ahead of UTC",
            json!({"Rule": {"Schedule": schedule(
                &at_expression(now, Tz::Asia__Tokyo),
                "PT1H",
                Some("Asia/Tokyo"),
            )}}),
            "ACTIVE",
        ),
        (
            "the same local instant read in UTC, which is hours ahead of it",
            json!({"Rule": {"Schedule": schedule(
                &at_expression(now, Tz::Asia__Tokyo),
                "PT1H",
                None,
            )}}),
            "SCHEDULED",
        ),
        (
            "a one-time window open now, read in a zone behind UTC",
            json!({"Rule": {"Schedule": schedule(
                &at_expression(now, Tz::America__Los_Angeles),
                "PT1H",
                Some("America/Los_Angeles"),
            )}}),
            "ACTIVE",
        ),
        (
            "the same local instant read in UTC, which is hours behind it",
            json!({"Rule": {"Schedule": schedule(
                &at_expression(now, Tz::America__Los_Angeles),
                "PT1H",
                None,
            )}}),
            "EXPIRED",
        ),
    ];

    for (index, (case, configuration, expected)) in cases.into_iter().enumerate() {
        let name = format!("status-row-{index}");
        let request = with_members(
            with_members(put_request(&name, "cron(* * * * *)", "PT1H"), configuration),
            json!({"Name": name}),
        );
        ok(&router, "PutAlarmMuteRule", &request).await;
        let read = stored_rule(&router, &name).await;
        assert_eq!(
            text(&read, "Status").as_deref(),
            Some(expected),
            "case {case:?}: expected status {expected:?}, got {read}"
        );
    }
}

#[tokio::test]
async fn a_schedule_naming_no_mute_window_is_refused_and_stores_nothing() {
    let router = build_router();

    // (case, the schedule, the member path the refusal names, the value it names).
    //
    // Rows differ along WHICH of the three unmodelled rules the schedule breaks and how: an
    // expression in neither documented form (prose, the wrong number of cron fields, a cron field
    // the grammar does not read, an instant that is no instant, another API's expression form), a
    // duration outside the published range or naming no fixed span, and a time zone no IANA zone
    // answers to.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "an expression that is prose",
            schedule("every minute", "PT1H", None),
            "Rule.Schedule.Expression",
            "every minute",
        ),
        (
            "a cron expression one field short",
            schedule("cron(0 2 * *)", "PT1H", None),
            "Rule.Schedule.Expression",
            "cron(0 2 * *)",
        ),
        (
            "a cron expression one field long",
            schedule("cron(0 2 * * * *)", "PT1H", None),
            "Rule.Schedule.Expression",
            "cron(0 2 * * * *)",
        ),
        (
            "a cron field the grammar does not read",
            schedule("cron(0 nope * * *)", "PT1H", None),
            "Rule.Schedule.Expression",
            "cron(0 nope * * *)",
        ),
        (
            "an at expression naming no instant",
            schedule("at(2024-13-45T99:99)", "PT1H", None),
            "Rule.Schedule.Expression",
            "at(2024-13-45T99:99)",
        ),
        (
            "an at expression without the date-time separator",
            schedule("at(2024-05-10 14:00)", "PT1H", None),
            "Rule.Schedule.Expression",
            "at(2024-05-10 14:00)",
        ),
        (
            "the rate expression another schedule member takes",
            schedule("rate(5 minutes)", "PT1H", None),
            "Rule.Schedule.Expression",
            "rate(5 minutes)",
        ),
        (
            "a window one second shorter than the shortest published",
            schedule("cron(0 2 * * *)", "PT59S", None),
            "Rule.Schedule.Duration",
            "PT59S",
        ),
        (
            "a window one second longer than the longest published",
            schedule("cron(0 2 * * *)", "P15DT1S", None),
            "Rule.Schedule.Duration",
            "P15DT1S",
        ),
        (
            "a window a day longer than the longest published",
            schedule("cron(0 2 * * *)", "P16D", None),
            "Rule.Schedule.Duration",
            "P16D",
        ),
        (
            "a duration of no length at all",
            schedule("cron(0 2 * * *)", "PT0S", None),
            "Rule.Schedule.Duration",
            "PT0S",
        ),
        (
            "a duration whose length depends on the calendar",
            schedule("cron(0 2 * * *)", "P1Y", None),
            "Rule.Schedule.Duration",
            "P1Y",
        ),
        (
            "a duration written as prose",
            schedule("cron(0 2 * * *)", "4 hours", None),
            "Rule.Schedule.Duration",
            "4 hours",
        ),
        (
            "a time zone no zone answers to",
            schedule("cron(0 2 * * *)", "PT1H", Some("Mars/Phobos")),
            "Rule.Schedule.Timezone",
            "Mars/Phobos",
        ),
        (
            "a time zone misspelled",
            schedule("cron(0 2 * * *)", "PT1H", Some("Europe/Londonn")),
            "Rule.Schedule.Timezone",
            "Europe/Londonn",
        ),
    ];

    for (index, (case, broken, parameter, value)) in cases.into_iter().enumerate() {
        let name = format!("refused-row-{index}");
        let request = json!({"Name": name, "Rule": {"Schedule": broken}});
        let (status, body) = call(&router, "PutAlarmMuteRule", &request).await;
        // CloudWatch answers a precondition its model cannot express with the code its
        // query-compatible error table maps this refusal to
        // (`crate::service::query_compatible_errors`), which the awsJson protocols carry as the
        // local name of `com.amazon.coral.validate#ValidationException`.
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        let expected = format!("The value {value} for parameter {parameter} is invalid.");
        let got = error_message(&body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?} in {body}"
        );
        // A refused write stored nothing: the name it carried holds no rule.
        let (status, body) = call(
            &router,
            "GetAlarmMuteRule",
            &json!({"AlarmMuteRuleName": name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused write must store nothing, got {status} with {body}"
        );
    }

    // The published examples of both members ARE accepted, so the rows above refuse the values
    // that break a rule rather than the forms the model documents
    // (`smithy_query(query="doc",shape="Schedule",member="Expression")` and its `Duration`).
    let accepted: Vec<(&str, &str, Option<&str>)> = vec![
        ("cron(0 2 * * *)", "PT4H", None),
        ("cron(0 2 * * SUN)", "PT6H", Some("America/New_York")),
        ("cron(0 1 1 * *)", "P2DT12H", Some("America/Los_Angeles")),
        ("cron(0 18 * * FRI)", "PT2H", Some("Europe/London")),
        ("cron(0 23 * * *)", "P7D", Some("Asia/Tokyo")),
        ("cron(0,30 8-17 * JAN-DEC MON-FRI)", "PT1M", Some("UTC")),
        ("at(2024-05-10T14:00)", "P15D", None),
        ("at(2024-12-23T00:00)", "PT1H", Some("America/New_York")),
    ];
    for (index, (expression, duration, timezone)) in accepted.into_iter().enumerate() {
        let name = format!("accepted-row-{index}");
        let request = json!({
            "Name": name,
            "Rule": {"Schedule": schedule(expression, duration, timezone)},
        });
        let case = format!("{expression} for {duration} in {timezone:?}");
        let (status, body) = call(&router, "PutAlarmMuteRule", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let read = stored_rule(&router, &name).await;
        assert_eq!(
            read["Rule"]["Schedule"]["Expression"].as_str(),
            Some(expression),
            "case {case:?}: expected the stored expression, got {read}"
        );
    }
}

#[tokio::test]
async fn a_name_no_mute_rule_carries_is_refused_by_its_code_alone() {
    let router = build_router();
    ok(
        &router,
        "PutAlarmMuteRule",
        &put_request("present-rule", "cron(0 2 * * *)", "PT4H"),
    )
    .await;

    // Rows differ along HOW the requested name misses the one rule the store holds.
    for (case, name) in [
        ("a name nothing was created under", "absent-rule"),
        ("a stored name in another case", "PRESENT-RULE"),
        ("a stored name with a suffix", "present-rule-2"),
        ("a stored name with a prefix", "my-present-rule"),
    ] {
        let (status, body) = call(
            &router,
            "GetAlarmMuteRule",
            &json!({"AlarmMuteRuleName": name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} with {body}"
        );
        // The class is answered by its code alone: the resource is named by the two members that
        // carry it, and no message member is sent at all.
        assert_eq!(
            (
                body.get("ResourceType").and_then(Value::as_str),
                body.get("ResourceId").and_then(Value::as_str),
                body.get("Message").and_then(Value::as_str),
            ),
            (Some("AlarmMuteRule"), Some(name), None),
            "case {case:?}: expected the resource named and no message, got {body}"
        );
    }

    // The rule the store does hold is untouched by the misses.
    let read = stored_rule(&router, "present-rule").await;
    assert_eq!(
        text(&read, "Name").as_deref(),
        Some("present-rule"),
        "case \"after the misses\": the stored rule must still read back, got {read}"
    );
}

#[tokio::test]
async fn a_mute_rule_belongs_to_the_region_it_was_written_in() {
    let router = build_router();

    // One name written in each of two Regions, with a configuration of its own in each.
    for (region, duration) in [(REGION, "PT4H"), (OTHER_REGION, "PT6H")] {
        let (status, body) = call_in_region(
            &router,
            region,
            "PutAlarmMuteRule",
            &put_request("regional-rule", "cron(0 2 * * *)", duration),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {body}"
        );
    }

    // Each Region reads its own rule, with its own configuration and its own ARN.
    for (region, duration) in [(REGION, "PT4H"), (OTHER_REGION, "PT6H")] {
        let read = call_in_region(
            &router,
            region,
            "GetAlarmMuteRule",
            &json!({"AlarmMuteRuleName": "regional-rule"}),
        )
        .await
        .1;
        assert_eq!(
            (
                read["Rule"]["Schedule"]["Duration"].as_str(),
                text(&read, "AlarmMuteRuleArn").as_deref(),
            ),
            (
                Some(duration),
                Some(
                    format!(
                        "arn:aws:cloudwatch:{region}:{}:alarm-mute-rule:regional-rule",
                        ::engine::EMULATOR_ACCOUNT_ID
                    )
                    .as_str()
                ),
            ),
            "case {region:?}: expected the rule of that Region, got {read}"
        );
    }

    // A rule written in one Region is not a rule of another.
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetAlarmMuteRule",
        &json!({"AlarmMuteRuleName": "only-in-us-east-1"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a name held in no Region\": expected HTTP 404, got {status} with {body}"
    );
    ok(
        &router,
        "PutAlarmMuteRule",
        &put_request("only-in-us-east-1", "cron(0 2 * * *)", "PT4H"),
    )
    .await;
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetAlarmMuteRule",
        &json!({"AlarmMuteRuleName": "only-in-us-east-1"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a rule of another Region\": expected HTTP 404, got {status} with {body}"
    );

    // A rule written in one Region is not in another Region's listing either.
    let listed = call_in_region(&router, OTHER_REGION, "ListAlarmMuteRules", &json!({}))
        .await
        .1;
    assert_eq!(
        listed_arns(&listed),
        vec![format!(
            "arn:aws:cloudwatch:{OTHER_REGION}:{}:alarm-mute-rule:regional-rule",
            ::engine::EMULATOR_ACCOUNT_ID
        )],
        "case \"the other Region's listing\": expected only that Region's rule, got {listed}"
    );
}

/// The ARN of the rule `name` names in `region`, which is how a listing identifies a rule:
/// `AlarmMuteRuleSummary` models no name member, only the ARN.
fn rule_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:cloudwatch:{region}:{}:alarm-mute-rule:{name}",
        ::engine::EMULATOR_ACCOUNT_ID
    )
}

/// The rule ARNs one `ListAlarmMuteRules` answer reports, in the order it reported them.
fn listed_arns(listed: &Value) -> Vec<String> {
    listed
        .get("AlarmMuteRuleSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| text(summary, "AlarmMuteRuleArn"))
                .collect()
        })
        .unwrap_or_default()
}

/// A `PutMetricAlarm` request with only the members the model makes required, so a row that
/// filters a listing by alarm name names an alarm the store holds.
fn put_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": "MyApp",
        "MetricName": "Latency",
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 1.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// The four rules every listing row below reads, each in a status the clock decides and each
/// targeting its own set of alarms, plus the two alarms they target.
async fn given_mute_rules(router: &MultiServiceRouter, now: DateTime<Utc>) {
    for alarm in ["WebServerCPUAlarm", "DatabaseConnectionAlarm"] {
        ok(router, "PutMetricAlarm", &put_alarm(alarm)).await;
    }
    let rules: Vec<(&str, Value)> = vec![
        (
            "active-rule",
            json!({"MuteTargets": {"AlarmNames": ["WebServerCPUAlarm"]}}),
        ),
        (
            "scheduled-rule",
            json!({
                "StartDate": (now + TimeDelta::hours(2)).timestamp(),
                "MuteTargets": {"AlarmNames": ["WebServerCPUAlarm", "DatabaseConnectionAlarm"]},
            }),
        ),
        (
            "expired-rule",
            json!({
                "ExpireDate": (now - TimeDelta::minutes(1)).timestamp(),
                "MuteTargets": {"AlarmNames": ["DatabaseConnectionAlarm"]},
            }),
        ),
        (
            "untargeted-rule",
            json!({"Rule": {"Schedule": schedule(&at_expression(now, Tz::UTC), "PT1H", None)}}),
        ),
    ];
    for (name, configuration) in rules {
        let request = with_members(
            put_request(name, "cron(* * * * *)", "PT1H"),
            configuration,
        );
        ok(router, "PutAlarmMuteRule", &request).await;
    }
}

#[tokio::test]
async fn every_stored_mute_rule_is_listed_filtered_and_paged() {
    let router = build_router();
    let now = Utc::now();
    given_mute_rules(&router, now).await;

    // (case, request, the rules the answer must report, in order).
    //
    // Rows differ along which filter the request carries and what it selects: nothing, one alarm
    // the rules target to differing extents, one status, two statuses, a filter that selects no
    // rule, and the two filters together. A listing is ordered by rule name, so each row states
    // the names it expects in that order.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec![
                "active-rule",
                "expired-rule",
                "scheduled-rule",
                "untargeted-rule",
            ],
        ),
        (
            "the rules targeting one alarm",
            json!({"AlarmName": "WebServerCPUAlarm"}),
            vec!["active-rule", "scheduled-rule"],
        ),
        (
            "the rules targeting the other alarm",
            json!({"AlarmName": "DatabaseConnectionAlarm"}),
            vec!["expired-rule", "scheduled-rule"],
        ),
        (
            "the rules muting now",
            json!({"Statuses": ["ACTIVE"]}),
            vec!["active-rule", "untargeted-rule"],
        ),
        (
            "the rules still to become active",
            json!({"Statuses": ["SCHEDULED"]}),
            vec!["scheduled-rule"],
        ),
        (
            "the rules that will not become active again",
            json!({"Statuses": ["EXPIRED"]}),
            vec!["expired-rule"],
        ),
        (
            "two statuses at once",
            json!({"Statuses": ["SCHEDULED", "EXPIRED"]}),
            vec!["expired-rule", "scheduled-rule"],
        ),
        (
            "a status filter naming no status",
            json!({"Statuses": []}),
            vec![
                "active-rule",
                "expired-rule",
                "scheduled-rule",
                "untargeted-rule",
            ],
        ),
        (
            "both filters together",
            json!({"AlarmName": "DatabaseConnectionAlarm", "Statuses": ["EXPIRED"]}),
            vec!["expired-rule"],
        ),
        (
            "both filters, selecting no rule",
            json!({"AlarmName": "WebServerCPUAlarm", "Statuses": ["EXPIRED"]}),
            vec![],
        ),
    ];

    for (case, request, expected) in cases {
        let listed = ok(&router, "ListAlarmMuteRules", &request).await;
        let expected_arns: Vec<String> = expected
            .iter()
            .map(|name| rule_arn(REGION, name))
            .collect();
        assert_eq!(
            listed_arns(&listed),
            expected_arns,
            "case {case:?}: expected {expected:?}, got {listed}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: one page holds every selected rule, so no token is owed, got {listed}"
        );
        // Each summary carries the members that shape models, read off the same rule its reader
        // reports: the status the clock decided and the kind of window the schedule opens.
        for summary in listed["AlarmMuteRuleSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let arn = text(&summary, "AlarmMuteRuleArn").unwrap_or_default();
            let name = arn.rsplit(':').next().unwrap_or_default().to_string();
            let read = stored_rule(&router, &name).await;
            assert_eq!(
                (
                    text(&summary, "Status"),
                    text(&summary, "MuteType"),
                    stamp(&summary, "LastUpdatedTimestamp"),
                    stamp(&summary, "ExpireDate"),
                ),
                (
                    text(&read, "Status"),
                    text(&read, "MuteType"),
                    stamp(&read, "LastUpdatedTimestamp"),
                    stamp(&read, "ExpireDate"),
                ),
                "case {case:?}: the summary of {name:?} must report what its reader does, \
                 got {summary} beside {read}"
            );
        }
    }

    // Paged: a walk reports every rule exactly once, in the same order, whatever page size it
    // asks for, and the last page carries no token.
    let every_rule: Vec<String> = [
        "active-rule",
        "expired-rule",
        "scheduled-rule",
        "untargeted-rule",
    ]
    .iter()
    .map(|name| rule_arn(REGION, name))
    .collect();
    for page_size in [1, 2, 3, 4, 100] {
        let mut walked: Vec<String> = vec![];
        let mut token: Option<String> = None;
        for _ in 0..=every_rule.len() {
            let mut request = json!({"MaxRecords": page_size});
            if let Some(token) = token.as_deref() {
                request = with_members(request, json!({"NextToken": token}));
            }
            let listed = ok(&router, "ListAlarmMuteRules", &request).await;
            let page = listed_arns(&listed);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case \"MaxRecords {page_size}\": a page may hold at most that many rules, \
                 got {listed}"
            );
            walked.extend(page);
            token = text(&listed, "NextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            (walked.as_slice(), token.as_deref()),
            (every_rule.as_slice(), None),
            "case \"MaxRecords {page_size}\": expected every rule once in name order and no \
             token left over"
        );
    }
}

#[tokio::test]
async fn a_listing_the_store_cannot_serve_is_refused() {
    let router = build_router();
    let now = Utc::now();
    given_mute_rules(&router, now).await;

    // The filter names an ALARM, so a name no alarm of any kind carries names no resource to
    // filter by. Rows differ along how the requested alarm name misses: never created, the name
    // of a MUTE RULE rather than of an alarm, and a stored alarm name in another case or with a
    // suffix.
    for (case, alarm_name) in [
        ("an alarm name nothing was created under", "absent-alarm"),
        ("the name of a mute rule rather than of an alarm", "active-rule"),
        ("a stored alarm name in another case", "webservercpualarm"),
        ("a stored alarm name with a suffix", "WebServerCPUAlarm-2"),
    ] {
        let (status, body) = call(
            &router,
            "ListAlarmMuteRules",
            &json!({"AlarmName": alarm_name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} with {body}"
        );
        assert_eq!(
            (
                body.get("ResourceType").and_then(Value::as_str),
                body.get("ResourceId").and_then(Value::as_str),
                body.get("Message").and_then(Value::as_str),
            ),
            (Some("Alarm"), Some(alarm_name), None),
            "case {case:?}: expected the alarm named and no message, got {body}"
        );
    }

    // A token this service did not mint names no page. Rows differ along how the token misses:
    // text that is no token at all, a truncated one, and one naming a rule in a listing of
    // another shape.
    let minted = text(
        &ok(&router, "ListAlarmMuteRules", &json!({"MaxRecords": 1})).await,
        "NextToken",
    )
    .unwrap_or_default();
    for (case, token) in [
        ("a token that is not one", "not-a-token".to_string()),
        (
            "a minted token truncated",
            minted
                .get(..minted.len().saturating_sub(1))
                .unwrap_or_default()
                .to_string(),
        ),
        ("a minted token with text appended", format!("{minted}zz")),
    ] {
        let (status, body) = call(
            &router,
            "ListAlarmMuteRules",
            &json!({"NextToken": token}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextToken"),
            "case {case:?}: expected HTTP 400 InvalidNextToken, got {status} with {body}"
        );
        let got = error_message(&body);
        assert_eq!(
            got, "Invalid NextToken",
            "case {case:?}: expected the service's own sentence, got {got:?} in {body}"
        );
    }

    // The rules the store holds are untouched by the refusals.
    let listed = ok(&router, "ListAlarmMuteRules", &json!({})).await;
    assert_eq!(
        listed_arns(&listed).len(),
        4,
        "case \"after the refusals\": every stored rule must still be listed, got {listed}"
    );
}
