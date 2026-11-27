// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetMaterializedViewRefreshTaskRun`: the read of ONE materialized
//! view refresh task run, addressed by the id a start minted.
//!
//! Three rules are pinned, each over a table of inputs: a well-formed id this account holds
//! no run under is the modelled `EntityNotFoundException` with its fixed sentence; an id
//! outside the member's modelled `@pattern` stays the constraint refusal the boundary
//! answers; and a run the store holds is reported from the store, member for member, by
//! every read of it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    service_state, MaterializedViewRefreshTaskRunEntry,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    call, constraint_message, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID,
};

/// The miss sentence this read answers an id no run carries with, written here
/// independently of the handler so this suite pins the wording — including its lack of a
/// closing full stop.
const RUN_NOT_FOUND: &str = "Materialized view refresh task run not found";

/// `UUIDv4`'s expression as the model declares it, minus the anchoring pair the shared
/// renderer drops from a quoted requirement
/// (`smithy_query(query="operation",operation="GetMaterializedViewRefreshTaskRun")` reports
/// `MaterializedViewRefreshTaskRunId` as `UUIDv4` with
/// `@pattern ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
const UUID_PATTERN: &str =
    "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";

/// The member path a constraint clause names the run id by, as the wire spells it.
const RUN_ID_MEMBER: &str = "materializedViewRefreshTaskRunId";

/// The instant the seeded runs below are stamped from: a whole number of seconds since the
/// epoch, so the timestamps a read reports compare exactly.
const SEEDED_EPOCH: Duration = Duration::from_secs(1_767_225_600);

/// `at` as the awsJson wire renders a timestamp: epoch seconds.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the seeded instants are after the epoch")
        .as_secs_f64()
}

/// The status and body of a read of `run_id` in the account's own Data Catalog.
async fn get(router: &MultiServiceRouter, run_id: &str) -> (u16, Value) {
    call(
        router,
        "GetMaterializedViewRefreshTaskRun",
        &json!({ "CatalogId": ACCOUNT_ID, "MaterializedViewRefreshTaskRunId": run_id }),
    )
    .await
}

/// A well-formed `MaterializedViewRefreshTaskRunId` this account holds no run under is the
/// modelled `EntityNotFoundException` with its fixed sentence — never a 501, and never a
/// server fault an SDK would retry.
///
/// The rows differ along the ids' case, their version and variant nibbles, and whether the
/// id is all zeroes or drawn at random.
#[tokio::test]
async fn a_run_id_naming_no_run_is_the_modelled_miss() {
    let router = emulator();

    for (case, run_id) in [
        ("the all-zero v4 id", "00000000-0000-4000-8000-000000000000"),
        ("a lowercase id", "7f3a1c2e-5b6d-4f80-9a1b-2c3d4e5f6a7b"),
        ("the same id in uppercase", "7F3A1C2E-5B6D-4F80-9A1B-2C3D4E5F6A7B"),
        ("a mixed-case id", "7f3A1c2E-5b6D-4F80-9a1B-2C3d4e5F6a7b"),
        ("an id whose every nibble is f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
    ] {
        let (status, body) = get(&router, run_id).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {run_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {run_id:?}, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            RUN_NOT_FOUND,
            "case {case}: unexpected wording for {run_id:?}, got {}",
            error_message(&body)
        );
    }
}

/// An id outside the member's modelled `@pattern` is a MALFORMED request and not a miss:
/// the constraint refusal the boundary validator answers, naming the member as the wire
/// spells it and quoting the expression the model declares.
///
/// The rows differ along how the id misses the expression: a word that is no id at all, a
/// group of the wrong length, a non-hex character, and the separators left out.
#[tokio::test]
async fn an_id_outside_the_modelled_pattern_stays_a_constraint_refusal() {
    let router = emulator();

    for (case, run_id) in [
        ("a word that is no id", "not-a-valid-uuid"),
        ("a group one character short", "7f3a1c2e-5b6d-4f80-9a1b-2c3d4e5f6a7"),
        ("a non-hex character", "7f3a1c2g-5b6d-4f80-9a1b-2c3d4e5f6a7b"),
        ("the separators left out", "7f3a1c2e5b6d4f809a1b2c3d4e5f6a7b"),
    ] {
        let (status, body) = get(&router, run_id).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {run_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected ValidationException for {run_id:?}, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            constraint_message(&body),
            format!(
                "1 validation error detected: Value '{run_id}' at '{RUN_ID_MEMBER}' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {UUID_PATTERN}"
            ),
            "case {case}: unexpected wording for {run_id:?}, got {}",
            constraint_message(&body)
        );
    }
}

/// A run the store holds is reported FROM the store: every member of the record comes back
/// on the read, and two reads of one run report the same run — a read moves nothing.
///
/// The rows differ along the status the run has reached, its refresh type, whether it has
/// finished (an end instant) or failed (an error message), and what its work consumed.
#[tokio::test]
async fn a_stored_run_is_reported_member_for_member() {
    let started_at = SystemTime::UNIX_EPOCH + SEEDED_EPOCH;

    let cases: Vec<(&str, MaterializedViewRefreshTaskRunEntry)> = vec![
        (
            "a run still starting",
            MaterializedViewRefreshTaskRunEntry {
                id: "11111111-1111-4111-8111-111111111111".to_string(),
                catalog_id: ACCOUNT_ID.to_string(),
                database_name: "analytics".to_string(),
                table_name: "daily_orders".to_string(),
                role: Some(format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-refresh")),
                status: "STARTING".to_string(),
                refresh_type: Some("FULL".to_string()),
                creation_time: started_at,
                start_time: started_at,
                last_updated: started_at,
                end_time: None,
                error_message: None,
                dpu_seconds: 0.0,
                processed_bytes: None,
            },
        ),
        (
            "a run that succeeded",
            MaterializedViewRefreshTaskRunEntry {
                id: "22222222-2222-4222-8222-222222222222".to_string(),
                catalog_id: ACCOUNT_ID.to_string(),
                database_name: "analytics".to_string(),
                table_name: "weekly_orders".to_string(),
                role: Some(format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-refresh")),
                status: "SUCCEEDED".to_string(),
                refresh_type: Some("INCREMENTAL".to_string()),
                creation_time: started_at,
                start_time: started_at,
                last_updated: started_at + Duration::from_secs(30),
                end_time: Some(started_at + Duration::from_secs(30)),
                error_message: None,
                dpu_seconds: 12.5,
                processed_bytes: Some(4_096),
            },
        ),
        (
            "a run that failed, carrying its reason",
            MaterializedViewRefreshTaskRunEntry {
                id: "33333333-3333-4333-8333-333333333333".to_string(),
                catalog_id: ACCOUNT_ID.to_string(),
                database_name: "reporting".to_string(),
                table_name: "revenue_mv".to_string(),
                role: None,
                status: "FAILED".to_string(),
                refresh_type: None,
                creation_time: started_at,
                start_time: started_at + Duration::from_secs(1),
                last_updated: started_at + Duration::from_secs(10),
                end_time: Some(started_at + Duration::from_secs(10)),
                error_message: Some("the source table could not be read".to_string()),
                dpu_seconds: 0.0,
                processed_bytes: Some(0),
            },
        ),
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    service_state(&*engine)
        .expect("the Glue store is reachable")
        .lock()
        .materialized_view_refresh_task_runs
        .extend(cases.iter().map(|(_, run)| run.clone()));

    for (case, run) in &cases {
        let body = ok(
            &router,
            "GetMaterializedViewRefreshTaskRun",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "MaterializedViewRefreshTaskRunId": run.id,
            }),
        )
        .await;
        let reported = body
            .get("MaterializedViewRefreshTaskRun")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the read must report the run, got {body}"));

        for (member, expected) in [
            ("CustomerId", json!(ACCOUNT_ID)),
            ("MaterializedViewRefreshTaskRunId", json!(run.id)),
            ("CatalogId", json!(run.catalog_id)),
            ("DatabaseName", json!(run.database_name)),
            ("TableName", json!(run.table_name)),
            ("Status", json!(run.status)),
            ("Role", json!(run.role)),
            ("RefreshType", json!(run.refresh_type)),
            ("ErrorMessage", json!(run.error_message)),
            ("ProcessedBytes", json!(run.processed_bytes)),
            ("DPUSeconds", json!(run.dpu_seconds)),
        ] {
            let actual = reported.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case}: {member} must be reported as stored, expected {expected}, got \
                 {actual} in {reported}"
            );
        }

        for (member, expected) in [
            ("CreationTime", Some(epoch_seconds(run.creation_time))),
            ("StartTime", Some(epoch_seconds(run.start_time))),
            ("LastUpdated", Some(epoch_seconds(run.last_updated))),
            ("EndTime", run.end_time.map(epoch_seconds)),
        ] {
            let actual = reported.get(member).and_then(Value::as_f64);
            assert_eq!(
                actual, expected,
                "case {case}: {member} must be reported as stored, expected {expected:?}, got \
                 {actual:?} in {reported}"
            );
        }

        // A read reports; it moves nothing. The second read of the same run answers the
        // same document, so nothing about the run was consumed by reading it.
        let again = ok(
            &router,
            "GetMaterializedViewRefreshTaskRun",
            &json!({
                "CatalogId": ACCOUNT_ID,
                "MaterializedViewRefreshTaskRunId": run.id,
            }),
        )
        .await;
        assert_eq!(
            again, body,
            "case {case}: a second read must report the same run, got {again} after {body}"
        );
    }
}

/// The `CatalogId` is resolved before the run is looked up: a catalog of ANOTHER account is
/// refused as unauthorized, a catalog this account holds nothing under is the catalog
/// miss, and an id that is no catalog id at all is refused as invalid input — so a run id is
/// never answered out of a catalog the caller cannot read.
#[tokio::test]
async fn the_catalog_is_resolved_before_the_run_is_looked_up() {
    let router = emulator();
    let run_id = "44444444-4444-4444-8444-444444444444";

    for (case, catalog_id, code) in [
        (
            "another account's Data Catalog",
            "111122223333".to_string(),
            "AccessDeniedException",
        ),
        (
            "a catalog of another account",
            "111122223333:analytics".to_string(),
            "AccessDeniedException",
        ),
        (
            "a catalog this account holds nothing under",
            format!("{ACCOUNT_ID}:never-created"),
            "EntityNotFoundException",
        ),
        (
            "an id carrying no account-id prefix, which is no catalog id at all",
            "never-created".to_string(),
            "InvalidInputException",
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetMaterializedViewRefreshTaskRun",
            &json!({
                "CatalogId": catalog_id,
                "MaterializedViewRefreshTaskRunId": run_id,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {catalog_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(code),
            "case {case}: expected {code} for {catalog_id:?}, got {} in {body}",
            error_code(&body)
        );
        assert_ne!(
            error_message(&body),
            RUN_NOT_FOUND,
            "case {case}: a catalog refusal must not read as the run miss, got {body}"
        );
    }
}

/// The catalog is part of a run's ADDRESS, not only a gate in front of it: a run recorded in
/// one catalog is not an answer to a read that names another, which is reported as the run
/// miss — while the same id read through the catalog that holds the run answers it.
#[tokio::test]
async fn a_run_of_another_catalog_is_not_an_answer() {
    let run_id = "55555555-5555-4555-8555-555555555555";
    let holding_catalog_name = "holding-catalog";
    // A `CatalogId` that SELECTS a Data Catalog carries the holding account's id prefix, so
    // a catalog registered inside this account is addressed by that id and not by its bare
    // name (see `crate::state::is_data_catalog_id`).
    let holding_catalog = format!("{ACCOUNT_ID}:{holding_catalog_name}");
    let at = SystemTime::UNIX_EPOCH + SEEDED_EPOCH;

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    // The catalog that holds the run is registered the only way `CreateCatalog` registers
    // one: a federation pointing at a source one of AWS's own managed integrations
    // registered.
    ok(
        &router,
        "CreateCatalog",
        &json!({
            "Name": holding_catalog_name,
            "CatalogInput": {
                "FederatedCatalog": {
                    "Identifier": format!("arn:aws:s3tables:us-east-1:{ACCOUNT_ID}:bucket/held"),
                    "ConnectionName": "aws:s3tables",
                },
            },
        }),
    )
    .await;
    service_state(&*engine)
        .expect("the Glue store is reachable")
        .lock()
        .materialized_view_refresh_task_runs
        .push(MaterializedViewRefreshTaskRunEntry {
            id: run_id.to_string(),
            catalog_id: holding_catalog.clone(),
            database_name: "analytics".to_string(),
            table_name: "daily_orders".to_string(),
            role: None,
            status: "RUNNING".to_string(),
            refresh_type: Some("FULL".to_string()),
            creation_time: at,
            start_time: at,
            last_updated: at,
            end_time: None,
            error_message: None,
            dpu_seconds: 0.0,
            processed_bytes: None,
        });

    // The account's own Data Catalog HOLDS that catalog but is not it, so the run is not
    // addressed by it.
    let (status, body) = get(&router, run_id).await;
    assert_eq!(
        status, 400,
        "case the account's own Data Catalog: expected 400 for a run of {holding_catalog:?}, got \
         {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case the account's own Data Catalog: expected EntityNotFoundException, got {} in {body}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        RUN_NOT_FOUND,
        "case the account's own Data Catalog: expected the run miss, got {}",
        error_message(&body)
    );

    // Read through the catalog that holds it, the same id answers the run.
    let reported = ok(
        &router,
        "GetMaterializedViewRefreshTaskRun",
        &json!({
            "CatalogId": holding_catalog,
            "MaterializedViewRefreshTaskRunId": run_id,
        }),
    )
    .await;
    assert_eq!(
        reported["MaterializedViewRefreshTaskRun"]["CatalogId"].as_str(),
        Some(holding_catalog.as_str()),
        "case the holding catalog: the read must report the run of that catalog, got {reported}"
    );
    assert_eq!(
        reported["MaterializedViewRefreshTaskRun"]["MaterializedViewRefreshTaskRunId"].as_str(),
        Some(run_id),
        "case the holding catalog: the read must report the run addressed, got {reported}"
    );
}
