// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE conflict a column statistics task run creates: while a
//! run of a table has not finished, every Glue operation that models
//! `ColumnStatisticsTaskRunningException` refuses a call addressing that table.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: while a column statistics task run of a table is in flight, each
//! operation that models `ColumnStatisticsTaskRunningException` —
//! `StartColumnStatisticsTaskRun` and `CreateColumnStatisticsTaskSettings`, the only two
//! the model declares it for — refuses a call addressing THAT table with HTTP 400, that
//! code, and that error class's own fixed message, persisting nothing; a call addressing
//! any other table is served; and the same call is served once the run has reached a
//! terminal state — whether the clock finished it or `StopColumnStatisticsTaskRun` did.
//!
//! The recorded cases — a second start of the busy table, and a create of settings for it
//! — are two rows of the table below; the rows differ along which operation is called,
//! which optional members the call names, how many columns it lists, the sample size, the
//! casing of the database and table names, and whether the call names a catalog id.
//!
//! The due instant the run lifecycle turns on is moved THROUGH THE STORE rather than slept
//! for, so the in-flight window is deterministic.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{
    COLUMN_STATISTICS_TASK_NOT_RUNNING, COLUMN_STATISTICS_TASK_RUN_ALREADY_STARTED,
    COLUMN_STATISTICS_TASK_SETTINGS_NOT_FOUND, COLUMN_STATISTICS_TASK_SETTINGS_RUN_ALREADY_STARTED,
};
use service_com_amazonaws_glue_awsglue::state::COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED;
use smithy_server_runtime::MultiServiceRouter;

// The run lifecycle's clock helpers live with the suite that owns that lifecycle, so this
// suite moves the clock through the same ones rather than a second copy.
use crate::column_statistics_task_runs::{already_past, far_future, set_due_instant};
use crate::wire::{call, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The IAM role a start and a create both name.
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// A cron expression of the documented form (`Schedule$ScheduleExpression`).
const DAILY_SCHEDULE: &str = "cron(15 12 * * ? *)";

/// The database every row below addresses, and the table whose run is in flight.
const BUSY_DATABASE: &str = "conflict_db";
const BUSY_TABLE: &str = "conflict_table";

/// A table of the same database that no run covers.
const QUIET_TABLE: &str = "quiet_table";

/// An emulator plus the engine it runs on, so the test can move the due instant the run
/// lifecycle turns on instead of waiting for it.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Store `database` holding `table` with two string columns, and answer nothing.
async fn store_table(router: &MultiServiceRouter, database: &str, table: &str) {
    let (status, parsed) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": format!("s3://bucket/{}/", table.to_lowercase()),
                    "Columns": [
                        { "Name": "a", "Type": "string" },
                        { "Name": "b", "Type": "string" },
                    ],
                },
            },
        }),
    )
    .await;
}

/// A body naming the three members both operations require, plus whatever `extra` adds.
fn body_for(database: &str, table: &str, extra: Value) -> Value {
    let mut body = json!({ "DatabaseName": database, "TableName": table, "Role": ROLE });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The fixed message the named operation's in-flight refusal carries. The two recorded
/// sentences differ in the case of "statistics", so each operation has its own.
fn expected_message(operation: &str) -> &'static str {
    match operation {
        "StartColumnStatisticsTaskRun" => COLUMN_STATISTICS_TASK_RUN_ALREADY_STARTED,
        "CreateColumnStatisticsTaskSettings" => {
            COLUMN_STATISTICS_TASK_SETTINGS_RUN_ALREADY_STARTED
        }
        other => panic!("no in-flight refusal is modelled for {other}"),
    }
}

/// Start a run of the busy table and hold the in-flight window open, so every row below
/// is served against one run that has not finished. Answers the id it minted.
async fn start_an_in_flight_run(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) -> String {
    let started = ok(
        router,
        "StartColumnStatisticsTaskRun",
        &body_for(BUSY_DATABASE, BUSY_TABLE, json!({})),
    )
    .await;
    let run_id = started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the fixture start must mint a run id, got {started}"))
        .to_string();
    set_due_instant(engine, far_future());
    run_id
}

// ---------------------------------------------------------------------------
// Every operation that models the conflict refuses it, with its own fixed message
// ---------------------------------------------------------------------------

/// Each operation that models `ColumnStatisticsTaskRunningException` refuses a call
/// addressing the busy table, whatever else that call names.
#[tokio::test]
async fn an_in_flight_run_refuses_every_operation_that_models_the_conflict() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, BUSY_DATABASE, BUSY_TABLE).await;
    start_an_in_flight_run(&router, &engine).await;

    // (case, operation, the call that must be refused)
    let refused: Vec<(&str, &str, Value)> = vec![
        (
            "a second start naming only the required members",
            "StartColumnStatisticsTaskRun",
            body_for(BUSY_DATABASE, BUSY_TABLE, json!({})),
        ),
        (
            "a second start listing one column",
            "StartColumnStatisticsTaskRun",
            body_for(BUSY_DATABASE, BUSY_TABLE, json!({ "ColumnNameList": ["a"] })),
        ),
        (
            "a second start naming the catalog and a sample size",
            "StartColumnStatisticsTaskRun",
            body_for(
                BUSY_DATABASE,
                BUSY_TABLE,
                json!({ "CatalogID": ACCOUNT_ID, "SampleSize": 100 }),
            ),
        ),
        (
            "a create naming only the required members",
            "CreateColumnStatisticsTaskSettings",
            body_for(BUSY_DATABASE, BUSY_TABLE, json!({})),
        ),
        (
            "a create carrying a cron schedule",
            "CreateColumnStatisticsTaskSettings",
            body_for(
                BUSY_DATABASE,
                BUSY_TABLE,
                json!({ "Schedule": DAILY_SCHEDULE }),
            ),
        ),
        (
            "a create addressing the table in another casing",
            "CreateColumnStatisticsTaskSettings",
            body_for(
                &BUSY_DATABASE.to_uppercase(),
                &BUSY_TABLE.to_uppercase(),
                json!({}),
            ),
        ),
        (
            "a create naming the catalog, several columns and a sample size",
            "CreateColumnStatisticsTaskSettings",
            body_for(
                BUSY_DATABASE,
                BUSY_TABLE,
                json!({
                    "CatalogID": ACCOUNT_ID,
                    "ColumnNameList": ["a", "b"],
                    "SampleSize": 50,
                    "SecurityConfiguration": "conflict-security-config",
                }),
            ),
        ),
    ];

    for (case, operation, body) in refused {
        let (status, parsed) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a call addressing a table whose run is in flight is refused, got \
             {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("ColumnStatisticsTaskRunningException"),
            "case {case:?}: expected ColumnStatisticsTaskRunningException, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            expected_message(operation),
            "case {case:?}: the refusal carries this error class's fixed message, got {:?}",
            error_message(&parsed)
        );
    }
}

/// A refused create persists nothing, so the table still carries no settings: the read is
/// the modelled miss both before and after it.
#[tokio::test]
async fn a_create_refused_by_an_in_flight_run_persists_nothing() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, BUSY_DATABASE, BUSY_TABLE).await;
    start_an_in_flight_run(&router, &engine).await;

    let read = json!({ "DatabaseName": BUSY_DATABASE, "TableName": BUSY_TABLE });
    assert_settings_are_missing(&router, "before the refused create", &read).await;

    let (status, parsed) = call(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &body_for(BUSY_DATABASE, BUSY_TABLE, json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "the create under an in-flight run is refused, got {status} with {parsed}"
    );

    assert_settings_are_missing(&router, "after the refused create", &read).await;
}

/// Assert that `read` reports no settings: the modelled miss, with its fixed message.
async fn assert_settings_are_missing(router: &MultiServiceRouter, case: &str, read: &Value) {
    let (status, parsed) = call(router, "GetColumnStatisticsTaskSettings", read).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a table that carries no settings is the modelled miss, got {status} with \
         {parsed}"
    );
    assert!(
        error_code(&parsed).ends_with("EntityNotFoundException"),
        "case {case:?}: expected EntityNotFoundException, got {}",
        error_code(&parsed)
    );
    assert_eq!(
        error_message(&parsed),
        COLUMN_STATISTICS_TASK_SETTINGS_NOT_FOUND,
        "case {case:?}: the miss carries its fixed message, got {:?}",
        error_message(&parsed)
    );
}

/// The conflict is per table and lasts exactly as long as the run: another table is served
/// while the run is in flight, and the busy table is served once the clock has finished it
/// — with the settings the create then wrote reading back.
#[tokio::test]
async fn the_conflict_covers_only_the_busy_table_and_only_while_its_run_is_in_flight() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, BUSY_DATABASE, BUSY_TABLE).await;
    store_table(&router, BUSY_DATABASE, QUIET_TABLE).await;
    start_an_in_flight_run(&router, &engine).await;

    // A table no run covers is unaffected: both operations serve it.
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &body_for(BUSY_DATABASE, QUIET_TABLE, json!({ "Schedule": DAILY_SCHEDULE })),
    )
    .await;
    let quiet_run = ok(
        &router,
        "StartColumnStatisticsTaskRun",
        &body_for(BUSY_DATABASE, QUIET_TABLE, json!({})),
    )
    .await;
    assert!(
        quiet_run.get("ColumnStatisticsTaskRunId").is_some(),
        "case {:?}: a start for a table of its own is served, got {quiet_run}",
        "another table while the first run is in flight"
    );

    // Once the clock has finished the in-flight run, the busy table's create is served and
    // what it wrote reads back.
    set_due_instant(&engine, already_past());
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &body_for(
            BUSY_DATABASE,
            BUSY_TABLE,
            json!({ "Schedule": DAILY_SCHEDULE }),
        ),
    )
    .await;
    let reported = ok(
        &router,
        "GetColumnStatisticsTaskSettings",
        &json!({ "DatabaseName": BUSY_DATABASE, "TableName": BUSY_TABLE }),
    )
    .await;
    let settings = reported
        .get("ColumnStatisticsTaskSettings")
        .unwrap_or_else(|| {
            panic!("the read must report the settings the create wrote, got {reported}")
        });
    assert_eq!(
        settings.get("Role").and_then(Value::as_str),
        Some(ROLE),
        "case {:?}: the settings round-trip the submitted Role, got {settings}",
        "after the run finished"
    );
    assert_eq!(
        settings
            .get("Schedule")
            .and_then(|schedule| schedule.get("ScheduleExpression"))
            .and_then(Value::as_str),
        Some(DAILY_SCHEDULE),
        "case {:?}: the settings round-trip the submitted Schedule, got {settings}",
        "after the run finished"
    );
}

// ---------------------------------------------------------------------------
// The stop that ends the conflict
// ---------------------------------------------------------------------------

/// `StopColumnStatisticsTaskRun` is the other way the in-flight run reaches a terminal
/// state: the stopped run reads back stopped, the create it was blocking is then served,
/// and a second stop of that table has nothing left to stop.
#[tokio::test]
async fn a_stop_ends_the_in_flight_run_and_with_it_the_conflict() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, BUSY_DATABASE, BUSY_TABLE).await;
    let run_id = start_an_in_flight_run(&router, &engine).await;

    // The conflict holds while the run is in flight.
    let (status, parsed) = call(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &body_for(BUSY_DATABASE, BUSY_TABLE, json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "the create under an in-flight run is refused, got {status} with {parsed}"
    );

    // The stop addresses the table the same way every other catalog operation does, so a
    // name in another casing names the same table.
    ok(
        &router,
        "StopColumnStatisticsTaskRun",
        &json!({
            "DatabaseName": BUSY_DATABASE.to_uppercase(),
            "TableName": BUSY_TABLE.to_uppercase(),
        }),
    )
    .await;

    let run = ok(
        &router,
        "GetColumnStatisticsTaskRun",
        &json!({ "ColumnStatisticsTaskRunId": run_id }),
    )
    .await;
    let reported = run
        .get("ColumnStatisticsTaskRun")
        .unwrap_or_else(|| panic!("the read must report the stopped run, got {run}"));
    assert_eq!(
        reported.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED),
        "case {:?}: a stopped run reads back in the stopped state, got {reported}",
        "after the stop"
    );
    assert!(
        reported.get("EndTime").is_some(),
        "case {:?}: a stopped run has ended, so it carries an EndTime, got {reported}",
        "after the stop"
    );
    assert!(
        reported.get("ErrorMessage").is_none(),
        "case {:?}: nothing failed, so a stopped run carries no ErrorMessage, got {reported}",
        "after the stop"
    );

    // With nothing in flight the create is served. It states a SCHEDULE, so it starts no
    // run of its own and the table is left with nothing to stop.
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &body_for(
            BUSY_DATABASE,
            BUSY_TABLE,
            json!({ "Schedule": DAILY_SCHEDULE }),
        ),
    )
    .await;

    // (case, the stop that must be refused, the modelled error it is refused with)
    for (case, body, expected_code) in [
        (
            "a second stop of the same table",
            json!({ "DatabaseName": BUSY_DATABASE, "TableName": BUSY_TABLE }),
            "ColumnStatisticsTaskNotRunningException",
        ),
        (
            "a table the database does not hold",
            json!({ "DatabaseName": BUSY_DATABASE, "TableName": "no_such_table" }),
            "EntityNotFoundException",
        ),
        (
            "a database the catalog does not hold",
            json!({ "DatabaseName": "no_such_db", "TableName": BUSY_TABLE }),
            "EntityNotFoundException",
        ),
    ] {
        let (status, parsed) = call(&router, "StopColumnStatisticsTaskRun", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a stop with nothing to stop is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with(expected_code),
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&parsed)
        );
    }

    let (_, parsed) = call(
        &router,
        "StopColumnStatisticsTaskRun",
        &json!({ "DatabaseName": BUSY_DATABASE, "TableName": BUSY_TABLE }),
    )
    .await;
    assert_eq!(
        error_message(&parsed),
        COLUMN_STATISTICS_TASK_NOT_RUNNING,
        "case {:?}: the refusal carries this error class's fixed message, got {:?}",
        "a second stop of the same table",
        error_message(&parsed)
    );
}
