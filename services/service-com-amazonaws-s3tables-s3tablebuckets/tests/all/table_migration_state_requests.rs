// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateTableMigration`.
//!
//! The rule under test: a request moves a migration only out of the states the
//! operation documents for the state it asks for — `queued` continues a
//! migration that is waiting for a plan review, and `cancelled` cancels one that
//! has not ended, which moves it to `cancelling` and from there to `cancelled` on
//! the clock. A request against a migration in any other state is the modelled
//! conflict and moves nothing; a request that addresses a migration by neither
//! whole form is the modelled client-input refusal.
//!
//! The table spans what the rule generalises over: the state requested, the state
//! the migration is in when the request arrives, and which of the two addressing
//! forms the request uses.

use serde_json::{json, Value};

use super::harness::{
    await_migration_state, served, start_migration_request, Emulator, Reply,
};

/// The fixed sentences this service states the classes below with.
const NOT_PENDING_REVIEW: &str = "The specified migration is not waiting for a plan review.";
const ALREADY_ENDED: &str = "The specified migration has already ended.";
const NO_MIGRATION: &str = "The specified migration does not exist.";
const ADDRESSING: &str = "Invalid input: Must provide either migrationId alone OR tableARN alone";

/// Create a table bucket holding one namespace, and answer its ARN.
async fn create_bucket(emulator: &Emulator, region: &str, bucket: &str) -> String {
    let created = served(
        emulator
            .create_table_bucket(region, &json!({ "name": bucket }))
            .await,
        "CreateTableBucket",
        bucket,
    );
    let bucket_arn = created.text("arn");
    served(
        emulator
            .create_namespace(region, &bucket_arn, &json!({ "namespace": ["sales"] }))
            .await,
        "CreateNamespace",
        bucket,
    );
    bucket_arn
}

/// Start a migration into `name` of `bucket_arn`, asking for a plan review when
/// `review` is set, and answer its id beside the destination table's ARN.
async fn start(
    emulator: &Emulator,
    region: &str,
    bucket_arn: &str,
    name: &str,
    review: bool,
    case: &str,
) -> (String, String) {
    let mut request = start_migration_request(bucket_arn, "sales", name);
    request
        .as_object_mut()
        .expect("a start request is an object")
        .insert("reviewMigrationPlan".to_string(), Value::Bool(review));
    let started = emulator.start_table_migration(region, &request).await;
    assert_eq!(
        started.status, 202,
        "case {case:?}: StartTableMigration answers its modelled 202, got {status} {body}",
        status = started.status,
        body = started.body
    );
    let table_arn = served(
        emulator.get_table(region, bucket_arn, "sales", name).await,
        "GetTable",
        case,
    )
    .text("tableARN");
    (started.text("migrationId"), table_arn)
}

/// Ask the migration `query` addresses to move to `requested`.
async fn request_state(
    emulator: &Emulator,
    region: &str,
    query: &[(&str, String)],
    requested: &str,
) -> Reply {
    emulator
        .update_table_migration(region, query, &json!({ "requestedState": requested }))
        .await
}

/// RULE: `queued` continues a migration waiting for a plan review, and the
/// migration goes on from there on the clock. Both addressing forms reach it.
#[tokio::test]
async fn a_migration_waiting_for_a_review_is_continued_by_queueing_it() {
    let region = "us-east-1";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-review").await;

    // The rows differ along the addressing form the request uses, which must
    // reach the same migration.
    for (case, name) in [("by id", "orders"), ("by destination table", "invoices")] {
        let (migration_id, table_arn) =
            start(&emulator, region, &bucket_arn, name, true, case).await;
        // The migration waits for the CALLER once planning has finished.
        let waiting =
            await_migration_state(&emulator, region, &migration_id, "pendingReview", case).await;
        assert_eq!(
            waiting.body.get("hasValidPlan"),
            Some(&Value::Bool(true)),
            "case {case:?}: a migration waiting for a review has finished planning, got {body}",
            body = waiting.body
        );

        let query = if case == "by id" {
            vec![("migrationId", migration_id.clone())]
        } else {
            vec![("tableARN", table_arn.clone())]
        };
        let continued = served(
            request_state(&emulator, region, &query, "queued").await,
            "UpdateTableMigration",
            case,
        );
        assert_eq!(
            (
                continued.text("migrationId"),
                continued.text("state"),
                continued.text("tableARN"),
            ),
            (migration_id.clone(), "queued".to_string(), table_arn),
            "case {case:?}: queueing a reviewed migration moves it to queued, got {body}",
            body = continued.body
        );

        // A migration that is no longer waiting for a review cannot be queued
        // again.
        let repeated = request_state(&emulator, region, &query, "queued").await;
        assert_eq!(
            (repeated.status, repeated.message()),
            (409, NOT_PENDING_REVIEW.to_string()),
            "case {case:?}: queueing a migration that is not waiting for a review is the \
             modelled conflict, got {status} {body}",
            status = repeated.status,
            body = repeated.body
        );

        // And it goes on from queued to its end on the clock.
        await_migration_state(&emulator, region, &migration_id, "succeeded", case).await;
    }
}

/// RULE: `cancelled` cancels a migration that has not ended — it moves to
/// `cancelling` and ends `cancelled` on the clock — and a migration that has
/// ended cannot be cancelled.
#[tokio::test]
async fn a_migration_that_has_not_ended_is_cancelled_and_then_ends_cancelled() {
    let region = "eu-west-2";
    let case = "cancel a running migration";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-cancel").await;
    let (migration_id, _) = start(&emulator, region, &bucket_arn, "orders", false, case).await;
    let query = vec![("migrationId", migration_id.clone())];

    let cancelling = served(
        request_state(&emulator, region, &query, "cancelled").await,
        "UpdateTableMigration",
        case,
    );
    // "the migration moves to cancelling, finishes the work in progress, and then
    // ends in the cancelled state".
    assert_eq!(
        cancelling.text("state"),
        "cancelling",
        "case {case:?}: cancelling a migration moves it to cancelling, got {body}",
        body = cancelling.body
    );
    await_migration_state(&emulator, region, &migration_id, "cancelled", case).await;

    let repeated = request_state(&emulator, region, &query, "cancelled").await;
    assert_eq!(
        (repeated.status, repeated.message()),
        (409, ALREADY_ENDED.to_string()),
        "case {case:?}: cancelling a migration that has ended is the modelled conflict, got \
         {status} {body}",
        status = repeated.status,
        body = repeated.body
    );
    let queued = request_state(&emulator, region, &query, "queued").await;
    assert_eq!(
        (queued.status, queued.message()),
        (409, NOT_PENDING_REVIEW.to_string()),
        "case {case:?}: queueing a migration that has ended is the modelled conflict, got \
         {status} {body}",
        status = queued.status,
        body = queued.body
    );
}

/// RULE: a request against a state the operation does not move a migration out
/// of, one that addresses no migration, and one naming a migration the store does
/// not hold are each refused — and each leaves the migration exactly as it was.
#[tokio::test]
async fn a_state_request_the_migration_does_not_admit_is_refused_and_moves_nothing() {
    let region = "sa-east-1";
    let case = "refusals";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-state-refusals").await;
    let (migration_id, table_arn) =
        start(&emulator, region, &bucket_arn, "orders", false, case).await;
    let query = vec![("migrationId", migration_id.clone())];

    // A migration that was never asked to wait for a review is not waiting for
    // one, so it cannot be continued.
    let queued = request_state(&emulator, region, &query, "queued").await;
    assert_eq!(
        (queued.status, queued.message()),
        (409, NOT_PENDING_REVIEW.to_string()),
        "case {case:?}: a migration that is not waiting for a review cannot be queued, got \
         {status} {body}",
        status = queued.status,
        body = queued.body
    );

    // The two addressing forms are whole and exclusive.
    for (form, query) in [
        ("neither member", vec![]),
        (
            "both members",
            vec![
                ("migrationId", migration_id.clone()),
                ("tableARN", table_arn.clone()),
            ],
        ),
    ] {
        let refused = request_state(&emulator, region, &query, "cancelled").await;
        assert_eq!(
            (refused.status, refused.message()),
            (400, ADDRESSING.to_string()),
            "case {case:?} ({form}): a request addressing no migration is refused, got {status} \
             {body}",
            status = refused.status,
            body = refused.body
        );
    }

    // An id nothing is stored under names no migration.
    let unknown = request_state(
        &emulator,
        region,
        &[("migrationId", "absent".to_string())],
        "cancelled",
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.message()),
        (404, NO_MIGRATION.to_string()),
        "case {case:?}: a migration the store does not hold is the modelled not-found, got \
         {status} {body}",
        status = unknown.status,
        body = unknown.body
    );

    // None of those refusals moved the migration: it is still running towards its
    // own end, which the clock is what moves it to.
    let read = served(
        emulator.get_table_migration(region, &query).await,
        "GetTableMigration",
        case,
    );
    assert!(
        !["cancelling", "cancelled"].contains(&read.text("state").as_str()),
        "case {case:?}: a refused request must move nothing, got {body}",
        body = read.body
    );
}
