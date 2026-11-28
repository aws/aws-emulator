// SPDX-License-Identifier: CC0-1.0

//! The page size the two ITEM READS SHARE is worded by each of them in the form that operation
//! gives it, and a CONTROL-PLANE listing words the same violation class in the walk's.
//!
//! `Query$Limit` and `Scan$Limit` are the same member name targeting the same shape under the
//! same trait — `PositiveIntegerObject` with `@range{min:1}`
//! (`smithy_query(query="operation", operation="Query,Scan")`) — so a page size below that
//! minimum is one violation class on one member, and NOTHING in the model tells the two reads
//! apart. They are nevertheless recorded wording it differently: the KEYED read states the
//! ITEM-REQUEST form, naming the member by its WIRE spelling with none of the rejected value
//! echoed, while the table WALK states the request validator's own form, naming the member by
//! its lowerCamel path with the rejected value quoted. `ListBackups$Limit` declares the same
//! violation class on a member of the same name and is recorded in that second form too — so
//! the form is a fact of the served OPERATION and the member, not of the member alone, which is
//! asserted here as an INEQUALITY between the two item reads' messages and an EQUALITY of form
//! between the walk's and the listing's, beside the literal each one carries, so a build that
//! collapses the two reads onto one wording fails here whichever wording it chose.
//!
//! The sweep spans what the rule generalises over: the rejected value (zero, one below the
//! minimum, a handful below, and both integral extremes a 32-bit member carries) and, for the
//! other side of the rule, the published bound itself, which is served rather than refused.
//! `tests/all/item_request_clause_form.rs` and `tests/all/page_size_clause_forms.rs` table the
//! forms of every other bounded member beside these.

use serde_json::json;

use crate::emulator::{call, describe_when_active, error_code, message, ok};

/// The table both reads address: one hash key, which is all a refusal about the page size
/// needs.
const TABLE: &str = "SharedPageSizeClause";

/// The whole message the KEYED read carries for a page size below the published minimum: the
/// member as the model spells it, with nothing echoed.
const KEYED_READ_FORM: &str = "1 validation error detected: Value at 'Limit' failed to satisfy \
                               constraint: Member must have value greater than or equal to 1";

/// The whole message the table WALK and a control-plane LISTING carry for the same violation:
/// the validator's lowerCamel path with the rejected value quoted, which is why it is built per
/// value.
fn echoing_form(limit: i64) -> String {
    format!(
        "1 validation error detected: Value '{limit}' at 'limit' failed to satisfy constraint: \
         Member must have value greater than or equal to 1"
    )
}

/// The page sizes the sweep rejects: zero, one below the minimum, a handful below it, and both
/// integral extremes a 32-bit member can carry below it.
const REJECTED: [i64; 5] = [0, -1, -9, -2_147_483_647, -2_147_483_648];

/// Create [`TABLE`] and wait out the `CREATING` -> `ACTIVE` transition the clock owes it.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = crate::emulator::emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "create the table both reads address", TABLE).await;
    router
}

/// The message the refusal of a page size of `limit` through `operation` carries, asserted to
/// be the 400 `ValidationException` the class is reported as first.
async fn refused_page_size(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    limit: i64,
) -> String {
    let request = match operation {
        "Query" => json!({"TableName": TABLE, "Limit": limit,
                          "KeyConditionExpression": "pk = :p",
                          "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        "Scan" => json!({"TableName": TABLE, "Limit": limit}),
        _ => json!({"Limit": limit}),
    };
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {operation} with Limit {limit}: expected 400 ValidationException, got {status} \
         {body}"
    );
    message(&body)
}

#[tokio::test]
async fn each_item_read_words_the_one_page_size_violation_its_own_way() {
    let router = seeded().await;

    for limit in REJECTED {
        let keyed = refused_page_size(&router, "Query", limit).await;
        let walk = refused_page_size(&router, "Scan", limit).await;
        assert_ne!(
            keyed, walk,
            "case \"Limit {limit}\": the two item reads word one violation class on one member \
             differently, got {keyed:?} for both"
        );
        assert_eq!(
            keyed, KEYED_READ_FORM,
            "case \"Query, Limit {limit}\": the keyed read names the member as the model spells \
             it and echoes nothing, got {keyed:?}"
        );
        assert!(
            !keyed.contains(&limit.to_string()),
            "case \"Query, Limit {limit}\": this form echoes no part of the rejected value, got \
             {keyed:?}"
        );
        assert_eq!(
            walk,
            echoing_form(limit),
            "case \"Scan, Limit {limit}\": the walk names the member by the validator's path and \
             quotes the rejected value, got {walk:?}"
        );
    }
}

#[tokio::test]
async fn a_control_plane_listing_words_its_page_size_the_walks_way() {
    let router = seeded().await;

    // The other half of the rule: `ListBackups$Limit` is a page size of the same name under the
    // same kind of bound, and the listing states it by the validator's lowerCamel path WITH the
    // rejected value — the walk's form, and not the keyed read's, so the inequality above is
    // not "every page-size clause is worded differently" either.
    for limit in REJECTED {
        let stated = refused_page_size(&router, "ListBackups", limit).await;
        assert_eq!(
            stated,
            echoing_form(limit),
            "case \"ListBackups Limit {limit}\": expected {:?}, got {stated:?}",
            echoing_form(limit)
        );
        assert_ne!(
            stated, KEYED_READ_FORM,
            "case \"ListBackups Limit {limit}\": the listing's form is not the keyed read's, got \
             {stated:?}"
        );
    }
}

#[tokio::test]
async fn the_published_minimum_is_served_by_both_item_reads() {
    let router = seeded().await;

    // The other side of the rule: the clauses above are about the BOUND, not about the member
    // being read, so the smallest page size the range admits is served by both reads.
    for operation in ["Query", "Scan"] {
        let request = match operation {
            "Query" => json!({"TableName": TABLE, "Limit": 1,
                              "KeyConditionExpression": "pk = :p",
                              "ExpressionAttributeValues": {":p": {"S": "one"}}}),
            _ => json!({"TableName": TABLE, "Limit": 1}),
        };
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (200, "<no __type>"),
            "case {operation:?}: the published minimum page size is served, got {status} {body}"
        );
        let stated = message(&body);
        assert!(
            !stated.contains("failed to satisfy constraint"),
            "case {operation:?}: a page size at its bound is no constraint violation, got \
             {stated:?}"
        );
    }
}
