// SPDX-License-Identifier: CC0-1.0

//! How a re-used `ClientToken` is REFUSED on the bulk-operation surface, and in whose
//! words.
//!
//! The rule pinned here: a token inside its idempotency window stands for the whole request
//! it was first used with, so a retry that changes ANY other member of that request is
//! refused 400 `IdempotentParameterMismatchException` ("DynamoDB rejected the request
//! because you retried a request with a different payload but with an idempotent token that
//! was already used"), with the fixed sentence the REFUSING SURFACE words for that error —
//! for a bulk operation, the one that states the token was ALREADY used. The sentence has no
//! part taken from the request: neither the token nor the member that changed appears in it,
//! so every row below earns exactly the same text.
//!
//! The rows span what the rule generalises over: WHICH member of the request the retry
//! changed (the write-unit ceiling, the source it reads, the compression that source is
//! staged with, the location outcomes are reported into, the performance tier, the table) —
//! one sentence for all of them. The byte-identical replay beside them is the positive
//! control: it is answered from the operation the token already started rather than refused.
//!
//! The last row is the CONTRAST: the transactional surface refuses the same modelled error
//! with its own sentence, so the two are asserted side by side and neither can drift into
//! the other.
//!
//! Amazon S3 is mounted beside DynamoDB, exactly as `core/router` assembles them, so the
//! source the first call reads is a real staged object.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    call, create_bucket, emulator_with_s3, error_code, message, ok, put_object,
};
use crate::table_items::create_active_table;

/// The bucket the source object is staged in.
const BUCKET: &str = "bulk-put-token-message";

/// The table the bulk put writes into, and a second one a row addresses instead.
const TABLE: &str = "BulkPutTokenMessage";
const OTHER_TABLE: &str = "BulkPutTokenMessageOther";

/// The token every call below carries.
const TOKEN: &str = "bulk-token-message";

/// AWS's fixed sentence for a bulk operation's token re-used with a different request.
const BULK_MISMATCH: &str = "Specified idempotent token was already used with different request \
                             parameters within the idempotency window";

/// AWS's fixed sentence for the same modelled error on the TRANSACTIONAL surface, which
/// words it its own way.
const TRANSACTION_MISMATCH: &str = "Specified idempotent token was used with different request \
                                    parameters within the idempotency window";

/// One `StartBulkPutItems` body carrying [`TOKEN`]: the table it writes into, the source it
/// reads and the write-unit ceiling it asks for.
///
/// `OutputS3Configuration` — and the `OutputType` it reports — are required of the input
/// (`smithy_query(query="closure", operation="StartBulkPutItems")`), so every body names
/// them.
fn start_body(table: &str, prefix: &str, max_write_request_units: i64) -> Value {
    json!({
        "TableName": table,
        "ClientToken": TOKEN,
        "InputS3Configuration": {
            "S3Bucket": BUCKET,
            "S3KeyPrefix": prefix,
            "InputFormat": "DYNAMODB_JSON",
        },
        "OutputS3Configuration": {
            "S3Bucket": BUCKET,
            "S3KeyPrefix": "outcomes/",
            "OutputType": ["ERROR"],
        },
        "MaxWriteRequestUnits": max_write_request_units,
    })
}

/// The emulator with Amazon S3 beside it, the two tables `ACTIVE`, and one item staged under
/// each source prefix a row names.
async fn seeded() -> MultiServiceRouter {
    let router = emulator_with_s3();
    create_bucket(&router, BUCKET).await;
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    create_active_table(&router, OTHER_TABLE, &[("pk", "HASH", "S")]).await;
    for (prefix, key) in [("first/", "one"), ("second/", "two")] {
        put_object(
            &router,
            BUCKET,
            &format!("{prefix}items.json"),
            format!("{}\n", json!({"Item": {"pk": {"S": key}}})).into_bytes(),
        )
        .await;
    }
    router
}

#[tokio::test]
async fn a_bulk_operation_token_reused_with_another_request_names_it_already_used() {
    let router = seeded().await;

    // The call the token comes to stand for.
    let first = ok(&router, "StartBulkPutItems", start_body(TABLE, "first/", 1024)).await;
    let started = first["BulkOperationDescription"]["BulkOperationArn"].clone();
    assert!(
        started.is_string(),
        "the first call records the operation the token stands for, got {first}"
    );

    // The byte-identical replay is answered from that operation rather than refused, so the
    // refusals below are about the members that CHANGED and not about the token.
    let replayed = ok(&router, "StartBulkPutItems", start_body(TABLE, "first/", 1024)).await;
    assert_eq!(
        replayed["BulkOperationDescription"]["BulkOperationArn"], started,
        "an identical replay answers the operation the token already started, got {replayed}"
    );

    // Each row changes ONE member of the request the token stands for; all of them earn the
    // same fixed sentence.
    let changed_source = start_body(TABLE, "second/", 1024);
    let changed_rate = start_body(TABLE, "first/", 2048);
    let changed_table = start_body(OTHER_TABLE, "first/", 1024);
    let changed_compression = {
        let mut body = start_body(TABLE, "first/", 1024);
        body["InputS3Configuration"]["InputCompressionType"] = json!("GZIP");
        body
    };
    let changed_outcomes = {
        let mut body = start_body(TABLE, "first/", 1024);
        body["OutputS3Configuration"]["S3KeyPrefix"] = json!("elsewhere/");
        body
    };
    let changed_tier = {
        let mut body = start_body(TABLE, "first/", 1024);
        body["PerformanceTier"] = json!("STANDARD");
        body
    };
    for (case, body) in [
        ("another source", changed_source),
        ("another write-unit ceiling", changed_rate),
        ("another table", changed_table),
        ("another compression", changed_compression),
        ("another outcome location", changed_outcomes),
        ("another performance tier", changed_tier),
    ] {
        let (status, answered) = call(&router, "StartBulkPutItems", body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "IdempotentParameterMismatchException"),
            "case {case:?}: a token reused with another parameter is refused, got {status} with \
             {answered}"
        );
        assert_eq!(
            message(&answered),
            BULK_MISMATCH,
            "case {case:?}: expected the bulk surface's fixed sentence, got {answered}"
        );
    }

    // The CONTRAST: the same modelled error on the transactional surface keeps that
    // surface's own sentence.
    let transaction = |key: &str| {
        json!({
            "ClientRequestToken": "txn-token-message",
            "TransactItems": [{"Put": {"TableName": TABLE, "Item": {"pk": {"S": key}}}}],
        })
    };
    ok(&router, "TransactWriteItems", transaction("txn-one")).await;
    let (status, answered) = call(&router, "TransactWriteItems", transaction("txn-two")).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "IdempotentParameterMismatchException"),
        "a transaction token reused with other actions is refused, got {status} with {answered}"
    );
    assert_eq!(
        message(&answered),
        TRANSACTION_MISMATCH,
        "expected the transactional surface's own fixed sentence, got {answered}"
    );
}
