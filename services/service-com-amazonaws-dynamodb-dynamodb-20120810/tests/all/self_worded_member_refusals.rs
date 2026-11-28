// SPDX-License-Identifier: CC0-1.0

//! The members whose VALUE this service resolves ITSELF, refused with the sentence that
//! resolution words rather than with the bound the model declares on them.
//!
//! The rule: a member of that group — an ARN this service parses, a tag the account would
//! carry, an attribute name an item would be keyed by — is refused `ValidationException`
//! (HTTP 400) with its own sentence WHATEVER way the value failed, the `@length` bound the
//! model declares on the member included. A length clause states a different rule from the
//! one the caller broke ("this is not a backup ARN", "this tag value is invalid"), and it
//! exposes a member path the service never states for these members.
//!
//! The suites are TABLES spanning what the rule generalises over: for each member, a value
//! below the modelled minimum, one inside it, and one above the maximum must all earn the
//! SAME sentence, and the sentence's variable part comes from the request.
//!
//! The second half pins the other value half a refusal states: a COLLECTION member past its
//! `@length` maximum is quoted as the collection the request carried, rendered from the
//! request itself, rather than as a count of what the validating walk measured.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message};
use crate::table_items::create_active_table;

/// The table the attribute-name rows address.
const TABLE: &str = "SelfWorded";

/// AWS's fixed sentence for a `BackupArn` this endpoint cannot read as a backup ARN.
const INVALID_BACKUP_ARN: &str = "Invalid Backup ARN";

/// The longest an attribute NAME may be, in bytes: the maximum the shape every member
/// carrying one targets declares (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#AttributeName")` gives `@length` 0..65535).
const MAX_ATTRIBUTE_NAME_BYTES: usize = 65535;

/// The longest a tag VALUE may be (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#TagValueString")` gives `@length` 0..256).
const MAX_TAG_VALUE: usize = 256;

/// Call `operation`, require 400 `ValidationException`, and answer the message.
async fn refusal(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
         {body}"
    );
    message(&body)
}

/// A string of `length` copies of `filler`.
fn filled(filler: char, length: usize) -> String {
    std::iter::repeat_n(filler, length).collect()
}

/// The head of a message, for the assertion text: a refusal that names a 64 KB value back
/// is a message no panic should print whole.
fn head_of(message: &str) -> String {
    let head: String = message.chars().take(140).collect();
    if head.len() < message.len() {
        format!("{head}… [{} bytes]", message.len())
    } else {
        head
    }
}

#[tokio::test]
async fn a_malformed_backup_arn_is_refused_by_its_own_sentence_at_every_length() {
    let router = emulator();

    // The modelled bound on the member is `@length` 37..1024
    // (`smithy_query(query="operation", operation="DeleteBackup")`), so the rows span one
    // below the minimum, the minimum itself, the maximum, and one above it — every one of
    // them a value that is no backup ARN, so every one earns the same sentence.
    for length in [1, 36, 37, 1024, 1025] {
        let arn = filled('z', length);
        for operation in ["DeleteBackup", "DescribeBackup"] {
            let case = format!("{operation} with a {length}-character BackupArn");
            let stated = refusal(&router, &case, operation, json!({"BackupArn": arn})).await;
            assert_eq!(
                stated, INVALID_BACKUP_ARN,
                "case {case:?}: expected the backup-ARN sentence, got {stated:?}"
            );
        }
    }

    // A well-formed ARN of this endpoint that names no backup is a LOOKUP miss and not this
    // refusal, so the sentence above is about the value's form and nothing else.
    let shaped = format!(
        "arn:aws:dynamodb:us-east-1:000000000000:table/{TABLE}/backup/01700000000000-abcdef12"
    );
    let (status, body) = call(&router, "DeleteBackup", json!({"BackupArn": shaped})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "BackupNotFoundException"),
        "case a well-formed ARN naming no backup: expected the lookup miss, got {status} with \
         {body}"
    );
}

#[tokio::test]
async fn a_tag_the_account_cannot_carry_is_refused_by_its_own_sentence() {
    let router = emulator();

    // A tag VALUE past the maximum the model declares, over the lengths that bracket it:
    // the bound itself is served (the table is created), and one past it is refused with the
    // sentence naming the value as the request spelled it.
    for length in [MAX_TAG_VALUE, MAX_TAG_VALUE + 1, MAX_TAG_VALUE + 64] {
        let value = filled('v', length);
        let request = json!({
            "TableName": format!("Tagged{length}"),
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "Tags": [{"Key": "env", "Value": value}],
        });
        let case = format!("a {length}-character tag value");
        if length <= MAX_TAG_VALUE {
            let (status, body) = call(&router, "CreateTable", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a value at the bound is served, got {status} with {body}"
            );
            continue;
        }
        let stated = refusal(&router, &case, "CreateTable", request).await;
        assert_eq!(
            stated,
            format!("The Tag Value provided is invalid, Value: {value}"),
            "case {case:?}: expected the tag-value sentence naming the value, got {stated:?}"
        );
    }

    // A tag KEY a removal cannot name — the model declares `@length` 1..128 on it, so the
    // EMPTY key is outside the bound — earns the key sentence, naming the key as sent.
    let arn = format!("arn:aws:dynamodb:us-east-1:000000000000:table/{TABLE}");
    for key in ["", &filled('k', 129)] {
        let case = format!("untagging a {}-character key", key.len());
        let stated = refusal(
            &router,
            &case,
            "UntagResource",
            json!({"ResourceArn": arn, "TagKeys": [key]}),
        )
        .await;
        assert_eq!(
            stated,
            format!("The Tag Key provided is invalid, Key: {key}"),
            "case {case:?}: expected the tag-key sentence naming the key, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_attribute_name_past_its_byte_limit_is_refused_wherever_it_is_carried() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;

    let over = filled('w', MAX_ATTRIBUTE_NAME_BYTES + 1);
    let at_the_bound = filled('w', MAX_ATTRIBUTE_NAME_BYTES);
    // Each row carries the over-long name in a SUBSTITUTION entry, under a different
    // placeholder, so the sentence's variable part is visibly the request's own. The two
    // READS and the transactional element state the sentence alone; the single-item write
    // states it inside the counting envelope, which is the split this service states
    // everywhere else about the members a write states its intent through.
    let rows: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "a substitution entry of a keyed read",
            "GetItem",
            "#wide",
            "",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "one"}},
                   "ProjectionExpression": "#wide",
                   "ExpressionAttributeNames": {"#wide": over.clone()}}),
        ),
        (
            "a substitution entry of a single-item write",
            "DeleteItem",
            "#gate",
            "1 validation error detected: ",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "one"}},
                   "ConditionExpression": "attribute_exists(#gate)",
                   "ExpressionAttributeNames": {"#gate": over.clone()}}),
        ),
        (
            "a substitution entry of one transactional action",
            "TransactWriteItems",
            "#col",
            "",
            json!({"TransactItems": [{"Delete": {
                "TableName": TABLE, "Key": {"pk": {"S": "one"}},
                "ConditionExpression": "attribute_exists(#col)",
                "ExpressionAttributeNames": {"#col": over.clone()},
            }}]}),
        ),
    ];
    for (case, operation, key, envelope, request) in rows {
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated,
            format!(
                "{envelope}Attribute name is too large, must be less than {} bytes for key \
                 {key}",
                MAX_ATTRIBUTE_NAME_BYTES + 1
            ),
            "case {case:?}: expected the attribute-name sentence naming the key, got {stated:?}"
        );
    }

    // The MAP KEY of a value — the one position no walk ahead of a handler reports, because
    // a key is a segment of the path a violation is named under — is held to the same bound:
    // an item whose nested map carries one is refused and stores nothing. The over-long name
    // IS the key here, so the sentence states the bound alone, inside the envelope this
    // single-item WRITE states a refusal about the members it writes through in.
    let nested = json!({"TableName": TABLE,
                        "Item": {"pk": {"S": "nested"},
                                 "doc": {"M": {over.clone(): {"S": "v"}}}}});
    let stated = refusal(&router, "a nested map key", "PutItem", nested).await;
    assert_eq!(
        stated,
        format!(
            "1 validation error detected: Attribute name is too large, must be less than {} \
             bytes",
            MAX_ATTRIBUTE_NAME_BYTES + 1
        ),
        "case a nested map key: expected the attribute-name sentence, got {stated:?}"
    );
    let (status, read) = call(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "nested"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        (status, read.get("Item")),
        (200, None),
        "case a nested map key: the refused write stored nothing, got {read}"
    );

    // And the BOUND itself is served: a name of exactly the maximum is a name an item can be
    // stored under and read back by.
    let (status, written) = call(
        &router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": "atbound"},
                        "doc": {"M": {at_the_bound.clone(): {"S": "v"}}}}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a map key at the bound: expected the write to be served, got {status} with \
         {written}"
    );
    let (status, read) = call(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "atbound"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        (status, read["Item"]["doc"]["M"][&at_the_bound]["S"].clone()),
        (200, json!("v")),
        "case a map key at the bound: the stored item reads back whole, got {read}"
    );
}

#[tokio::test]
async fn a_legacy_filter_and_a_legacy_update_hold_their_own_keys_to_the_same_bound() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    let over = filled('m', MAX_ATTRIBUTE_NAME_BYTES + 1);
    let sentence = format!(
        "Attribute name is too large, must be less than {} bytes",
        MAX_ATTRIBUTE_NAME_BYTES + 1
    );

    // Both members are MAPS keyed by an attribute name, so the key is the position the rule
    // is about; the rows differ in which member carried it, in which operation read it, and
    // in the envelope that operation states a refusal about such a member in — a READ narrows
    // its result with the member and states the sentence alone, while a single-item WRITE
    // states its intent through it and carries the counting envelope.
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a legacy scan filter keyed by an over-long name",
            "Scan",
            "",
            json!({"TableName": TABLE,
                   "ScanFilter": {over.clone(): {"ComparisonOperator": "EQ",
                                                 "AttributeValueList": [{"S": "a"}]}}}),
        ),
        (
            "a legacy query filter keyed by an over-long name",
            "Query",
            "",
            json!({"TableName": TABLE,
                   "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"S": "one"}]}},
                   "QueryFilter": {over.clone(): {"ComparisonOperator": "EQ",
                                                  "AttributeValueList": [{"S": "a"}]}}}),
        ),
        (
            "a legacy per-attribute update keyed by an over-long name",
            "UpdateItem",
            "1 validation error detected: ",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "one"}},
                   "AttributeUpdates": {over.clone(): {"Action": "PUT",
                                                       "Value": {"S": "x"}}}}),
        ),
    ];
    for (case, operation, envelope, request) in rows {
        let stated = refusal(&router, case, operation, request).await;
        assert!(
            stated == format!("{envelope}{sentence}"),
            "case {case:?}: expected the attribute-name sentence, got {head:?}",
            head = head_of(&stated)
        );
    }

    // The refused update wrote nothing: no item was created at the key it addressed.
    let (status, read) = call(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "one"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        (status, read.get("Item")),
        (200, None),
        "case a refused legacy update: nothing was written, got {read}"
    );
}

#[tokio::test]
async fn a_collection_past_its_bound_is_quoted_as_the_collection_the_request_carried() {
    let router = emulator();

    // The projection list of an index declaration is `@length` 1..20
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#NonKeyAttributeNameList")`),
    // so the rows span the bound itself (served) and one past it (refused, quoting the list).
    for count in [20usize, 21, 25] {
        let names: Vec<String> = (0..count).map(|at| format!("p{at:02}")).collect();
        let request = json!({
            "TableName": format!("Projected{count}"),
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"},
                                     {"AttributeName": "sk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": "by-sk",
                "KeySchema": [{"AttributeName": "sk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "INCLUDE", "NonKeyAttributes": names},
            }],
        });
        let case = format!("{count} non-key attributes");
        if count <= 20 {
            let (status, body) = call(&router, "CreateTable", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a list at the bound is served, got {status} with {body}"
            );
            continue;
        }
        let stated = refusal(&router, &case, "CreateTable", request).await;
        assert_eq!(
            stated,
            format!(
                "1 validation error detected: Value '[{}]' at \
                 'globalSecondaryIndexes.1.member.projection.nonKeyAttributes' failed to \
                 satisfy constraint: Member must have length less than or equal to 20",
                names.join(", ")
            ),
            "case {case:?}: expected the clause to quote the list the request carried, got \
             {stated:?}"
        );
    }
}
