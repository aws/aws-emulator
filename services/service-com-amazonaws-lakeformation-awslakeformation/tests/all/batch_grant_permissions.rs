// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `BatchGrantPermissions`: what a batch of permission requests
//! applies, what it reports as failed, and what `ListPermissions` reads back afterwards.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! The rule the rows span: a batch of permission requests applies every entry it does not
//! report in `Failures`, each entry accepted or refused for exactly what the single-entry
//! call accepts or refuses it for, so a permission granted by a batch entry is readable
//! back through `ListPermissions` filtered by that entry's principal and resource.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::glue_catalog::{
    account_catalog, emulator_with_catalog, error_code, invalid_arn_message,
    lake_formation as call, list_permissions, revoke_default_database_grant,
    revoke_table_default_grants,
};

/// A Data Catalog holding the `sales` database with its `orders` table, with the grants
/// their CREATION made taken away again — the creator's Super, and the
/// `IAM_ALLOWED_PRINCIPALS` defaults a database and its tables are created carrying.
///
/// This suite reads back the records its own batch wrote, filtered by principal AND
/// resource — a listing that reports EFFECTIVE permissions, which resolves an
/// `IAM_ALLOWED_PRINCIPALS` grant onto the IAM principal it asks about. Those grants have
/// their own suites (`creator_table_grants.rs`, `table_default_grants.rs`,
/// `default_database_grant.rs`).
async fn seeded() -> MultiServiceRouter {
    let router = emulator_with_catalog(&[("sales", &["orders"])]).await;
    revoke_table_default_grants(&router, "sales", "orders").await;
    revoke_default_database_grant(&router, "sales").await;
    router
}

/// One batch entry as a request writes it.
fn entry(id: &str, principal: &str, resource: &Value, permissions: &[&str]) -> Value {
    json!({
        "Id": id,
        "Principal": { "DataLakePrincipalIdentifier": principal },
        "Resource": resource,
        "Permissions": permissions,
    })
}

/// `BatchGrantPermissions` of `entries`, asserting 200 and that `Failures` is present (the
/// list carries no length constraint, so a batch that failed nothing answers an EMPTY list
/// rather than an absent member).
async fn batch_grant(router: &MultiServiceRouter, case: &str, entries: &[Value]) -> Vec<Value> {
    let (status, payload) = call(router, "BatchGrantPermissions", &json!({ "Entries": entries })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected BatchGrantPermissions to answer 200, got {status} {payload:?}"
    );
    payload
        .get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a Failures list, got {payload:?}")
        })
}

/// The `(Id, ErrorCode, ErrorMessage)` of one reported failure.
fn failure(entry: &Value) -> (String, String, String) {
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let request_entry = entry.get("RequestEntry");
    let error = entry.get("Error");
    (
        text(request_entry.and_then(|entry| entry.get("Id"))),
        text(error.and_then(|error| error.get("ErrorCode"))),
        text(error.and_then(|error| error.get("ErrorMessage"))),
    )
}

/// Every permission `principal` holds, across every record and every scope, as the listing
/// filtered by that principal alone reports them.
async fn held(router: &MultiServiceRouter, case: &str, principal: &str) -> BTreeSet<String> {
    let listed = list_permissions(
        router,
        case,
        &json!({ "Principal": { "DataLakePrincipalIdentifier": principal } }),
    )
    .await;
    listed
        .iter()
        .flat_map(|entry| {
            entry
                .get("Permissions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the mixed-batch table: the entry's id, the resource it names, the
/// permissions it asks for, and either the permissions the grantee ends up holding (an
/// applied entry) or the whole message the failure must carry (a refused one).
struct BatchEntryCase {
    id: &'static str,
    resource: Value,
    permissions: Vec<&'static str>,
    /// `Ok` names what the grantee holds afterwards, `Err` the failure's whole message.
    outcome: Result<Vec<&'static str>, String>,
}

#[tokio::test]
async fn a_batch_applies_every_entry_it_does_not_report_as_failed() {
    // One batch whose entries differ along the resource kind they grant on, how many
    // scopes their permissions are held at, and — for the refused ones — WHY the
    // single-entry call would refuse them: an absent table, an absent database, a grantee
    // that identifies nobody, no permission at all, no resource, and two resources at once.
    // Each entry names its own principal, so what one entry wrote cannot be read back as
    // another's. (A value the `Permission` enum does not carry is refused for the whole
    // request by the boundary validator, before any entry is read, so no entry can carry
    // one.)
    let catalog = account_catalog();
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let database = json!({ "Database": { "Name": "sales" } });
    let cases: Vec<BatchEntryCase> = vec![
        BatchEntryCase {
            id: "database-describe",
            resource: database.clone(),
            permissions: vec!["DESCRIBE"],
            outcome: Ok(vec!["DESCRIBE"]),
        },
        BatchEntryCase {
            id: "table-two-scopes",
            resource: table.clone(),
            // `SELECT` is held at column scope and `DESCRIBE` at the table's own, so this
            // one entry writes two records.
            permissions: vec!["SELECT", "DESCRIBE"],
            outcome: Ok(vec!["DESCRIBE", "SELECT"]),
        },
        BatchEntryCase {
            id: "table-alter-and-drop",
            resource: table.clone(),
            permissions: vec!["ALTER", "DROP"],
            outcome: Ok(vec!["ALTER", "DROP"]),
        },
        BatchEntryCase {
            id: "absent-table",
            resource: json!({ "Table": { "DatabaseName": "sales", "Name": "absent" } }),
            permissions: vec!["SELECT"],
            outcome: Err("Table not found.".to_string()),
        },
        BatchEntryCase {
            id: "absent-database",
            resource: json!({ "Database": { "Name": "absent" } }),
            permissions: vec!["DESCRIBE"],
            outcome: Err("Database not found.".to_string()),
        },
        BatchEntryCase {
            id: "no-permission",
            resource: database.clone(),
            permissions: vec![],
            outcome: Err("Permissions must name at least one permission to grant".to_string()),
        },
    ];

    let router = seeded().await;
    // The principal of row `index`, so each row reads back only its own records.
    let principal = |index: usize| format!("arn:aws:iam::{catalog}:role/batch-entry-{index}");
    let mut entries: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(index, row)| entry(row.id, &principal(index), &row.resource, &row.permissions))
        .collect();
    // Two entries no `Resource` can be read out of: one naming none, one naming two.
    entries.push(json!({
        "Id": "no-resource",
        "Principal": { "DataLakePrincipalIdentifier": principal(entries.len()) },
        "Permissions": ["DESCRIBE"],
    }));
    entries.push(json!({
        "Id": "two-resources",
        "Principal": { "DataLakePrincipalIdentifier": principal(entries.len()) },
        "Resource": { "Database": { "Name": "sales" }, "Table": { "DatabaseName": "sales", "Name": "orders" } },
        "Permissions": ["DESCRIBE"],
    }));
    // One entry whose grantee is a bare role name rather than an ARN.
    entries.push(entry(
        "bare-principal",
        "batch-entry-grantee",
        &database,
        &["DESCRIBE"],
    ));

    let failures = batch_grant(&router, "mixed batch", &entries).await;

    // Only the refused entries are reported, in the order the request named them, each
    // carrying the error class and the whole message the single-entry call answers.
    let mut expected_failures: Vec<(String, String, String)> = cases
        .iter()
        .filter_map(|row| {
            row.outcome
                .as_ref()
                .err()
                .map(|message| (row.id.to_string(), "InvalidInputException".to_string(), message.clone()))
        })
        .collect();
    for (id, message) in [
        (
            "no-resource",
            "Resource must name exactly one resource type, but none was provided",
        ),
        (
            "two-resources",
            "Resource must name exactly one resource type, but 2 were provided: Database, Table",
        ),
    ] {
        expected_failures.push((
            id.to_string(),
            "InvalidInputException".to_string(),
            message.to_string(),
        ));
    }
    expected_failures.push((
        "bare-principal".to_string(),
        "InvalidInputException".to_string(),
        invalid_arn_message("batch-entry-grantee"),
    ));
    let reported: Vec<(String, String, String)> = failures.iter().map(failure).collect();
    assert_eq!(
        reported, expected_failures,
        "case \"mixed batch\": expected exactly the refused entries in request order, got \
         {failures:?}"
    );

    // Every entry the batch did not report is applied, and readable back.
    for (index, row) in cases.iter().enumerate() {
        let principal = principal(index);
        let expected: BTreeSet<String> = match &row.outcome {
            Ok(permissions) => permissions.iter().map(|value| value.to_string()).collect(),
            // A refused entry wrote nothing, so its grantee holds nothing.
            Err(_) => BTreeSet::new(),
        };
        let actual = held(&router, row.id, &principal).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected {principal} to hold {expected:?}, got {actual:?}",
            row.id
        );
    }
}

/// One row of the echoed-entry table: the entry a batch sends, and the `RequestEntry` the
/// failure must report back for it.
struct EchoCase {
    case: &'static str,
    sent: Value,
    echoed: Value,
}

#[tokio::test]
async fn a_reported_failure_echoes_the_entry_as_the_service_read_it() {
    // Rows differ along what the entry left out and what it spelled: an absent
    // `CatalogId` is filled in with the catalog the request addresses, an absent
    // permission list is reported as the empty list, a `CatalogId` the entry named is
    // kept, and a name the entry wrote is echoed exactly as written — the echo RESOLVES
    // what was left out without rewriting what was said. Every row names a table the Data
    // Catalog does not hold, so every row is a failure.
    let catalog = account_catalog();
    let cases: Vec<EchoCase> = vec![
        EchoCase {
            case: "nothing but the table and one permission",
            sent: json!({
                "Id": "one",
                "Principal": { "DataLakePrincipalIdentifier": format!("arn:aws:iam::{catalog}:role/echo") },
                "Resource": { "Table": { "DatabaseName": "sales", "Name": "absent" } },
                "Permissions": ["SELECT"],
            }),
            echoed: json!({
                "Id": "one",
                "Principal": { "DataLakePrincipalIdentifier": format!("arn:aws:iam::{catalog}:role/echo") },
                "Resource": {
                    "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "absent" },
                },
                "Permissions": ["SELECT"],
                "PermissionsWithGrantOption": [],
            }),
        },
        EchoCase {
            case: "a catalog id, a grant option and a condition the entry named itself",
            sent: json!({
                "Id": "two",
                "Principal": { "DataLakePrincipalIdentifier": format!("arn:aws:iam::{catalog}:role/echo") },
                "Resource": {
                    "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "absent" },
                },
                "Permissions": ["SELECT", "DESCRIBE"],
                "PermissionsWithGrantOption": ["SELECT"],
                "Condition": { "Expression": "region = 'eu'" },
            }),
            echoed: json!({
                "Id": "two",
                "Principal": { "DataLakePrincipalIdentifier": format!("arn:aws:iam::{catalog}:role/echo") },
                "Resource": {
                    "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "absent" },
                },
                "Permissions": ["SELECT", "DESCRIBE"],
                "PermissionsWithGrantOption": ["SELECT"],
                "Condition": { "Expression": "region = 'eu'" },
            }),
        },
        EchoCase {
            case: "a mixed-case name is echoed as written",
            sent: json!({
                "Id": "three",
                "Principal": { "DataLakePrincipalIdentifier": format!("arn:aws:iam::{catalog}:role/echo") },
                "Resource": { "Table": { "DatabaseName": "sales", "Name": "AbsentTable" } },
                "Permissions": ["SELECT"],
            }),
            echoed: json!({
                "Id": "three",
                "Principal": { "DataLakePrincipalIdentifier": format!("arn:aws:iam::{catalog}:role/echo") },
                "Resource": {
                    "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "AbsentTable" },
                },
                "Permissions": ["SELECT"],
                "PermissionsWithGrantOption": [],
            }),
        },
    ];

    let router = seeded().await;
    for row in &cases {
        let failures = batch_grant(&router, row.case, std::slice::from_ref(&row.sent)).await;
        assert_eq!(
            failures.len(),
            1,
            "case {:?}: expected the entry to be reported as failed, got {failures:?}",
            row.case
        );
        assert_eq!(
            failures[0].get("RequestEntry"),
            Some(&row.echoed),
            "case {:?}: expected RequestEntry {:?}, got {:?}",
            row.case,
            row.echoed,
            failures[0]
        );
    }
}

/// One row of the read-back table: what a single valid entry grants, and the entry the
/// listing must answer for it.
struct ReadBackCase {
    case: &'static str,
    resource: Value,
    permissions: Vec<&'static str>,
    /// The `Permissions` of the one entry the listing reports, and the `Resource` it
    /// carries.
    listed: (Vec<&'static str>, Value),
}

#[tokio::test]
async fn a_batch_granted_permission_is_listed_for_its_grantee_and_resource() {
    // The shape a client audits a bulk grant with: ListPermissions filtered by the
    // grantee AND the resource the batch granted on, which reports the grantee's
    // EFFECTIVE permissions. Rows differ along the resource kind and the permission
    // granted; each batch also carries an entry that FAILS, so what is read back is read
    // back from a partial batch.
    let catalog = account_catalog();
    let cases: Vec<ReadBackCase> = vec![
        ReadBackCase {
            case: "describe on a table",
            resource: json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } }),
            permissions: vec!["DESCRIBE"],
            listed: (
                vec!["DESCRIBE"],
                json!({
                    "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "orders" },
                }),
            ),
        },
        ReadBackCase {
            case: "alter on a table",
            resource: json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } }),
            permissions: vec!["ALTER"],
            listed: (
                vec!["ALTER"],
                json!({
                    "Table": { "CatalogId": catalog, "DatabaseName": "sales", "Name": "orders" },
                }),
            ),
        },
        ReadBackCase {
            case: "describe on a database",
            resource: json!({ "Database": { "Name": "sales" } }),
            permissions: vec!["DESCRIBE"],
            listed: (
                vec!["DESCRIBE"],
                json!({ "Database": { "CatalogId": catalog, "Name": "sales" } }),
            ),
        },
    ];

    for row in &cases {
        let router = seeded().await;
        let principal = format!("arn:aws:iam::{catalog}:role/{}", "batch-audit");
        let failures = batch_grant(
            &router,
            row.case,
            &[
                entry("applied", &principal, &row.resource, &row.permissions),
                // The entry that fails: a table the Data Catalog does not hold.
                entry(
                    "failed",
                    &principal,
                    &json!({ "Table": { "DatabaseName": "sales", "Name": "absent" } }),
                    &["SELECT"],
                ),
            ],
        )
        .await;
        assert_eq!(
            failures.len(),
            1,
            "case {:?}: expected exactly the failing entry to be reported, got {failures:?}",
            row.case
        );

        let listed = list_permissions(
            &router,
            row.case,
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": principal },
                "Resource": row.resource,
            }),
        )
        .await;
        assert_eq!(
            listed.len(),
            1,
            "case {:?}: expected one listed entry for the batch-granted permission, got \
             {listed:?}",
            row.case
        );
        let entry = &listed[0];
        assert_eq!(
            entry.get("Permissions"),
            Some(&json!(row.listed.0)),
            "case {:?}: expected Permissions {:?}, got {entry:?}",
            row.case,
            row.listed.0
        );
        assert_eq!(
            entry.get("PermissionsWithGrantOption"),
            Some(&json!([] as [&str; 0])),
            "case {:?}: expected an empty PermissionsWithGrantOption, got {entry:?}",
            row.case
        );
        assert_eq!(
            entry
                .get("Principal")
                .and_then(|value| value.get("DataLakePrincipalIdentifier"))
                .and_then(Value::as_str),
            Some(principal.as_str()),
            "case {:?}: expected the grantee {principal:?}, got {entry:?}",
            row.case
        );
        assert_eq!(
            entry.get("Resource"),
            Some(&row.listed.1),
            "case {:?}: expected Resource {:?}, got {entry:?}",
            row.case,
            row.listed.1
        );
    }
}

#[tokio::test]
async fn a_batch_entry_grants_exactly_what_the_single_grant_call_grants() {
    // The same grant asked for both ways must leave the same state, whatever it names:
    // rows differ along the resource kind, the number of scopes the permissions are held
    // at, whether a grant option is asked for, and whether the grant carries a condition.
    let catalog = account_catalog();
    let principal = format!("arn:aws:iam::{catalog}:role/batch-vs-single");
    let cases: Vec<(&str, Value)> = vec![
        (
            "a database grant",
            json!({ "Resource": { "Database": { "Name": "sales" } }, "Permissions": ["DESCRIBE"] }),
        ),
        (
            "a table grant held at two scopes",
            json!({
                "Resource": { "Table": { "DatabaseName": "sales", "Name": "orders" } },
                "Permissions": ["SELECT", "DESCRIBE"],
            }),
        ),
        (
            "a grant with the grant option",
            json!({
                "Resource": { "Database": { "Name": "sales" } },
                "Permissions": ["DESCRIBE"],
                "PermissionsWithGrantOption": ["DESCRIBE"],
            }),
        ),
        (
            "a column-scoped grant",
            json!({
                "Resource": {
                    "TableWithColumns": {
                        "DatabaseName": "sales",
                        "Name": "orders",
                        "ColumnNames": ["order_id"],
                    },
                },
                "Permissions": ["SELECT"],
            }),
        ),
        (
            "a grant under a condition",
            json!({
                "Resource": { "Database": { "Name": "sales" } },
                "Permissions": ["DESCRIBE"],
                "Condition": { "Expression": "region = 'eu'" },
            }),
        ),
    ];

    for (case, grant) in &cases {
        // One emulator per path, so each listing reads only what that path wrote.
        let single = seeded().await;
        let mut body = grant.clone();
        body["Principal"] = json!({ "DataLakePrincipalIdentifier": principal });
        let (status, payload) = call(&single, "GrantPermissions", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GrantPermissions to answer 200, got {status} {payload:?}"
        );

        let batched = seeded().await;
        let mut entry = body.clone();
        entry["Id"] = json!("one");
        let failures = batch_grant(&batched, case, &[entry]).await;
        assert!(
            failures.is_empty(),
            "case {case:?}: expected the batch entry to be applied, got {failures:?}"
        );

        let filter = json!({ "Principal": { "DataLakePrincipalIdentifier": principal } });
        // Compared member by member, with the one member the two paths cannot agree on left
        // out: `LastUpdated` is the instant each path's own write ran, so what is compared is
        // the record each left behind, plus that both recorded the write at all.
        let from_single = list_permissions(&single, case, &filter).await;
        let from_batch = list_permissions(&batched, case, &filter).await;
        let without_the_instant = |listed: &[Value]| -> Vec<Value> {
            listed
                .iter()
                .map(|entry| {
                    let mut entry = entry.clone();
                    let held = entry
                        .as_object_mut()
                        .expect("a listed entry is an object")
                        .remove("LastUpdated");
                    assert!(
                        held.is_some(),
                        "case {case:?}: every written record reports its write's instant, got \
                         {entry:?}"
                    );
                    entry
                })
                .collect()
        };
        assert_eq!(
            without_the_instant(&from_batch),
            without_the_instant(&from_single),
            "case {case:?}: expected the batch to leave what the single grant leaves \
             ({from_single:?}), got {from_batch:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_is_refused_whole_past_the_documented_entry_limit() {
    // "A list of up to 20 entries for resource permissions to be granted by batch
    // operation to the principal" (`BatchGrantPermissionsRequest$Entries`): the bound
    // itself is applied, one past it is refused, and a refusal of the request applies
    // NONE of its entries.
    let catalog = account_catalog();
    let database = json!({ "Database": { "Name": "sales" } });
    let entries = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|index| {
                entry(
                    &format!("entry-{index}"),
                    &format!("arn:aws:iam::{catalog}:role/limit-{count}-{index}"),
                    &database,
                    &["DESCRIBE"],
                )
            })
            .collect()
    };

    for count in [1_usize, 20] {
        let router = seeded().await;
        let failures = batch_grant(&router, &format!("{count} entries"), &entries(count)).await;
        assert!(
            failures.is_empty(),
            "case \"{count} entries\": expected every entry to be applied, got {failures:?}"
        );
        for index in 0..count {
            let principal = format!("arn:aws:iam::{catalog}:role/limit-{count}-{index}");
            let actual = held(&router, "within the limit", &principal).await;
            assert_eq!(
                actual,
                BTreeSet::from(["DESCRIBE".to_string()]),
                "case \"{count} entries\": expected {principal} to hold DESCRIBE, got {actual:?}"
            );
        }
    }

    let count = 21_usize;
    let router = seeded().await;
    let (status, payload) = call(
        &router,
        "BatchGrantPermissions",
        &json!({ "Entries": entries(count) }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"{count} entries\": expected 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidInputException",
        "case \"{count} entries\": expected InvalidInputException, got {payload:?}"
    );
    for index in 0..count {
        let principal = format!("arn:aws:iam::{catalog}:role/limit-{count}-{index}");
        let actual = held(&router, "past the limit", &principal).await;
        assert!(
            actual.is_empty(),
            "case \"{count} entries\": a refused batch applies nothing, but {principal} \
             holds {actual:?}"
        );
    }
}
