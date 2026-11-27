// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `BatchRevokePermissions`: what a batch of revoke requests takes
//! away, what it reports as failed, and what `ListPermissions` reads back afterwards.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! The rule the rows span is the grant batch's, in the other direction: a batch of
//! permission requests applies every entry it does not report in `Failures`, each entry
//! accepted or refused for exactly what the single-entry call accepts or refuses it for, so
//! a permission a batch entry revoked stops being readable back through `ListPermissions`
//! while one whose entry failed stays exactly as it was.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::glue_catalog::{
    account_catalog, emulator_with_catalog, error_code, lake_formation as call, list_permissions,
    permissions_not_held_message, revoke_default_database_grant, revoke_table_default_grants,
    NOTHING_TO_REVOKE,
};

/// A Data Catalog holding the `sales` database with its `orders` table, with the grants
/// their CREATION made taken away again — so a listing reads back only what this suite
/// granted (those grants have their own suites).
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

/// `GrantPermissions`, asserting it was accepted.
async fn grant(
    router: &MultiServiceRouter,
    case: &str,
    principal: &str,
    resource: &Value,
    permissions: &[&str],
) {
    let (status, payload) = call(
        router,
        "GrantPermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": principal },
            "Resource": resource,
            "Permissions": permissions,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GrantPermissions to answer 200, got {status} {payload:?}"
    );
}

/// `BatchRevokePermissions` of `entries`, asserting 200 and that `Failures` is present.
async fn batch_revoke(router: &MultiServiceRouter, case: &str, entries: &[Value]) -> Vec<Value> {
    let (status, payload) =
        call(router, "BatchRevokePermissions", &json!({ "Entries": entries })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected BatchRevokePermissions to answer 200, got {status} {payload:?}"
    );
    payload
        .get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Failures list, got {payload:?}"))
}

/// The `(Id, ErrorCode, ErrorMessage)` of one reported failure.
fn failure(entry: &Value) -> (String, String, String) {
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (
        text(entry.get("RequestEntry").and_then(|entry| entry.get("Id"))),
        text(entry.get("Error").and_then(|error| error.get("ErrorCode"))),
        text(entry.get("Error").and_then(|error| error.get("ErrorMessage"))),
    )
}

/// Every permission `principal` holds, across every record and every scope.
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

/// One row of the mixed-batch table: the entry's id, what was granted to its principal
/// first, the resource and permissions the entry revokes, and either what the principal
/// still holds (an applied entry) or the whole message the failure must carry.
struct BatchEntryCase {
    id: &'static str,
    granted: Option<(Value, Vec<&'static str>)>,
    resource: Value,
    permissions: Vec<&'static str>,
    /// `Ok` names what the grantee still holds, `Err` the failure's whole message.
    outcome: Result<Vec<&'static str>, String>,
}

#[tokio::test]
async fn a_batch_revoke_applies_every_entry_it_does_not_report_as_failed() {
    // One batch whose entries differ along the resource kind they revoke on, how many
    // scopes the revoke reaches, and — for the refused ones — WHY the single-entry call
    // would refuse them: nothing granted at all, a permission the grant does not carry, an
    // absent table, an absent database, no permission named, and no resource named. Each
    // entry names its own principal, so one entry's revoke cannot reach another's grant.
    let catalog = account_catalog();
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let database = json!({ "Database": { "Name": "sales" } });
    let cases: Vec<BatchEntryCase> = vec![
        BatchEntryCase {
            id: "database-describe",
            granted: Some((database.clone(), vec!["DESCRIBE"])),
            resource: database.clone(),
            permissions: vec!["DESCRIBE"],
            outcome: Ok(vec![]),
        },
        BatchEntryCase {
            id: "table-one-of-two-scopes",
            // `SELECT` is held at column scope and `DESCRIBE` at the table's own, so this
            // revoke empties one record and leaves the other.
            granted: Some((table.clone(), vec!["SELECT", "DESCRIBE"])),
            resource: table.clone(),
            permissions: vec!["DESCRIBE"],
            outcome: Ok(vec!["SELECT"]),
        },
        BatchEntryCase {
            id: "table-both-scopes",
            granted: Some((table.clone(), vec!["SELECT", "DESCRIBE"])),
            resource: table.clone(),
            permissions: vec!["SELECT", "DESCRIBE"],
            outcome: Ok(vec![]),
        },
        BatchEntryCase {
            id: "nothing-granted",
            granted: None,
            resource: database.clone(),
            permissions: vec!["DESCRIBE"],
            outcome: Err(NOTHING_TO_REVOKE.to_string()),
        },
        BatchEntryCase {
            id: "permission-not-held",
            granted: Some((database.clone(), vec!["DESCRIBE"])),
            resource: database.clone(),
            permissions: vec!["DROP"],
            // This grantee DOES hold something on the database, so the failure reports the
            // difference rather than saying the grantee holds nothing.
            outcome: Err(permissions_not_held_message(&["DROP"])),
        },
        BatchEntryCase {
            id: "absent-table",
            granted: None,
            resource: json!({ "Table": { "DatabaseName": "sales", "Name": "absent" } }),
            permissions: vec!["DESCRIBE"],
            outcome: Err("Table not found.".to_string()),
        },
        BatchEntryCase {
            id: "absent-database",
            granted: None,
            resource: json!({ "Database": { "Name": "absent" } }),
            permissions: vec!["DESCRIBE"],
            outcome: Err("Database not found.".to_string()),
        },
        BatchEntryCase {
            id: "no-permission",
            granted: Some((database.clone(), vec!["DESCRIBE"])),
            resource: database.clone(),
            permissions: vec![],
            outcome: Err("Permissions must name at least one permission to revoke".to_string()),
        },
    ];

    let router = seeded().await;
    let principal = |index: usize| format!("arn:aws:iam::{catalog}:role/batch-revoke-{index}");
    for (index, row) in cases.iter().enumerate() {
        if let Some((resource, permissions)) = &row.granted {
            grant(&router, row.id, &principal(index), resource, permissions).await;
        }
    }

    let mut entries: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(index, row)| entry(row.id, &principal(index), &row.resource, &row.permissions))
        .collect();
    // An entry no `Resource` can be read out of.
    entries.push(json!({
        "Id": "no-resource",
        "Principal": { "DataLakePrincipalIdentifier": principal(entries.len()) },
        "Permissions": ["DESCRIBE"],
    }));

    let failures = batch_revoke(&router, "mixed batch", &entries).await;

    // Only the refused entries are reported, in the order the request named them, each
    // carrying the error class and the whole message the single-entry call answers.
    let mut expected_failures: Vec<(String, String, String)> = cases
        .iter()
        .filter_map(|row| {
            row.outcome.as_ref().err().map(|message| {
                (
                    row.id.to_string(),
                    "InvalidInputException".to_string(),
                    message.clone(),
                )
            })
        })
        .collect();
    expected_failures.push((
        "no-resource".to_string(),
        "InvalidInputException".to_string(),
        "Resource must name exactly one resource type, but none was provided".to_string(),
    ));
    let reported: Vec<(String, String, String)> = failures.iter().map(failure).collect();
    assert_eq!(
        reported, expected_failures,
        "case \"mixed batch\": expected exactly the refused entries in request order, got \
         {failures:?}"
    );

    // Every entry the batch did not report took its permissions away; every entry it did
    // report left its grant exactly as it was.
    for (index, row) in cases.iter().enumerate() {
        let principal = principal(index);
        let expected: BTreeSet<String> = match &row.outcome {
            Ok(remaining) => remaining.iter().map(|value| value.to_string()).collect(),
            // A refused entry took nothing away, so whatever was granted is still held.
            Err(_) => row
                .granted
                .as_ref()
                .map(|(_, permissions)| permissions.iter().map(|value| value.to_string()).collect())
                .unwrap_or_default(),
        };
        let actual = held(&router, row.id, &principal).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected {principal} to hold {expected:?}, got {actual:?}",
            row.id
        );
    }
}

#[tokio::test]
async fn a_batch_revoke_entry_takes_exactly_what_the_single_revoke_call_takes() {
    // The same revoke asked for both ways must leave the same state, whatever it names:
    // rows differ along the resource kind, how many scopes the revoke reaches, whether the
    // grant option is revoked with the permission, and whether the grant was made under a
    // condition.
    let catalog = account_catalog();
    let principal = format!("arn:aws:iam::{catalog}:role/revoke-batch-vs-single");
    let database = json!({ "Database": { "Name": "sales" } });
    let table = json!({ "Table": { "DatabaseName": "sales", "Name": "orders" } });
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a database revoke",
            json!({ "Resource": database, "Permissions": ["DESCRIBE", "ALTER"] }),
            json!({ "Resource": database, "Permissions": ["DESCRIBE"] }),
        ),
        (
            "a table revoke reaching two scopes",
            json!({ "Resource": table, "Permissions": ["SELECT", "DESCRIBE"] }),
            json!({ "Resource": table, "Permissions": ["SELECT", "DESCRIBE"] }),
        ),
        (
            "the grant option with the permission",
            json!({
                "Resource": database,
                "Permissions": ["DESCRIBE"],
                "PermissionsWithGrantOption": ["DESCRIBE"],
            }),
            json!({
                "Resource": database,
                "Permissions": ["DESCRIBE"],
                "PermissionsWithGrantOption": ["DESCRIBE"],
            }),
        ),
        (
            "a revoke under the condition the grant carried",
            json!({
                "Resource": database,
                "Permissions": ["DESCRIBE"],
                "Condition": { "Expression": "region = 'eu'" },
            }),
            json!({
                "Resource": database,
                "Permissions": ["DESCRIBE"],
                "Condition": { "Expression": "region = 'eu'" },
            }),
        ),
    ];

    for (case, granted, revoked) in &cases {
        // One emulator per path, so each listing reads only what that path left.
        let mut routers = Vec::new();
        for _ in 0..2 {
            let router = seeded().await;
            let mut body = granted.clone();
            body["Principal"] = json!({ "DataLakePrincipalIdentifier": principal });
            let (status, payload) = call(&router, "GrantPermissions", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the set-up grant to answer 200, got {status} \
                 {payload:?}"
            );
            routers.push(router);
        }

        let mut body = revoked.clone();
        body["Principal"] = json!({ "DataLakePrincipalIdentifier": principal });
        let (status, payload) = call(&routers[0], "RevokePermissions", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RevokePermissions to answer 200, got {status} {payload:?}"
        );

        let mut entry = body.clone();
        entry["Id"] = json!("one");
        let failures = batch_revoke(&routers[1], case, &[entry]).await;
        assert!(
            failures.is_empty(),
            "case {case:?}: expected the batch entry to be applied, got {failures:?}"
        );

        let filter = json!({ "Principal": { "DataLakePrincipalIdentifier": principal } });
        // Compared member by member, with the one member the two paths cannot agree on left
        // out: `LastUpdated` is the instant each path's own write ran, so what is compared is
        // the record each left behind, plus that both recorded the write at all.
        let from_single = list_permissions(&routers[0], case, &filter).await;
        let from_batch = list_permissions(&routers[1], case, &filter).await;
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
            "case {case:?}: expected the batch to leave what the single revoke leaves \
             ({from_single:?}), got {from_batch:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_revoke_is_refused_whole_past_the_documented_entry_limit() {
    // "A list of up to 20 entries for resource permissions to be revoked by batch operation
    // to the principal" (`BatchRevokePermissionsRequest$Entries`): the bound itself is
    // applied, one past it is refused, and a refusal of the request takes NOTHING away.
    let catalog = account_catalog();
    let database = json!({ "Database": { "Name": "sales" } });
    let principal = |count: usize, index: usize| {
        format!("arn:aws:iam::{catalog}:role/revoke-limit-{count}-{index}")
    };

    for count in [1_usize, 20, 21] {
        let router = seeded().await;
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                entry(
                    &format!("entry-{index}"),
                    &principal(count, index),
                    &database,
                    &["DESCRIBE"],
                )
            })
            .collect();
        for index in 0..count {
            grant(
                &router,
                "within the limit",
                &principal(count, index),
                &database,
                &["DESCRIBE"],
            )
            .await;
        }

        let case = format!("{count} entries");
        if count > 20 {
            let (status, payload) =
                call(&router, "BatchRevokePermissions", &json!({ "Entries": entries })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {payload:?}"
            );
            assert_eq!(
                error_code(&payload),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {payload:?}"
            );
        } else {
            let failures = batch_revoke(&router, &case, &entries).await;
            assert!(
                failures.is_empty(),
                "case {case:?}: expected every entry to be applied, got {failures:?}"
            );
        }

        for index in 0..count {
            let principal = principal(count, index);
            let actual = held(&router, &case, &principal).await;
            let expected: BTreeSet<String> = if count > 20 {
                // A refused batch took nothing away.
                BTreeSet::from(["DESCRIBE".to_string()])
            } else {
                BTreeSet::new()
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {principal} to hold {expected:?}, got {actual:?}"
            );
        }
    }
}
