// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the Lake Formation permission grant store:
//! `GrantPermissions`, `ListPermissions`, `RevokePermissions`.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store. Each suite under
//! `tests/all/` carries its own dispatch harness, which is the convention this crate's
//! other suite (`lf_tags.rs`) already follows.
//!
//! The tables below span the classes the rules generalise over (resource kind, permission
//! set, repeated grant, filter member, page bound); the issue's own request — a grant to
//! `IAM_ALLOWED_PRINCIPALS` on a table, granted twice and revoked — is one row among
//! them.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one copy of the harness every permission suite shares: the Glue+LakeFormation
// emulator, the seeded catalog, the `restJson1` wire path and the error-code extractor.
use crate::glue_catalog::{
    account_catalog, create_database, create_database_with_input, create_table, emulator,
    error_code, lake_formation as call, permissions_not_held_message,
    revoke_default_database_grant, revoke_table_default_grants, NOTHING_TO_REVOKE,
};

/// One row of the repeated-grant table: the case name, the permissions each successive
/// `GrantPermissions` call names, the permissions the records hold afterwards, and how
/// many records that is — one per SCOPE the permissions apply at (a table's own record,
/// its "table w columns" record, or both).
type RepeatedGrantCase = (
    &'static str,
    Vec<Vec<&'static str>>,
    Vec<&'static str>,
    usize,
);

/// One row of the absent-revoke table: the case name, what was granted first (if
/// anything), the principal, resource and permissions the revoke names, and the whole
/// sentence the refusal must carry.
type AbsentRevokeCase = (
    &'static str,
    Option<Vec<&'static str>>,
    &'static str,
    Value,
    Vec<&'static str>,
    String,
);

/// One row of the partial-revoke table: the case name, the permissions granted, those
/// granted WITH the grant option, those revoked, and what remains afterwards (`None`
/// when the record itself is gone).
type PartialRevokeCase = (
    &'static str,
    Vec<&'static str>,
    Vec<&'static str>,
    Vec<&'static str>,
    Option<Vec<&'static str>>,
);

/// The Data Catalog this suite's rows grant on: the `sales` database with its `orders`
/// and `returns` tables, seeded through the shared harness.
///
/// This suite counts the records its own grants wrote, so the grants creating the two
/// tables made — their creator's and the database's table-level default — are revoked
/// first, the same reason its rows revoke the database's creation-time default. Those
/// grants are covered by the `creator_table_grants.rs` and `table_default_grants.rs`
/// suites.
async fn emulator_with_catalog() -> MultiServiceRouter {
    let router =
        crate::glue_catalog::emulator_with_catalog(&[("sales", &["orders", "returns"])]).await;
    for table in ["orders", "returns"] {
        revoke_table_default_grants(&router, "sales", table).await;
    }
    router
}

/// `GrantPermissions`, asserting it was accepted.
async fn grant(
    router: &MultiServiceRouter,
    case: &str,
    principal: &str,
    resource: &Value,
    permissions: &[&str],
) {
    let body = json!({
        "Principal": { "DataLakePrincipalIdentifier": principal },
        "Resource": resource,
        "Permissions": permissions,
    });
    let (status, payload) = call(router, "GrantPermissions", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GrantPermissions to answer 200, got {status} {payload:?}"
    );
}

/// The `PrincipalResourcePermissions` of a `ListPermissions` call, asserting 200 and that
/// the member is present (an empty listing is an EMPTY LIST, never an absent member).
async fn list(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<Value> {
    let (status, payload) = call(router, "ListPermissions", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPermissions to answer 200, got {status} {payload:?}"
    );
    let entries = payload.get("PrincipalResourcePermissions").cloned();
    let entries = entries.unwrap_or_else(|| {
        panic!("case {case:?}: ListPermissions must carry PrincipalResourcePermissions, got {payload:?}")
    });
    entries
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PrincipalResourcePermissions must be a list, got {entries:?}")
        })
        .clone()
}

/// The `Permissions` of one entry, as owned strings.
fn permissions_of(entry: &Value) -> Vec<String> {
    entry
        .get("Permissions")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `DataLakePrincipalIdentifier` of one entry.
fn principal_of(entry: &Value) -> String {
    entry
        .get("Principal")
        .and_then(|principal| principal.get("DataLakePrincipalIdentifier"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_grant_is_read_back_on_every_resource_kind_the_model_carries() {
    // One row per member of `Resource`
    // (`smithy_query(query="closure",operation="ListPermissions")`), each with the
    // permissions `Permission` admits for it (`PERMISSION_VALUES`), and the resource the
    // listing must echo: the catalog id the model documents a default for is filled in
    // ("By default, it is the account ID of the caller." — `TableResource$CatalogId`),
    // while `CatalogResource$Id` and `DataCellsFilterResource$TableCatalogId`, which
    // document no default, are echoed exactly as granted.
    let catalog = account_catalog();
    let cases: Vec<(&str, Value, Vec<&str>, Value)> = vec![
        (
            "catalog",
            json!({ "Catalog": {} }),
            vec!["CREATE_DATABASE"],
            json!({ "Catalog": {} }),
        ),
        (
            "database",
            json!({ "Database": { "Name": "sales" } }),
            vec!["CREATE_TABLE", "DESCRIBE"],
            json!({ "Database": { "CatalogId": catalog, "Name": "sales" } }),
        ),
        (
            "table",
            json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } }),
            vec!["ALL"],
            json!({ "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "orders" } }),
        ),
        (
            "table wildcard",
            json!({ "Table": { "DatabaseName": "sales", "TableWildcard": {} } }),
            vec!["SELECT"],
            json!({
                "Table": { "CatalogId": catalog, "DatabaseName": "sales", "TableWildcard": {} }
            }),
        ),
        (
            "table with named columns",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["amount", "currency"],
                }
            }),
            vec!["SELECT"],
            json!({
                "TableWithColumns": {
                    "CatalogId": catalog,
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["amount", "currency"],
                }
            }),
        ),
        (
            "table with a column wildcard",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "returns",
                    "ColumnWildcard": { "ExcludedColumnNames": ["ssn"] },
                }
            }),
            vec!["SELECT"],
            json!({
                "TableWithColumns": {
                    "CatalogId": catalog,
                    "DatabaseName": "sales",
                    "Name": "returns",
                    "ColumnWildcard": { "ExcludedColumnNames": ["ssn"] },
                }
            }),
        ),
        (
            "data location",
            json!({ "DataLocation": { "ResourceArn": "arn:aws:s3:::lake/raw" } }),
            vec!["DATA_LOCATION_ACCESS"],
            json!({
                "DataLocation": { "CatalogId": catalog, "ResourceArn": "arn:aws:s3:::lake/raw" }
            }),
        ),
        (
            "data cells filter",
            json!({
                "DataCellsFilter": {
                    "DatabaseName": "sales",
                    "TableName": "orders",
                    "Name": "eu-rows",
                }
            }),
            vec!["SELECT"],
            json!({
                "DataCellsFilter": {
                    "TableCatalogId": catalog,
                    "DatabaseName": "sales",
                    "TableName": "orders",
                    "Name": "eu-rows",
                }
            }),
        ),
        (
            "lf-tag",
            json!({ "LFTag": { "TagKey": "env", "TagValues": ["dev", "prod"] } }),
            vec!["ASSOCIATE"],
            json!({
                "LFTag": { "CatalogId": catalog, "TagKey": "env", "TagValues": ["dev", "prod"] }
            }),
        ),
        (
            "lf-tag policy",
            json!({
                "LFTagPolicy": {
                    "ResourceType": "TABLE",
                    "Expression": [{ "TagKey": "env", "TagValues": ["prod"] }],
                }
            }),
            vec!["SELECT", "DESCRIBE"],
            json!({
                "LFTagPolicy": {
                    "CatalogId": catalog,
                    "ResourceType": "TABLE",
                    "Expression": [{ "TagKey": "env", "TagValues": ["prod"] }],
                }
            }),
        ),
        (
            "lf-tag expression",
            json!({ "LFTagExpression": { "Name": "prod-only" } }),
            vec!["GRANT_WITH_LF_TAG_EXPRESSION"],
            json!({ "LFTagExpression": { "CatalogId": catalog, "Name": "prod-only" } }),
        ),
        (
            "rls policy",
            json!({ "RLSPolicy": { "Name": "eu-only" } }),
            vec!["SELECT"],
            json!({ "RLSPolicy": { "CatalogId": catalog, "Name": "eu-only" } }),
        ),
    ];

    for (case, resource, permissions, expected_resource) in cases {
        // A fresh emulator per row, so a listing's single entry can only be this grant —
        // with the database's creation-time default grant revoked, since this row is about
        // the record the grant wrote rather than the one the database was created with.
        let router = emulator_with_catalog().await;
        revoke_default_database_grant(&router, "sales").await;
        let principal = format!("arn:aws:iam::{}:role/analyst", account_catalog());
        grant(&router, case, &principal, &resource, &permissions).await;

        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly one entry on the granted resource, got {entries:?}"
        );
        let entry = &entries[0];
        assert_eq!(
            principal_of(entry),
            principal,
            "case {case:?}: expected the grantee {principal:?}, got {entry:?}"
        );
        assert_eq!(
            permissions_of(entry),
            permissions,
            "case {case:?}: expected the granted permissions {permissions:?}, got {entry:?}"
        );
        assert_eq!(
            entry.get("Resource"),
            Some(&expected_resource),
            "case {case:?}: expected the resource {expected_resource:?}, got {entry:?}"
        );
        assert_eq!(
            entry.get("PermissionsWithGrantOption"),
            Some(&json!([])),
            "case {case:?}: a grant with no grant option carries an empty list, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn granting_again_unions_into_the_records_it_already_holds() {
    // Rows differ along what the rule generalises over: the same permissions repeated, a
    // disjoint second set, an overlapping one, more than two calls, and a repeat inside
    // one call.
    let resource = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let cases: Vec<RepeatedGrantCase> = vec![
        // The issue's own case: the same grant twice is exactly one entry.
        ("repeated", vec![vec!["ALL"], vec!["ALL"]], vec!["ALL"], 1),
        (
            "disjoint",
            vec![vec!["SELECT"], vec!["ALTER"]],
            vec!["ALTER", "SELECT"],
            2,
        ),
        (
            "overlapping",
            vec![vec!["SELECT", "DESCRIBE"], vec!["DESCRIBE", "DROP"]],
            vec!["DESCRIBE", "DROP", "SELECT"],
            2,
        ),
        (
            "three calls",
            vec![vec!["SELECT"], vec!["SELECT"], vec!["INSERT"]],
            vec!["INSERT", "SELECT"],
            2,
        ),
        (
            "repeated within one call",
            vec![vec!["SELECT", "SELECT"]],
            vec!["SELECT"],
            1,
        ),
    ];

    for (case, grants, expected, scopes_expected) in cases {
        let router = emulator_with_catalog().await;
        for permissions in &grants {
            grant(
                &router,
                case,
                "IAM_ALLOWED_PRINCIPALS",
                &resource,
                permissions,
            )
            .await;
        }

        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        // A table grant is recorded at the SCOPE each permission applies at, so a
        // column-scoped `SELECT` is its own "table w columns" entry beside the table's
        // own (see `crate::state::scoped_grant_records`). What repeating a grant must not
        // do is open another entry at a scope: there is EXACTLY one entry per scope the
        // permissions apply at, and together they hold exactly the union granted.
        assert_eq!(
            entries.len(),
            scopes_expected,
            "case {case:?}: {} grants leave {scopes_expected} record(s), got {entries:?}",
            grants.len()
        );
        let scopes: BTreeSet<String> = entries
            .iter()
            .map(|entry| entry["Resource"].to_string())
            .collect();
        assert_eq!(
            scopes.len(),
            entries.len(),
            "case {case:?}: no scope may carry two records, got {entries:?}"
        );
        let mut held: Vec<String> = entries.iter().flat_map(permissions_of).collect();
        held.sort();
        let want: Vec<String> = expected
            .iter()
            .map(|permission| permission.to_string())
            .collect();
        assert_eq!(
            held, want,
            "case {case:?}: expected the union {want:?}, got {entries:?}"
        );
    }
}

#[tokio::test]
async fn revoking_what_was_never_granted_is_refused_and_changes_nothing() {
    // Rows differ along WHY nothing is there to revoke: no grant at all, a permission the
    // grant does not carry, another principal's grant, another resource — and, across them,
    // along WHICH of the two nothing-was-revoked sentences that draws: the grantee the
    // service always models is told which permissions it does not have, a grantee that
    // holds nothing on the resource is told it holds nothing.
    let resource = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let other_resource = json!({ "Table": { "DatabaseName": "sales", "Name": "returns" } });
    let cases: Vec<AbsentRevokeCase> = vec![
        // The issue's own case: a revoke of an absent IAM_ALLOWED_PRINCIPALS grant.
        (
            "no grant at all",
            None,
            "IAM_ALLOWED_PRINCIPALS",
            resource.clone(),
            vec!["ALL"],
            permissions_not_held_message(&["ALL"]),
        ),
        (
            "permission not held",
            Some(vec!["SELECT"]),
            "IAM_ALLOWED_PRINCIPALS",
            resource.clone(),
            vec!["DROP"],
            permissions_not_held_message(&["DROP"]),
        ),
        (
            "one held and one not",
            Some(vec!["SELECT"]),
            "IAM_ALLOWED_PRINCIPALS",
            resource.clone(),
            vec!["SELECT", "DROP"],
            permissions_not_held_message(&["SELECT", "DROP"]),
        ),
        (
            // Another principal's grant is not this grantee's, and this grantee holds
            // nothing of its own here, so it is told exactly that.
            "another principal",
            Some(vec!["ALL"]),
            "arn:aws:iam::111122223333:role/other",
            resource.clone(),
            vec!["ALL"],
            NOTHING_TO_REVOKE.to_string(),
        ),
        (
            "another resource",
            Some(vec!["ALL"]),
            "IAM_ALLOWED_PRINCIPALS",
            other_resource,
            vec!["ALL"],
            permissions_not_held_message(&["ALL"]),
        ),
    ];

    for (case, granted, principal, revoked_resource, revoked, expected) in cases {
        let router = emulator_with_catalog().await;
        if let Some(permissions) = &granted {
            grant(
                &router,
                case,
                "IAM_ALLOWED_PRINCIPALS",
                &resource,
                permissions,
            )
            .await;
        }

        let (status, payload) = call(
            &router,
            "RevokePermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": principal },
                "Resource": revoked_resource,
                "Permissions": revoked,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a revoke of {revoked:?} to be refused with 400, got \
             {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {payload:?}"
        );
        // Which of the two nothing-was-revoked sentences this row draws: the per-permission
        // difference for a grantee the service models on the resource, the fixed
        // holds-nothing sentence otherwise.
        let message = payload
            .get("message")
            .or_else(|| payload.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected Message {expected:?}, got {message:?}"
        );

        // The refusal took nothing away: whatever was granted is still there.
        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        match &granted {
            Some(permissions) => {
                assert_eq!(
                    entries.len(),
                    1,
                    "case {case:?}: the refused revoke must leave the grant, got {entries:?}"
                );
                assert_eq!(
                    permissions_of(&entries[0]),
                    *permissions,
                    "case {case:?}: expected {permissions:?} to still be held, got {:?}",
                    entries[0]
                );
            }
            None => assert!(
                entries.is_empty(),
                "case {case:?}: nothing was granted, so nothing is listed, got {entries:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_revoke_takes_away_only_what_it_names() {
    // Rows differ along how much of the grant the revoke names: a part, every part (which
    // drops the record), and the grant-option set alone. Every row names a permission a
    // DATABASE holds, so each is about the revoke rather than about what may be granted
    // there at all.
    let resource = json!({ "Database": { "Name": "sales" } });
    let cases: Vec<PartialRevokeCase> = vec![
        (
            "one of two",
            vec!["DESCRIBE", "ALTER"],
            vec![],
            vec!["DESCRIBE"],
            Some(vec!["ALTER"]),
        ),
        (
            "both of two",
            vec!["DESCRIBE", "ALTER"],
            vec![],
            vec!["DESCRIBE", "ALTER"],
            None,
        ),
        (
            "the only one",
            vec!["ALL"],
            vec![],
            vec!["ALL"],
            None,
        ),
        (
            "leaving the grant option",
            vec!["DESCRIBE"],
            vec!["DESCRIBE"],
            vec!["DESCRIBE"],
            Some(vec![]),
        ),
    ];

    for (case, granted, with_grant_option, revoked, remaining) in cases {
        // The database's creation-time default grant goes first: this table is about what
        // a revoke takes out of the record its own grant wrote.
        let router = emulator_with_catalog().await;
        revoke_default_database_grant(&router, "sales").await;
        let (status, payload) = call(
            &router,
            "GrantPermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                "Resource": resource,
                "Permissions": granted,
                "PermissionsWithGrantOption": with_grant_option,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the grant to be accepted, got {status} {payload:?}"
        );

        let (status, payload) = call(
            &router,
            "RevokePermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                "Resource": resource,
                "Permissions": revoked,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the revoke of {revoked:?} to be accepted, got {status} \
             {payload:?}"
        );

        let entries = list(&router, case, &json!({ "Resource": resource })).await;
        match &remaining {
            Some(permissions) => {
                assert_eq!(
                    entries.len(),
                    1,
                    "case {case:?}: a partly revoked grant is still one entry, got {entries:?}"
                );
                assert_eq!(
                    permissions_of(&entries[0]),
                    *permissions,
                    "case {case:?}: expected {permissions:?} to remain, got {:?}",
                    entries[0]
                );
            }
            None => assert!(
                entries.is_empty(),
                "case {case:?}: a fully revoked grant is dropped, got {entries:?}"
            ),
        }
    }
}

#[tokio::test]
async fn the_filters_select_by_principal_resource_and_resource_type() {
    let router = emulator_with_catalog().await;
    // This table is about the filters over the explicit grant store, so the database's
    // creation-time default grant is revoked before the grants below are made.
    revoke_default_database_grant(&router, "sales").await;
    let catalog = account_catalog();
    let analyst = format!("arn:aws:iam::{catalog}:role/analyst");
    let auditor = format!("arn:aws:iam::{catalog}:role/auditor");
    let database = json!({ "Database": { "Name": "sales" } });
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let location = json!({ "DataLocation": { "ResourceArn": "arn:aws:s3:::lake/raw" } });

    grant(&router, "seed", &analyst, &database, &["DESCRIBE"]).await;
    grant(&router, "seed", &analyst, &table, &["SELECT"]).await;
    grant(&router, "seed", &auditor, &table, &["DESCRIBE"]).await;
    grant(
        &router,
        "seed",
        &analyst,
        &location,
        &["DATA_LOCATION_ACCESS"],
    )
    .await;

    // Rows differ along which filter member narrows the listing, including the catalog
    // that holds no grant at all and a `ResourceType` value with no grant of its kind
    // (`DATA_LAKE_RESOURCE_TYPE_VALUES` carries nine values; CATALOG is tested here, the
    // kinds themselves above).
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no filter", json!({}), 4),
        (
            "principal",
            json!({ "Principal": { "DataLakePrincipalIdentifier": analyst } }),
            3,
        ),
        (
            "other principal",
            json!({ "Principal": { "DataLakePrincipalIdentifier": auditor } }),
            1,
        ),
        ("resource: the table", json!({ "Resource": table }), 2),
        (
            "resource: the database",
            json!({ "Resource": database }),
            1,
        ),
        (
            "resource and principal",
            json!({
                "Resource": table,
                "Principal": { "DataLakePrincipalIdentifier": auditor },
            }),
            1,
        ),
        ("resource type: TABLE", json!({ "ResourceType": "TABLE" }), 2),
        (
            "resource type: DATABASE",
            json!({ "ResourceType": "DATABASE" }),
            1,
        ),
        (
            "resource type: DATA_LOCATION",
            json!({ "ResourceType": "DATA_LOCATION" }),
            1,
        ),
        (
            "resource type with no grant: CATALOG",
            json!({ "ResourceType": "CATALOG" }),
            0,
        ),
        (
            "another catalog",
            json!({ "CatalogId": "111122223333" }),
            0,
        ),
        (
            "principal with no grant",
            json!({ "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" } }),
            0,
        ),
    ];

    for (case, filter, expected) in cases {
        let entries = list(&router, case, &filter).await;
        assert_eq!(
            entries.len(),
            expected,
            "case {case:?}: expected {expected} entries for {filter:?}, got {entries:?}"
        );
    }
}

#[tokio::test]
async fn a_table_listing_also_reports_the_column_level_grant_on_that_table() {
    // `ListPermissions$Resource`: "This operation does not support getting privileges on a
    // table with columns. Instead, call this operation on the table, and the operation
    // returns the table and the table w columns."
    let router = emulator_with_catalog().await;
    let principal = "IAM_ALLOWED_PRINCIPALS";
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let with_columns = json!({
        "TableWithColumns": {
            "DatabaseName": "sales",
            "Name": "orders",
            "ColumnWildcard": {},
        }
    });
    let other_table_columns = json!({
        "TableWithColumns": {
            "DatabaseName": "sales",
            "Name": "returns",
            "ColumnWildcard": {},
        }
    });
    // The same cell filter, named WITHOUT the catalog id the model defaults to the
    // caller's account: one resource, so one record.
    let cells = json!({
        "DataCellsFilter": {
            "DatabaseName": "sales",
            "TableName": "orders",
            "Name": "eu-rows",
        }
    });
    let cells_with_catalog = json!({
        "DataCellsFilter": {
            "TableCatalogId": account_catalog(),
            "DatabaseName": "sales",
            "TableName": "orders",
            "Name": "eu-rows",
        }
    });

    grant(&router, "table", principal, &table, &["DESCRIBE"]).await;
    grant(&router, "columns", principal, &with_columns, &["SELECT"]).await;
    grant(
        &router,
        "other columns",
        principal,
        &other_table_columns,
        &["SELECT"],
    )
    .await;
    grant(&router, "cells", principal, &cells, &["SELECT"]).await;
    grant(
        &router,
        "cells named with the catalog id",
        principal,
        &cells_with_catalog,
        &["SELECT"],
    )
    .await;

    // Rows: the table listing carries the table and its column-level grant but not
    // another table's; the cell filter joins it only with IncludeRelated=TRUE
    // ("Set the field to TRUE to show the cell filters on a table resource").
    let cases: Vec<(&str, Value, usize)> = vec![
        ("the table", json!({ "Resource": table }), 2),
        (
            "the table, related included",
            json!({ "Resource": table, "IncludeRelated": "TRUE" }),
            3,
        ),
        (
            "the table, related excluded explicitly",
            json!({ "Resource": table, "IncludeRelated": "FALSE" }),
            2,
        ),
        (
            "the column-level resource itself",
            json!({ "Resource": with_columns }),
            1,
        ),
        ("the cell filter itself", json!({ "Resource": cells }), 1),
        (
            "the cell filter named with the catalog id",
            json!({ "Resource": cells_with_catalog }),
            1,
        ),
    ];

    for (case, filter, expected) in cases {
        let entries = list(&router, case, &filter).await;
        assert_eq!(
            entries.len(),
            expected,
            "case {case:?}: expected {expected} entries for {filter:?}, got {entries:?}"
        );
    }

    // "The Principal parameter must not be specified when requesting cell filter
    // information." — `ListPermissions$IncludeRelated`.
    let (status, payload) = call(
        &router,
        "ListPermissions",
        &json!({
            "Resource": table,
            "IncludeRelated": "TRUE",
            "Principal": { "DataLakePrincipalIdentifier": principal },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"principal with IncludeRelated\": expected 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidInputException",
        "case \"principal with IncludeRelated\": expected InvalidInputException, got {payload:?}"
    );
}

#[tokio::test]
async fn a_listing_pages_through_every_grant_exactly_once() {
    let router = emulator();
    let principal = "IAM_ALLOWED_PRINCIPALS";
    let databases: Vec<String> = (0..7).map(|index| format!("database-{index}")).collect();
    // Each grant is on a database the catalog holds, so each is accepted.
    for database in &databases {
        create_database(&router, database).await;
    }
    for database in &databases {
        grant(
            &router,
            "seed",
            principal,
            &json!({ "Database": { "Name": database } }),
            &["DESCRIBE"],
        )
        .await;
    }

    // `MaxResults` is modelled `range 1..1000`: every page size from the published lower
    // bound up, plus the upper bound, must walk the seven grants exactly once.
    for page_size in [1_i64, 2, 3, 6, 7, 8, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let (status, payload) = call(&router, "ListPermissions", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {pages}, got {status} {payload:?}"
            );
            let entries = payload["PrincipalResourcePermissions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() as i64 <= page_size,
                "case {case:?}: a page carries at most MaxResults entries, got {entries:?}"
            );
            for entry in &entries {
                seen.push(
                    entry["Resource"]["Database"]["Name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            match payload.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, databases,
            "case {case:?}: expected every grant exactly once, saw {seen:?}"
        );
    }

    // One past each published bound is refused by the modelled `range 1..1000`.
    for page_size in [0_i64, 1001] {
        let case = format!("MaxResults={page_size}");
        let (status, payload) = call(
            &router,
            "ListPermissions",
            &json!({ "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults outside range 1..1000 is refused, got {status} {payload:?}"
        );
    }

    // A token this service did not mint is a bad request, not page one.
    let (status, payload) = call(
        &router,
        "ListPermissions",
        &json!({ "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"foreign NextToken\": expected 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidInputException",
        "case \"foreign NextToken\": expected InvalidInputException, got {payload:?}"
    );
}

#[tokio::test]
async fn a_resource_that_names_no_single_target_is_refused() {
    let router = emulator_with_catalog().await;
    // The closing assertion is that nothing was recorded, so the database's creation-time
    // default grant is revoked first and the listing can only hold what a call wrote.
    revoke_default_database_grant(&router, "sales").await;
    // Rows differ along HOW the `Resource` fails to name exactly one target: none, two,
    // a table named both ways, a table named neither way, columns named both ways and an
    // empty column list.
    let cases: Vec<(&str, Value)> = vec![
        ("no member", json!({})),
        (
            "two members",
            json!({
                "Database": { "Name": "sales" },
                "Table": { "DatabaseName": "sales", "Name": "orders" },
            }),
        ),
        (
            "name and wildcard",
            json!({
                "Table": { "DatabaseName": "sales", "Name": "orders", "TableWildcard": {} }
            }),
        ),
        (
            "neither name nor wildcard",
            json!({ "Table": { "DatabaseName": "sales" } }),
        ),
        (
            "columns and column wildcard",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": ["amount"],
                    "ColumnWildcard": {},
                }
            }),
        ),
        (
            "no column selection",
            json!({ "TableWithColumns": { "DatabaseName": "sales", "Name": "orders" } }),
        ),
        (
            "an empty column list",
            json!({
                "TableWithColumns": {
                    "DatabaseName": "sales",
                    "Name": "orders",
                    "ColumnNames": [],
                }
            }),
        ),
    ];

    for (case, resource) in cases {
        for operation in ["GrantPermissions", "RevokePermissions", "ListPermissions"] {
            let (status, payload) = call(
                &router,
                operation,
                &json!({
                    "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                    "Resource": resource,
                    "Permissions": ["SELECT"],
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {payload:?}"
            );
            assert_eq!(
                error_code(&payload),
                "InvalidInputException",
                "case {case:?} on {operation}: expected InvalidInputException, got {payload:?}"
            );
        }
    }

    // Nothing above was recorded: the store is still empty.
    let entries = list(&router, "after the refusals", &json!({})).await;
    assert!(
        entries.is_empty(),
        "case \"after the refusals\": a refused grant records nothing, got {entries:?}"
    );
}

/// A permission value the `Permission` enum does not carry is refused on BOTH write
/// paths, and nothing is recorded for it.
///
/// `Permission` is modelled as an enum, and a value it does not carry is refused with the
/// `InvalidInputException` both write paths model (HTTP 400), carrying the service's
/// invalid-permissions sentence: the value never reaches the grant store, so no later
/// `ListPermissions` can report it. This pins that for both write paths, so a handler can
/// never start recording an unmodelled value; the sentence itself is pinned by
/// `client_error_bodies.rs`.
///
/// The rows differ along WHERE the unmodelled value sits (alone, beside a modelled one, in
/// `PermissionsWithGrantOption`) and along its form (an invented name, a lower-cased
/// modelled value, a modelled value with trailing whitespace, the empty string); the last
/// row is the control, naming only values the enum carries, which must still be accepted
/// and recorded.
#[tokio::test]
async fn a_permission_the_model_does_not_carry_is_refused_on_both_write_paths() {
    // One row per class of unmodelled value, plus the accepted control. `Permissions`,
    // `PermissionsWithGrantOption`, and whether the request is expected to be accepted.
    // Every modelled value a row names is one the DATABASE the rows grant on holds, so a
    // refused row is refused for the unmodelled value and nothing else.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>, bool)> = vec![
        ("an invented value alone", vec!["NOT_A_PERMISSION"], vec![], false),
        ("an invented value beside a modelled one", vec!["DESCRIBE", "SUPER"], vec![], false),
        // `Permission`'s values are upper-case (`PERMISSION_VALUES`), so a lower-cased
        // spelling is a different string and not a member of the enum.
        ("a lower-cased modelled value", vec!["select"], vec![], false),
        ("a modelled value with trailing whitespace", vec!["SELECT "], vec![], false),
        ("the empty string", vec![""], vec![], false),
        (
            "an invented value in PermissionsWithGrantOption",
            vec!["DESCRIBE"],
            vec!["DESCRIBE", "NOT_A_PERMISSION"],
            false,
        ),
        // The control: every value here IS a member of the enum, so this one is recorded.
        ("only modelled values", vec!["DESCRIBE"], vec!["DESCRIBE"], true),
    ];

    let router = emulator_with_catalog().await;
    // Each row reads back the record its own grant wrote, so the database's creation-time
    // default grant — which a Principal-and-Resource listing would resolve onto the row's
    // principal — is revoked first.
    revoke_default_database_grant(&router, "sales").await;
    for (index, (case, permissions, with_grant_option, accepted)) in
        cases.into_iter().enumerate()
    {
        // One principal per row, so no row can read another's record.
        let principal = format!("arn:aws:iam::000000000000:role/enum-{index}");
        let resource = json!({ "Database": { "Name": "sales" } });
        let mut body = json!({
            "Principal": { "DataLakePrincipalIdentifier": principal },
            "Resource": resource,
            "Permissions": permissions,
        });
        if !with_grant_option.is_empty() {
            body["PermissionsWithGrantOption"] = json!(with_grant_option);
        }

        for operation in ["GrantPermissions", "RevokePermissions"] {
            // The revoke row for the accepted control runs after its grant below, so it
            // is the only one that may answer 200 here; every other row must be refused
            // by both paths for the same reason.
            if accepted && operation == "RevokePermissions" {
                continue;
            }
            let (status, payload) = call(&router, operation, &body).await;
            let expected = if accepted { 200 } else { 400 };
            assert_eq!(
                status, expected,
                "case {case:?}: expected {operation} to answer {expected}, got {status} {payload:?}"
            );
            if !accepted {
                let code = error_code(&payload);
                assert_eq!(
                    code, "InvalidInputException",
                    "case {case:?}: expected {operation} to refuse with InvalidInputException, \
                     got {code:?} {payload:?}"
                );
            }
        }

        let entries = list(
            &router,
            case,
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": principal },
                "Resource": resource,
            }),
        )
        .await;
        let stored: Vec<String> = entries.iter().flat_map(permissions_of).collect();
        let expected: Vec<String> = if accepted {
            vec!["DESCRIBE".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the stored permissions to be {expected:?}, got {stored:?}"
        );
    }
}

/// One grant may name every permission the resource it addresses can hold, and what it
/// records is the SET of the values it named — so the ceiling on how many DISTINCT
/// permissions one grant can push into a record is at most the `Permission` enum's own
/// size, which no request naming only modelled values can pass.
///
/// `PermissionList` carries no length constraint of its own, so the bound is inherent
/// rather than enforced: the boundary validator admits only values the `Permission` enum
/// carries, and the handler de-duplicates, so a stored record can hold at most
/// `state::MAX_PERMISSIONS_PER_GRANT` of them. The rows differ along how many times each
/// value is named: once, many times over, and every value twice — all three collapse to
/// the same stored set.
#[tokio::test]
async fn a_grant_records_the_set_of_the_permissions_it_names_up_to_the_modelled_ceiling() {
    use service_com_amazonaws_lakeformation_awslakeformation::state::MAX_PERMISSIONS_PER_GRANT;

    /// Every value of `Permission`
    /// (`smithy_query(query="shape",shape="Permission")`, rendered in this crate as
    /// `ops::types::PERMISSION_VALUES`).
    const MODELLED: [&str; 17] = [
        "ALL",
        "ALTER",
        "ASSOCIATE",
        "CREATE_CATALOG",
        "CREATE_DATABASE",
        "CREATE_LF_TAG",
        "CREATE_LF_TAG_EXPRESSION",
        "CREATE_TABLE",
        "DATA_LOCATION_ACCESS",
        "DELETE",
        "DESCRIBE",
        "DROP",
        "EXECUTE",
        "GRANT_WITH_LF_TAG_EXPRESSION",
        "INSERT",
        "SELECT",
        "SUPER_USER",
    ];

    /// Every permission a DATABASE holds — Lake Formation's database-level privileges:
    /// creating a table in it, altering, dropping and describing the database, and the
    /// "Super" permission over the whole of it. The other values of `Permission` name a
    /// privilege a database does not have, and a grant asking for one of those on a
    /// database is refused (`invalid_permissions_modification.rs`).
    const ON_A_DATABASE: [&str; 5] = ["ALL", "ALTER", "CREATE_TABLE", "DESCRIBE", "DROP"];

    // The ceiling is the enum's size, which bounds every kind's own set: naming every
    // permission a database holds is at or under the bound, and no request of modelled
    // values can pass it.
    assert_eq!(
        MODELLED.len(),
        MAX_PERMISSIONS_PER_GRANT,
        "case \"the ceiling\": expected the distinct-permission ceiling to be the size of \
         Permission ({}), got {MAX_PERMISSIONS_PER_GRANT}",
        MODELLED.len()
    );
    assert!(
        ON_A_DATABASE.len() <= MAX_PERMISSIONS_PER_GRANT,
        "case \"the ceiling\": a database holds {} permissions, which must not pass the \
         ceiling {MAX_PERMISSIONS_PER_GRANT}",
        ON_A_DATABASE.len()
    );

    let every_twice: Vec<&str> = ON_A_DATABASE
        .iter()
        .chain(ON_A_DATABASE.iter())
        .copied()
        .collect();
    let repeated: Vec<&str> = std::iter::repeat_n("DESCRIBE", 64).collect();
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "every permission the resource holds, once",
            ON_A_DATABASE.to_vec(),
            ON_A_DATABASE.to_vec(),
        ),
        (
            "one permission named 64 times",
            repeated,
            vec!["DESCRIBE"],
        ),
        (
            "every permission the resource holds, twice",
            every_twice,
            ON_A_DATABASE.to_vec(),
        ),
    ];

    let router = emulator_with_catalog().await;
    // Each row reads back the set its own grant recorded, so the database's creation-time
    // default grant is revoked first.
    revoke_default_database_grant(&router, "sales").await;
    for (index, (case, granted, held)) in cases.into_iter().enumerate() {
        // One principal per row, so the rows cannot merge into one another's record.
        let principal = format!("arn:aws:iam::000000000000:role/ceiling-{index}");
        let resource = json!({ "Database": { "Name": "sales" } });
        grant(&router, case, &principal, &resource, &granted).await;

        let entries = list(
            &router,
            case,
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": principal },
                "Resource": resource,
            }),
        )
        .await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly one record, got {entries:?}"
        );
        let actual: BTreeSet<String> = permissions_of(&entries[0]).into_iter().collect();
        let expected: BTreeSet<String> = held.iter().map(|value| value.to_string()).collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored permissions to be {expected:?}, got {actual:?}"
        );
    }
}

/// An emulator whose Data Catalog holds a GOVERNED database — `CreateTableDefaultPermissions`
/// is the empty list, so the tables created in it are handed NO `IAM_ALLOWED_PRINCIPALS`
/// grant — with one table in it.
///
/// The condition both nothing-was-revoked wordings are measured under: the table holds no
/// grant for the group at all, and none for any principal but its creator.
async fn emulator_with_governed_table() -> MultiServiceRouter {
    let router = emulator();
    create_database_with_input(&router, &crate::creator_table_grants::governed_database("lake"))
        .await;
    create_table(&router, "lake", "events").await;
    router
}

/// `RevokePermissions` that must be refused, asserting the status, the modelled error and
/// `expected` as the WHOLE message.
async fn assert_revoke_refused(
    router: &MultiServiceRouter,
    case: &str,
    principal: &str,
    resource: &Value,
    permissions: &[&str],
    with_grant_option: &[&str],
    expected: &str,
) {
    let (status, payload) = call(
        router,
        "RevokePermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": principal },
            "Resource": resource,
            "Permissions": permissions,
            "PermissionsWithGrantOption": with_grant_option,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a refused revoke to answer 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {payload:?}"
    );
    let message = payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected Message {expected:?}, got {message:?}"
    );
}

/// One row of the refusal-wording table: the case name, the grantee the revoke names, the
/// `Permissions` and `PermissionsWithGrantOption` it names, and the whole sentence the
/// refusal must carry.
type RefusalWordingCase = (
    &'static str,
    String,
    Vec<&'static str>,
    Vec<&'static str>,
    String,
);

#[tokio::test]
async fn a_revoke_that_takes_nothing_away_is_worded_by_whether_the_grantee_is_modelled() {
    // Nothing is held on this table by anyone but its creator, so every row below revokes
    // what is not there — and the rows differ along WHICH grantee asks: the group the
    // service always models (`IAM_ALLOWED_PRINCIPALS`, which stands for every principal IAM
    // itself allows, so it is never absent from a resource's permission model) against the
    // other forms a `DataLakePrincipalIdentifier` takes — an IAM role, an IAM user, an
    // account id — none of which holds anything here. They differ along the permissions
    // named too: one, several, the column-scoped one, and a request that names them in
    // `PermissionsWithGrantOption` only.
    let account = account_catalog();
    let table = json!({ "Table": { "DatabaseName": "lake", "Name": "events" } });
    let cases: Vec<RefusalWordingCase> = vec![
        // The issue's own request: the group, `["ALL"]`, on a governed table.
        (
            "the group, one permission",
            "IAM_ALLOWED_PRINCIPALS".to_string(),
            vec!["ALL"],
            vec![],
            permissions_not_held_message(&["ALL"]),
        ),
        (
            "the group, several permissions",
            "IAM_ALLOWED_PRINCIPALS".to_string(),
            vec!["DESCRIBE", "ALTER", "DROP"],
            vec![],
            permissions_not_held_message(&["DESCRIBE", "ALTER", "DROP"]),
        ),
        (
            "the group, the column-scoped permission",
            "IAM_ALLOWED_PRINCIPALS".to_string(),
            vec!["SELECT"],
            vec![],
            permissions_not_held_message(&["SELECT"]),
        ),
        (
            "the group, named with the grant option only",
            "IAM_ALLOWED_PRINCIPALS".to_string(),
            vec![],
            vec!["DESCRIBE"],
            permissions_not_held_message(&["DESCRIBE"]),
        ),
        (
            "an IAM role that holds nothing",
            format!("arn:aws:iam::{account}:role/analyst"),
            vec!["ALL"],
            vec![],
            NOTHING_TO_REVOKE.to_string(),
        ),
        (
            "an IAM user that holds nothing",
            format!("arn:aws:iam::{account}:user/auditor"),
            vec!["DESCRIBE", "ALTER"],
            vec![],
            NOTHING_TO_REVOKE.to_string(),
        ),
        (
            "an account that holds nothing",
            account.clone(),
            vec!["ALL"],
            vec![],
            NOTHING_TO_REVOKE.to_string(),
        ),
    ];

    let router = emulator_with_governed_table().await;
    // The group holds nothing on this table: what the issue's check confirms before it
    // revokes.
    let listed = list(
        &router,
        "the group holds nothing",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
            "Resource": table,
        }),
    )
    .await;
    assert!(
        listed.is_empty(),
        "case \"the group holds nothing\": a governed table hands the group no grant, got \
         {listed:?}"
    );

    for (case, principal, permissions, with_grant_option, expected) in cases {
        assert_revoke_refused(
            &router,
            case,
            &principal,
            &table,
            &permissions,
            &with_grant_option,
            &expected,
        )
        .await;
    }
}

#[tokio::test]
async fn the_wording_of_a_refused_revoke_follows_what_the_grantee_still_holds() {
    // The same rule read the other way: the sentence a grantee draws follows what it holds
    // at the moment of the call, so a grant and a revoke move a principal between the two
    // classes. The group is in the modelled class throughout, held or not.
    let router = emulator_with_governed_table().await;
    let table = json!({ "Table": { "DatabaseName": "lake", "Name": "events" } });
    let analyst = format!("arn:aws:iam::{}:role/analyst", account_catalog());

    // Granting the group `ALL` twice records one grant: a repeated grant is idempotent.
    for attempt in ["first grant", "second grant"] {
        grant(&router, attempt, "IAM_ALLOWED_PRINCIPALS", &table, &["ALL"]).await;
    }
    let listed = list(
        &router,
        "the group after two grants",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
            "Resource": table,
        }),
    )
    .await;
    assert_eq!(
        listed.len(),
        1,
        "case \"the group after two grants\": expected one entry, got {listed:?}"
    );
    assert_eq!(
        permissions_of(&listed[0]),
        vec!["ALL".to_string()],
        "case \"the group after two grants\": expected [ALL], got {:?}",
        listed[0]
    );

    // The first revoke takes that grant away, so the group holds nothing again.
    let (status, payload) = call(
        &router,
        "RevokePermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
            "Resource": table,
            "Permissions": ["ALL"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first revoke\": expected 200, got {status} {payload:?}"
    );

    // A second revoke of the same grant has nothing left to take — and the group is still
    // the grantee the service models, so it is told which permissions it does not have.
    assert_revoke_refused(
        &router,
        "the group revoked twice",
        "IAM_ALLOWED_PRINCIPALS",
        &table,
        &["ALL"],
        &[],
        &permissions_not_held_message(&["ALL"]),
    )
    .await;

    // A role that was never granted anything here holds nothing, so it draws the other
    // sentence.
    assert_revoke_refused(
        &router,
        "a role before its grant",
        &analyst,
        &table,
        &["DROP"],
        &[],
        NOTHING_TO_REVOKE,
    )
    .await;

    // Once that role holds something on the table, a revoke naming what it does not hold
    // reports the difference instead.
    grant(&router, "the role's grant", &analyst, &table, &["DESCRIBE"]).await;
    let held_cases: Vec<(&str, Vec<&str>)> = vec![
        ("a role holding one of them", vec!["DROP"]),
        ("a role holding one of two named", vec!["DESCRIBE", "DROP"]),
    ];
    for (case, permissions) in held_cases {
        assert_revoke_refused(
            &router,
            case,
            &analyst,
            &table,
            &permissions,
            &[],
            &permissions_not_held_message(&permissions),
        )
        .await;
    }
}


