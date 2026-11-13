// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue column-statistics task-SETTINGS slice:
//! `CreateColumnStatisticsTaskSettings` and the `GetColumnStatisticsTaskSettings` that
//! reports the settings it wrote.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the settings of a column statistics task belong to ONE stored
//! table, so a create persists the configuration it states against that table and a read
//! of that table reports it back unchanged — its stored catalog, database and table names,
//! the stated schedule as a scheduled cron schedule, the columns, role, sample size and
//! security configuration as stated — while a create addressing a table the catalog does
//! not hold, a second create for a table that already carries settings, and a schedule
//! that is not the documented cron expression are each refused with the operation's own
//! modelled error and leave the store as it was. The `LastExecutionAttempt` the read
//! reports is not held on the settings: it is the most recent column statistics task run
//! of that table, so it is absent until one has been started and follows that run's
//! status.
//!
//! The recorded case — one table whose settings carry a cron schedule — is ONE row of the
//! table below; the rows differ along whether a schedule is stated, how many columns are
//! listed (none, an empty list, one, several), the sample size (each published bound and
//! one in between), the casing of the names, and whether the create names a catalog id,
//! a security configuration or tags.
//!
//! The due instant the run lifecycle turns on is moved THROUGH THE STORE rather than
//! slept for, so the attempt a settings read reports is deterministic.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    COLUMN_STATISTICS_EXECUTION_STATUS_FAILED, COLUMN_STATISTICS_EXECUTION_STATUS_STARTED,
    COLUMN_STATISTICS_SCHEDULE_TYPE_CRON, COLUMN_STATISTICS_SETTING_SOURCE_TABLE,
    SCHEDULE_STATE_SCHEDULED,
};
use smithy_server_runtime::MultiServiceRouter;

// The run lifecycle's clock helpers live with the suite that owns that lifecycle; the
// attempt a settings read reports is driven through the same ones rather than a second copy.
use crate::column_statistics_task_runs::{
    already_past, far_future, set_due_instant, unreadable_reason,
};
use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// The IAM role a create names: "The role used for running the column statistics."
///
/// Shared with the deletion suite ([`crate::column_statistics_task_settings_deletion`]),
/// which writes the entries it deletes through the same create, so both suites state the
/// same role rather than spelling it twice.
pub const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// A cron expression of the documented form, "to run something every day at 12:15 UTC"
/// (`Schedule$ScheduleExpression`).
pub const DAILY_SCHEDULE: &str = "cron(15 12 * * ? *)";

/// An emulator plus the engine it runs on, so a test can move the due instant the run
/// lifecycle turns on instead of waiting for it.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Store `database` holding `table` with the columns `columns` names, and answer nothing:
/// the catalog state every create below is resolved against. A database several rows share
/// is created once — a repeat create is the catalog's `AlreadyExistsException`, which is
/// not what these tests are about.
pub async fn store_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    columns: &[&str],
) {
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
    let mut table_input = json!({ "Name": table, "TableType": "EXTERNAL_TABLE" });
    if !columns.is_empty() {
        table_input["StorageDescriptor"] = json!({
            "Location": format!("s3://bucket/{}/", table.to_lowercase()),
            "Columns": columns
                .iter()
                .map(|name| json!({ "Name": name, "Type": "string" }))
                .collect::<Vec<Value>>(),
        });
    }
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": table_input }),
    )
    .await;
}

/// A create body naming the three `@required` members, plus whatever `extra` adds.
pub fn create_body(database: &str, table: &str, extra: Value) -> Value {
    let mut body = json!({ "DatabaseName": database, "TableName": table, "Role": ROLE });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The `ColumnStatisticsTaskSettings` structure a read reports for one table.
pub async fn get_settings(router: &MultiServiceRouter, body: &Value) -> Value {
    let reported = ok(router, "GetColumnStatisticsTaskSettings", body).await;
    reported
        .get("ColumnStatisticsTaskSettings")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "GetColumnStatisticsTaskSettings must report a ColumnStatisticsTaskSettings, got \
                 {reported}"
            )
        })
}

/// The read body addressing one table.
pub fn read_body(database: &str, table: &str) -> Value {
    json!({ "DatabaseName": database, "TableName": table })
}

/// The `ColumnNameList` reported settings carry, in the order they report them.
fn reported_columns(settings: &Value) -> Option<Vec<String>> {
    settings
        .get("ColumnNameList")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
}

/// A numeric member of reported settings, taking an absent member as the model's own
/// default of 0 — awsJson omits a member whose value equals its modelled default.
fn reported_number(settings: &Value, member: &str) -> f64 {
    settings.get(member).and_then(Value::as_f64).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// One row of the create-and-read-back table below: a stored table, a create of its
/// settings, and what the read of them must carry. A named structure rather than a tuple,
/// so every column of the table reads as what it is.
struct SettingsCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The columns the stored table declares.
    columns: &'a [&'a str],
    /// The create members beyond the three `@required` ones.
    extra: Value,
    /// The cron expression the settings must report, if any.
    expected_schedule: Option<&'a str>,
    /// The column list the settings must report, if any.
    expected_columns: Option<Vec<&'a str>>,
    /// The `SampleSize` the settings must report.
    expected_sample: f64,
}

/// What `CreateColumnStatisticsTaskSettings` records is what
/// `GetColumnStatisticsTaskSettings` reports: the stored table the settings belong to and
/// the configuration the create stated.
#[tokio::test]
async fn created_settings_are_read_back_through_get_column_statistics_task_settings() {
    let router = emulator();

    let cases: Vec<SettingsCase<'_>> = vec![
        SettingsCase {
            // The recorded case: settings carrying a cron schedule.
            case: "a cron schedule and an explicit column list",
            database: "settings_db",
            table: "events",
            columns: &["id", "amount", "country"],
            extra: json!({ "Schedule": DAILY_SCHEDULE, "ColumnNameList": ["id", "amount"] }),
            expected_schedule: Some(DAILY_SCHEDULE),
            expected_columns: Some(vec!["id", "amount"]),
            expected_sample: 0.0,
        },
        SettingsCase {
            // `Schedule` is optional, so settings may state none at all.
            case: "no schedule at all",
            database: "settings_db",
            table: "orders",
            columns: &["order_id", "total"],
            extra: json!({ "ColumnNameList": ["total"] }),
            expected_schedule: None,
            expected_columns: Some(vec!["total"]),
            expected_sample: 0.0,
        },
        SettingsCase {
            // No column list states no column, so the settings report none: which columns
            // a run then covers is the run's own default.
            case: "no column list at all",
            database: "settings_db",
            table: "customers",
            columns: &["customer_id", "email"],
            extra: json!({ "Schedule": "cron(0 1 * * ? *)" }),
            expected_schedule: Some("cron(0 1 * * ? *)"),
            expected_columns: None,
            expected_sample: 0.0,
        },
        SettingsCase {
            // An empty list states no column either.
            case: "an empty column list",
            database: "settings_db",
            table: "empty_list",
            columns: &["a", "b"],
            extra: json!({ "ColumnNameList": [] }),
            expected_schedule: None,
            expected_columns: None,
            expected_sample: 0.0,
        },
        SettingsCase {
            case: "every column the table declares",
            database: "settings_db",
            table: "all_columns",
            columns: &["a", "b", "c"],
            extra: json!({ "ColumnNameList": ["a", "b", "c"] }),
            expected_schedule: None,
            expected_columns: Some(vec!["a", "b", "c"]),
            expected_sample: 0.0,
        },
        SettingsCase {
            // `SampleSize` has `range 0..100`: the lower bound, which is also the model's
            // default.
            case: "the lower sample-size bound",
            database: "settings_db",
            table: "sampled_low",
            columns: &["a"],
            extra: json!({ "SampleSize": 0.0 }),
            expected_schedule: None,
            expected_columns: None,
            expected_sample: 0.0,
        },
        SettingsCase {
            case: "the upper sample-size bound",
            database: "settings_db",
            table: "sampled_high",
            columns: &["a"],
            extra: json!({ "SampleSize": 100.0, "Schedule": DAILY_SCHEDULE }),
            expected_schedule: Some(DAILY_SCHEDULE),
            expected_columns: None,
            expected_sample: 100.0,
        },
        SettingsCase {
            case: "a sample size between the bounds",
            database: "settings_db",
            table: "sampled_mid",
            columns: &["a"],
            extra: json!({ "SampleSize": 12.5 }),
            expected_schedule: None,
            expected_columns: None,
            expected_sample: 12.5,
        },
        SettingsCase {
            // Database and table names are folded to lowercase when stored, so the
            // settings report the stored spelling whatever casing the create sent.
            case: "mixed-case names",
            database: "Settings_DB_Mixed",
            table: "Daily_Sales",
            columns: &["Day", "Revenue"],
            extra: json!({ "ColumnNameList": ["Day"] }),
            expected_schedule: None,
            expected_columns: Some(vec!["Day"]),
            expected_sample: 0.0,
        },
        SettingsCase {
            // "The ID of the Data Catalog in which the database resides", named
            // explicitly, plus the two members a create may carry beside it.
            case: "the catalog, a security configuration and tags named explicitly",
            database: "settings_db_catalog",
            table: "explicit_catalog",
            columns: &["a"],
            extra: json!({
                "CatalogID": ACCOUNT_ID,
                "SecurityConfiguration": "stats-security",
                "Tags": { "team": "analytics" },
            }),
            expected_schedule: None,
            expected_columns: None,
            expected_sample: 0.0,
        },
    ];

    for SettingsCase {
        case,
        database,
        table,
        columns,
        extra,
        expected_schedule,
        expected_columns,
        expected_sample,
    } in cases
    {
        store_table(&router, database, table, columns).await;
        let created = ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body(database, table, extra.clone()),
        )
        .await;
        assert!(
            created.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: the create answers an empty body, got {created}"
        );

        let settings = get_settings(&router, &read_body(database, table)).await;
        assert_eq!(
            settings.get("DatabaseName").and_then(Value::as_str),
            Some(database.to_lowercase().as_str()),
            "case {case:?}: the settings belong to the stored database, got {settings}"
        );
        assert_eq!(
            settings.get("TableName").and_then(Value::as_str),
            Some(table.to_lowercase().as_str()),
            "case {case:?}: the settings belong to the stored table, got {settings}"
        );
        assert_eq!(
            settings.get("CatalogID").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case {case:?}: the settings report the catalog they were resolved in, got {settings}"
        );
        assert_eq!(
            settings.get("Role").and_then(Value::as_str),
            Some(ROLE),
            "case {case:?}: the settings report the role the create named, got {settings}"
        );
        assert_eq!(
            reported_number(&settings, "SampleSize"),
            expected_sample,
            "case {case:?}: expected SampleSize {expected_sample}, got {settings}"
        );
        assert_eq!(
            settings.get("SecurityConfiguration").and_then(Value::as_str),
            extra.get("SecurityConfiguration").and_then(Value::as_str),
            "case {case:?}: the settings report the security configuration the create named, got \
             {settings}"
        );
        assert_eq!(
            reported_columns(&settings),
            expected_columns.as_ref().map(|names| names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>()),
            "case {case:?}: expected the configured columns {expected_columns:?}, got {settings}"
        );
        assert_eq!(
            settings
                .get("Schedule")
                .and_then(|schedule| schedule.get("ScheduleExpression"))
                .and_then(Value::as_str),
            expected_schedule,
            "case {case:?}: expected the schedule {expected_schedule:?}, got {settings}"
        );
        if expected_schedule.is_some() {
            assert_eq!(
                settings
                    .get("Schedule")
                    .and_then(|schedule| schedule.get("State"))
                    .and_then(Value::as_str),
                Some(SCHEDULE_STATE_SCHEDULED),
                "case {case:?}: a stated schedule is one the service runs, got {settings}"
            );
            assert_eq!(
                settings.get("ScheduleType").and_then(Value::as_str),
                Some(COLUMN_STATISTICS_SCHEDULE_TYPE_CRON),
                "case {case:?}: a schedule stated in CRON syntax is a CRON schedule type, got \
                 {settings}"
            );
        } else {
            assert!(
                settings.get("ScheduleType").is_none(),
                "case {case:?}: settings that state no schedule carry no schedule type, got \
                 {settings}"
            );
        }
        assert_eq!(
            settings.get("SettingSource").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_SETTING_SOURCE_TABLE),
            "case {case:?}: settings created for one table are sourced from that table, got \
             {settings}"
        );
        // A create that states a SCHEDULE starts no run — the schedule is when its runs
        // are due — so the settings report no attempt yet; one that states none starts the
        // first run it configures, which is the attempt the settings report.
        match expected_schedule {
            Some(_) => assert!(
                settings.get("LastExecutionAttempt").is_none(),
                "case {case:?}: a scheduled create starts no run, so there is no execution \
                 attempt, got {settings}"
            ),
            None => {
                let attempt = settings
                    .get("LastExecutionAttempt")
                    .unwrap_or_else(|| panic!(
                        "case {case:?}: a create with no schedule starts the first run it \
                         configures, which the settings report as the last attempt, got \
                         {settings}"
                    ));
                assert!(
                    attempt
                        .get("ColumnStatisticsTaskRunId")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty()),
                    "case {case:?}: the attempt names the run the create started, got {attempt}"
                );
            }
        }

        // The settings are addressed by the stored names, so either casing reaches them.
        let folded = get_settings(
            &router,
            &read_body(&database.to_uppercase(), &table.to_uppercase()),
        )
        .await;
        assert_eq!(
            folded, settings,
            "case {case:?}: the settings are reached under the stored name whatever casing the \
             read sends, got {folded}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a create refuses
// ---------------------------------------------------------------------------

/// A create for a table the addressed catalog does not hold is the modelled miss, and
/// stores nothing.
#[tokio::test]
async fn a_create_for_a_table_the_catalog_does_not_hold_is_refused() {
    let router = emulator();
    store_table(&router, "miss_db", "miss_table", &["a"]).await;

    // (case, the create that must be refused, the modelled error it is refused with)
    let refused: Vec<(&str, Value, &str)> = vec![
        (
            "a database the catalog does not hold",
            create_body("no_such_db", "miss_table", json!({})),
            "EntityNotFoundException",
        ),
        (
            "a table the database does not hold",
            create_body("miss_db", "no_such_table", json!({})),
            "EntityNotFoundException",
        ),
        (
            // A catalog of another account is reachable only through that catalog's own
            // resource policy, so that create is refused before any entity lookup.
            "a catalog of another account",
            create_body("miss_db", "miss_table", json!({ "CatalogID": "210987654321" })),
            "AccessDeniedException",
        ),
    ];
    for (case, body, expected_code) in refused {
        let (status, parsed) = call(&router, "CreateColumnStatisticsTaskSettings", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a create that addresses no stored table is refused, got {status} with \
             {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with(expected_code),
            "case {case:?}: expected {expected_code}, got {} with {}",
            error_code(&parsed),
            error_message(&parsed)
        );
    }

    // Nothing was stored for the one table that does exist.
    let (status, parsed) = call(
        &router,
        "GetColumnStatisticsTaskSettings",
        &read_body("miss_db", "miss_table"),
    )
    .await;
    assert_eq!(
        status, 400,
        "a refused create stores nothing, so the read is still the modelled miss, got {status} \
         with {parsed}"
    );
    assert!(
        error_code(&parsed).ends_with("EntityNotFoundException"),
        "a table with no settings is the modelled miss, got {}",
        error_code(&parsed)
    );
}

/// A second create for a table that already carries SCHEDULED settings is the modelled
/// duplicate, and the stored settings stay exactly as the first create left them: a
/// scheduled column statistics task is a standing record of that table's, which a create
/// does not replace.
#[tokio::test]
async fn a_second_create_for_the_same_table_is_refused_and_changes_nothing() {
    let router = emulator();
    store_table(&router, "dup_db", "dup_table", &["a", "b"]).await;
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body(
            "dup_db",
            "dup_table",
            json!({ "Schedule": DAILY_SCHEDULE, "SampleSize": 25.0 }),
        ),
    )
    .await;

    // (case, the second create)
    let refused: Vec<(&str, Value)> = vec![
        (
            "the same configuration again",
            create_body(
                "dup_db",
                "dup_table",
                json!({ "Schedule": DAILY_SCHEDULE, "SampleSize": 25.0 }),
            ),
        ),
        (
            "another configuration",
            create_body("dup_db", "dup_table", json!({ "SampleSize": 90.0 })),
        ),
        (
            "the same table in another casing",
            create_body("DUP_DB", "Dup_Table", json!({ "SampleSize": 90.0 })),
        ),
    ];
    for (case, body) in refused {
        let (status, parsed) = call(&router, "CreateColumnStatisticsTaskSettings", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second create for a table that already carries settings is refused, \
             got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("AlreadyExistsException"),
            "case {case:?}: expected AlreadyExistsException, got {}",
            error_code(&parsed)
        );

        let settings = get_settings(&router, &read_body("dup_db", "dup_table")).await;
        assert_eq!(
            reported_number(&settings, "SampleSize"),
            25.0,
            "case {case:?}: a refused create leaves the first create's settings in place, got \
             {settings}"
        );
    }
}

/// Settings that carry NO schedule are the configuration of the on-demand run their create
/// started, not a standing task: while that run is in flight a second create for the table
/// is the modelled in-flight refusal, and once the run is no longer in flight the create is
/// served, reconfiguring the generation and starting it afresh.
///
/// The rows differ along what the second create reconfigures: another role, another sample
/// size, another column list, and a schedule — which turns the table's settings into the
/// standing scheduled task the duplicate refusal above protects.
#[tokio::test]
async fn a_create_over_schedule_less_settings_waits_for_the_run_and_then_reconfigures() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "reconfig_db", "reconfig_table", &["a", "b"]).await;

    // (case, what the second create states, what the read must report afterwards)
    let rows: Vec<(&str, Value, &str, Value)> = vec![
        (
            "another sample size",
            json!({ "SampleSize": 40.0 }),
            "SampleSize",
            json!(40.0),
        ),
        (
            "another column list",
            json!({ "ColumnNameList": ["b"] }),
            "ColumnNameList",
            json!(["b"]),
        ),
        (
            "a schedule, which makes it a standing task",
            json!({ "Schedule": DAILY_SCHEDULE }),
            "ScheduleType",
            json!(COLUMN_STATISTICS_SCHEDULE_TYPE_CRON),
        ),
    ];

    for (case, extra, member, expected) in rows {
        // The create with no schedule starts a run, so the next create meets the in-flight
        // refusal rather than a duplicate one.
        ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body("reconfig_db", "reconfig_table", json!({})),
        )
        .await;
        let (status, parsed) = call(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body("reconfig_db", "reconfig_table", extra.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a create under the run the first one started is refused, got \
             {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("ColumnStatisticsTaskRunningException"),
            "case {case:?}: expected ColumnStatisticsTaskRunningException, got {}",
            error_code(&parsed)
        );

        // Once the clock has finished that run, the same create is served and what it
        // states is what the read reports.
        set_due_instant(&engine, already_past());
        ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body("reconfig_db", "reconfig_table", extra),
        )
        .await;
        let settings = get_settings(&router, &read_body("reconfig_db", "reconfig_table")).await;
        assert_eq!(
            settings.get(member),
            Some(&expected),
            "case {case:?}: the served create reconfigured {member}, got {settings}"
        );

        // A scheduled record is a standing task: the next create for the table is the
        // duplicate refusal, so the row that wrote one deletes it before the next row runs.
        if member == "ScheduleType" {
            let (status, parsed) = call(
                &router,
                "CreateColumnStatisticsTaskSettings",
                &create_body("reconfig_db", "reconfig_table", json!({})),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a create over a standing scheduled task is refused, got \
                 {status} with {parsed}"
            );
            assert!(
                error_code(&parsed).ends_with("AlreadyExistsException"),
                "case {case:?}: expected AlreadyExistsException, got {}",
                error_code(&parsed)
            );
        }
        ok(
            &router,
            "DeleteColumnStatisticsTaskSettings",
            &read_body("reconfig_db", "reconfig_table"),
        )
        .await;
        set_due_instant(&engine, already_past());
    }
}

/// A `Schedule` that is not the documented `cron(..)` expression is refused as invalid
/// input, and stores nothing; one that is, is stored.
#[tokio::test]
async fn a_schedule_that_is_not_a_cron_expression_is_refused() {
    let router = emulator();

    // (case, the schedule, whether it is the documented form). The accepted spellings are
    // six-field `cron(..)` expressions, the form `Schedule$ScheduleExpression` documents
    // ("cron(15 12 * * ? *)").
    let schedules: Vec<(&str, &str, bool)> = vec![
        ("the documented example", "cron(15 12 * * ? *)", true),
        ("every hour, six fields", "cron(0 * * * ? *)", true),
        ("surrounding whitespace", "  cron(0 0 1 * ? *)  ", true),
        ("one field short", "cron(15 12 * * ?)", false),
        ("a rate expression", "rate(1 hour)", false),
        ("bare cron fields", "15 12 * * ? *", false),
        ("prose", "every day at noon", false),
        ("an unclosed expression", "cron(15 12 * * ? *", false),
    ];
    for (index, (case, schedule, documented)) in schedules.into_iter().enumerate() {
        let table = format!("schedule_table_{index}");
        store_table(&router, "schedule_db", &table, &["a"]).await;
        let (status, parsed) = call(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body("schedule_db", &table, json!({ "Schedule": schedule })),
        )
        .await;
        if documented {
            assert_eq!(
                status, 200,
                "case {case:?}: a cron expression of the documented form is stored, got {status} \
                 with {parsed}"
            );
            let settings = get_settings(&router, &read_body("schedule_db", &table)).await;
            assert_eq!(
                settings
                    .get("Schedule")
                    .and_then(|reported| reported.get("ScheduleExpression"))
                    .and_then(Value::as_str),
                Some(schedule),
                "case {case:?}: the settings report the schedule the create stated, got {settings}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a schedule that is not a cron expression is refused, got {status} with \
             {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {}",
            error_code(&parsed)
        );
        let (read_status, read_parsed) = call(
            &router,
            "GetColumnStatisticsTaskSettings",
            &read_body("schedule_db", &table),
        )
        .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused create stores nothing, got {read_status} with {read_parsed}"
        );
    }
}

/// A `SampleSize` outside the modelled `range 0..100` is refused at the boundary, and
/// stores nothing.
#[tokio::test]
async fn a_sample_size_outside_the_modelled_range_is_refused() {
    let router = emulator();
    store_table(&router, "range_db", "range_table", &["a"]).await;

    // One step past each published bound of `SampleSizePercentage` (`range 0..100`).
    for (case, sample) in [("below the lower bound", -0.5), ("above the upper bound", 100.5)] {
        let (status, parsed) = call(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body("range_db", "range_table", json!({ "SampleSize": sample })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a sample size of {sample} is outside the modelled range, got {status} \
             with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("ValidationException"),
            "case {case:?}: a modelled constraint is refused at the boundary, got {} with {}",
            error_code(&parsed),
            error_message(&parsed)
        );
    }

    let (status, parsed) = call(
        &router,
        "GetColumnStatisticsTaskSettings",
        &read_body("range_db", "range_table"),
    )
    .await;
    assert_eq!(
        status, 400,
        "a refused create stores nothing, got {status} with {parsed}"
    );
}

// ---------------------------------------------------------------------------
// The attempt a settings read reports
// ---------------------------------------------------------------------------

/// `LastExecutionAttempt` is the most recent column statistics task run of the table the
/// settings belong to: absent until one has been started, then that run, in the status the
/// clock has reached.
#[tokio::test]
async fn the_read_reports_the_last_run_of_that_table_as_the_execution_attempt() {
    let (router, engine) = emulator_with_engine();
    store_table(&router, "attempt_db", "attempt_table", &["a", "b"]).await;
    store_table(&router, "attempt_db", "other_table", &["a"]).await;
    // The settings are created WITH a schedule, so the create starts no run of its own and
    // the attempt below is the one the explicit start makes.
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body(
            "attempt_db",
            "attempt_table",
            json!({ "Schedule": DAILY_SCHEDULE }),
        ),
    )
    .await;

    let settings = get_settings(&router, &read_body("attempt_db", "attempt_table")).await;
    assert!(
        settings.get("LastExecutionAttempt").is_none(),
        "no run has been started, so the settings report no attempt, got {settings}"
    );

    // A run of ANOTHER table is not this table's attempt.
    ok(
        &router,
        "StartColumnStatisticsTaskRun",
        &json!({ "DatabaseName": "attempt_db", "TableName": "other_table", "Role": ROLE }),
    )
    .await;
    let settings = get_settings(&router, &read_body("attempt_db", "attempt_table")).await;
    assert!(
        settings.get("LastExecutionAttempt").is_none(),
        "a run of another table is not this table's attempt, got {settings}"
    );

    // This table's run, read while the clock still owes it its terminal status.
    let started = ok(
        &router,
        "StartColumnStatisticsTaskRun",
        &json!({ "DatabaseName": "attempt_db", "TableName": "attempt_table", "Role": ROLE }),
    )
    .await;
    let first_run = started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .expect("the start reports a run id")
        .to_string();
    set_due_instant(&engine, far_future());
    let attempt = get_settings(&router, &read_body("attempt_db", "attempt_table"))
        .await
        .get("LastExecutionAttempt")
        .cloned()
        .unwrap_or_else(|| panic!("the settings report the run that was started"));
    assert_eq!(
        attempt
            .get("ColumnStatisticsTaskRunId")
            .and_then(Value::as_str),
        Some(first_run.as_str()),
        "the attempt is the run this table was started with, got {attempt}"
    );
    assert_eq!(
        attempt.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_EXECUTION_STATUS_STARTED),
        "a run still in flight is reported as the attempt it started, got {attempt}"
    );
    assert!(
        attempt.get("ExecutionTimestamp").is_some(),
        "the attempt reports when the run occurred, got {attempt}"
    );
    assert!(
        attempt.get("ErrorMessage").is_none(),
        "nothing has failed yet, so the attempt carries no error message, got {attempt}"
    );

    // Once the clock has finished the run, the attempt follows it.
    set_due_instant(&engine, already_past());
    let attempt = get_settings(&router, &read_body("attempt_db", "attempt_table"))
        .await
        .get("LastExecutionAttempt")
        .cloned()
        .unwrap_or_else(|| panic!("the settings still report the run that was started"));
    assert_eq!(
        attempt.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_EXECUTION_STATUS_FAILED),
        "the status the clock owed the run is what the attempt reports, got {attempt}"
    );
    assert_eq!(
        attempt.get("ErrorMessage").and_then(Value::as_str),
        Some(unreadable_reason("attempt_table", &["a", "b"]).as_str()),
        "the attempt reports why the run generated nothing, got {attempt}"
    );

    // The LAST run is the attempt: a second one replaces the first.
    let started = ok(
        &router,
        "StartColumnStatisticsTaskRun",
        &json!({ "DatabaseName": "attempt_db", "TableName": "attempt_table", "Role": ROLE }),
    )
    .await;
    let second_run = started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .expect("the second start reports a run id")
        .to_string();
    set_due_instant(&engine, far_future());
    let attempt = get_settings(&router, &read_body("attempt_db", "attempt_table"))
        .await
        .get("LastExecutionAttempt")
        .cloned()
        .unwrap_or_else(|| panic!("the settings report the most recent run"));
    assert_eq!(
        attempt
            .get("ColumnStatisticsTaskRunId")
            .and_then(Value::as_str),
        Some(second_run.as_str()),
        "the attempt is the MOST RECENT run of this table, got {attempt}"
    );
    assert_ne!(
        first_run, second_run,
        "each start mints its own run id, got {first_run} twice"
    );
}

/// A read for a table that carries no settings, for a table the catalog does not hold and
/// for a database it does not hold is the modelled miss rather than an empty answer — and
/// every one of them is the SAME fixed sentence, which names neither of the two names the
/// request addressed the entry by.
#[tokio::test]
async fn a_read_without_stored_settings_is_the_modelled_miss() {
    let router = emulator();
    store_table(&router, "read_db", "read_table", &["a"]).await;

    // (case, the read that must be refused)
    for (case, body) in [
        ("a table that carries no settings", read_body("read_db", "read_table")),
        ("a table the database does not hold", read_body("read_db", "no_such_table")),
        ("a database the catalog does not hold", read_body("no_such_db", "read_table")),
        (
            "the shortest names the model admits",
            read_body("d", "t"),
        ),
        (
            "the longest names the model admits",
            read_body(&"d".repeat(255), &"t".repeat(255)),
        ),
    ] {
        let (status, parsed) = call(&router, "GetColumnStatisticsTaskSettings", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a read with nothing to report is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            READ_SETTINGS_MISS_MESSAGE,
            "case {case:?}: expected the fixed settings-miss sentence, got {:?}",
            error_message(&parsed)
        );
    }
}

// ---------------------------------------------------------------------------
// The wording of the misses
// ---------------------------------------------------------------------------

/// The sentence a create addressed at a table the database does not hold is refused with:
/// fixed text that names the members the request addressed the table by and carries no
/// value read off the request.
const CREATE_TABLE_MISS_MESSAGE: &str = "Table passed as input within input database not found.";

/// The sentence a read with no settings entry to report is refused with: fixed text that
/// names the two members the entry is addressed by, whether the table carries no settings
/// or the database holds no such table at all.
const READ_SETTINGS_MISS_MESSAGE: &str =
    "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist";

/// A create addressed at a table the database does not hold is refused with the one fixed
/// sentence for that miss, whichever way the name misses — the requested `TableName` is
/// never part of it — and the read that follows reports the entry it holds none of in its
/// own fixed sentence. The database miss keeps its own wording, so a client can still tell
/// which half of the address was wrong, and the stored table still takes a create.
#[tokio::test]
async fn a_table_miss_is_refused_in_one_fixed_sentence_that_withholds_the_name() {
    let router = emulator();
    store_table(&router, "wording_db", "stored_table", &["a", "b"]).await;
    store_table(&router, "other_db", "elsewhere_table", &["a"]).await;

    // (case, the `TableName` the create addresses) — the rows differ along HOW the name
    // misses: a name nothing created, the name of a table another database holds, a stored
    // name carrying a suffix, and the shortest and longest names `NameString` admits
    // (`length 1..255`, `smithy_query operation=CreateColumnStatisticsTaskSettings`).
    let cases: Vec<(&str, String)> = vec![
        ("a name nothing created", "no_such_table".to_string()),
        (
            "the name of a table another database holds",
            "elsewhere_table".to_string(),
        ),
        (
            "a stored name carrying a suffix",
            "stored_table-does-not-exist".to_string(),
        ),
        ("the shortest name the model admits", "t".to_string()),
        ("the longest name the model admits", "t".repeat(255)),
    ];

    for (case, table) in cases {
        let (status, parsed) = call(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body("wording_db", &table, json!({})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a create for a table the database does not hold is refused, got \
             {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&parsed)
        );
        assert_eq!(
            error_message(&parsed),
            CREATE_TABLE_MISS_MESSAGE,
            "case {case:?}: expected the fixed table-miss sentence, got {:?}",
            error_message(&parsed)
        );

        // The refused create stored nothing, and the read of the same address reports the
        // entry it holds none of rather than echoing the name either.
        let (read_status, read_parsed) =
            call(&router, "GetColumnStatisticsTaskSettings", &read_body("wording_db", &table)).await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: the read of the same address is refused too, got {read_status} with \
             {read_parsed}"
        );
        assert!(
            error_code(&read_parsed).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException from the read, got {}",
            error_code(&read_parsed)
        );
        assert_eq!(
            error_message(&read_parsed),
            READ_SETTINGS_MISS_MESSAGE,
            "case {case:?}: expected the fixed settings-miss sentence, got {:?}",
            error_message(&read_parsed)
        );
    }

    // A database the catalog does not hold is a DIFFERENT miss: its sentence is not the
    // table's, so the two halves of the address stay distinguishable.
    let (status, parsed) = call(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body("no_such_db", "stored_table", json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: a create for a database the catalog does not hold is refused, got {status} \
         with {parsed}",
        "a database the catalog does not hold"
    );
    assert_ne!(
        error_message(&parsed),
        CREATE_TABLE_MISS_MESSAGE,
        "case {:?}: the database miss must not answer the table's sentence, got {:?}",
        "a database the catalog does not hold",
        error_message(&parsed)
    );

    // The stored table still takes a create, so the refusals above are misses and not a
    // broken lookup.
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body("wording_db", "stored_table", json!({})),
    )
    .await;
    let settings = get_settings(&router, &read_body("wording_db", "stored_table")).await;
    assert_eq!(
        settings.get("Role").and_then(Value::as_str),
        Some(ROLE),
        "case {:?}: the stored table's settings report the role the create named, got {settings}",
        "the stored table"
    );
}
