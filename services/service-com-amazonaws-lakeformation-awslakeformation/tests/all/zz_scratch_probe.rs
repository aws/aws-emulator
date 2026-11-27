// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the provenance a `ListPermissions` entry reports for a grant no call
//! of this API made: one a Data Catalog resource was CREATED carrying.
//!
//! RULE: the provenance a listed entry reports is the write that established the grant. For a
//! grant a resource was created carrying — the Super a table's creator holds on it, the
//! default permissions its database hands the tables created in it, and the database-level
//! default the account's data lake settings grant — that write is the CREATE of the resource
//! in the Data Catalog, so `LastUpdated` is the instant the catalog records for it rather than
//! the instant the listing ran: the entry reports the same instant however often it is read,
//! and that instant is not after the read that reports it.
//!
//! The rows span the two kinds of creation-time grant the service renders (a table's and a
//! database's) and the filters a listing narrows them by.

use serde_json::{json, Value};

use crate::glue_catalog::{create_database_with_input, create_table, emulator, list_permissions};

/// The database the rows grant on, created with the catalog's own default so the database
/// hands its tables their default permissions too.
const DATABASE: &str = "warehouse";

/// Its table.
const TABLE: &str = "shipments";

/// The member carrying the instant of the write that established a grant.
const LAST_UPDATED: &str = "LastUpdated";

/// The instant one entry reports, as the epoch-seconds number the wire carries.
fn reported_instant(case: &str, entry: &Value) -> f64 {
    entry
        .get(LAST_UPDATED)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {LAST_UPDATED} on the entry, got {entry:?}")
        })
}

#[tokio::test]
async fn a_grant_a_resource_was_created_carrying_reports_the_creates_own_instant() {
    let router = emulator();
    create_database_with_input(&router, &json!({ "Name": DATABASE })).await;
    create_table(&router, DATABASE, TABLE).await;

    // The rows: the grants the table's creation made, the one the database's creation made,
    // and every creation-time grant of the catalog at once.
    for (case, filter) in [
        (
            "the grants a table's creation made",
            json!({ "Resource": { "Table": { "DatabaseName": DATABASE, "Name": TABLE } } }),
        ),
        (
            "the grant a database's creation made",
            json!({ "Resource": { "Database": { "Name": DATABASE } } }),
        ),
        ("every creation-time grant", json!({})),
    ] {
        let listed = list_permissions(&router, case, &filter).await;
        assert!(
            !listed.is_empty(),
            "case {case:?}: expected the creation-time grants to be reported, got no entry"
        );

        for entry in &listed {
            let at = reported_instant(case, entry);
            assert!(
                at > 0.0,
                "case {case:?}: expected an instant past the epoch, got {at} in {entry:?}"
            );
        }

        // Read AGAIN: nothing was written in between, so every entry reports the instant its
        // resource was created at and not the instant of the read.
        let again = list_permissions(&router, case, &filter).await;
        let before: Vec<f64> = listed
            .iter()
            .map(|entry| reported_instant(case, entry))
            .collect();
        let after: Vec<f64> = again
            .iter()
            .map(|entry| reported_instant(case, entry))
            .collect();
        assert_eq!(
            before, after,
            "case {case:?}: two reads of the same creation-time grants must report one \
             instant each, got {before:?} then {after:?}"
        );
    }
}
