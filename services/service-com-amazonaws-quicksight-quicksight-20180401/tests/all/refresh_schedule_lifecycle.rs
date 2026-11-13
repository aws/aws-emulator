// SPDX-License-Identifier: CC0-1.0

//! Data set refresh schedules: the create and the read that reports it, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a refresh schedule is a child of one data set, identified by
//! `(AwsAccountId, DataSetId, ScheduleId)`; `CreateRefreshSchedule` persists the
//! submitted schedule under that identity and reports the ARN it minted for it, and
//! `DescribeRefreshSchedule` answers from that stored record. So the rows below vary the
//! inputs the rule generalises over — the interval, the refresh type, which optional
//! frequency members the schedule carries, the id's form, the data set and the account —
//! rather than re-running one recorded call. The allowances the model documents for a
//! single data set (at most five schedules, one schedule per sub-daily interval, the
//! sub-15/30-minute intervals only for incremental refreshes, a day named for the weekly
//! and monthly intervals) are asserted on both sides of each bound.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
use super::topic_lifecycle::json_contains;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

/// The `RefreshInterval` members. Catalog source:
/// `smithy_query(query="shape", shape="RefreshInterval")`.
const REFRESH_INTERVALS: &[&str] = &[
    "MINUTE15",
    "MINUTE30",
    "HOURLY",
    "DAILY",
    "WEEKLY",
    "MONTHLY",
];

/// The `RefreshInterval` members the model documents as usable for one schedule per
/// dataset. Source: `smithy_query(query="doc", shape="RefreshFrequency",
/// member="Interval")`.
const SINGLE_SCHEDULE_INTERVALS: &[&str] = &["MINUTE15", "MINUTE30", "HOURLY"];

/// The `RefreshInterval` members the model documents as supported only for incremental
/// refreshes. Same source as [`SINGLE_SCHEDULE_INTERVALS`].
const INCREMENTAL_ONLY_INTERVALS: &[&str] = &["MINUTE15", "MINUTE30"];

/// The `RefreshInterval` members the model documents as requiring a `RefreshOnDay`.
/// Source: `smithy_query(query="doc", shape="RefreshFrequency", member="RefreshOnDay")` —
/// "This value is required for weekly and monthly refresh intervals".
const REFRESH_ON_DAY_REQUIRED_INTERVALS: &[&str] = &["WEEKLY", "MONTHLY"];

/// How many refresh schedules one data set may hold. Source: the
/// `CreateRefreshSchedule` documentation — "You can create up to 5 different schedules
/// for a single dataset".
///
/// `pub(crate)` so the suite about the wording of that allowance's refusal reads the
/// same allowance rather than restating it.
pub(crate) const MAX_SCHEDULES_PER_DATA_SET: usize = 5;

fn refresh_schedule_arn(account: &str, data_set_id: &str, schedule_id: &str) -> String {
    format!(
        "arn:aws:quicksight:{REGION}:{account}:dataset/{data_set_id}/refresh-schedule/{schedule_id}"
    )
}

/// A one-entry `PhysicalTableMap` holding a relational table on `data_source_arn`, so a
/// row can create the data set a schedule refreshes.
fn relational_table_map(data_source_arn: &str) -> Value {
    json!({
        "orders-table": {
            "RelationalTable": {
                "DataSourceArn": data_source_arn,
                "Schema": "public",
                "Name": "orders",
                "InputColumns": [
                    { "Name": "order_id", "Type": "INTEGER" },
                    { "Name": "revenue", "Type": "DECIMAL" },
                ],
            }
        }
    })
}

/// Create one data source and one data set under `account`, so a row has a data set to
/// hang refresh schedules on.
async fn data_set_fixture(harness: &Harness, account: &str, data_set_id: &str) {
    data_set_fixture_in(harness, REGION, account, data_set_id).await;
}

/// [`data_set_fixture`], signed for `region`: a row that drives one emulator from two
/// regions needs its data set created in the region it will address it from.
pub(crate) async fn data_set_fixture_in(
    harness: &Harness,
    region: &str,
    account: &str,
    data_set_id: &str,
) {
    let data_source_id = format!("src-{data_set_id}");
    let (status, body) = harness
        .call_in_region(
            region,
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(json!({
                "DataSourceId": data_source_id,
                "Name": "fixture source",
                "Type": "ATHENA",
            })),
        )
        .await;
    assert_eq!(
        status, 202,
        "fixture {data_source_id}: create expected 202, got {status} with body {body}"
    );
    let source_arn = body["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {data_source_id}: create returned no Arn: {body}"))
        .to_string();

    let (status, body) = harness
        .call_in_region(
            region,
            "POST",
            &format!("/accounts/{account}/data-sets"),
            Some(json!({
                "DataSetId": data_set_id,
                "Name": "fixture set",
                "ImportMode": "SPICE",
                "PhysicalTableMap": relational_table_map(&source_arn),
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "fixture {data_set_id}: create expected 200, got {status} with body {body}"
    );
}

async fn create_schedule(
    harness: &Harness,
    account: &str,
    data_set_id: &str,
    schedule: Value,
) -> (u16, Value) {
    create_schedule_in(harness, REGION, account, data_set_id, schedule).await
}

/// [`create_schedule`], signed for `region`: the handler takes the region of the ARNs it
/// mints and reports from the request's credential scope.
pub(crate) async fn create_schedule_in(
    harness: &Harness,
    region: &str,
    account: &str,
    data_set_id: &str,
    schedule: Value,
) -> (u16, Value) {
    harness
        .call_in_region(
            region,
            "POST",
            &format!("/accounts/{account}/data-sets/{data_set_id}/refresh-schedules"),
            Some(json!({ "Schedule": schedule })),
        )
        .await
}

async fn describe_schedule(
    harness: &Harness,
    account: &str,
    data_set_id: &str,
    schedule_id: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!(
                "/accounts/{account}/data-sets/{data_set_id}/refresh-schedules/{schedule_id}"
            ),
            None,
        )
        .await
}

/// The day a schedule on `interval` must name, or `None` for an interval that needs none:
/// a weekly schedule refreshes on a day of the week and a monthly one on a day of the
/// month.
fn refresh_on_day(interval: &str) -> Option<Value> {
    match interval {
        "WEEKLY" => Some(json!({ "DayOfWeek": "THURSDAY" })),
        "MONTHLY" => Some(json!({ "DayOfMonth": "17" })),
        _ => None,
    }
}

/// The simplest schedule body on `interval`: a full refresh, which every interval but
/// the sub-30-minute ones supports, carrying the `RefreshOnDay` the interval requires.
pub(crate) fn daily_schedule(schedule_id: &str, interval: &str) -> Value {
    let mut frequency = json!({ "Interval": interval });
    if let Some(day) = refresh_on_day(interval) {
        frequency["RefreshOnDay"] = day;
    }
    json!({
        "ScheduleId": schedule_id,
        "RefreshType": "FULL_REFRESH",
        "ScheduleFrequency": frequency,
    })
}

/// One `CreateRefreshSchedule` and what the schedule must report afterwards.
struct ScheduleCase {
    /// Case name, and the `ScheduleId` the row creates.
    schedule_id: &'static str,
    /// The `Schedule` (`RefreshSchedule`) the row submits.
    schedule: Value,
}

/// Rows spanning the interval (one longer interval each, plus one sub-daily and one
/// sub-30-minute one), the refresh type, which optional frequency members the schedule
/// carries (`RefreshOnDay` by day of week and by day of month, `Timezone`,
/// `TimeOfTheDay`, `StartAfterDateTime`, and none of them) and the id's form. There are
/// exactly [`MAX_SCHEDULES_PER_DATA_SET`] of them, so the set also sits on the
/// documented allowance for one data set.
fn schedule_cases() -> Vec<ScheduleCase> {
    vec![
        ScheduleCase {
            schedule_id: "nightly",
            schedule: json!({
                "ScheduleId": "nightly",
                "RefreshType": "FULL_REFRESH",
                "ScheduleFrequency": {
                    "Interval": "DAILY",
                    "TimeOfTheDay": "03:15",
                    "Timezone": "Europe/Berlin",
                },
                // Epoch seconds: restJson1 renders a timestamp as an epoch number.
                "StartAfterDateTime": 1_767_225_600,
            }),
        },
        ScheduleCase {
            schedule_id: "weekly_on-monday.v2",
            schedule: json!({
                "ScheduleId": "weekly_on-monday.v2",
                "RefreshType": "FULL_REFRESH",
                "ScheduleFrequency": {
                    "Interval": "WEEKLY",
                    "RefreshOnDay": { "DayOfWeek": "MONDAY" },
                    "TimeOfTheDay": "23:59",
                    "Timezone": "UTC",
                },
            }),
        },
        ScheduleCase {
            schedule_id: "month-end",
            schedule: json!({
                "ScheduleId": "month-end",
                "RefreshType": "FULL_REFRESH",
                "ScheduleFrequency": {
                    "Interval": "MONTHLY",
                    // The model's `DayOfMonth` pattern allows this sentinel.
                    "RefreshOnDay": { "DayOfMonth": "LAST_DAY_OF_MONTH" },
                },
            }),
        },
        ScheduleCase {
            schedule_id: "hourly",
            schedule: json!({
                "ScheduleId": "hourly",
                "RefreshType": "INCREMENTAL_REFRESH",
                "ScheduleFrequency": { "Interval": "HOURLY" },
            }),
        },
        ScheduleCase {
            schedule_id: "quarter-hourly",
            schedule: json!({
                "ScheduleId": "quarter-hourly",
                "RefreshType": "INCREMENTAL_REFRESH",
                "ScheduleFrequency": { "Interval": "MINUTE15", "Timezone": "America/New_York" },
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_refresh_schedule_reads_back_for_every_kind_of_frequency() {
    let harness = Harness::new();
    data_set_fixture(&harness, ACCOUNT, "orders").await;

    let cases = schedule_cases();
    assert_eq!(
        cases.len(),
        MAX_SCHEDULES_PER_DATA_SET,
        "the table is meant to sit exactly on the documented allowance, got {} row(s)",
        cases.len()
    );
    let mut request_ids: Vec<String> = Vec::new();

    for case in &cases {
        let schedule_id = case.schedule_id;
        let (status, body) =
            create_schedule(&harness, ACCOUNT, "orders", case.schedule.clone()).await;
        assert_eq!(
            status, 200,
            "case {schedule_id}: create expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(refresh_schedule_arn(ACCOUNT, "orders", schedule_id)),
            "case {schedule_id}: create expected the data-set-scoped ARN, got {body}"
        );
        assert_eq!(
            body["ScheduleId"],
            json!(schedule_id),
            "case {schedule_id}: create echoed the wrong id: {body}"
        );
        // `Status` is `@httpResponseCode`-bound, so it IS the HTTP status asserted
        // above rather than a body member.
        assert_eq!(
            body["Status"],
            Value::Null,
            "case {schedule_id}: Status is bound to the HTTP status, so it is not a body member: {body}"
        );
        let request_id = body["RequestId"].as_str().unwrap_or_default().to_string();
        assert!(
            !request_id.is_empty(),
            "case {schedule_id}: create expected a RequestId, got {body}"
        );
        request_ids.push(request_id);
    }

    for case in &cases {
        let schedule_id = case.schedule_id;
        let (status, body) = describe_schedule(&harness, ACCOUNT, "orders", schedule_id).await;
        assert_eq!(
            status, 200,
            "case {schedule_id}: describe expected 200, got {status} with body {body}"
        );
        let expected_arn = refresh_schedule_arn(ACCOUNT, "orders", schedule_id);
        assert_eq!(
            body["Arn"],
            json!(expected_arn),
            "case {schedule_id}: describe expected the create's ARN, got {body}"
        );
        assert!(
            json_contains(&case.schedule, &body["RefreshSchedule"]),
            "case {schedule_id}: expected the stored schedule {}, got {}",
            case.schedule,
            body["RefreshSchedule"]
        );
        assert_eq!(
            body["RefreshSchedule"]["Arn"],
            json!(expected_arn),
            "case {schedule_id}: the reported schedule must carry the minted ARN, got {body}"
        );

        // A read reports; it changes nothing a later caller sees.
        let (again_status, again) = describe_schedule(&harness, ACCOUNT, "orders", schedule_id).await;
        assert_eq!(
            again_status, 200,
            "case {schedule_id}: second describe expected 200, got {again_status} with body {again}"
        );
        assert_eq!(
            again["RefreshSchedule"], body["RefreshSchedule"],
            "case {schedule_id}: a read changed what the next read sees: {} then {}",
            body["RefreshSchedule"], again["RefreshSchedule"]
        );
    }

    // The store holds one record per created schedule, under the data-set-scoped key.
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    for case in &cases {
        let schedule_id = case.schedule_id;
        let record = store
            .refresh_schedule(ACCOUNT, "orders", schedule_id)
            .unwrap_or_else(|| panic!("case {schedule_id}: the store holds no such schedule"));
        assert_eq!(
            record.refresh_type,
            case.schedule["RefreshType"].as_str().unwrap_or_default(),
            "case {schedule_id}: expected the submitted RefreshType, got {:?}",
            record.refresh_type
        );
        assert_eq!(
            record.schedule_frequency.interval,
            case.schedule["ScheduleFrequency"]["Interval"]
                .as_str()
                .unwrap_or_default(),
            "case {schedule_id}: expected the submitted Interval, got {:?}",
            record.schedule_frequency.interval
        );
    }
    assert_eq!(
        store.refresh_schedules_in(ACCOUNT, "orders").len(),
        cases.len(),
        "expected one stored record per created schedule, got {}",
        store.refresh_schedules_in(ACCOUNT, "orders").len()
    );

    // Every response reported its own request id.
    request_ids.sort();
    let minted = request_ids.len();
    request_ids.dedup();
    assert_eq!(
        request_ids.len(),
        minted,
        "each create must report its own RequestId, got {request_ids:?}"
    );
}

#[tokio::test]
async fn a_refresh_schedule_is_identified_by_account_data_set_and_schedule_id() {
    let harness = Harness::new();
    // The SAME ScheduleId on two data sets in one account, and on a data set of the
    // same id in another account: three schedules, each addressed by its own triple.
    let addresses = [
        (ACCOUNT, "orders"),
        (ACCOUNT, "inventory"),
        (OTHER_ACCOUNT, "orders"),
    ];
    for (account, data_set_id) in addresses {
        data_set_fixture(&harness, account, data_set_id).await;
        let (status, body) = create_schedule(
            &harness,
            account,
            data_set_id,
            daily_schedule("shared-id", "DAILY"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account}/{data_set_id}: create expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(refresh_schedule_arn(account, data_set_id, "shared-id")),
            "case {account}/{data_set_id}: create expected an ARN scoped to its own account and data set, got {body}"
        );
    }

    for (account, data_set_id) in addresses {
        let (status, body) = describe_schedule(&harness, account, data_set_id, "shared-id").await;
        assert_eq!(
            status, 200,
            "case {account}/{data_set_id}: describe expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(refresh_schedule_arn(account, data_set_id, "shared-id")),
            "case {account}/{data_set_id}: describe expected its own ARN, got {body}"
        );
    }

    // An id that exists on another data set is not found on this one.
    data_set_fixture(&harness, ACCOUNT, "returns").await;
    let (status, body) = describe_schedule(&harness, ACCOUNT, "returns", "shared-id").await;
    assert_eq!(
        status, 404,
        "case data set holding no schedule: expected 404, got {status} with body {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "case data set holding no schedule: expected ResourceNotFoundException, got {body}"
    );

    // A second create under the same triple is the resource that already exists.
    let (status, body) = create_schedule(
        &harness,
        ACCOUNT,
        "orders",
        daily_schedule("shared-id", "WEEKLY"),
    )
    .await;
    assert_eq!(
        status, 409,
        "case duplicate ScheduleId: expected 409, got {status} with body {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceExistsException"),
        "case duplicate ScheduleId: expected ResourceExistsException, got {body}"
    );
    // The duplicate create left the stored schedule alone.
    let (_, body) = describe_schedule(&harness, ACCOUNT, "orders", "shared-id").await;
    assert_eq!(
        body["RefreshSchedule"]["ScheduleFrequency"]["Interval"],
        json!("DAILY"),
        "case duplicate ScheduleId: the refused create must not overwrite the schedule, got {body}"
    );
}

#[tokio::test]
async fn a_refresh_schedule_needs_a_data_set_of_its_own_account() {
    let harness = Harness::new();
    data_set_fixture(&harness, ACCOUNT, "orders").await;

    // Rows: a data set nothing created, and a data set that exists under ANOTHER
    // account — neither is a data set this account holds.
    for (case, account, data_set_id) in [
        ("unknown data set", ACCOUNT, "no-such-set"),
        ("data set of another account", OTHER_ACCOUNT, "orders"),
    ] {
        let (status, body) =
            create_schedule(&harness, account, data_set_id, daily_schedule("s1", "DAILY")).await;
        assert_eq!(
            status, 404,
            "case {case}: create expected 404, got {status} with body {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = describe_schedule(&harness, account, data_set_id, "s1").await;
        assert_eq!(
            status, 404,
            "case {case}: describe expected 404, got {status} with body {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_data_set_holds_at_most_the_documented_number_of_schedules() {
    let harness = Harness::new();
    data_set_fixture(&harness, ACCOUNT, "orders").await;
    data_set_fixture(&harness, ACCOUNT, "inventory").await;

    // Up to the allowance, every create is accepted; `DAILY` carries no per-interval
    // restriction, so the only bound these rows meet is the per-data-set one.
    for index in 0..MAX_SCHEDULES_PER_DATA_SET {
        let schedule_id = format!("schedule-{index}");
        let (status, body) = create_schedule(
            &harness,
            ACCOUNT,
            "orders",
            daily_schedule(&schedule_id, "DAILY"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {schedule_id}: create below the allowance expected 200, got {status} with body {body}"
        );
    }

    // One past the allowance.
    let (status, body) = create_schedule(
        &harness,
        ACCOUNT,
        "orders",
        daily_schedule("one-too-many", "DAILY"),
    )
    .await;
    assert_eq!(
        status, 409,
        "case one past the allowance: expected 409, got {status} with body {body}"
    );
    assert_eq!(
        body["__type"],
        json!("LimitExceededException"),
        "case one past the allowance: expected LimitExceededException, got {body}"
    );

    // The refused create persisted nothing.
    let (status, body) = describe_schedule(&harness, ACCOUNT, "orders", "one-too-many").await;
    assert_eq!(
        status, 404,
        "case one past the allowance: the refused schedule must not exist, got {status} with body {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "case one past the allowance: expected ResourceNotFoundException, got {body}"
    );

    // The allowance is per data set, so a sibling data set still accepts one.
    let (status, body) = create_schedule(
        &harness,
        ACCOUNT,
        "inventory",
        daily_schedule("schedule-0", "DAILY"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case sibling data set: expected 200, got {status} with body {body}"
    );

    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    assert_eq!(
        store.refresh_schedules_in(ACCOUNT, "orders").len(),
        MAX_SCHEDULES_PER_DATA_SET,
        "expected the data set to hold exactly the allowance, got {}",
        store.refresh_schedules_in(ACCOUNT, "orders").len()
    );
    assert_eq!(
        store.refresh_schedules_in(ACCOUNT, "inventory").len(),
        1,
        "expected the sibling data set to hold its own schedule, got {}",
        store.refresh_schedules_in(ACCOUNT, "inventory").len()
    );
}

#[tokio::test]
async fn every_interval_takes_a_second_schedule_unless_the_model_forbids_it() {
    // One data set per interval, so the per-data-set allowance is never what refuses a
    // row here.
    for interval in REFRESH_INTERVALS {
        let harness = Harness::new();
        let data_set_id = format!("set-{}", interval.to_lowercase());
        data_set_fixture(&harness, ACCOUNT, &data_set_id).await;

        // The sub-30-minute intervals are documented as incremental-only, so that is
        // the refresh type their rows use.
        let refresh_type = if INCREMENTAL_ONLY_INTERVALS.contains(interval) {
            "INCREMENTAL_REFRESH"
        } else {
            "FULL_REFRESH"
        };
        let schedule = |schedule_id: &str| {
            let mut frequency = json!({ "Interval": interval });
            if let Some(day) = refresh_on_day(interval) {
                frequency["RefreshOnDay"] = day;
            }
            json!({
                "ScheduleId": schedule_id,
                "RefreshType": refresh_type,
                "ScheduleFrequency": frequency,
            })
        };

        let (status, body) =
            create_schedule(&harness, ACCOUNT, &data_set_id, schedule("first")).await;
        assert_eq!(
            status, 200,
            "case {interval} first: expected 200, got {status} with body {body}"
        );

        let (status, body) =
            create_schedule(&harness, ACCOUNT, &data_set_id, schedule("second")).await;
        if SINGLE_SCHEDULE_INTERVALS.contains(interval) {
            assert_eq!(
                status, 409,
                "case {interval} second: an interval usable for one schedule per data set expected 409, got {status} with body {body}"
            );
            assert_eq!(
                body["__type"],
                json!("LimitExceededException"),
                "case {interval} second: expected LimitExceededException, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {interval} second: an unrestricted interval expected 200, got {status} with body {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_sub_30_minute_interval_is_refused_for_a_full_refresh() {
    let harness = Harness::new();
    data_set_fixture(&harness, ACCOUNT, "orders").await;

    // Every interval, each submitted as a FULL_REFRESH: the two the model documents as
    // incremental-only are refused, the rest are accepted. Each row goes on its own
    // data set, so no other allowance is what answers.
    for interval in REFRESH_INTERVALS {
        let data_set_id = format!("full-{}", interval.to_lowercase());
        data_set_fixture(&harness, ACCOUNT, &data_set_id).await;
        let (status, body) = create_schedule(
            &harness,
            ACCOUNT,
            &data_set_id,
            daily_schedule("full", interval),
        )
        .await;
        if INCREMENTAL_ONLY_INTERVALS.contains(interval) {
            assert_eq!(
                status, 400,
                "case {interval} as FULL_REFRESH: expected 400, got {status} with body {body}"
            );
            assert_eq!(
                body["__type"],
                json!("InvalidParameterValueException"),
                "case {interval} as FULL_REFRESH: expected InvalidParameterValueException, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {interval} as FULL_REFRESH: expected 200, got {status} with body {body}"
            );
        }
    }

    // A value no `RefreshInterval` member names is not an interval at all.
    let (status, body) = create_schedule(
        &harness,
        ACCOUNT,
        "orders",
        daily_schedule("yearly", "YEARLY"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case interval outside the catalog: expected 400, got {status} with body {body}"
    );

    // Nor is an empty `ScheduleId` an id.
    let (status, body) = create_schedule(
        &harness,
        ACCOUNT,
        "orders",
        daily_schedule("", "DAILY"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case empty ScheduleId: expected 400, got {status} with body {body}"
    );
}

#[tokio::test]
async fn a_schedule_on_a_weekly_or_monthly_interval_must_name_the_day() {
    let harness = Harness::new();

    // Every interval, each submitted with NO `RefreshOnDay`: the two the model documents
    // as requiring one are refused, the rest are accepted. Each row goes on its own data
    // set, so no other allowance is what answers.
    for interval in REFRESH_INTERVALS {
        let data_set_id = format!("no-day-{}", interval.to_lowercase());
        data_set_fixture(&harness, ACCOUNT, &data_set_id).await;
        let refresh_type = if INCREMENTAL_ONLY_INTERVALS.contains(interval) {
            "INCREMENTAL_REFRESH"
        } else {
            "FULL_REFRESH"
        };
        let (status, body) = create_schedule(
            &harness,
            ACCOUNT,
            &data_set_id,
            json!({
                "ScheduleId": "no-day",
                "RefreshType": refresh_type,
                "ScheduleFrequency": { "Interval": interval },
            }),
        )
        .await;
        if REFRESH_ON_DAY_REQUIRED_INTERVALS.contains(interval) {
            assert_eq!(
                status, 400,
                "case {interval} without RefreshOnDay: expected 400, got {status} with body {body}"
            );
            assert_eq!(
                body["__type"],
                json!("InvalidParameterValueException"),
                "case {interval} without RefreshOnDay: expected InvalidParameterValueException, got {body}"
            );
            // The refused create persisted nothing.
            let (status, body) =
                describe_schedule(&harness, ACCOUNT, &data_set_id, "no-day").await;
            assert_eq!(
                status, 404,
                "case {interval} without RefreshOnDay: the refused schedule must not exist, got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {interval} without RefreshOnDay: an interval needing none expected 200, got {status} with body {body}"
            );
        }

        // The same interval WITH the day it requires is accepted, each kind of day on
        // the interval that takes it.
        let (status, body) = create_schedule(
            &harness,
            ACCOUNT,
            &data_set_id,
            json!({
                "ScheduleId": "with-day",
                "RefreshType": refresh_type,
                "ScheduleFrequency": {
                    "Interval": interval,
                    "RefreshOnDay": refresh_on_day(interval)
                        .unwrap_or_else(|| json!({ "DayOfWeek": "SUNDAY" })),
                },
            }),
        )
        .await;
        let expected = if SINGLE_SCHEDULE_INTERVALS.contains(interval) {
            // This interval already has its one schedule from the row above.
            409
        } else {
            200
        };
        assert_eq!(
            status, expected,
            "case {interval} with RefreshOnDay: expected {expected}, got {status} with body {body}"
        );
    }
}
