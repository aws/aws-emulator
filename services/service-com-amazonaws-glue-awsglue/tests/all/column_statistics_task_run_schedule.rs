// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue column-statistics task-run SCHEDULE slice:
//! `StartColumnStatisticsTaskRunSchedule`, the `StopColumnStatisticsTaskRunSchedule` that is
//! its opposite write, and the `GetColumnStatisticsTaskSettings` that reports the state they
//! decide.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the column statistics task run schedule of a table IS the cron
//! expression its settings carry, and the schedule state a settings read reports follows the
//! LAST start or stop of it — a start puts the schedule in force (`SCHEDULED`) and a stop
//! suspends it (`NOT_SCHEDULED`) while leaving the expression on the settings — so either
//! write repeated is served and changes nothing further, and neither starts a statistics run.
//! An address that holds no such schedule is refused with `EntityNotFoundException` and
//! leaves the stored settings exactly as they were. WHICH entity the refusal names follows
//! what these two operations look up: the schedule is a member of the table's column
//! statistics task SETTINGS, so an address holding no settings — whether the catalog holds
//! no such database, no such table, or a table no create has written settings for — is the
//! absence of the SETTINGS record, named by its account-qualified key, while settings that
//! exist carrying no cron expression are the schedule that does not exist.
//!
//! The recorded case — a start addressed at a table with no settings — is ONE row of the
//! refusal table below; the rows differ along WHICH part of the address is absent, whether
//! the request names a catalog id, and which of the two operations is asked. The state rows
//! differ along the cron expression, the casing the schedule is addressed in, and whether the
//! request names a catalog id.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    COLUMN_STATISTICS_SCHEDULE_TYPE_CRON, SCHEDULE_STATE_NOT_SCHEDULED, SCHEDULE_STATE_SCHEDULED,
};
use smithy_server_runtime::MultiServiceRouter;

// The settings these two operations decide one member of are written and read through the
// suite that owns them, so this suite stages its tables and settings through the SAME
// helpers rather than a second copy of them.
use crate::column_statistics_task_settings::{create_body, get_settings, read_body, store_table};
use crate::wire::{call, emulator, error_code, error_message, ok, string_list, ACCOUNT_ID};

/// The two operations this suite drives, so every row of a table below is asked of both.
///
/// `pub(crate)` so the suite that tables the VALUES of the settings key
/// ([`crate::column_statistics_schedule_settings_key`]) asks the same two operations rather
/// than naming them a second time.
pub(crate) const SCHEDULE_OPERATIONS: [&str; 2] = [
    "StartColumnStatisticsTaskRunSchedule",
    "StopColumnStatisticsTaskRunSchedule",
];

// The sentences the refusals below must carry, spelled out HERE rather than read off the
// implementation, so a row fails when the wording moves: the absence of the settings record
// these two operations look up, named by its own key; the service's sentence for a schedule
// that was not found (the one its crawler-schedule pair answers for a record with no
// expression to run); and the catalog miss.

/// The absence of the column statistics task settings an address names, as the schedule
/// start and stop report it: fixed text around the record's composite key — the catalog the
/// request addressed, the account whose estate it is in, and the database and table the
/// settings would belong to, as the request named them.
/// `pub(crate)` for the reason [`SCHEDULE_OPERATIONS`] is: the wording lives in ONE place, so
/// it cannot drift between the suites that assert it.
pub(crate) fn settings_not_found(catalog_id: &str, database: &str, table: &str) -> String {
    format!(
        "ColumnStatisticsTaskSettings {catalog_id}:{ACCOUNT_ID}:{database}-{table} not found."
    )
}

/// Settings that exist while carrying no cron expression: there is no schedule.
const NO_SCHEDULE_FOUND: &str = "No schedule found.";

/// A `CatalogID` that addresses no account's Data Catalog and names no catalog this account
/// holds.
const CATALOG_NOT_FOUND: &str = "Catalog not found.";

/// A `CatalogID` that is outside the catalog-id grammar, so it addresses no Data Catalog any
/// account could hold: fixed text stating the grammar.
const CATALOG_ID_INVALID: &str = "Catalog id is invalid. The catalog id must be prepended \
     with the account id prefix and can only contain lowercase letters, numbers, \
     underscores, and slashes.";

/// The body that addresses one table's schedule, plus whatever `extra` adds — the same three
/// members both operations take (`DatabaseName`, `TableName`, the optional `CatalogID`).
/// `pub(crate)` for the reason [`SCHEDULE_OPERATIONS`] is.
pub(crate) fn schedule_body(database: &str, table: &str, extra: &Value) -> Value {
    let mut body = json!({ "DatabaseName": database, "TableName": table });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The `Schedule` structure a settings read reports for one table, which must be there for
/// every table these tests address: each carries a stated cron expression.
async fn reported_schedule(router: &MultiServiceRouter, database: &str, table: &str) -> Value {
    let settings = get_settings(router, &read_body(database, table)).await;
    settings
        .get("Schedule")
        .cloned()
        .unwrap_or_else(|| panic!("the settings of {database}.{table} must report a Schedule, got {settings}"))
}

/// Assert that the settings of `database`.`table` report the schedule `expression` in `state`,
/// and that it is still the cron schedule type it was created as.
async fn assert_schedule(
    case: &str,
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    expression: &str,
    state: &str,
) {
    let schedule = reported_schedule(router, database, table).await;
    assert_eq!(
        schedule.get("ScheduleExpression").and_then(Value::as_str),
        Some(expression),
        "case {case:?}: a start or a stop leaves the cron expression on the settings, got \
         {schedule}"
    );
    assert_eq!(
        schedule.get("State").and_then(Value::as_str),
        Some(state),
        "case {case:?}: expected the schedule state {state}, got {schedule}"
    );
    let settings = get_settings(router, &read_body(database, table)).await;
    assert_eq!(
        settings.get("ScheduleType").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_SCHEDULE_TYPE_CRON),
        "case {case:?}: the schedule type a cron schedule was created with does not move, got \
         {settings}"
    );
}

/// One row of the state table below: a stored table whose settings carry `schedule`, the
/// address the start and the stop are sent with, and the extra members those requests carry.
struct ScheduleCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The cron expression the create states.
    schedule: &'a str,
    /// The `DatabaseName` the start and the stop address the schedule by, which the catalog
    /// resolves without regard to case.
    address_database: &'a str,
    /// The `TableName` they address it by, resolved the same way.
    address_table: &'a str,
    /// The members those requests carry beyond the address, such as a stated `CatalogID`.
    extra: Value,
}

/// The schedule state a settings read reports follows the last start or stop of that
/// schedule, and either write repeated is served and moves nothing further.
#[tokio::test]
async fn the_schedule_state_follows_the_last_start_or_stop() {
    let router = emulator();

    let cases: Vec<ScheduleCase<'_>> = vec![
        ScheduleCase {
            // The daily expression the settings suite's recorded case carries.
            case: "a daily schedule, addressed as it was created",
            database: "schedule_db",
            table: "events",
            schedule: "cron(15 12 * * ? *)",
            address_database: "schedule_db",
            address_table: "events",
            extra: json!({}),
        },
        ScheduleCase {
            // The catalog the settings live in, stated rather than defaulted.
            case: "an hourly schedule, with the catalog stated",
            database: "schedule_db",
            table: "orders",
            schedule: "cron(0 * * * ? *)",
            address_database: "schedule_db",
            address_table: "orders",
            extra: json!({ "CatalogID": ACCOUNT_ID }),
        },
        ScheduleCase {
            // The catalog addresses a database and a table without regard to case, so the
            // schedule a differently cased address names is the same schedule.
            case: "a yearly schedule, addressed in another case",
            database: "Schedule_DB",
            table: "Mixed_Case",
            schedule: "cron(0 0 1 1 ? *)",
            address_database: "SCHEDULE_db",
            address_table: "mixed_CASE",
            extra: json!({}),
        },
        ScheduleCase {
            case: "a schedule with a list of minutes",
            database: "schedule_db",
            table: "quarter_hourly",
            schedule: "cron(15,45 * * * ? *)",
            address_database: "schedule_db",
            address_table: "quarter_hourly",
            extra: json!({}),
        },
    ];

    for row in &cases {
        let case = row.case;
        store_table(&router, row.database, row.table, &["id", "amount"]).await;
        ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body(
                row.database,
                row.table,
                json!({ "Schedule": row.schedule }),
            ),
        )
        .await;
        // A stated schedule is in force the moment the create returns, so this is the state
        // the stop below has to move.
        assert_schedule(
            case,
            &router,
            row.database,
            row.table,
            row.schedule,
            SCHEDULE_STATE_SCHEDULED,
        )
        .await;

        let body = schedule_body(row.address_database, row.address_table, &row.extra);
        // A stop suspends the schedule, and a second stop is served: the state it writes is
        // the state the schedule stands in.
        for attempt in 1..=2 {
            let answer = ok(&router, "StopColumnStatisticsTaskRunSchedule", &body).await;
            assert!(
                answer.as_object().is_some_and(|members| members.is_empty()),
                "case {case:?}: stop {attempt} answers an empty document, got {answer}"
            );
            assert_schedule(
                case,
                &router,
                row.database,
                row.table,
                row.schedule,
                SCHEDULE_STATE_NOT_SCHEDULED,
            )
            .await;
        }

        // A start puts it back in force, and a second start is served for the same reason.
        for attempt in 1..=2 {
            let answer = ok(&router, "StartColumnStatisticsTaskRunSchedule", &body).await;
            assert!(
                answer.as_object().is_some_and(|members| members.is_empty()),
                "case {case:?}: start {attempt} answers an empty document, got {answer}"
            );
            assert_schedule(
                case,
                &router,
                row.database,
                row.table,
                row.schedule,
                SCHEDULE_STATE_SCHEDULED,
            )
            .await;
        }

        // Starting or stopping a SCHEDULE is not starting a run: nothing has been generated
        // for any of these tables, so this account holds no statistics task run at all and no
        // settings record reports an execution attempt.
        let listed = ok(&router, "ListColumnStatisticsTaskRuns", &json!({})).await;
        assert!(
            string_list(&listed, "ColumnStatisticsTaskRunIds").is_empty(),
            "case {case:?}: starting or stopping a schedule starts no statistics run, got \
             {listed}"
        );
        let settings = get_settings(&router, &read_body(row.database, row.table)).await;
        assert!(
            settings.get("LastExecutionAttempt").is_none(),
            "case {case:?}: no run was started, so the settings report no execution attempt, \
             got {settings}"
        );
    }
}

/// Neither operation touches the schedule of ANOTHER table: each writes the one record its
/// own address names.
#[tokio::test]
async fn a_start_or_a_stop_moves_only_the_addressed_table() {
    let router = emulator();
    let schedule = "cron(30 2 * * ? *)";
    for table in ["left", "right"] {
        store_table(&router, "pair_db", table, &["id"]).await;
        ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body("pair_db", table, json!({ "Schedule": schedule })),
        )
        .await;
    }

    ok(
        &router,
        "StopColumnStatisticsTaskRunSchedule",
        &schedule_body("pair_db", "left", &json!({})),
    )
    .await;

    assert_schedule(
        "the stopped table",
        &router,
        "pair_db",
        "left",
        schedule,
        SCHEDULE_STATE_NOT_SCHEDULED,
    )
    .await;
    assert_schedule(
        "the table nothing was asked of",
        &router,
        "pair_db",
        "right",
        schedule,
        SCHEDULE_STATE_SCHEDULED,
    )
    .await;
}

/// One row of the refusal table below: an address that holds no column statistics task run
/// schedule, and the modelled error each of the two operations answers it with.
struct RefusalCase<'a> {
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The members the request carries beyond the address.
    extra: Value,
    /// The error code the START answers, which models an authorization class the stop does
    /// not.
    start_code: &'a str,
    /// The error code the STOP answers.
    stop_code: &'a str,
    /// The wording both answer when the refusal is the same for the two, or `None` when it
    /// is not — a catalog of another account earns a different class, and so a different
    /// sentence, from each.
    expected_message: Option<String>,
}

/// An address that holds no schedule is refused with the operation's modelled miss, and the
/// settings of the one table that does carry a schedule are left exactly as they were.
#[tokio::test]
async fn an_address_that_holds_no_schedule_is_refused() {
    let router = emulator();
    let schedule = "cron(5 4 * * ? *)";
    // The one table whose schedule exists: the store every refusal below must leave alone.
    store_table(&router, "refusal_db", "scheduled", &["id"]).await;
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body("refusal_db", "scheduled", json!({ "Schedule": schedule })),
    )
    .await;
    // A stored table whose settings carry NO schedule: settings exist, a schedule does not.
    store_table(&router, "refusal_db", "unscheduled", &["id"]).await;
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body("refusal_db", "unscheduled", json!({})),
    )
    .await;
    // A stored table no create has written settings for at all.
    store_table(&router, "refusal_db", "no_settings", &["id"]).await;

    let cases: Vec<RefusalCase<'_>> = vec![
        RefusalCase {
            // The recorded case: no settings exist for the pair the request names, because
            // the table it names is not one the catalog holds. The SETTINGS are what these
            // operations look up, so they are what the refusal names.
            case: "a table the database does not hold",
            database: "refusal_db",
            table: "no_such_table",
            extra: json!({}),
            start_code: "EntityNotFoundException",
            stop_code: "EntityNotFoundException",
            expected_message: Some(settings_not_found(
                ACCOUNT_ID,
                "refusal_db",
                "no_such_table",
            )),
        },
        RefusalCase {
            case: "a database the catalog does not hold",
            database: "no_such_db",
            table: "scheduled",
            extra: json!({}),
            start_code: "EntityNotFoundException",
            stop_code: "EntityNotFoundException",
            expected_message: Some(settings_not_found(ACCOUNT_ID, "no_such_db", "scheduled")),
        },
        RefusalCase {
            case: "a stored table no create has written settings for",
            database: "refusal_db",
            table: "no_settings",
            extra: json!({}),
            start_code: "EntityNotFoundException",
            stop_code: "EntityNotFoundException",
            expected_message: Some(settings_not_found(ACCOUNT_ID, "refusal_db", "no_settings")),
        },
        RefusalCase {
            // The catalog component of the key comes from the request when it states one,
            // which for this account's own Data Catalog is its account id.
            case: "a stored table with no settings, with the catalog stated",
            database: "refusal_db",
            table: "no_settings",
            extra: json!({ "CatalogID": ACCOUNT_ID }),
            start_code: "EntityNotFoundException",
            stop_code: "EntityNotFoundException",
            expected_message: Some(settings_not_found(ACCOUNT_ID, "refusal_db", "no_settings")),
        },
        RefusalCase {
            case: "settings carrying no cron expression",
            database: "refusal_db",
            table: "unscheduled",
            extra: json!({}),
            start_code: "EntityNotFoundException",
            stop_code: "EntityNotFoundException",
            expected_message: Some(NO_SCHEDULE_FOUND.to_string()),
        },
        RefusalCase {
            // A catalog of another account is reachable only through that catalog's own
            // resource policy, so the start — which models an authorization class — refuses
            // it as the authorization failure it is, while the stop, which models none,
            // answers the miss the address is in a catalog this endpoint does not serve.
            case: "a catalog of another account",
            database: "refusal_db",
            table: "scheduled",
            extra: json!({ "CatalogID": "210987654321" }),
            start_code: "AccessDeniedException",
            stop_code: "EntityNotFoundException",
            expected_message: None,
        },
        RefusalCase {
            // An id that addresses no account's catalog and names no catalog this account
            // registered names nothing at all: a miss, for both operations.
            case: "a catalog this account does not hold",
            database: "refusal_db",
            table: "scheduled",
            extra: json!({ "CatalogID": format!("{ACCOUNT_ID}:no_such_catalog") }),
            start_code: "EntityNotFoundException",
            stop_code: "EntityNotFoundException",
            expected_message: Some(CATALOG_NOT_FOUND.to_string()),
        },
        RefusalCase {
            // An id carrying no account-id prefix is no catalog id at all: it addresses no
            // Data Catalog any account could hold, which no create can change, so both
            // operations refuse it on the request — the grammar sentence as the
            // `InvalidInputException` each models — before the address is resolved against
            // the store.
            case: "an id that is no catalog id at all",
            database: "refusal_db",
            table: "scheduled",
            extra: json!({ "CatalogID": "no_such_catalog" }),
            start_code: "InvalidInputException",
            stop_code: "InvalidInputException",
            expected_message: Some(CATALOG_ID_INVALID.to_string()),
        },
    ];

    for row in &cases {
        let body = schedule_body(row.database, row.table, &row.extra);
        for operation in SCHEDULE_OPERATIONS {
            let expected_code = if operation == SCHEDULE_OPERATIONS[0] {
                row.start_code
            } else {
                row.stop_code
            };
            let (status, parsed) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {:?} / {operation}: an address that holds no schedule is refused, got \
                 {status} with {parsed}",
                row.case
            );
            assert!(
                error_code(&parsed).ends_with(expected_code),
                "case {:?} / {operation}: expected {expected_code}, got {} with {:?}",
                row.case,
                error_code(&parsed),
                error_message(&parsed)
            );
            if let Some(expected_message) = row.expected_message.as_deref() {
                assert_eq!(
                    error_message(&parsed),
                    expected_message,
                    "case {:?} / {operation}: expected the refusal {expected_message:?}, got \
                     {parsed}",
                    row.case
                );
            }
        }
    }

    // Every refusal above left the stored schedule where it was: in force, carrying the
    // expression its create stated.
    assert_schedule(
        "the schedule no refusal touched",
        &router,
        "refusal_db",
        "scheduled",
        schedule,
        SCHEDULE_STATE_SCHEDULED,
    )
    .await;
    // And the settings that carry no schedule still carry none: a refused start writes
    // nothing, least of all a schedule.
    let unscheduled = get_settings(&router, &read_body("refusal_db", "unscheduled")).await;
    assert!(
        unscheduled.get("Schedule").is_none(),
        "a refused start adds no schedule to settings that carried none, got {unscheduled}"
    );
}
