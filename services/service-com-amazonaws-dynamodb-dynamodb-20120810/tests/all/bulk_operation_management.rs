// SPDX-License-Identifier: CC0-1.0

//! The bulk-operation MANAGEMENT calls: `DescribeBulkOperation` and `CancelBulkOperation`,
//! beside the listing and the start that feed them.
//!
//! RULE under test: every operation target this endpoint BINDS is routed to its own handler
//! and answered with that operation's own response — its validation refusal, its
//! not-found refusal, or the resource it reports — and only a target the endpoint binds
//! NOTHING for is answered with the front door's unknown-operation class. So a value that is
//! no bulk-operation ARN is a `ValidationException` naming that fault, an ARN the account
//! holds no operation under is a `ResourceNotFoundException`, a reader reports the operation
//! from the catalogue, and a cancel stops it — all four distinguishable from "this API does
//! not exist".
//!
//! The rows span what the rule generalises over: WHICH of the bound targets was named (both
//! management calls, each driven through every row), HOW the addressed value fails the ARN
//! form (no ARN at all, an empty value, too few fields, an empty field, another service's
//! ARN, another resource type, a mis-spelled resource type, no resource id), and WHICH state
//! the addressed operation is in (absent, running, cancelling, finished).
//!
//! What a cancel did is read back through `DescribeBulkOperation` AND `ListBulkOperations` —
//! the two readers a client observes a bulk operation with — so the assertions are about
//! STORED state rather than about what the cancel echoed. The transitional status the clock
//! owes is observed by POLLING for the condition under a generous deadline, never by
//! sleeping a fixed time.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::{
    InputS3Configuration, OutputS3Configuration,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    self, BulkOperation, STATUS_TRANSITION,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::ComAmazonawsDynamodbDynamodb20120810State;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    call, create_bucket, emulator_and_engine, emulator_with_s3, error_code, error_type, message,
    ok, put_object, validation_envelope, REGION,
};
use crate::table_items::create_active_table;

/// The table every seeded operation acts on.
const TABLE: &str = "BulkOperationManagement";

/// The `__type` the front door answers a target it binds NOTHING for, which is what these
/// calls used to be answered with and what the control row still is.
const UNKNOWN_OPERATION_TYPE: &str = "com.amazon.coral.service#UnknownOperationException";

/// The `__type` the service framework names a refusal of a request's own contents by.
const VALIDATION_TYPE: &str = "com.amazon.coral.validate#ValidationException";

/// The `__type` a modelled error of this service is named by on the wire: the shape id,
/// qualified by the API version.
const NOT_FOUND_TYPE: &str = "com.amazonaws.dynamodb.v20120810#ResourceNotFoundException";

/// The two targets this endpoint binds for the management of one bulk operation, each row
/// below driven through both.
const MANAGEMENT_CALLS: [&str; 2] = ["DescribeBulkOperation", "CancelBulkOperation"];

/// How long a poll for a clock-owed transition may take before the test calls it a defect:
/// generously more than the span itself ([`STATUS_TRANSITION`]), so a loaded or slow host is
/// not one.
fn settle_deadline() -> Duration {
    STATUS_TRANSITION * 10 + Duration::from_secs(10)
}

/// How often such a poll looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The emulator with its engine in hand, and [`TABLE`] `ACTIVE`, so a suite row can seed a
/// bulk operation in whichever state it is about.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_and_engine();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    (router, engine)
}

/// Record one bulk operation in the store, reaching the same catalogue `StartBulkPutItems`
/// writes into.
///
/// `began` is the instant it began and `ends_at` the instant it is due to end — so a row
/// can address an operation that is still running (an instant ahead of now) or one that has
/// already reached `settles_into` (one behind now), without waiting for either. The ARN is
/// minted by the service's own minter, so no row transcribes the form of one.
fn seed_operation(
    engine: &Arc<engine::InMemoryEngine>,
    began: SystemTime,
    ends_at: SystemTime,
    settles_into: &str,
) -> String {
    let arn = state::new_bulk_operation_arn(REGION, began);
    let operation = BulkOperation {
        arn: arn.clone(),
        region: REGION.to_string(),
        table_name: TABLE.to_string(),
        table_id: "00000000-0000-4000-8000-000000000001".to_string(),
        client_token: None,
        input_s3_configuration: InputS3Configuration {
            s3_bucket: "bulk-operation-management-source".to_string(),
            input_format: "DYNAMODB_JSON".to_string(),
            ..InputS3Configuration::default()
        },
        output_s3_configuration: OutputS3Configuration {
            s3_bucket: Some("bulk-operation-management-outcomes".to_string()),
            output_type: vec!["ERROR".to_string()],
            ..OutputS3Configuration::default()
        },
        max_write_request_units: None,
        performance_tier: None,
        processed_item_count: 7,
        written_item_count: 7,
        failed_item_count: 0,
        processed_input_bytes: 512,
        bulk_put_bytes: 448,
        failure: None,
        started_at: began,
        ended_at: ends_at,
        cancelled_at: None,
        status: settles_into.to_string(),
    };
    engine
        .state::<ComAmazonawsDynamodbDynamodb20120810State>()
        .lock()
        .insert_bulk_operation(operation);
    arn
}

/// One bulk operation still doing its work: due to end well after this test can read it.
fn seed_running(engine: &Arc<engine::InMemoryEngine>) -> String {
    let now = SystemTime::now();
    seed_operation(
        engine,
        now - Duration::from_secs(1),
        now + settle_deadline(),
        state::BULK_OPERATION_STATUS_COMPLETED,
    )
}

/// One that has already finished, in the status it settled into.
fn seed_settled(engine: &Arc<engine::InMemoryEngine>, settles_into: &str) -> String {
    let now = SystemTime::now();
    seed_operation(
        engine,
        now - Duration::from_secs(60),
        now - Duration::from_secs(30),
        settles_into,
    )
}

/// The description a management call answered.
fn described(answered: &Value) -> Value {
    answered["BulkOperationDescription"].clone()
}

/// The status `DescribeBulkOperation` reports for `arn`.
async fn reported_status(router: &MultiServiceRouter, arn: &str) -> Value {
    let answered = ok(
        router,
        "DescribeBulkOperation",
        json!({"BulkOperationArn": arn}),
    )
    .await;
    described(&answered)["BulkOperationStatus"].clone()
}

/// The summary `ListBulkOperations` reports for `arn`, or `Value::Null` where it reports
/// none.
async fn listed_summary(router: &MultiServiceRouter, arn: &str) -> Value {
    let page = ok(router, "ListBulkOperations", json!({})).await;
    page["BulkOperationsSummaryList"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["BulkOperationArn"] == json!(arn))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Poll `read` until it answers `expected`, or fail naming the last value it answered.
///
/// Waits for the CONDITION rather than for a span: the transition a cancel owes is due at an
/// instant the record carries, and a host that answers slowly must not change the verdict.
async fn poll_until(router: &MultiServiceRouter, case: &str, arn: &str, expected: &Value) -> Value {
    let deadline = std::time::Instant::now() + settle_deadline();
    loop {
        let read = reported_status(router, arn).await;
        if &read == expected {
            return read;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the status to reach {expected:?}, last {read:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_value_that_addresses_no_bulk_operation_is_refused_as_that_fault() {
    let (router, _engine) = seeded().await;

    // Each row: the case, and the value the request addresses. They span the ways a value
    // fails the form a bulk operation's ARN takes. Every row is at least as long as the
    // member's own minimum (the length bound below), so what refuses it is the form and not
    // that bound.
    let rows: Vec<(&str, Value)> = vec![
        (
            "no ARN at all",
            json!("not-an-arn-nor-anything-else-of-that-shape"),
        ),
        (
            "the ARN prefix with no fields after it",
            json!("arn:no-fields-after-the-prefix-at-all-here"),
        ),
        (
            "too few fields",
            json!("arn:aws:dynamodb:us-east-1:000000000000"),
        ),
        (
            "an empty region field",
            json!("arn:aws:dynamodb::000000000000:bulk-operation/1"),
        ),
        (
            "an empty account field",
            json!("arn:aws:dynamodb:us-east-1::bulk-operation/1"),
        ),
        (
            "another service's ARN",
            json!("arn:aws:s3:us-east-1:000000000000:bulk-operation/1"),
        ),
        (
            "this service's ARN naming another resource type",
            json!(format!("arn:aws:dynamodb:{REGION}:000000000000:table/{TABLE}")),
        ),
        (
            "a mis-spelled resource type",
            json!("arn:aws:dynamodb:us-east-1:000000000000:bulkoperation/1"),
        ),
        (
            "no resource id after the type",
            json!("arn:aws:dynamodb:us-east-1:000000000000:bulk-operation/"),
        ),
        (
            "no resource path at all",
            json!("arn:aws:dynamodb:us-east-1:000000000000:bulk-operation"),
        ),
    ];

    for (case, addressed) in rows {
        for operation in MANAGEMENT_CALLS {
            let (status, answered) =
                call(&router, operation, json!({"BulkOperationArn": addressed})).await;
            assert_eq!(
                (status, error_type(&answered)),
                (400, VALIDATION_TYPE.to_string()),
                "case {case:?} ({operation}): expected 400 {VALIDATION_TYPE}, got {status} with \
                 {answered}"
            );
            let stated = message(&answered);
            assert_eq!(
                stated,
                state::INVALID_BULK_OPERATION_ARN,
                "case {case:?} ({operation}): expected the message \
                 {:?}, got {stated:?}",
                state::INVALID_BULK_OPERATION_ARN
            );
        }
    }

    // A value outside the LENGTH the member declares is refused by that bound instead, in
    // the clause the validating walk states for it — the member declares it
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BulkOperationArn")` gives
    // `@length` 37..1024), so no handler reads it: the empty value and one character short of
    // the minimum are below it, and a value one past 1024 characters is above it.
    let too_long = format!(
        "arn:aws:dynamodb:{REGION}:000000000000:bulk-operation/{}",
        "a".repeat(1025)
    );
    for (case, addressed, requirement) in [
        (
            "the empty value",
            String::new(),
            "Member must have length greater than or equal to 37",
        ),
        (
            "one character short of the minimum",
            "a".repeat(36),
            "Member must have length greater than or equal to 37",
        ),
        (
            "one character past the maximum",
            too_long,
            "Member must have length less than or equal to 1024",
        ),
    ] {
        for operation in MANAGEMENT_CALLS {
            let (status, answered) = call(
                &router,
                operation,
                json!({"BulkOperationArn": addressed.clone()}),
            )
            .await;
            let stated = message(&answered);
            assert_eq!(
                (status, error_type(&answered)),
                (400, VALIDATION_TYPE.to_string()),
                "case {case:?} ({operation}): expected 400 {VALIDATION_TYPE}, got {status} with \
                 {answered}"
            );
            assert!(
                stated.contains(&format!(
                    "at 'bulkOperationArn' failed to satisfy constraint: {requirement}"
                )),
                "case {case:?} ({operation}): expected the clause stating {requirement:?}, got \
                 {stated:?}"
            );
        }
    }

    // The control for every row above: a target this endpoint really binds NOTHING for is
    // still the front door's unknown-operation class, so the rows are about the request and
    // not about the whole family being unrouted.
    for unbound in ["DescribeBulkOperations", "StopBulkOperation"] {
        let (status, answered) = call(
            &router,
            unbound,
            json!({"BulkOperationArn": "not-an-arn"}),
        )
        .await;
        assert_eq!(
            (status, error_type(&answered)),
            (400, UNKNOWN_OPERATION_TYPE.to_string()),
            "case an unbound target {unbound:?}: expected the front door's own refusal, got \
             {status} with {answered}"
        );
    }
}

#[tokio::test]
async fn a_request_that_addresses_no_operation_at_all_is_refused_as_the_absent_member() {
    let (router, _engine) = seeded().await;
    let expected = state::absent_member_message(state::BULK_OPERATION_ARN_MEMBER_PATH);

    // Each row: the case and the whole body. A body stating no value for the member, and one
    // stating it `null`, are the same absence.
    for (case, body) in [
        ("a body stating no member at all", json!({})),
        ("a body stating the member null", json!({"BulkOperationArn": null})),
        (
            "a body stating only another member",
            json!({"TableName": TABLE}),
        ),
    ] {
        for operation in MANAGEMENT_CALLS {
            let (status, answered) = call(&router, operation, body.clone()).await;
            assert_eq!(
                (status, error_type(&answered)),
                (400, VALIDATION_TYPE.to_string()),
                "case {case:?} ({operation}): expected 400 {VALIDATION_TYPE}, got {status} with \
                 {answered}"
            );
            let stated = message(&answered);
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_arn_the_account_holds_no_operation_under_is_not_found() {
    let (router, engine) = seeded().await;
    // One operation the account DOES hold, so the rows below are about the ARN they address
    // and not about an empty catalogue.
    let held = seed_running(&engine);

    // Each row: the case and the well-formed ARN it addresses. They span an ARN of this
    // endpoint's own region, one of another region, and one of another account.
    let minted = state::new_bulk_operation_arn(REGION, SystemTime::now());
    let elsewhere = state::new_bulk_operation_arn("eu-west-1", SystemTime::now());
    let other_account = "arn:aws:dynamodb:us-east-1:111122223333:bulk-operation/00000000-abcdef12";
    let rows: Vec<(&str, String)> = vec![
        ("an ARN of this region the catalogue holds nothing under", minted),
        ("an ARN of another region", elsewhere),
        ("an ARN of another account", other_account.to_string()),
    ];

    for (case, addressed) in rows {
        for operation in MANAGEMENT_CALLS {
            let (status, answered) =
                call(&router, operation, json!({"BulkOperationArn": addressed})).await;
            assert_eq!(
                (status, error_type(&answered)),
                (400, NOT_FOUND_TYPE.to_string()),
                "case {case:?} ({operation}): expected 400 {NOT_FOUND_TYPE}, got {status} with \
                 {answered}"
            );
            let expected = format!("Requested resource not found: Bulk operation: {addressed} not found");
            let stated = message(&answered);
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
            );
        }
    }

    // And the operation the account DOES hold is reported rather than refused.
    let read = reported_status(&router, &held).await;
    assert_eq!(
        read,
        json!(state::BULK_OPERATION_STATUS_IN_PROGRESS),
        "the operation the catalogue holds is reported, got {read:?}"
    );
}

#[tokio::test]
async fn a_started_bulk_operation_is_reported_by_its_own_describe() {
    let router = emulator_with_s3();
    let bucket = "bulk-operation-describe-source";
    create_bucket(&router, bucket).await;
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    put_object(
        &router,
        bucket,
        "described/data.json",
        format!("{}\n", json!({"Item": {"pk": {"S": "one"}}})).into_bytes(),
    )
    .await;

    let started = described(
        &ok(
            &router,
            "StartBulkPutItems",
            json!({
                "TableName": TABLE,
                "InputS3Configuration": {
                    "S3Bucket": bucket,
                    "S3KeyPrefix": "described/",
                    "InputFormat": "DYNAMODB_JSON",
                },
                "OutputS3Configuration": {
                    "S3Bucket": bucket,
                    "S3KeyPrefix": "outcomes/",
                    "OutputType": ["ERROR"],
                },
            }),
        )
        .await,
    );
    let arn = started["BulkOperationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The reader answers the operation the start recorded, member for member: the same ARN,
    // the same table and the same source, read back out of the catalogue.
    let read = described(
        &ok(
            &router,
            "DescribeBulkOperation",
            json!({"BulkOperationArn": arn}),
        )
        .await,
    );
    assert_eq!(
        read["BulkOperationArn"], started["BulkOperationArn"],
        "the reader answers the operation that was started, got {read}"
    );
    assert_eq!(
        read["TableDetails"][0]["TableName"],
        json!(TABLE),
        "the reader names the table the operation acts on, got {read}"
    );
    assert_eq!(
        read["InputS3Configuration"]["S3Bucket"],
        json!(bucket),
        "the reader answers the source the operation reads, got {read}"
    );
    assert!(
        read["StartTime"].is_number(),
        "the reader answers the instant the operation began, got {read}"
    );

    // And the listing reports the same operation, so the two readers agree.
    let summary = listed_summary(&router, &arn).await;
    assert_eq!(
        summary["BulkOperationStatus"], read["BulkOperationStatus"],
        "the two readers report one status, got {summary} and {read}"
    );
}

#[tokio::test]
async fn a_cancel_stops_the_operation_and_every_reader_reports_it() {
    let (router, engine) = seeded().await;
    let arn = seed_running(&engine);

    // Before the cancel the operation is doing its work.
    let before = reported_status(&router, &arn).await;
    assert_eq!(
        before,
        json!(state::BULK_OPERATION_STATUS_IN_PROGRESS),
        "a running operation reports its work, got {before:?}"
    );

    // The cancel answers the operation's ARN and the status it stands in, which is the whole
    // output the model declares for it
    // (`smithy_query(query="operation", operation="CancelBulkOperation")`).
    let cancelled = ok(
        &router,
        "CancelBulkOperation",
        json!({"BulkOperationArn": arn}),
    )
    .await;
    assert_eq!(
        cancelled["BulkOperationArn"],
        json!(arn),
        "the cancel answers the operation it stopped, got {cancelled}"
    );
    assert_eq!(
        cancelled["BulkOperationStatus"],
        json!(state::BULK_OPERATION_STATUS_CANCELLING),
        "stopping work takes a span, so the cancel answers the operation stopping, got \
         {cancelled}"
    );

    // What the cancel DID is read back out of both readers rather than off its own answer.
    let read = reported_status(&router, &arn).await;
    assert_eq!(
        read,
        json!(state::BULK_OPERATION_STATUS_CANCELLING),
        "the reader reports the cancel, got {read:?}"
    );
    let summary = listed_summary(&router, &arn).await;
    assert_eq!(
        summary["BulkOperationStatus"],
        json!(state::BULK_OPERATION_STATUS_CANCELLING),
        "the listing reports the cancel, got {summary}"
    );

    // A REPEATED cancel asks for the cancellation that is already under way: it answers the
    // same operation and does not start the span again.
    let again = ok(
        &router,
        "CancelBulkOperation",
        json!({"BulkOperationArn": arn}),
    )
    .await;
    assert_eq!(
        again["BulkOperationArn"],
        json!(arn),
        "a repeated cancel answers the one operation, got {again}"
    );

    // And the transition the clock owes lands: the operation settles CANCELLED and reports
    // the instant it stopped.
    poll_until(
        &router,
        "a cancelled operation settles",
        &arn,
        &json!(state::BULK_OPERATION_STATUS_CANCELLED),
    )
    .await;
    let settled = described(
        &ok(
            &router,
            "DescribeBulkOperation",
            json!({"BulkOperationArn": arn}),
        )
        .await,
    );
    assert!(
        settled["EndTime"].is_number(),
        "a stopped operation reports the instant it ended, got {settled}"
    );
    let summary = listed_summary(&router, &arn).await;
    assert_eq!(
        summary["BulkOperationStatus"],
        json!(state::BULK_OPERATION_STATUS_CANCELLED),
        "the listing reports the status it settled into, got {summary}"
    );
}

#[tokio::test]
async fn a_cancel_of_work_that_is_over_is_refused_and_changes_nothing() {
    let (router, engine) = seeded().await;

    // Each row: the terminal status the seeded operation already reached. Stopping work that
    // is over changes nothing, whichever way it ended.
    for settled in [
        state::BULK_OPERATION_STATUS_COMPLETED,
        state::BULK_OPERATION_STATUS_FAILED,
    ] {
        let arn = seed_settled(&engine, settled);
        let (status, answered) = call(
            &router,
            "CancelBulkOperation",
            json!({"BulkOperationArn": arn}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {settled:?}: expected a refusal, got {status} with {answered}"
        );
        let expected = format!(
            "Bulk operation {arn} has already reached status {settled} and cannot be cancelled"
        );
        let stated = message(&answered);
        assert_eq!(
            stated, expected,
            "case {settled:?}: expected the message {expected:?}, got {stated:?}"
        );

        // The refused cancel left the operation exactly as it was.
        let read = reported_status(&router, &arn).await;
        assert_eq!(
            read,
            json!(settled),
            "case {settled:?}: a refused cancel changes nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_the_regions_operations_and_the_page_size_it_asks_for() {
    let (router, engine) = seeded().await;
    let running = seed_running(&engine);

    // The listing takes NO member that narrows it
    // (`smithy_query(query="operation", operation="ListBulkOperations")` gives `MaxResults`
    // and `NextToken` and nothing else), so every bulk operation of the region the endpoint
    // serves is reported.
    let listed = listed_summary(&router, &running).await;
    assert_eq!(
        listed["BulkOperationArn"],
        json!(running),
        "the listing reports the region's operations, got {listed}"
    );

    // The page size is held to the range the model declares for the member
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ListBulkOperationsMaxLimit")`
    // gives `@range` 1..25, which is the bound AWS's own refusal of this member states):
    // each published bound is served, and one past either end is refused, naming the member
    // and the bound it broke.
    for served in [1, 25] {
        let page = ok(
            &router,
            "ListBulkOperations",
            json!({"MaxResults": served}),
        )
        .await;
        assert!(
            page["BulkOperationsSummaryList"].is_array(),
            "case MaxResults {served}: a page size at the bound is served, got {page}"
        );
    }
    for (refused, requirement) in [
        (0, "Member must have value greater than or equal to 1"),
        (-1, "Member must have value greater than or equal to 1"),
        (26, "Member must have value less than or equal to 25"),
        (101, "Member must have value less than or equal to 25"),
    ] {
        let (status, answered) = call(
            &router,
            "ListBulkOperations",
            json!({"MaxResults": refused}),
        )
        .await;
        assert_eq!(
            (status, error_type(&answered)),
            (400, VALIDATION_TYPE.to_string()),
            "case MaxResults {refused}: expected 400 {VALIDATION_TYPE}, got {status} with \
             {answered}"
        );
        let expected = validation_envelope(&[format!(
            "Value '{refused}' at 'maxResults' failed to satisfy constraint: {requirement}"
        )]);
        let stated = message(&answered);
        assert_eq!(
            stated, expected,
            "case MaxResults {refused}: expected the message {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_cursor_is_held_to_the_form_its_member_models() {
    let (router, engine) = seeded().await;
    for _ in 0..3 {
        seed_running(&engine);
    }

    // A value outside the form the member models is refused by that bound, naming the
    // member, echoing the value and stating the expression it must satisfy
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BulkOperationNextToken")`
    // gives `@length` 1..1024 and `@pattern ^([0-9a-f]{16})+$`, which a refusal states
    // without its anchors). The rows span the ways a value falls outside it, and the empty
    // value \u2014 which is below the member's own minimum as well \u2014 earns both clauses.
    for (case, cursor, clauses) in [
        ("a value that is not hex at all", "not-a-cursor", 1),
        ("hex in the other case", "ABCDEF0123456789", 1),
        ("fewer digits than one group", "00112233", 1),
        ("a part-filled second group", "0011223344556677aabb", 1),
        ("the empty value", "", 2),
    ] {
        let (status, answered) = call(
            &router,
            "ListBulkOperations",
            json!({"NextToken": cursor}),
        )
        .await;
        let pattern = format!(
            "Value '{cursor}' at 'nextToken' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: ([0-9a-f]{{16}})+"
        );
        let expected = if clauses == 2 {
            validation_envelope(&[
                format!(
                    "Value '{cursor}' at 'nextToken' failed to satisfy constraint: Member must \
                     have length greater than or equal to 1"
                ),
                pattern,
            ])
        } else {
            validation_envelope(&[pattern])
        };
        let stated = message(&answered);
        assert_eq!(
            (status, error_type(&answered), stated.clone()),
            (400, VALIDATION_TYPE.to_string(), expected.clone()),
            "case {case:?}: expected 400 {VALIDATION_TYPE} with {expected:?}, got {status} with \
             {answered}"
        );
    }

    // A value that TAKES that form and is still no cursor of this operation's stands for no
    // page, and is refused as that rather than quietly taken for the first one.
    for (case, cursor) in [
        ("a group of bytes no cursor is written in", "ffffffffffffffff"),
        ("a group of empty bytes", "0000000000000000"),
    ] {
        let (status, answered) = call(
            &router,
            "ListBulkOperations",
            json!({"NextToken": cursor}),
        )
        .await;
        let stated = message(&answered);
        assert_eq!(
            (status, error_code(&answered).as_str(), stated.as_str()),
            (400, "ValidationException", "Invalid NextToken"),
            "case {case:?}: expected the cursor refusal, got {status} with {answered}"
        );
    }

    // And the cursor this operation MINTS is one the next request may send straight back:
    // it takes the member's own form, so the page it stands for is served rather than
    // refused, and paging reports each operation once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({"MaxResults": 1});
        if let Some(cursor) = token.as_deref() {
            body["NextToken"] = json!(cursor);
        }
        let page = ok(&router, "ListBulkOperations", body).await;
        seen.extend(
            page["BulkOperationsSummaryList"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|summary| summary["BulkOperationArn"].as_str())
                .map(ToString::to_string),
        );
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        (seen.len(), unique.len()),
        (3, 3),
        "paging by a minted cursor reports each operation once, got {seen:?}"
    );
}
