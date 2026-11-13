// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteColumnStatisticsTaskSettings`: the removal of the column
//! statistics task settings entry a create wrote for ONE stored table.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a column statistics task settings entry belongs to the one stored
//! table it was created for and is addressed by the database and table names (in the
//! catalog the request names, or the account's own), so a delete removes exactly that
//! entry — leaving the table itself, the statistics runs recorded for it and every other
//! table's entry untouched, and leaving the table free to take a create again — while a
//! delete that finds no entry under the addressed names removes nothing and is refused
//! with the operation's modelled `EntityNotFoundException`.
//!
//! The recorded case — one table whose settings a create wrote and a delete removes — is
//! ONE row of the tables below; the rows differ along what the settings carry (a schedule
//! or none, no columns, one, several, each published `SampleSize` bound), how the delete
//! addresses the entry (the stored casing of the names, another casing, with and without an
//! explicit `CatalogID`), how many entries the catalog holds beside it, and — for the
//! refusals — HOW the address misses.
//!
//! The helpers that store a table and write and read an entry are the settings suite's
//! ([`crate::column_statistics_task_settings`]): one copy drives both suites, so what this
//! one deletes is exactly what that one creates and reads.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The run lifecycle's clock helper lives with the suite that owns that lifecycle: a create
// stating no schedule starts a run, and a test that wants the table idle again moves that
// run's due instant rather than sleeping for it.
use crate::column_statistics_task_runs::{already_past, set_due_instant};
use crate::column_statistics_task_settings::{
    create_body, get_settings, read_body, store_table, DAILY_SCHEDULE,
};
use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// An emulator plus the engine it runs on, so a test can settle the run a create started
/// instead of waiting for it.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// The sentence a request that finds no settings entry under the names it addressed is
/// refused with: fixed text naming the two members the entry is addressed by, with no value
/// read off the request. The same sentence the read answers with, because it is the same
/// entry, addressed the same way — and the same one for every way the address can miss, the
/// pair that names no stored table included.
const SETTINGS_MISS_MESSAGE: &str =
    "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist";

/// The delete body addressing one entry, plus whatever `extra` adds (a `CatalogID`).
fn delete_body(database: &str, table: &str, extra: Value) -> Value {
    let mut body = json!({ "DatabaseName": database, "TableName": table });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Delete one entry, asserting the operation answers 200 with a body carrying no member of
/// its own (`DeleteColumnStatisticsTaskSettingsResponse` declares none).
async fn delete_settings(router: &MultiServiceRouter, body: &Value) {
    let answered = ok(router, "DeleteColumnStatisticsTaskSettings", body).await;
    let empty = answered
        .as_object()
        .map(|members| members.is_empty())
        .unwrap_or(answered.is_null());
    assert!(
        empty,
        "a delete answers an empty response, got {answered} for {body}"
    );
}

/// Assert the read of this address is the modelled miss carrying `message`.
async fn assert_read_is_miss(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
    message: &str,
) {
    let (status, parsed) = call(router, "GetColumnStatisticsTaskSettings", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the read of an address holding no entry is refused, got {status} with \
         {parsed}"
    );
    assert!(
        error_code(&parsed).ends_with("EntityNotFoundException"),
        "case {case:?}: expected EntityNotFoundException from the read, got {:?}",
        error_code(&parsed)
    );
    assert_eq!(
        error_message(&parsed),
        message,
        "case {case:?}: expected the fixed settings-miss sentence from the read, got {:?}",
        error_message(&parsed)
    );
}

/// Assert the delete of this address is the modelled miss carrying `message`.
async fn assert_delete_is_miss(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
    message: &str,
) {
    let (status, parsed) = call(router, "DeleteColumnStatisticsTaskSettings", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a delete with no entry to remove is refused, got {status} with {parsed}"
    );
    assert!(
        error_code(&parsed).ends_with("EntityNotFoundException"),
        "case {case:?}: expected EntityNotFoundException, got {:?}",
        error_code(&parsed)
    );
    assert_eq!(
        error_message(&parsed),
        message,
        "case {case:?}: expected the refusal's own sentence, got {:?}",
        error_message(&parsed)
    );
}

// ---------------------------------------------------------------------------
// The delete removes the entry
// ---------------------------------------------------------------------------

/// One row of the delete table below: a stored table, the settings a create writes for it,
/// and the address the delete removes them by. A named structure rather than a tuple, so
/// every column of the table reads as what it is.
struct DeletionCase<'a> {
    /// What this row varies, named in each of its assertions.
    case: &'a str,
    database: &'a str,
    table: &'a str,
    /// The columns the stored table declares.
    columns: &'a [&'a str],
    /// The create members beyond the three `@required` ones.
    extra: Value,
    /// The `DatabaseName` the delete addresses the entry by, which need not be the casing
    /// the table was created under.
    delete_database: &'a str,
    /// The `TableName` the delete addresses the entry by.
    delete_table: &'a str,
    /// The delete members beyond the two `@required` ones.
    delete_extra: Value,
}

/// What a create wrote, a delete removes: the read that reported the entry reports the
/// modelled miss afterwards, a second delete of the same address has nothing left to remove,
/// and the table takes a create again — so the delete freed the one entry the table carries
/// rather than marking it unusable.
#[tokio::test]
async fn a_delete_removes_the_entry_a_create_wrote() {
    let (router, engine) = emulator_with_engine();

    let cases: Vec<DeletionCase<'_>> = vec![
        DeletionCase {
            case: "settings carrying a cron schedule",
            database: "delete_db",
            table: "scheduled_table",
            columns: &["a", "b"],
            extra: json!({ "Schedule": DAILY_SCHEDULE, "SampleSize": 25.0 }),
            delete_database: "delete_db",
            delete_table: "scheduled_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "settings carrying no schedule",
            database: "delete_db",
            table: "unscheduled_table",
            columns: &["a"],
            extra: json!({}),
            delete_database: "delete_db",
            delete_table: "unscheduled_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "settings listing every column of the table",
            database: "delete_db",
            table: "columns_table",
            columns: &["a", "b", "c"],
            extra: json!({ "ColumnNameList": ["a", "b", "c"] }),
            delete_database: "delete_db",
            delete_table: "columns_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "settings listing one column",
            database: "delete_db",
            table: "one_column_table",
            columns: &["a", "b"],
            extra: json!({ "ColumnNameList": ["b"] }),
            delete_database: "delete_db",
            delete_table: "one_column_table",
            delete_extra: json!({}),
        },
        // `SampleSize` is `range 0..100`
        // (`smithy_query operation=CreateColumnStatisticsTaskSettings`): both published
        // bounds are deleted the same way as a value in between.
        DeletionCase {
            case: "settings sampling the smallest share the model admits",
            database: "delete_db",
            table: "sample_low_table",
            columns: &["a"],
            extra: json!({ "SampleSize": 0.0 }),
            delete_database: "delete_db",
            delete_table: "sample_low_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "settings sampling the largest share the model admits",
            database: "delete_db",
            table: "sample_high_table",
            columns: &["a"],
            extra: json!({ "SampleSize": 100.0 }),
            delete_database: "delete_db",
            delete_table: "sample_high_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "settings naming a security configuration",
            database: "delete_db",
            table: "secured_table",
            columns: &["a"],
            extra: json!({ "SecurityConfiguration": "stats-logs", "Schedule": DAILY_SCHEDULE }),
            delete_database: "delete_db",
            delete_table: "secured_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "a delete naming the account's own catalog explicitly",
            database: "delete_db",
            table: "catalog_table",
            columns: &["a"],
            extra: json!({}),
            delete_database: "delete_db",
            delete_table: "catalog_table",
            delete_extra: json!({ "CatalogID": ACCOUNT_ID }),
        },
        DeletionCase {
            case: "a create that named the catalog and a delete that does not",
            database: "delete_db",
            table: "created_in_catalog_table",
            columns: &["a"],
            extra: json!({ "CatalogID": ACCOUNT_ID }),
            delete_database: "delete_db",
            delete_table: "created_in_catalog_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "a delete addressing the names in another casing",
            database: "Delete_Mixed_DB",
            table: "Mixed_Case_Table",
            columns: &["a"],
            extra: json!({ "Schedule": DAILY_SCHEDULE }),
            delete_database: "delete_mixed_db",
            delete_table: "mixed_case_table",
            delete_extra: json!({}),
        },
        DeletionCase {
            case: "an entry in a second database of the same catalog",
            database: "other_delete_db",
            table: "elsewhere_table",
            columns: &["a", "b"],
            extra: json!({ "ColumnNameList": ["a"] }),
            delete_database: "other_delete_db",
            delete_table: "elsewhere_table",
            delete_extra: json!({}),
        },
    ];

    for row in cases {
        let case = row.case;
        store_table(&router, row.database, row.table, row.columns).await;
        ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body(row.database, row.table, row.extra.clone()),
        )
        .await;
        // The entry is there to be deleted: the read reports it before the delete runs.
        let before = get_settings(&router, &read_body(row.database, row.table)).await;
        assert!(
            before.is_object(),
            "case {case:?}: the read must report the created settings, got {before}"
        );

        let body = delete_body(row.delete_database, row.delete_table, row.delete_extra.clone());
        delete_settings(&router, &body).await;

        // The entry is gone, however the read addresses it, and a second delete of the same
        // address has nothing left to remove.
        assert_read_is_miss(
            &router,
            case,
            &read_body(row.database, row.table),
            SETTINGS_MISS_MESSAGE,
        )
        .await;
        assert_read_is_miss(
            &router,
            case,
            &read_body(row.delete_database, row.delete_table),
            SETTINGS_MISS_MESSAGE,
        )
        .await;
        assert_delete_is_miss(&router, case, &body, SETTINGS_MISS_MESSAGE).await;

        // The table itself survived the delete: it still takes a create of its settings,
        // and the read reports what that second create stated. A create stating no schedule
        // starts a run, and a table with a run in flight takes no create — so the run the
        // first create started is settled first, which is what the clock owed it anyway.
        set_due_instant(&engine, already_past());
        ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body(row.database, row.table, json!({ "SampleSize": 10.0 })),
        )
        .await;
        let recreated = get_settings(&router, &read_body(row.database, row.table)).await;
        assert_eq!(
            recreated.get("SampleSize").and_then(Value::as_f64),
            Some(10.0),
            "case {case:?}: the re-created settings are the ones the second create stated, got \
             {recreated}"
        );
        delete_settings(&router, &body).await;
    }
}

// ---------------------------------------------------------------------------
// The delete removes NOTHING else
// ---------------------------------------------------------------------------

/// A delete removes the one entry it addresses: the entries of every other table — in the
/// same database and in another — still read back exactly as their creates wrote them, and
/// the deleted entry's own table is still a stored table.
#[tokio::test]
async fn a_delete_leaves_every_other_entry_and_the_table_itself_in_place() {
    let router = emulator();

    // (database, table) — three entries in one database and one in a second, so a delete is
    // checked against neighbours on both sides of the address.
    let entries = [
        ("neighbour_db", "first_table"),
        ("neighbour_db", "second_table"),
        ("neighbour_db", "third_table"),
        ("second_neighbour_db", "first_table"),
    ];
    for (database, table) in entries {
        // The table declares the column its own settings list, so each entry's
        // `ColumnNameList` names a real column of its table AND identifies the entry.
        store_table(&router, database, table, &[table, "a"]).await;
        ok(
            &router,
            "CreateColumnStatisticsTaskSettings",
            &create_body(
                database,
                table,
                json!({ "ColumnNameList": [table], "Schedule": DAILY_SCHEDULE }),
            ),
        )
        .await;
    }

    for (deleted_so_far, (database, table)) in entries.iter().enumerate() {
        let (database, table) = (*database, *table);
        let case = format!("deleting {database}.{table}");
        delete_settings(&router, &delete_body(database, table, json!({}))).await;

        // The table the entry hung on is still stored, so only the entry was removed.
        let read_back = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            read_back["Table"]["Name"].as_str(),
            Some(table),
            "case {case:?}: the delete leaves the table itself stored, got {read_back}"
        );

        // Every entry this delete did not address still reports what its create wrote.
        for (index, (other_database, other_table)) in entries.iter().enumerate() {
            let (other_database, other_table) = (*other_database, *other_table);
            let body = read_body(other_database, other_table);
            if index <= deleted_so_far {
                assert_read_is_miss(
                    &router,
                    &format!("{case}: {other_database}.{other_table} is already deleted"),
                    &body,
                    SETTINGS_MISS_MESSAGE,
                )
                .await;
                continue;
            }
            let settings = get_settings(&router, &body).await;
            assert_eq!(
                settings["ColumnNameList"],
                json!([other_table]),
                "case {case:?}: the entry of {other_database}.{other_table} is untouched, got \
                 {settings}"
            );
        }
    }
}

/// A delete removes the settings, not the history: the column statistics task runs recorded
/// for that table are still reported after it, because they are the table's runs and not
/// part of the settings entry.
#[tokio::test]
async fn a_delete_leaves_the_statistics_runs_of_that_table_in_place() {
    let router = emulator();
    store_table(&router, "history_db", "history_table", &["a", "b"]).await;
    // The settings state a SCHEDULE, so the create starts no run of its own and the one run
    // below is the whole history this delete must leave in place.
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body(
            "history_db",
            "history_table",
            json!({ "Schedule": DAILY_SCHEDULE }),
        ),
    )
    .await;
    let started = ok(
        &router,
        "StartColumnStatisticsTaskRun",
        &json!({
            "DatabaseName": "history_db",
            "TableName": "history_table",
            "Role": crate::column_statistics_task_settings::ROLE,
        }),
    )
    .await;
    let run_id = started["ColumnStatisticsTaskRunId"]
        .as_str()
        .unwrap_or_else(|| panic!("a start answers a run id, got {started}"))
        .to_string();

    delete_settings(
        &router,
        &delete_body("history_db", "history_table", json!({})),
    )
    .await;

    let run = ok(
        &router,
        "GetColumnStatisticsTaskRun",
        &json!({ "ColumnStatisticsTaskRunId": run_id }),
    )
    .await;
    assert_eq!(
        run["ColumnStatisticsTaskRun"]["ColumnStatisticsTaskRunId"].as_str(),
        Some(run_id.as_str()),
        "case {:?}: the run recorded for the table outlives its settings, got {run}",
        "a run of a table whose settings were deleted"
    );
    assert_read_is_miss(
        &router,
        "a run of a table whose settings were deleted",
        &read_body("history_db", "history_table"),
        SETTINGS_MISS_MESSAGE,
    )
    .await;
}

// ---------------------------------------------------------------------------
// The misses
// ---------------------------------------------------------------------------

/// A delete that finds no entry under the names it addressed removes nothing and is refused
/// with the modelled miss — in the settings entry's own fixed sentence, which withholds both
/// names the request addressed the entry by, whichever half of that address missed.
#[tokio::test]
async fn a_delete_with_no_entry_to_remove_is_the_modelled_miss() {
    let router = emulator();
    store_table(&router, "miss_db", "bare_table", &["a"]).await;
    store_table(&router, "miss_db", "kept_table", &["a", "b"]).await;
    store_table(&router, "elsewhere_db", "elsewhere_table", &["a"]).await;
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body("miss_db", "kept_table", json!({ "ColumnNameList": ["a"] })),
    )
    .await;

    // (case, the delete that must be refused, the sentence it must carry) — the rows differ
    // along HOW the address misses: a stored table carrying no entry, a table name nothing
    // created, the name of a table another database holds, a stored name with a suffix, the
    // shortest and longest names `NameString` admits (`length 1..255`,
    // `smithy_query operation=DeleteColumnStatisticsTaskSettings`), a catalog of another
    // account, and a database the catalog does not hold.
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "a stored table carrying no entry",
            delete_body("miss_db", "bare_table", json!({})),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
        (
            "a table name nothing created",
            delete_body("miss_db", "no_such_table", json!({})),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
        (
            "the name of a table another database holds",
            delete_body("miss_db", "elsewhere_table", json!({})),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
        (
            "a stored name carrying a suffix",
            delete_body("miss_db", "kept_table-does-not-exist", json!({})),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
        (
            "the shortest table name the model admits",
            delete_body("miss_db", "t", json!({})),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
        (
            "the longest table name the model admits",
            delete_body("miss_db", &"t".repeat(255), json!({})),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
        (
            "a catalog of another account",
            delete_body("miss_db", "kept_table", json!({ "CatalogID": "210987654321" })),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
        (
            "a database the catalog does not hold",
            delete_body("no_such_db", "kept_table", json!({})),
            SETTINGS_MISS_MESSAGE.to_string(),
        ),
    ];

    for (case, body, message) in cases {
        assert_delete_is_miss(&router, case, &body, &message).await;

        // The refused delete removed nothing: the one stored entry still reads back.
        let settings = get_settings(&router, &read_body("miss_db", "kept_table")).await;
        assert_eq!(
            settings["ColumnNameList"],
            json!(["a"]),
            "case {case:?}: a refused delete removes nothing, got {settings}"
        );
    }

    // And the entry a delete DOES address is still removable afterwards.
    delete_settings(&router, &delete_body("miss_db", "kept_table", json!({}))).await;
    assert_read_is_miss(
        &router,
        "the stored entry after every refused delete",
        &read_body("miss_db", "kept_table"),
        SETTINGS_MISS_MESSAGE,
    )
    .await;
}

/// A table name outside the modelled `length 1..255` is refused at the boundary, before the
/// handler runs, and removes nothing.
#[tokio::test]
async fn a_table_name_outside_the_modelled_length_is_refused() {
    let router = emulator();
    store_table(&router, "bounds_db", "bounds_table", &["a"]).await;
    ok(
        &router,
        "CreateColumnStatisticsTaskSettings",
        &create_body("bounds_db", "bounds_table", json!({})),
    )
    .await;

    // (case, the `TableName`) — one past each published bound of `NameString`
    // (`length 1..255`, `smithy_query operation=DeleteColumnStatisticsTaskSettings`).
    for (case, table) in [
        ("a name shorter than the model admits", String::new()),
        ("a name longer than the model admits", "t".repeat(256)),
    ] {
        let (status, parsed) = call(
            &router,
            "DeleteColumnStatisticsTaskSettings",
            &delete_body("bounds_db", &table, json!({})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name outside the modelled length is refused, got {status} with \
             {parsed}"
        );
        // The refused delete removed nothing.
        let settings = get_settings(&router, &read_body("bounds_db", "bounds_table")).await;
        assert!(
            settings.is_object(),
            "case {case:?}: the stored entry survives a refused delete, got {settings}"
        );
    }
}
