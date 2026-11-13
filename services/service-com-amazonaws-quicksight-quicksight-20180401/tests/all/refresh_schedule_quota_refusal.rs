// SPDX-License-Identifier: CC0-1.0

//! The message `CreateRefreshSchedule` carries once a data set holds every refresh
//! schedule it may hold.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the status, the error code and the message body the handler rendered.
//!
//! The rule under test: a refusal about an exhausted allowance names the resource whose
//! allowance ran out, and for refresh schedules that resource is the data set they
//! refresh — the refusal reports `"Reached max limit of <max> refresh schedules for
//! preparedDataSource <data-set-arn>"`, with the allowance the service documents and the
//! ARN assembled from the region the request was signed for and the request's own
//! `AwsAccountId` and `DataSetId`, not from the schedule the refused call asked for.
//!
//! So the rows below vary what the rule generalises over — the region, the account, the
//! form of the data set id, and which intervals the allowance was filled with — and each
//! row refuses under two different `ScheduleId`s, rather than re-running one recorded
//! call.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
use super::refresh_schedule_lifecycle::{
    create_schedule_in, daily_schedule, data_set_fixture_in, MAX_SCHEDULES_PER_DATA_SET,
};

/// A data set's ARN, as QuickSight builds it:
/// `arn:aws:quicksight:<region>:<account>:dataset/<data-set-id>`. Source:
/// `smithy_query(query="operation", operation="DescribeDataSet")` — the `Arn` a data set
/// reports.
fn data_set_arn(region: &str, account: &str, data_set_id: &str) -> String {
    format!("arn:aws:quicksight:{region}:{account}:dataset/{data_set_id}")
}

/// The sentence the service answers with when the data set's allowance has run out.
fn expected_message(region: &str, account: &str, data_set_id: &str) -> String {
    format!(
        "Reached max limit of {MAX_SCHEDULES_PER_DATA_SET} refresh schedules for \
         preparedDataSource {}",
        data_set_arn(region, account, data_set_id)
    )
}

/// One data set driven up to its allowance, and the refusal the next create must carry.
struct QuotaCase {
    /// Case name, reported by every assertion below.
    name: &'static str,
    /// The region the row signs for — the region in the ARN the refusal must name.
    region: &'static str,
    /// The 12-digit account the row addresses.
    account: &'static str,
    /// The data set the row hangs its schedules on.
    data_set_id: &'static str,
    /// The intervals the row fills the allowance with, one schedule per entry. Every
    /// entry is an interval a full refresh supports, and no sub-daily interval appears
    /// twice, so the only bound a row meets is the per-data-set one.
    fill: &'static [&'static str],
}

/// Rows varying the region, the account, the id's form and the interval mix. The
/// `RefreshInterval` members each `fill` names come from
/// `smithy_query(query="shape", shape="RefreshInterval")`.
const QUOTA_CASES: &[QuotaCase] = &[
    QuotaCase {
        name: "all daily",
        region: "us-west-2",
        account: "000000000000",
        data_set_id: "orders",
        fill: &["DAILY", "DAILY", "DAILY", "DAILY", "DAILY"],
    },
    QuotaCase {
        name: "daily, weekly and monthly in another region",
        region: "eu-central-1",
        account: "000000000000",
        data_set_id: "sales.q3_2026-rollup",
        fill: &["DAILY", "DAILY", "DAILY", "WEEKLY", "MONTHLY"],
    },
    QuotaCase {
        name: "one of every interval a full refresh takes, another account",
        region: "ap-southeast-2",
        account: "210987654321",
        data_set_id: "i",
        fill: &["HOURLY", "DAILY", "WEEKLY", "MONTHLY", "DAILY"],
    },
];

#[tokio::test]
async fn an_exhausted_refresh_schedule_allowance_names_its_data_set_by_arn() {
    for case in QUOTA_CASES {
        let name = case.name;
        assert_eq!(
            case.fill.len(),
            MAX_SCHEDULES_PER_DATA_SET,
            "case {name}: a row must fill the allowance exactly, got {} entr(ies)",
            case.fill.len()
        );

        let harness = Harness::new();
        data_set_fixture_in(&harness, case.region, case.account, case.data_set_id).await;

        for (index, interval) in case.fill.iter().enumerate() {
            let schedule_id = format!("fill-{index}");
            let (status, body) = create_schedule_in(
                &harness,
                case.region,
                case.account,
                case.data_set_id,
                daily_schedule(&schedule_id, interval),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name}: filling the allowance with {interval} expected 200, got {status} with body {body}"
            );
        }

        // Two refused creates under different ids: the message names the parent data
        // set, so it does not vary with the schedule the refused call asked for.
        let expected = expected_message(case.region, case.account, case.data_set_id);
        for schedule_id in ["one-too-many", "another.schedule_2"] {
            let (status, body) = create_schedule_in(
                &harness,
                case.region,
                case.account,
                case.data_set_id,
                daily_schedule(schedule_id, "DAILY"),
            )
            .await;
            assert_eq!(
                status, 409,
                "case {name} / {schedule_id}: one past the allowance expected 409, got {status} with body {body}"
            );
            assert_eq!(
                body["__type"],
                json!("LimitExceededException"),
                "case {name} / {schedule_id}: expected LimitExceededException, got {body}"
            );
            assert_eq!(
                body["Message"],
                json!(expected),
                "case {name} / {schedule_id}: expected the message {expected:?}, got {:?}",
                body["Message"]
            );
        }

        // A sibling data set of the same account and region has an allowance of its own,
        // so nothing refuses its first schedule: the exhausted resource really is the
        // one the message names.
        let sibling = format!("{}-sibling", case.data_set_id);
        data_set_fixture_in(&harness, case.region, case.account, &sibling).await;
        let (status, body) = create_schedule_in(
            &harness,
            case.region,
            case.account,
            &sibling,
            daily_schedule("first", "DAILY"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: a sibling data set's first schedule expected 200, got {status} with body {body}"
        );
    }
}

/// `DescribeRefreshSchedule`:
/// `GET /accounts/{AwsAccountId}/data-sets/{DataSetId}/refresh-schedules/{ScheduleId}`.
/// Source: `smithy_query(query="operation", operation="DescribeRefreshSchedule")`.
async fn describe_schedule_in(
    harness: &Harness,
    region: &str,
    account: &str,
    data_set_id: &str,
    schedule_id: &str,
) -> (u16, Value) {
    harness
        .call_in_region(
            region,
            "GET",
            &format!(
                "/accounts/{account}/data-sets/{data_set_id}/refresh-schedules/{schedule_id}"
            ),
            None,
        )
        .await
}

#[tokio::test]
async fn the_refused_create_leaves_the_data_set_as_it_was() {
    for case in QUOTA_CASES {
        let name = case.name;
        let harness = Harness::new();
        data_set_fixture_in(&harness, case.region, case.account, case.data_set_id).await;
        for (index, interval) in case.fill.iter().enumerate() {
            let schedule_id = format!("fill-{index}");
            let (status, body) = create_schedule_in(
                &harness,
                case.region,
                case.account,
                case.data_set_id,
                daily_schedule(&schedule_id, interval),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name} fill-{index}: expected 200, got {status} with body {body}"
            );
        }

        // What the data set's schedules report before the refused create.
        let mut before: Vec<Value> = Vec::new();
        for index in 0..case.fill.len() {
            let (status, body) = describe_schedule_in(
                &harness,
                case.region,
                case.account,
                case.data_set_id,
                &format!("fill-{index}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name} fill-{index}: describe expected 200, got {status} with body {body}"
            );
            before.push(body["RefreshSchedule"].clone());
        }

        let (status, body) = create_schedule_in(
            &harness,
            case.region,
            case.account,
            case.data_set_id,
            daily_schedule("one-too-many", "DAILY"),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name}: one past the allowance expected 409, got {status} with body {body}"
        );

        // The refusal persisted nothing and changed nothing.
        let (status, body) = describe_schedule_in(
            &harness,
            case.region,
            case.account,
            case.data_set_id,
            "one-too-many",
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name}: the refused schedule must not exist, got {status} with body {body}"
        );
        for (index, stored) in before.iter().enumerate() {
            let (status, body) = describe_schedule_in(
                &harness,
                case.region,
                case.account,
                case.data_set_id,
                &format!("fill-{index}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name} fill-{index}: describe after the refusal expected 200, got {status} with body {body}"
            );
            assert_eq!(
                &body["RefreshSchedule"], stored,
                "case {name} fill-{index}: the refused create changed a stored schedule: expected {stored}, got {}",
                body["RefreshSchedule"]
            );
        }

        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let held = state
            .lock()
            .refresh_schedules_in(case.account, case.data_set_id)
            .len();
        assert_eq!(
            held, MAX_SCHEDULES_PER_DATA_SET,
            "case {name}: expected the allowance's worth of stored schedules, got {held}"
        );
    }
}
