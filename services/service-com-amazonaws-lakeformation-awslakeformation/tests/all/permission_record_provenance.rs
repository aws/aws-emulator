// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the update PROVENANCE a `ListPermissions` entry reports.
//!
//! RULE: an entry that reports a permission RECORD carries the provenance of the write that
//! last set that record — `PrincipalResourcePermissions$LastUpdated` ("The date and time when
//! the resource was last updated") and `$LastUpdatedBy` ("The user who updated the record",
//! `NameString`, length 1..255) — recorded when the write happened, so two reads of one
//! unchanged record report one instant and a later write to it moves that instant forward.
//! A listing that answers EFFECTIVE permissions instead — the one that names BOTH a
//! `Principal` and a `Resource`, where "the response returns effective permissions rather
//! than the explicitly granted permissions" (`ListPermissions`) — answers what the principal
//! HOLDS rather than the records holding it, so its entries carry no record provenance.
//!
//! The rows span what the rule generalises over: the WRITE PATH that set the record
//! (`GrantPermissions`, `BatchGrantPermissions`, a second grant that added to a record, and
//! the two revoke paths that trim one), the resource FORM the record is held on (a database,
//! a table, a table's columns), and the FILTER the listing narrows by (the principal, the
//! resource, nothing at all, and both together).
//!
//! Every call goes over the wire the way an SDK client does, so a response can only be what
//! the handler computed from stored state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::glue_catalog::{
    account_catalog, create_database_with_input, create_table, emulator, lake_formation as call,
    list_permissions as list,
};

/// The database every row grants on, created handing its tables no default permissions, so
/// what a listing reports is this suite's own grants and what the table's creation made.
const DATABASE: &str = "ledger";

/// Its table.
const TABLE: &str = "entries";

/// The two members that carry a record's provenance.
const LAST_UPDATED: &str = "LastUpdated";
const LAST_UPDATED_BY: &str = "LastUpdatedBy";

/// An emulator whose catalog holds [`DATABASE`] and its [`TABLE`].
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_database_with_input(
        &router,
        &json!({ "Name": DATABASE, "CreateTableDefaultPermissions": [] }),
    )
    .await;
    create_table(&router, DATABASE, TABLE).await;
    router
}

/// The resource forms a record is held on.
fn on_database() -> Value {
    json!({ "Database": { "Name": DATABASE } })
}

fn on_table() -> Value {
    json!({ "Table": { "DatabaseName": DATABASE, "Name": TABLE } })
}

fn on_columns() -> Value {
    json!({
        "TableWithColumns": { "DatabaseName": DATABASE, "Name": TABLE, "ColumnWildcard": {} }
    })
}

/// The instant one entry reports, as the epoch-seconds number the wire carries.
fn reported_instant(case: &str, entry: &Value) -> f64 {
    entry
        .get(LAST_UPDATED)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {LAST_UPDATED} on the entry, got {entry:?}")
        })
}

/// Assert every entry of `listed` reports the provenance of the write that set it, and the
/// members its record holds beside it.
fn assert_entries_with_provenance(case: &str, listed: &[Value]) {
    assert!(
        !listed.is_empty(),
        "case {case:?}: expected the listing to report the record, got no entry"
    );
    for entry in listed {
        let at = reported_instant(case, entry);
        assert!(
            at > 0.0,
            "case {case:?}: expected an instant past the epoch, got {at} in {entry:?}"
        );
        let by = entry
            .get(LAST_UPDATED_BY)
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            (1..=255).contains(&by.chars().count()),
            "case {case:?}: expected {LAST_UPDATED_BY} inside NameString's 1..255, got {entry:?}"
        );
        assert!(
            entry.get("Principal").is_some()
                && entry.get("Resource").is_some()
                && entry
                    .get("Permissions")
                    .and_then(Value::as_array)
                    .is_some_and(|held| !held.is_empty()),
            "case {case:?}: expected the entry to report what the record holds, got {entry:?}"
        );
    }
}

/// Call `operation` with `body`, asserting the service accepted it.
async fn accepted(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to be accepted, got {status} {payload:?}"
    );
}

/// One row: the case, the resource form the record is held on, and the calls that make the
/// record — each an operation name and the body it sends.
type Row = (&'static str, Value, Vec<(&'static str, Value)>);

#[tokio::test]
async fn every_write_path_leaves_a_record_reporting_its_provenance() {
    let catalog = account_catalog();
    let analyst = format!("arn:aws:iam::{catalog}:role/analyst");

    // Each row: the case, the resource form the record is held on, and the calls that make
    // the record — the write paths this API has for writing one.
    let rows: Vec<Row> = vec![
        (
            "a database record one grant wrote",
            on_database(),
            vec![(
                "GrantPermissions",
                json!({
                    "Principal": { "DataLakePrincipalIdentifier": analyst },
                    "Resource": on_database(),
                    "Permissions": ["DESCRIBE"],
                }),
            )],
        ),
        (
            "a table record a batch grant wrote",
            on_table(),
            vec![(
                "BatchGrantPermissions",
                json!({ "Entries": [{
                    "Id": "one",
                    "Principal": { "DataLakePrincipalIdentifier": analyst },
                    "Resource": on_table(),
                    "Permissions": ["DESCRIBE"],
                }]}),
            )],
        ),
        (
            "a column record one grant wrote",
            on_columns(),
            vec![(
                "GrantPermissions",
                json!({
                    "Principal": { "DataLakePrincipalIdentifier": analyst },
                    "Resource": on_columns(),
                    "Permissions": ["SELECT"],
                }),
            )],
        ),
        (
            "a table record a second grant added to",
            on_table(),
            vec![
                (
                    "GrantPermissions",
                    json!({
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Resource": on_table(),
                        "Permissions": ["DESCRIBE"],
                    }),
                ),
                (
                    "GrantPermissions",
                    json!({
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Resource": on_table(),
                        "Permissions": ["ALTER"],
                    }),
                ),
            ],
        ),
        (
            "a table record a revoke trimmed",
            on_table(),
            vec![
                (
                    "GrantPermissions",
                    json!({
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Resource": on_table(),
                        "Permissions": ["DESCRIBE", "ALTER"],
                    }),
                ),
                (
                    "RevokePermissions",
                    json!({
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Resource": on_table(),
                        "Permissions": ["ALTER"],
                    }),
                ),
            ],
        ),
        (
            "a table record a batch revoke trimmed",
            on_table(),
            vec![
                (
                    "GrantPermissions",
                    json!({
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Resource": on_table(),
                        "Permissions": ["DESCRIBE", "ALTER"],
                    }),
                ),
                (
                    "BatchRevokePermissions",
                    json!({ "Entries": [{
                        "Id": "one",
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Resource": on_table(),
                        "Permissions": ["ALTER"],
                    }]}),
                ),
            ],
        ),
    ];

    for (case, resource, writes) in rows {
        let router = seeded().await;
        for (operation, body) in &writes {
            accepted(&router, case, operation, body).await;
        }

        // Read back under each filter a listing of RECORDS can be narrowed by, since the
        // projection is one for every entry of every answer.
        for (filter_case, filter) in [
            (
                "by principal",
                json!({ "Principal": { "DataLakePrincipalIdentifier": analyst } }),
            ),
            ("by resource", json!({ "Resource": resource.clone() })),
            ("unfiltered", json!({})),
        ] {
            let case = format!("{case}, listed {filter_case}");
            let listed = list(&router, &case, &filter).await;
            assert_entries_with_provenance(&case, &listed);
        }

        // The EFFECTIVE listing of the same records — both a Principal and a Resource —
        // answers what the principal holds rather than the records, so it reports no record
        // provenance.
        let case = format!("{case}, listed effectively");
        let listed = list(
            &router,
            &case,
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": analyst },
                "Resource": resource,
            }),
        )
        .await;
        assert!(
            !listed.is_empty(),
            "case {case:?}: expected the effective listing to report what the principal holds, \
             got no entry"
        );
        for entry in &listed {
            for member in [LAST_UPDATED, LAST_UPDATED_BY] {
                assert_eq!(
                    entry.get(member),
                    None,
                    "case {case:?}: an effective answer reports no {member}, got {entry:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_reported_instant_is_the_writes_own_and_moves_only_with_a_write() {
    let catalog = account_catalog();
    let analyst = format!("arn:aws:iam::{catalog}:role/analyst");
    let router = seeded().await;
    let filter = json!({ "Principal": { "DataLakePrincipalIdentifier": analyst } });
    let grant = |permissions: Value| {
        json!({
            "Principal": { "DataLakePrincipalIdentifier": analyst },
            "Resource": on_table(),
            "Permissions": permissions,
        })
    };

    accepted(&router, "the first grant", "GrantPermissions", &grant(json!(["DESCRIBE"]))).await;
    let first = list(&router, "the record as first written", &filter).await;
    let written = reported_instant("the record as first written", &first[0]);

    // Read AGAIN, without writing anything: a read reports the write's own instant, so the
    // same record reports the same instant however often it is read.
    let again = list(&router, "the record read again", &filter).await;
    assert_eq!(
        reported_instant("the record read again", &again[0]),
        written,
        "two reads of one unchanged record must report one instant, got {first:?} then {again:?}"
    );

    // A WRITE to the record moves it: the grant that adds a permission is the write a later
    // read reports.
    accepted(&router, "the second grant", "GrantPermissions", &grant(json!(["ALTER"]))).await;
    let updated = list(&router, "the record after the second grant", &filter).await;
    assert!(
        reported_instant("the record after the second grant", &updated[0]) >= written,
        "a write must not move the reported instant backwards, got {updated:?} after {first:?}"
    );
}
