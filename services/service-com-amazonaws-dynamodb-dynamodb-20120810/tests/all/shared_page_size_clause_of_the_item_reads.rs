// SPDX-License-Identifier: CC0-1.0

//! The two ITEM READS word the page-size clause they SHARE the one way, and a CONTROL-PLANE
//! listing words the same violation class its own way.
//!
//! `Query$Limit` and `Scan$Limit` are the same member name targeting the same shape under the
//! same trait — `PositiveIntegerObject` with `@range{min:1}`
//! (`smithy_query(query="operation", operation="Query,Scan")`) — so a page size below that
//! minimum is one violation class on one member, and NOTHING in the model tells the two reads
//! apart. They are recorded stating it in the ITEM-REQUEST form: the member by its WIRE
//! spelling, with none of the rejected value echoed. `ListBackups$Limit` declares the same
//! violation class on a member of the same name and is recorded stating it in the request
//! validator's own form, naming the member by its lowerCamel path with the rejected value
//! quoted — so the form is a fact of the served OPERATION and not of the member, which is
//! asserted here as an EQUALITY between the two item reads' messages and an INEQUALITY against
//! the listing's, beside the literal each one carries, so a build that collapses the listing's
//! wording onto the reads' or splits the two reads apart fails here whichever wording it chose.
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

/// The whole message either ITEM READ carries for a page size below the published minimum: the
/// member as the model spells it, with nothing echoed.
const ITEM_READ_FORM: &str = "1 validation error detected: Value at 'Limit' failed to satisfy \
                              constraint: Member must have value greater than or equal to 1";

/// The whole message a control-plane LISTING carries for the same violation: the validator's
/// lowerCamel path with the rejected value quoted, which is why it is built per value.
fn listing_form(limit: i64) -> String {
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
async fn both_item_reads_word_the_one_page_size_violation_the_same_way() {
    let router = seeded().await;

    for limit in REJECTED {
        let keyed = refused_page_size(&router, "Query", limit).await;
        let walk = refused_page_size(&router, "Scan", limit).await;
        assert_eq!(
            keyed, walk,
            "case \"Limit {limit}\": the two item reads state one violation class on one \
             member the one way, got {keyed:?} and {walk:?}"
        );
        for (operation, stated) in [("Query", &keyed), ("Scan", &walk)] {
            assert_eq!(
                stated, ITEM_READ_FORM,
                "case \"{operation}, Limit {limit}\": the item read names the member as the \
                 model spells it and echoes nothing, got {stated:?}"
            );
            assert!(
                !stated.contains(&limit.to_string()),
                "case \"{operation}, Limit {limit}\": this form echoes no part of the rejected \
                 value, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_control_plane_listing_words_its_own_page_size_the_other_way() {
    let router = seeded().await;

    // The contrast: `ListBackups$Limit` is a page size of the same name under the same kind of
    // bound, and the listing states it by the validator's lowerCamel path WITH the rejected
    // value — the form neither item read takes, so the equality above is not "every page-size
    // clause is worded alike".
    for limit in REJECTED {
        let stated = refused_page_size(&router, "ListBackups", limit).await;
        assert_eq!(
            stated,
            listing_form(limit),
            "case \"ListBackups Limit {limit}\": expected {:?}, got {stated:?}",
            listing_form(limit)
        );
        assert_ne!(
            stated, ITEM_READ_FORM,
            "case \"ListBackups Limit {limit}\": the listing's own form is not the item reads', \
             got {stated:?}"
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
