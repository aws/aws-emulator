// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal the data plane states, where one defect has more than one wording and the
//! surface that answers decides between them.
//!
//! The rules pinned here, each over a table of inputs rather than one request:
//!
//! 1. a tag key in the reserved namespace is refused with the sentence that states the
//!    prefix a key may not carry and then names the offending key as the request spelled
//!    it, and no tag of that call is applied;
//! 2. the non-emptiness a key value must have is stated in two wordings, and which one a
//!    resume token earns follows the READ: a `Query` names the key attribute alone,
//!    whichever schema it pages, while a `Scan` states the value unsupported for a
//!    SECONDARY INDEX key and names the index it validated the token against beside the
//!    element — for every element of the token, the table's key attributes that index does
//!    not declare included;
//! 3. a resume token that is no key of the schema being paged earns ONE fixed sentence
//!    with nothing appended, a key element of another type than the schema declares among
//!    the ways a token can fail to be that key;
//! 4. a `LastEvaluatedKey` of a parallel scan belongs to the segment it came from, so a
//!    `Scan` that presents it under another segment of the same scan is refused rather
//!    than served a slice it was not asked for;
//! 5. an expression stops at the token the grammar cannot take there, and a word this
//!    grammar reads as an OPERATOR is never an attribute name — so a dangling logical
//!    operator reports THAT operator and the submitted text around it;
//! 6. a refusal the expression languages raise names the request member that carried the
//!    text, and the PartiQL surface — whose request carries a statement rather than an
//!    expression member — states the same sentence bare, top level and per batch slot
//!    alike;
//! 7. a PartiQL `DELETE` may ask only for the whole item it removed, so every other
//!    `RETURNING` form is refused and the item stays;
//! 8. a write addressed THROUGH an index-qualified path (`UPDATE`, `DELETE FROM`) states
//!    that the operation is not supported on an index, while the verb whose clause only
//!    names where a new item goes (`INSERT INTO`) is refused for that clause.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front doors use, from `crate::emulator`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok, REGION};

/// The account every ARN below names: the one this emulator serves.
const ACCOUNT: &str = "000000000000";

/// The ARN form a tagging call addresses a table by.
fn table_arn(name: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{name}")
}

/// Create one table from its whole `CreateTable` body and wait for it to report `ACTIVE`.
async fn create_table(router: &MultiServiceRouter, case: &str, body: Value) {
    let name = body["TableName"]
        .as_str()
        .expect("the create names a table")
        .to_string();
    ok(router, "CreateTable", body).await;
    describe_when_active(router, case, &name).await;
}

/// Create a table whose primary key is `pk` (`S`) alone.
async fn create_keyed_table(router: &MultiServiceRouter, name: &str) {
    create_table(
        router,
        name,
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

/// Run one request that must be refused `ValidationException`, and report its message.
async fn refused(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> String {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected a 400 ValidationException, got {status} with \
         {answered}"
    );
    message(&answered)
}

// ---------------------------------------------------------------------------
// Rule 1: the reserved tag-key namespace
// ---------------------------------------------------------------------------

/// The sentence a key in the reserved namespace earns, over the key as the request
/// spelled it.
fn reserved_tag_key(key: &str) -> String {
    format!("Tag Key cannot be prefixed with aws:, Key: {key}")
}

/// The tags a resource carries, as the eventually consistent reader reports them right
/// now.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> Value {
    let listed = ok(router, "ListTagsOfResource", json!({"ResourceArn": arn})).await;
    let mut tags = serde_json::Map::new();
    for tag in listed["Tags"].as_array().cloned().unwrap_or_default() {
        let key = tag["Key"].as_str().unwrap_or_default().to_string();
        tags.insert(key, tag["Value"].clone());
    }
    Value::Object(tags)
}

/// How long a poll for a tag write to become visible may take before a suite calls it a
/// hang: generous slack over the emulator's own propagation span, because a loaded machine
/// is not a defect and the point of the bound is that the wait ENDS.
const TAG_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long [`tags_when_visible`] waits between pollings.
const TAG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Poll `ListTagsOfResource` until it reports a set carrying `key`, answering that set.
///
/// "TagResource is an asynchronous operation ... ListTagsOfResource API will only reflect
/// the changes after a few seconds" (the `TagResource` doc), so a client reads a tag write
/// back by polling. The wait is bounded, and a write that never shows up panics naming the
/// last set rather than hanging.
async fn tags_when_visible(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    key: &str,
) -> Value {
    let started = std::time::Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < TAG_DEADLINE {
        let tags = listed_tags(router, arn).await;
        if tags.get(key).is_some() {
            return tags;
        }
        last = tags;
        tokio::time::sleep(TAG_POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the tag {key:?} never became visible within {TAG_DEADLINE:?}, last {last}")
}

#[tokio::test]
async fn a_reserved_tag_key_states_the_prefix_rule_and_applies_no_tag_of_the_call() {
    let router = emulator();
    create_keyed_table(&router, "ReservedTagKeys").await;
    let arn = table_arn("ReservedTagKeys");

    // The rows differ in how the key spells the reserved prefix — the prefix alone, the
    // prefix with a namespace AWS itself mints, and the case the caller sent, since the
    // prefix is matched however it is spelled — and in WHERE the offending key sits in
    // the request's list, because the message names the key and not the position.
    for (case, key) in [
        ("the prefix with a key behind it", "aws:owner"),
        ("the prefix alone", "aws:"),
        (
            "a prefixed key AWS itself mints",
            "aws:cloudformation:stack-name",
        ),
        ("the prefix in upper case", "AWS:billing"),
        ("the prefix in mixed case", "Aws:Owner"),
    ] {
        for (position, companion) in [("first", 0), ("last", 1)] {
            let tags = if companion == 0 {
                json!([{"Key": key, "Value": "v"}, {"Key": "stage", "Value": "prod"}])
            } else {
                json!([{"Key": "stage", "Value": "prod"}, {"Key": key, "Value": "v"}])
            };
            let case = format!("{case}, stated {position}");
            let stated = refused(
                &router,
                &case,
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tags}),
            )
            .await;
            assert_eq!(
                stated,
                reserved_tag_key(key),
                "case {case:?}: expected {:?}, got {stated:?}",
                reserved_tag_key(key)
            );
        }
    }

    // That no tag of a refused call is applied is read through the eventually consistent
    // tag reader: a LEGAL write sent after the refused one becomes visible no earlier than
    // the refused one's tags would have, so the set reported once that write shows up is
    // what the refused call left behind.
    ok(
        &router,
        "TagResource",
        json!({"ResourceArn": arn, "Tags": [{"Key": "env", "Value": "prod"}]}),
    )
    .await;
    tags_when_visible(&router, "the tag set a legal write applies", &arn, "env").await;
    refused(
        &router,
        "a refused call beside a key that would be legal",
        "TagResource",
        json!({"ResourceArn": arn,
               "Tags": [{"Key": "aws:owner", "Value": "v"}, {"Key": "stage", "Value": "prod"}]}),
    )
    .await;
    ok(
        &router,
        "TagResource",
        json!({"ResourceArn": arn, "Tags": [{"Key": "marker", "Value": "1"}]}),
    )
    .await;
    let tags = tags_when_visible(&router, "the write after the refused call", &arn, "marker").await;
    assert_eq!(
        tags,
        json!({"env": "prod", "marker": "1"}),
        "case a refused call beside a key that would be legal: neither key of the refused call \
         is applied, got {tags}"
    );

    // The same rule on the OTHER member that takes a tag list, and the control that keys
    // merely resembling the prefix are stored.
    let stated = refused(
        &router,
        "a create carrying a reserved key",
        "CreateTable",
        json!({
            "TableName": "ReservedTagKeysOnCreate",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "Tags": [{"Key": "aws:owner", "Value": "v"}],
        }),
    )
    .await;
    assert_eq!(
        stated,
        reserved_tag_key("aws:owner"),
        "case a create carrying a reserved key: expected the same sentence, got {stated:?}"
    );
    ok(
        &router,
        "TagResource",
        json!({"ResourceArn": arn,
               "Tags": [{"Key": "awsome", "Value": "1"}, {"Key": "team:aws:owner", "Value": "2"}]}),
    )
    .await;
    let tags = tags_when_visible(
        &router,
        "keys that only resemble the prefix",
        &arn,
        "team:aws:owner",
    )
    .await;
    assert_eq!(
        tags,
        json!({"env": "prod", "marker": "1", "awsome": "1", "team:aws:owner": "2"}),
        "case keys that only resemble the prefix: both are stored, got {tags}"
    );
}

// ---------------------------------------------------------------------------
// Rules 2 and 3: what a paged read says about the resume token it was handed
// ---------------------------------------------------------------------------

/// The table the cursor rows page: a composite primary key, a global secondary index
/// whose key is composite, and a second one whose key is BINARY — so a row can vary the
/// schema the token is read through and the kind of empty value it carries.
const CURSORS: &str = "CursorRefusalVariants";

/// The index declared FIRST, which is the one a `Scan` naming no index validates its
/// token against.
const BY_G: &str = "by-g";

/// The second index, whose key is binary.
const BY_B: &str = "by-b";

/// AWS's sentence for an empty key value stated against the TABLE's key schema.
fn empty_key(kind: &str, attribute: &str) -> String {
    format!(
        "One or more parameter values are not valid. The AttributeValue for a key attribute \
         cannot contain an empty {kind} value. Key: {attribute}"
    )
}

/// AWS's sentence for the same value stated against a SECONDARY INDEX key.
fn empty_index_key(kind: &str, index: &str, attribute: &str) -> String {
    format!(
        "One or more parameter values are not valid. A value specified for a secondary index key \
         is not supported. The AttributeValue for a key attribute cannot contain an empty {kind} \
         value. IndexName: {index}, IndexKey: {attribute}"
    )
}

/// AWS's one sentence for a token that is no key of the schema being paged.
const START_KEY_INVALID: &str = "The provided starting key is invalid";

/// Create the cursor table and store one item that carries every key attribute, so a
/// refusal below is about the token and not about an empty table.
async fn cursor_table(router: &MultiServiceRouter) {
    create_table(
        router,
        "the cursor table",
        json!({
            "TableName": CURSORS,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gpk", "AttributeType": "S"},
                {"AttributeName": "gsk", "AttributeType": "S"},
                {"AttributeName": "bpk", "AttributeType": "B"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": BY_G,
                    "KeySchema": [
                        {"AttributeName": "gpk", "KeyType": "HASH"},
                        {"AttributeName": "gsk", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": BY_B,
                    "KeySchema": [{"AttributeName": "bpk", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
        }),
    )
    .await;
    ok(
        router,
        "PutItem",
        json!({"TableName": CURSORS, "Item": {
            "pk": {"S": "p"}, "sk": {"S": "s"},
            "gpk": {"S": "g"}, "gsk": {"S": "g2"}, "bpk": {"B": "YQ=="},
        }}),
    )
    .await;
}

/// The `KeyConditionExpression` and values a `Query` of the schema `index` pages needs:
/// an equality on the partition key of that schema, with the type it declares.
fn condition_on(index: Option<&str>) -> (String, Value) {
    match index {
        Some(BY_G) => ("gpk = :p".to_string(), json!({":p": {"S": "g"}})),
        Some(BY_B) => ("bpk = :p".to_string(), json!({":p": {"B": "YQ=="}})),
        _ => ("pk = :p".to_string(), json!({":p": {"S": "p"}})),
    }
}

/// One paged read of `CURSORS`, over the schema `index` names, resuming `token`.
fn paged_read(operation: &str, index: Option<&str>, token: &Value) -> Value {
    let mut request = json!({"TableName": CURSORS, "ExclusiveStartKey": token.clone()});
    if let Some(index) = index {
        request["IndexName"] = json!(index);
    }
    if operation == "Query" {
        let (condition, values) = condition_on(index);
        request["KeyConditionExpression"] = json!(condition);
        request["ExpressionAttributeValues"] = values;
    }
    request
}

#[tokio::test]
async fn an_empty_element_of_a_resume_token_is_worded_by_the_read_that_was_handed_it() {
    let router = emulator();
    cursor_table(&router).await;

    // Each row: the schema paged, the token, and the sentence each reader states. The rows
    // span the schema the token is read through (the base table, each index), WHICH
    // element of the token is empty (a partition key, a sort key, and a table key
    // attribute carried by an INDEX cursor, which that index declares no key for) and the
    // KIND of empty value (a string and a blob).
    let rows: Vec<(&str, Option<&str>, Value, String, String)> = vec![
        (
            "the table's partition key, empty",
            None,
            json!({"pk": {"S": ""}, "sk": {"S": "s"}}),
            empty_key("string", "pk"),
            empty_index_key("string", BY_G, "pk"),
        ),
        (
            "the table's sort key, empty",
            None,
            json!({"pk": {"S": "p"}, "sk": {"S": ""}}),
            empty_key("string", "sk"),
            empty_index_key("string", BY_G, "sk"),
        ),
        (
            "an index's own partition key, empty",
            Some(BY_G),
            json!({"gpk": {"S": ""}, "gsk": {"S": "g2"},
                   "pk": {"S": "p"}, "sk": {"S": "s"}}),
            empty_key("string", "gpk"),
            empty_index_key("string", BY_G, "gpk"),
        ),
        (
            "an index's own sort key, empty",
            Some(BY_G),
            json!({"gpk": {"S": "g"}, "gsk": {"S": ""},
                   "pk": {"S": "p"}, "sk": {"S": "s"}}),
            empty_key("string", "gsk"),
            empty_index_key("string", BY_G, "gsk"),
        ),
        (
            // The table key attribute an index cursor carries, which the index itself
            // declares no key for: the sentence still names the index being paged.
            "a table key attribute of an index cursor, empty",
            Some(BY_G),
            json!({"gpk": {"S": "g"}, "gsk": {"S": "g2"},
                   "pk": {"S": ""}, "sk": {"S": "s"}}),
            empty_key("string", "pk"),
            empty_index_key("string", BY_G, "pk"),
        ),
        (
            "an index's binary partition key, empty",
            Some(BY_B),
            json!({"bpk": {"B": ""}, "pk": {"S": "p"}, "sk": {"S": "s"}}),
            empty_key("binary", "bpk"),
            empty_index_key("binary", BY_B, "bpk"),
        ),
    ];

    for (case, index, token, queried, scanned) in rows {
        for operation in ["Query", "Scan"] {
            let case = format!("{case} on {operation}");
            let expected = if operation == "Query" {
                queried.clone()
            } else {
                scanned.clone()
            };
            let stated = refused(
                &router,
                &case,
                operation,
                paged_read(operation, index, &token),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    // The control: the same tokens with NON-empty values page both reads, so the rows
    // above are about the empty value and not about the token's shape.
    for (case, index, token) in [
        (
            "the table's own key",
            None,
            json!({"pk": {"S": "p"}, "sk": {"S": "s"}}),
        ),
        (
            "an index's key beside the table's",
            Some(BY_G),
            json!({"gpk": {"S": "g"}, "gsk": {"S": "g2"},
                   "pk": {"S": "p"}, "sk": {"S": "s"}}),
        ),
    ] {
        for operation in ["Query", "Scan"] {
            let page = ok(&router, operation, paged_read(operation, index, &token)).await;
            assert_eq!(
                page["Count"],
                json!(0),
                "case {case:?} on {operation}: the page resumes strictly after the only item, \
                 got {page}"
            );
        }
    }
}

#[tokio::test]
async fn a_resume_token_that_is_no_key_of_the_schema_states_one_sentence_with_nothing_appended() {
    let router = emulator();
    cursor_table(&router).await;

    // The rows span HOW the token fails to be a key of the schema it is read through: a
    // key element of another TYPE than the schema declares (the defect that used to be
    // diagnosed further), an element the schema declares no key for, an element left out,
    // a spare element beside the key, and no element at all. An element of an INDEX's own
    // key schema stated in another type is NOT one of them on the table WALK: that element
    // is a key of the order being paged, so the walk states the typed parameter sentence for
    // it instead — and so it does for a key attribute of the TABLE that an index cursor
    // carries beside it (see
    // [`an_index_cursor_states_a_typed_mismatch_for_an_index_key_element`], which also drives
    // the `Query` that states this sentence for them).
    let rows: Vec<(&str, Option<&str>, Value)> = vec![
        (
            "the table's partition key as a number",
            None,
            json!({"pk": {"N": "1"}, "sk": {"S": "s"}}),
        ),
        (
            "the table's sort key as a number",
            None,
            json!({"pk": {"S": "p"}, "sk": {"N": "1"}}),
        ),
        (
            "the table's partition key as a string set",
            None,
            json!({"pk": {"SS": ["p"]}, "sk": {"S": "s"}}),
        ),
        (
            "an attribute the schema declares no key for",
            None,
            json!({"nope": {"S": "x"}}),
        ),
        (
            "a declared key element left out",
            None,
            json!({"pk": {"S": "p"}}),
        ),
        (
            "a spare element beside the key",
            None,
            json!({"pk": {"S": "p"}, "sk": {"S": "s"}, "spare": {"S": "x"}}),
        ),
        ("no element at all", None, json!({})),
    ];

    for (case, index, token) in rows {
        for operation in ["Query", "Scan"] {
            let case = format!("{case} on {operation}");
            let stated = refused(
                &router,
                &case,
                operation,
                paged_read(operation, index, &token),
            )
            .await;
            assert_eq!(
                stated, START_KEY_INVALID,
                "case {case:?}: expected exactly {START_KEY_INVALID:?}, got {stated:?}"
            );
        }
    }
}

/// AWS's sentence for an index key value whose type is not the one the table's
/// `AttributeDefinitions` declare for that attribute, which an index cursor's own key
/// element earns.
fn index_key_type_mismatch(attribute: &str, expected: &str, actual: &str, index: &str) -> String {
    format!(
        "One or more parameter values were invalid: Type mismatch for Index Key {attribute} \
         Expected: {expected} Actual: {actual} IndexName: {index}"
    )
}

#[tokio::test]
async fn an_index_cursor_states_a_typed_mismatch_for_an_index_key_element() {
    let router = emulator();
    cursor_table(&router).await;

    // The rows span WHICH element of the cursor carries the wrong type and WHICH type it
    // carries: the sort key of the composite index as a number and as a string set, the
    // partition key of the binary index as a string, and a key attribute of the TABLE that an
    // index cursor carries beside the index key — which the walk reports under the index this
    // read was pointed at, like the rest.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "an index's own sort key as a number",
            BY_G,
            json!({"gpk": {"S": "g"}, "gsk": {"N": "2"},
                   "pk": {"S": "p"}, "sk": {"S": "s"}}),
            index_key_type_mismatch("gsk", "S", "N", BY_G),
        ),
        (
            "an index's own sort key as a string set",
            BY_G,
            json!({"gpk": {"S": "g"}, "gsk": {"SS": ["g2"]},
                   "pk": {"S": "p"}, "sk": {"S": "s"}}),
            index_key_type_mismatch("gsk", "S", "SS", BY_G),
        ),
        (
            "an index's own partition key as a string",
            BY_B,
            json!({"bpk": {"S": "a"}, "pk": {"S": "p"}, "sk": {"S": "s"}}),
            index_key_type_mismatch("bpk", "B", "S", BY_B),
        ),
        (
            "the table's partition key, in an index cursor, as a number",
            BY_G,
            json!({"gpk": {"S": "g"}, "gsk": {"S": "g2"},
                   "pk": {"N": "1"}, "sk": {"S": "s"}}),
            index_key_type_mismatch("pk", "S", "N", BY_G),
        ),
    ];

    for (case, index, token, detailed) in rows {
        // The table WALK details the element; a `Query` states the one invalid-token
        // sentence for every token it cannot place in the schema it pages, that element
        // included, so the same cursor earns the two sentences on the two reads.
        for (operation, expected) in [
            ("Query", START_KEY_INVALID.to_string()),
            ("Scan", detailed.clone()),
        ] {
            let case = format!("{case} on {operation}");
            let stated = refused(
                &router,
                &case,
                operation,
                paged_read(operation, Some(index), &token),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected exactly {expected:?}, got {stated:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a parallel scan's resume token belongs to its segment
// ---------------------------------------------------------------------------

/// The sentence a token presented under another segment earns, over the pair the request
/// asked for.
fn wrong_segment(total: i64, segment: i64) -> String {
    format!(
        "{START_KEY_INVALID}: Invalid ExclusiveStartKey. Please use ExclusiveStartKey with \
         correct Segment. TotalSegments: {total} Segment: {segment}"
    )
}

/// One `Scan` request over the segmented table.
fn segmented_scan(name: &str, segment: i64, total: i64, token: Option<&Value>) -> Value {
    let mut request = json!({"TableName": name, "Segment": segment, "TotalSegments": total});
    if let Some(token) = token {
        request["ExclusiveStartKey"] = token.clone();
    }
    request
}

#[tokio::test]
async fn a_parallel_scan_refuses_a_resume_token_that_came_from_another_segment() {
    let router = emulator();
    let name = "SegmentResumeTokens";
    create_keyed_table(&router, name).await;
    // Enough partition keys that the segments of each division below hold items to page.
    for position in 0..24 {
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"pk": {"S": format!("item-{position:03}")}}}),
        )
        .await;
    }

    // The rows are two divisions of the same table, so the rule is asserted over more than
    // one `TotalSegments` and over every segment of each.
    for total in [2_i64, 4] {
        // What each segment's first page stopped at, for the segments that hold items.
        let mut tokens: Vec<(i64, Value)> = Vec::new();
        for segment in 0..total {
            let case = format!("segment {segment} of {total}");
            let mut first = segmented_scan(name, segment, total, None);
            // Only a page that stopped with rows left reports a key to resume from, so the
            // first page of each segment is held to one item.
            first["Limit"] = json!(1);
            let page = ok(&router, "Scan", first).await;
            if let Some(token) = page.get("LastEvaluatedKey") {
                assert!(
                    token["pk"]["S"].is_string(),
                    "case {case:?}: a reported stop key is the table's own key, got {page}"
                );
                tokens.push((segment, token.clone()));
            }
        }
        assert!(
            tokens.len() >= 2,
            "case TotalSegments {total}: the seeded items must leave at least two segments \
             with a page to resume, got {} tokens",
            tokens.len()
        );

        for (segment, token) in &tokens {
            // The same segment resumes: the token is this segment's own.
            let resumed = ok(
                &router,
                "Scan",
                segmented_scan(name, *segment, total, Some(token)),
            )
            .await;
            assert!(
                resumed["ScannedCount"].as_u64().is_some(),
                "case segment {segment} of {total} resuming its own token: expected a page, \
                 got {resumed}"
            );
            // Every OTHER segment of the same division is refused.
            for other in (0..total).filter(|other| other != segment) {
                let case = format!("a token of segment {segment} presented to segment {other} of \
                                    {total}");
                let stated = refused(
                    &router,
                    &case,
                    "Scan",
                    segmented_scan(name, other, total, Some(token)),
                )
                .await;
                let expected = wrong_segment(total, other);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected {expected:?}, got {stated:?}"
                );
            }
            // A scan that asks for NO division asked for no segment either, so the same
            // token resumes it: the rule is about the pair the request states.
            let sequential = ok(
                &router,
                "Scan",
                json!({"TableName": name, "ExclusiveStartKey": token.clone()}),
            )
            .await;
            assert!(
                sequential["ScannedCount"].as_u64().is_some(),
                "case a token presented to a sequential scan: expected a page, got {sequential}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: where an expression stops
// ---------------------------------------------------------------------------

/// The clause a syntax error states, over the member the text arrived in.
fn syntax_clause(member: &str, token: &str, near: &str) -> String {
    format!("Invalid {member}: Syntax error; token: \"{token}\", near: \"{near}\"")
}

#[tokio::test]
async fn an_operator_keyword_where_an_operand_belongs_is_where_the_parse_stops() {
    let router = emulator();
    let name = "OperatorKeywordPositions";
    create_keyed_table(&router, name).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": name, "Item": {"pk": {"S": "a"}, "s": {"S": "x"}}}),
    )
    .await;

    // Each row: the filter, the token the parse must stop at, and the submitted text
    // around it. The rows span WHICH operator stands where an operand belongs (`AND`,
    // `OR`, `BETWEEN`), what precedes it (a logical operator of the same kind, one of
    // the other kind, a `NOT`, a function's `(`), and the position past the last token,
    // which has no token to name.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a doubled AND",
            "#s = :v AND AND #s = :v",
            "AND",
            "AND AND #s",
        ),
        ("a doubled OR", "#s = :v OR OR #s = :v", "OR", "OR OR #s"),
        (
            "an OR after an AND",
            "#s = :v AND OR #s = :v",
            "OR",
            "AND OR #s",
        ),
        ("an AND after a NOT", "NOT AND #s = :v", "AND", "NOT AND #s"),
        (
            "a BETWEEN where an operand belongs",
            "#s = :v AND BETWEEN :v AND :v",
            "BETWEEN",
            "AND BETWEEN :v",
        ),
        (
            "an operator keyword inside a function's path operand",
            "attribute_exists(AND)",
            "AND",
            "(AND)",
        ),
        (
            "a dangling AND at the end of the expression",
            "#s = :v AND",
            "<EOF>",
            "AND",
        ),
    ];

    for (case, expression, token, near) in rows {
        let stated = refused(
            &router,
            case,
            "Scan",
            json!({
                "TableName": name,
                "FilterExpression": expression,
                "ExpressionAttributeNames": {"#s": "s"},
                "ExpressionAttributeValues": {":v": {"S": "x"}},
            }),
        )
        .await;
        let expected = syntax_clause("FilterExpression", token, near);
        assert_eq!(
            stated, expected,
            "case {case:?} ({expression}): expected {expected:?}, got {stated:?}"
        );
    }

    // The same rule in the member whose own lexer makes the keyword a token, which is how
    // a key condition has always read it.
    let stated = refused(
        &router,
        "a doubled AND in a key condition",
        "Query",
        json!({
            "TableName": name,
            "KeyConditionExpression": "pk = :p AND AND pk = :p",
            "ExpressionAttributeValues": {":p": {"S": "a"}},
        }),
    )
    .await;
    let expected = syntax_clause("KeyConditionExpression", "AND", "AND AND pk");
    assert_eq!(
        stated, expected,
        "case a doubled AND in a key condition: expected {expected:?}, got {stated:?}"
    );

    // The controls: the OPERATOR position still reports the token standing there, and an
    // expression the grammar reads is served.
    let stated = refused(
        &router,
        "a comparison with no operator",
        "Scan",
        json!({"TableName": name, "FilterExpression": "pk AND"}),
    )
    .await;
    let expected = syntax_clause("FilterExpression", "AND", "pk AND");
    assert_eq!(
        stated, expected,
        "case a comparison with no operator: expected {expected:?}, got {stated:?}"
    );
    let served = ok(
        &router,
        "Scan",
        json!({
            "TableName": name,
            "FilterExpression": "#s = :v AND attribute_exists(pk)",
            "ExpressionAttributeNames": {"#s": "s"},
            "ExpressionAttributeValues": {":v": {"S": "x"}},
        }),
    )
    .await;
    assert_eq!(
        served["Count"],
        json!(1),
        "case a filter the grammar reads: expected the stored item, got {served}"
    );
}

// ---------------------------------------------------------------------------
// Rules 6, 7 and 8: what the PartiQL surface states
// ---------------------------------------------------------------------------

/// The table the statement rows address.
const STATEMENTS: &str = "StatementRefusalVariants";

/// Create that table and store two items: one whose `s` is a STRING, so an update
/// computing on it meets a stored value of the wrong type, and one whose `n` is a NUMBER,
/// which the sibling slot of a batch computes on.
async fn statement_table(router: &MultiServiceRouter) {
    create_keyed_table(router, STATEMENTS).await;
    for item in [
        json!({"pk": {"S": "a"}, "s": {"S": "text"}, "n": {"N": "1"}}),
        json!({"pk": {"S": "b"}, "n": {"N": "1"}}),
    ] {
        ok(
            router,
            "PutItem",
            json!({"TableName": STATEMENTS, "Item": item}),
        )
        .await;
    }
}

/// The sentence an over-long `IN` list earns, over the operand count the text states.
fn too_many_in_operands(operands: usize) -> String {
    format!("The IN operator is provided with too many operands; number of operands: {operands}")
}

/// A `SELECT` whose `IN` list carries `operands` literals.
fn in_list_statement(operands: usize) -> String {
    let list: Vec<String> = (0..operands).map(|at| format!("'v{at}'")).collect();
    format!(
        "SELECT * FROM \"{STATEMENTS}\" WHERE pk = 'a' AND s IN ({})",
        list.join(", ")
    )
}

#[tokio::test]
async fn a_statement_refusal_states_the_sentence_alone_on_every_statement_surface() {
    let router = emulator();
    statement_table(&router).await;

    // Each row: a statement and the WHOLE sentence it earns. The rows span the defect
    // class — a bound on the text the statement states, a stored value the update's
    // operator cannot be applied to, and a document path the stored item cannot hold —
    // and none of them names a request member, because the request carries a statement.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an IN list one operand past its ceiling",
            in_list_statement(101),
            too_many_in_operands(101),
        ),
        (
            "an IN list well past its ceiling",
            in_list_statement(150),
            too_many_in_operands(150),
        ),
        (
            "an update computing on a stored string",
            format!("UPDATE \"{STATEMENTS}\" SET s = s + 1 WHERE pk = 'a'"),
            "An operand in the update expression has an incorrect data type".to_string(),
        ),
        (
            "an update path through a stored scalar",
            format!("UPDATE \"{STATEMENTS}\" SET s.deeper = 1 WHERE pk = 'a'"),
            "The document path provided in the update expression is invalid for update".to_string(),
        ),
    ];

    for (case, statement, expected) in &rows {
        let stated = refused(
            &router,
            case,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        assert_eq!(
            stated, *expected,
            "case {case:?} ({statement}): expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.starts_with("Invalid Statement"),
            "case {case:?}: the sentence carries no member prefix, got {stated:?}"
        );
    }

    // In a BATCH the sentence is the slot's own `Error.Message`, and the sibling slot
    // still applies — which is what a client reading the per-slot error inspects.
    let (status, answered) = call(
        &router,
        "BatchExecuteStatement",
        json!({"Statements": [
            {"Statement": format!("UPDATE \"{STATEMENTS}\" SET s = s + 1 WHERE pk = 'a'")},
            {"Statement": format!("UPDATE \"{STATEMENTS}\" SET n = n + 1 WHERE pk = 'b'")},
        ]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a batch with one refused slot: a batch answers 200 whatever its slots say, got \
         {status} with {answered}"
    );
    assert_eq!(
        answered["Responses"][0]["Error"],
        json!({
            "Code": "ValidationError",
            "Message": "An operand in the update expression has an incorrect data type",
        }),
        "case a batch with one refused slot: expected the bare sentence under ValidationError, \
         got {answered}"
    );
    assert!(
        answered["Responses"][1].get("Error").is_none(),
        "case a batch with one refused slot: the sibling slot still applies, got {answered}"
    );
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": STATEMENTS, "Key": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["s"],
        json!({"S": "text"}),
        "case a batch with one refused slot: the refused slot wrote nothing, got {read}"
    );
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": STATEMENTS, "Key": {"pk": {"S": "b"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["n"],
        json!({"N": "2"}),
        "case a batch with one refused slot: the sibling slot applied, got {read}"
    );

    // The control the rule draws its line against: the SAME defect stated through an
    // expression MEMBER names that member, because the request carries one.
    let stated = refused(
        &router,
        "the same IN ceiling in a FilterExpression",
        "Scan",
        json!({
            "TableName": STATEMENTS,
            "FilterExpression": format!(
                "#s IN ({})",
                (0..101).map(|at| format!(":v{at}")).collect::<Vec<String>>().join(", ")
            ),
            "ExpressionAttributeNames": {"#s": "s"},
            "ExpressionAttributeValues": (0..101)
                .map(|at| (format!(":v{at}"), json!({"S": format!("v{at}")})))
                .collect::<serde_json::Map<String, Value>>(),
        }),
    )
    .await;
    let expected = format!("Invalid FilterExpression: {}", too_many_in_operands(101));
    assert_eq!(
        stated, expected,
        "case the same IN ceiling in a FilterExpression: expected {expected:?}, got {stated:?}"
    );

    // And the control that the ceiling is the ceiling: the published count is served.
    let served = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": in_list_statement(100)}),
    )
    .await;
    assert!(
        served.get("Items").is_some(),
        "case an IN list AT its ceiling: expected a served statement, got {served}"
    );
}

#[tokio::test]
async fn a_statement_delete_may_only_ask_for_the_whole_item_it_removed() {
    let router = emulator();
    statement_table(&router).await;

    // Every `RETURNING` form the grammar spells, with whether a `DELETE` may ask for it:
    // the whole item as it stood is the one projection a removal has, and each other form
    // names a projection of an update.
    for (case, clause, allowed) in [
        ("the whole item as it stood", "ALL OLD *", true),
        ("the changed attributes as they stood", "MODIFIED OLD *", false),
        ("the whole item after the write", "ALL NEW *", false),
        ("the changed attributes after the write", "MODIFIED NEW *", false),
    ] {
        // Each row starts from the stored item, so what it asserts is its own clause.
        ok(
            &router,
            "PutItem",
            json!({"TableName": STATEMENTS, "Item": {"pk": {"S": "d"}, "s": {"S": "text"}}}),
        )
        .await;
        let statement =
            format!("DELETE FROM \"{STATEMENTS}\" WHERE pk = 'd' RETURNING {clause}");
        let key = json!({"TableName": STATEMENTS, "Key": {"pk": {"S": "d"}}});
        if allowed {
            let deleted = ok(&router, "ExecuteStatement", json!({"Statement": statement})).await;
            assert_eq!(
                deleted["Items"],
                json!([{"pk": {"S": "d"}, "s": {"S": "text"}}]),
                "case {case:?}: expected the image of the removed item, got {deleted}"
            );
            let read = ok(&router, "GetItem", key).await;
            assert!(
                read.get("Item").is_none(),
                "case {case:?}: the item is removed, got {read}"
            );
        } else {
            let stated = refused(
                &router,
                case,
                "ExecuteStatement",
                json!({"Statement": statement}),
            )
            .await;
            let expected = format!(
                "Invalid returning clause: RETURNING {clause}. Only RETURNING ALL OLD * is \
                 allowed in DELETE statements."
            );
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
            let read = ok(&router, "GetItem", key).await;
            assert_eq!(
                read["Item"],
                json!({"pk": {"S": "d"}, "s": {"S": "text"}}),
                "case {case:?}: a refused statement removes nothing, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_write_through_an_index_qualified_path_states_the_rule_its_verb_broke() {
    let router = emulator();
    cursor_table(&router).await;

    // The two verbs that ADDRESS a stored item through the path they name state the index
    // sentence, whichever way the path spells its qualifier; the verb whose clause only
    // names where a new item goes is refused for that clause, at the token the path opens
    // with.
    const ON_AN_INDEX: &str = "This operation is not supported on an index";
    for (case, statement) in [
        (
            "an UPDATE through a quoted qualifier",
            format!(
                "UPDATE \"{CURSORS}\".\"{BY_G}\" SET note = 'x' WHERE pk = 'p' AND sk = 's'"
            ),
        ),
        (
            "an UPDATE through a bare qualifier",
            format!("UPDATE {CURSORS}.anyindex SET note = 'x' WHERE pk = 'p' AND sk = 's'"),
        ),
        (
            "a DELETE through a quoted qualifier",
            format!("DELETE FROM \"{CURSORS}\".\"{BY_G}\" WHERE pk = 'p' AND sk = 's'"),
        ),
        (
            "a DELETE through a bare qualifier",
            format!("DELETE FROM {CURSORS}.anyindex WHERE pk = 'p' AND sk = 's'"),
        ),
    ] {
        let stated = refused(
            &router,
            case,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        assert_eq!(
            stated, ON_AN_INDEX,
            "case {case:?} ({statement}): expected {ON_AN_INDEX:?}, got {stated:?}"
        );
    }

    // The `INSERT` names its target inside a clause, so its refusal is about that clause
    // and reports where the path OPENS: line, column and the width of that token, counted
    // in characters from 1.
    for (case, statement, column, width) in [
        (
            "an INSERT through a quoted qualifier",
            format!("INSERT INTO \"{CURSORS}\".\"{BY_G}\" VALUE {{'pk':'q','sk':'s'}}"),
            13,
            CURSORS.chars().count() + 2,
        ),
        (
            "an INSERT through a bare qualifier",
            format!("INSERT INTO {CURSORS}.anyindex VALUE {{'pk':'q','sk':'s'}}"),
            13,
            CURSORS.chars().count(),
        ),
    ] {
        let stated = refused(
            &router,
            case,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        let expected = format!(
            "FROM clause may only contain a single table name in data manipulation statements \
             at 1:{column}:{width}"
        );
        assert_eq!(
            stated, expected,
            "case {case:?} ({statement}): expected {expected:?}, got {stated:?}"
        );
    }

    // The control: the same qualified path is a legal source for a READ, and each write
    // verb naming the table alone is served.
    let read = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": format!("SELECT * FROM \"{CURSORS}\".\"{BY_G}\" WHERE gpk = 'g'")}),
    )
    .await;
    assert_eq!(
        read["Items"].as_array().map(Vec::len),
        Some(1),
        "case a read through the same qualified path: expected the stored item, got {read}"
    );
    for (case, statement) in [
        (
            "an INSERT naming the table alone",
            format!("INSERT INTO \"{CURSORS}\" VALUE {{'pk':'q','sk':'s'}}"),
        ),
        (
            "an UPDATE naming the table alone",
            format!("UPDATE \"{CURSORS}\" SET note = 'x' WHERE pk = 'p' AND sk = 's'"),
        ),
        (
            "a DELETE naming the table alone",
            format!("DELETE FROM \"{CURSORS}\" WHERE pk = 'p' AND sk = 's'"),
        ),
    ] {
        let (status, answered) =
            call(&router, "ExecuteStatement", json!({"Statement": statement})).await;
        assert_eq!(
            status, 200,
            "case {case}: statement {statement:?} must be served, got {status} with {answered}"
        );
    }
}
