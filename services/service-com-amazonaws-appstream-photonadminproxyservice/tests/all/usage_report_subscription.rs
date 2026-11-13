// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's usage report subscription over the wire.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over the one
//! recorded request:
//!
//! * usage reporting is ONE account-and-Region-wide setting: enabling it persists a
//!   single subscription whose bucket is named for the account and Region it belongs to,
//!   and enabling it again reports that same subscription instead of adding a second;
//! * a reader renders the stored subscription back, so the bucket and schedule a
//!   `DescribeUsageReportSubscriptions` reports are the ones the create persisted, and no
//!   report date or subscription error is claimed for a report nothing generated;
//! * an account with usage reporting disabled lists NO subscriptions — an empty
//!   collection, not a refusal — and disabling a subscription it does not have is
//!   refused with `ResourceNotFoundException` (400);
//! * disabling and enabling again reports the bucket the account already had, because
//!   the bucket is unique to the account and Region;
//! * `MaxResults` sizes the page: every size that can hold a record reports the whole
//!   collection with no `NextToken`, a size below one record is refused, and so is a
//!   `NextToken` this service never issued.

use serde_json::{json, Value};

use crate::stack_fleet_lifecycle::{assert_error, Emulator};

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Regions the cases below enable usage reporting in. The rows differ along the
/// Region the request's SigV4 credential scope names — the Region whose bucket the
/// reports are written to — and each is a real AWS Region token.
const REGIONS: &[&str] = &["us-east-1", "eu-central-1", "ap-southeast-2"];

/// The only `UsageReportSchedule` the model declares (`USAGE_REPORT_SCHEDULE_VALUES` in
/// `src/ops/types/shared_types.rs`), matching the operation's "Usage reports are
/// generated daily".
const SCHEDULE: &str = "DAILY";

/// The `S3BucketName` of a create or of a listed subscription.
fn bucket(case: &str, payload: &Value) -> String {
    payload
        .get("S3BucketName")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an S3BucketName, got {payload}"))
        .to_string()
}

/// Assert `name` is the bucket AppStream creates for `region`'s usage reports: an
/// `appstream-logs-<region>-<account>-<identifier>` name, whose identifier is what keeps
/// a bucket name globally unique.
fn assert_log_bucket_name(case: &str, region: &str, name: &str) {
    let prefix = format!("appstream-logs-{region}-{ACCOUNT}-");
    assert!(
        name.starts_with(&prefix),
        "case {case:?}: the reports bucket must be named {prefix}<identifier>, got {name:?}"
    );
    let identifier = &name[prefix.len()..];
    assert!(
        !identifier.is_empty()
            && identifier
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
        "case {case:?}: the bucket identifier must be a lowercase S3-safe suffix, \
         got {identifier:?} in {name:?}"
    );
}

/// The subscriptions a `DescribeUsageReportSubscriptions` reports, and the pagination
/// token it carries.
async fn describe(emulator: &Emulator, case: &str, request: Value) -> (Vec<Value>, Option<Value>) {
    let payload = emulator
        .ok(case, "DescribeUsageReportSubscriptions", request)
        .await;
    let subscriptions = payload
        .get("UsageReportSubscriptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a UsageReportSubscriptions list, got {payload}")
        })
        .clone();
    (subscriptions, payload.get("NextToken").cloned())
}

// ---------------------------------------------------------------------------
// Enabling usage reporting, and reading it back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enabling_usage_reporting_persists_the_subscription_a_later_read_reports() {
    for region in REGIONS {
        let case = format!("enable in {region}");
        let emulator = Emulator::in_region(region);

        // Nothing is enabled yet, so the account lists no subscription at all.
        let (before, next_token) = describe(&emulator, &case, json!({})).await;
        assert!(
            before.is_empty(),
            "case {case:?}: an account with usage reporting disabled must list no \
             subscription, got {before:?}"
        );
        assert!(
            next_token.is_none(),
            "case {case:?}: an empty collection leaves nothing to resume from, got \
             {next_token:?}"
        );

        let created = emulator
            .ok(&case, "CreateUsageReportSubscription", json!({}))
            .await;
        let created_bucket = bucket(&case, &created);
        assert_log_bucket_name(&case, region, &created_bucket);
        assert_eq!(
            created.get("Schedule").and_then(Value::as_str),
            Some(SCHEDULE),
            "case {case:?}: reports are generated daily, got {created}"
        );

        // The read renders the record the create persisted: the same bucket and
        // schedule, and nothing claimed about a report that has not been generated.
        let (listed, next_token) = describe(&emulator, &case, json!({})).await;
        assert_eq!(
            listed,
            vec![json!({ "S3BucketName": created_bucket, "Schedule": SCHEDULE })],
            "case {case:?}: the read must report exactly the subscription the create \
             persisted, got {listed:?}"
        );
        assert!(
            next_token.is_none(),
            "case {case:?}: one subscription fits in one page, got {next_token:?}"
        );
    }
}

#[tokio::test]
async fn enabling_usage_reporting_again_reports_the_one_subscription_the_account_has() {
    // Rows: how many times usage reporting is enabled in a row. Each extra call must
    // answer with the subscription the account already has, and leave it at one.
    for enables in [2_usize, 3, 5] {
        let case = format!("{enables} enables in a row");
        let emulator = Emulator::new();

        let first = emulator
            .ok(&case, "CreateUsageReportSubscription", json!({}))
            .await;
        let first_bucket = bucket(&case, &first);

        for attempt in 2..=enables {
            let again = emulator
                .ok(&case, "CreateUsageReportSubscription", json!({}))
                .await;
            assert_eq!(
                again, first,
                "case {case:?}: enable #{attempt} must report the subscription the \
                 account already has, got {again}"
            );
        }

        let (listed, _) = describe(&emulator, &case, json!({})).await;
        assert_eq!(
            listed,
            vec![json!({ "S3BucketName": first_bucket, "Schedule": SCHEDULE })],
            "case {case:?}: the account must still hold exactly one subscription, got \
             {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Disabling usage reporting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disabling_usage_reporting_drops_the_subscription_and_keeps_the_bucket() {
    // Rows: the Region the subscription is enabled, disabled and enabled again in. The
    // bucket is unique to the account and Region, so the second enable must report the
    // bucket the first one did.
    for region in REGIONS {
        let case = format!("disable and re-enable in {region}");
        let emulator = Emulator::in_region(region);

        let created = emulator
            .ok(&case, "CreateUsageReportSubscription", json!({}))
            .await;
        let created_bucket = bucket(&case, &created);

        let deleted = emulator
            .ok(&case, "DeleteUsageReportSubscription", json!({}))
            .await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: disabling report generation reports no members, got {deleted}"
        );

        let (listed, _) = describe(&emulator, &case, json!({})).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a disabled account must list no subscription, got {listed:?}"
        );

        // Nothing is left to disable, so a second disable is refused rather than
        // answering as though it had disabled something.
        assert_error(
            &emulator,
            &case,
            "DeleteUsageReportSubscription",
            json!({}),
            400,
            "ResourceNotFoundException",
        )
        .await;

        let re_enabled = emulator
            .ok(&case, "CreateUsageReportSubscription", json!({}))
            .await;
        assert_eq!(
            bucket(&case, &re_enabled),
            created_bucket,
            "case {case:?}: the bucket is unique to the account and Region, so enabling \
             again must report the same one, got {re_enabled}"
        );
    }
}

#[tokio::test]
async fn disabling_usage_reporting_that_was_never_enabled_is_refused() {
    let case = "disable without enabling";
    let emulator = Emulator::new();

    assert_error(
        &emulator,
        case,
        "DeleteUsageReportSubscription",
        json!({}),
        400,
        "ResourceNotFoundException",
    )
    .await;

    // The refusal changed nothing: the account still has no subscription, and enabling
    // one still works.
    let (listed, _) = describe(&emulator, case, json!({})).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: the refused disable must leave the account with no \
         subscription, got {listed:?}"
    );
    let created = emulator
        .ok(case, "CreateUsageReportSubscription", json!({}))
        .await;
    assert_log_bucket_name(case, "us-east-1", &bucket(case, &created));
}

// ---------------------------------------------------------------------------
// Paging the read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_results_sizes_the_page_the_read_reports() {
    // Rows: the page size the request asks for. Every size that can hold a record must
    // report the whole collection in one page, whether the account has a subscription or
    // not — the collection never exceeds one record, so no page is ever cut short.
    let emulator = Emulator::new();
    for max_results in [1_i64, 2, 50, i64::from(i32::MAX)] {
        let case = format!("MaxResults {max_results}, nothing enabled");
        let (listed, next_token) =
            describe(&emulator, &case, json!({ "MaxResults": max_results })).await;
        assert!(
            listed.is_empty() && next_token.is_none(),
            "case {case:?}: a disabled account reports an empty page and no token, got \
             {listed:?} / {next_token:?}"
        );
    }

    let created = emulator
        .ok("enable", "CreateUsageReportSubscription", json!({}))
        .await;
    let created_bucket = bucket("enable", &created);

    for max_results in [1_i64, 2, 50, i64::from(i32::MAX)] {
        let case = format!("MaxResults {max_results}, one subscription");
        let (listed, next_token) =
            describe(&emulator, &case, json!({ "MaxResults": max_results })).await;
        assert_eq!(
            listed,
            vec![json!({ "S3BucketName": created_bucket, "Schedule": SCHEDULE })],
            "case {case:?}: the page must carry the account's subscription, got {listed:?}"
        );
        assert!(
            next_token.is_none(),
            "case {case:?}: the whole collection fits in the page, got {next_token:?}"
        );
    }

    // Rows: page sizes that cannot hold a record, and a pagination token this service
    // never issued. Both are refusals, not an empty answer that silently drops the
    // record.
    for max_results in [0_i64, -1, i64::from(i32::MIN)] {
        let case = format!("MaxResults {max_results}");
        assert_error(
            &emulator,
            &case,
            "DescribeUsageReportSubscriptions",
            json!({ "MaxResults": max_results }),
            400,
            "ValidationException",
        )
        .await;
    }

    for token in ["", "0", "not-a-token-this-service-minted"] {
        let case = format!("NextToken {token:?}");
        assert_error(
            &emulator,
            &case,
            "DescribeUsageReportSubscriptions",
            json!({ "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }
}
