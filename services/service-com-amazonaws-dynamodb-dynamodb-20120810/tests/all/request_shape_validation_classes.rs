// SPDX-License-Identifier: CC0-1.0

//! Four classes of REQUEST-SHAPE refusal, each driven end to end over the wire.
//!
//! Every suite here is a TABLE spanning what its rule generalises over rather than one
//! recorded request, and every assertion names its case and the value it actually read.
//!
//! RULE 1 — a `RequestItems` map KEY that satisfies none of the constraints a table name
//! is held to is refused as a map key, and the clause QUOTES the map that key sat in, as
//! the JSON the request carried it as: a request-validator clause states the value it
//! refused, which is the only place the offending map is reported. The rows span the way
//! the key breaks the constraints, the members the entry carried, the datatypes its keys
//! hold, the number of entries in the map, and a value whose characters JSON escapes.
//!
//! RULE 2 — the read/write capacity mode a table is created or updated with is stated
//! through either of the two members the model declares for it, `BillingMode` and
//! `TableThroughputMode` (`smithy_query(query="operation", operation="CreateTable")`), both
//! targeting the `BillingMode` enum, so a value outside that set is refused in the
//! request-shape pass — the rejected value echoed, the path of the member that carried it,
//! and the whole admissible set — BEFORE any capacity semantics are evaluated and before
//! the table is looked up, and a value INSIDE it sets the mode whichever member carried it.
//! The rows span the member, the form of the rejected value, the operation that carries it,
//! and what else is wrong with the same request. The boundary rows are the capacity
//! sentences the mode is then held to, which the create and the update state in the same
//! words.
//!
//! RULE 3 — a substitution table (`ExpressionAttributeNames`/`ExpressionAttributeValues`)
//! sent by a request that names no expression THAT TABLE can be substituted into is refused
//! with the sentence that says to add an expression, which is a different defect from
//! entries an expression that IS present does not reference (the sentence that says to drop
//! the entry). A `#alias` goes into any expression member; a `:placeholder` goes only into
//! one that compares or writes a value, so a projection is not one. Whether the sentence
//! carries the `<N> validation error(s) detected: ` envelope follows the rule every refusal
//! about a request's expressions follows: the members a WRITE states its intent through
//! carry it, the members a READ narrows its result with do not, and an ELEMENT's refusal is
//! stated outside it. The rows span the table, the operation, the expression member the
//! request did carry, and whether the members belong to the request or to one of its
//! elements.
//!
//! RULE 4 — a `SELECT` projection list reports one entry per path it names, keyed by the
//! identifier that path is reported under (its alias, else its last named step), so a list
//! that states one identifier twice asks for an item in which one key carries two values
//! and is refused naming the repeated identifier(s). The rows span the ways two items
//! collapse onto one identifier, and the controls span the lists that do not.
//!
//! RULE 5 — a KEY of either substitution table is held to the length a substitution token
//! may be (255 bytes, the whole key as the request spelled it), and the refusal names the
//! member, says the key is too long and states the key's own size; it is reported ahead of
//! the expression that would have dereferenced a token, and ahead of the key's SPELLING,
//! because a key past the limit is past it whatever characters it holds. The rows span the
//! member, the operation, both sides of the published bound, and a key that breaks the
//! length and the spelling at once.
//!
//! RULE 6 — a `KeyConditionExpression` operand that is itself an OPERATION — a name applied
//! to arguments, which is how every value-returning function of the expression language is
//! written — is refused as a nested operation the member does not take, ahead of the generic
//! syntax error the call's `(` would otherwise earn. The rows span the function, the
//! position in the condition and the spelling; the controls span the member's own
//! `begins_with`, the CONDITION functions that keep their own sentence, and the same
//! function in a `FilterExpression`, which is a member that does take it.
//!
//! RULE 7 — every independent violation of one request's declared constraints is reported
//! in ONE counted envelope, so a member whose absence stops the request being read does not
//! mask the bounds its other members broke. The rows span how many constraints one request
//! breaks, which classes they are (an absent member, a length, an enumerated set) and the
//! operation that carries them; the control is the refusal of a single member a check of
//! the service's own words in its own sentence.
//!
//! RULE 8 — a value the request CARRIES is held to the documented nesting depth before the
//! request's expressions are held to the substitution tables beside them, and after what
//! each table says about itself. The rows span what the deep value is stated beside, and
//! every one of them writes nothing.
//!
//! RULE 9 — a transaction past the documented 4 MB aggregate is refused naming the ceiling
//! and the payload the request carried, writes nothing, and a subset of the same items
//! inside the ceiling commits.
//!
//! RULE 10 — an `UpdateItem` is sized against the documented 400 KB item ceiling over the
//! item it leaves behind PLUS a frame for each FURTHER clause it states, so the largest
//! operand one write can carry moves with the clauses beside it. The rows span the clause
//! kinds — a further assignment, an arithmetic operand, a function operand, `ADD`, `DELETE`
//! and `REMOVE` (whose frame is the path it names) — each read off the boundary itself.
//!
//! RULE 12 — and the boundary a SINGLE-clause update is held to is the ceiling itself: the
//! item exactly at 400 KB is stored and one byte more is refused, the clause every update
//! carries costing the operand nothing. The rows span the member the update travels in, the
//! length of the attribute name written and whether the key already holds an item.
//!
//! RULE 11 — the class boundary IN FRONT of all of them: a body value whose JSON class the
//! modelled type of its member cannot be read from never becomes a value, so it fails
//! DESERIALIZATION rather than a rule — answered as the protocol's own
//! `com.amazon.coral.service#SerializationException` with the reason under the capitalized
//! `Message` member, and nothing of the request read or applied — while a value that WAS
//! read and then broke a declared bound keeps the `ValidationException` the rules above are
//! refused with. The rows span the operation, the member, the modelled type of that member,
//! the class the request sent there and the DEPTH the value stands at (a top-level member, a
//! member of a nested structure, an element of a list, a value of a map); the controls are
//! the values that do read.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

// ===========================================================================
// RULE 1. A refused RequestItems key quotes the map it sat in
// ===========================================================================

/// The table the batched rows address when they address a real one.
const BATCHED: &str = "Batched";

/// The whole message a refused `RequestItems` key earns, over the map the clause quotes.
///
/// The constraint set is the one a table name is held to (3..=255 and `[a-zA-Z0-9_.-]+`),
/// which the model does not carry on the map's key shape
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BatchGetRequestMap")` gives
/// a key of `TableArn`, length 1..1024).
fn map_key_clause(quoted: Option<&str>) -> String {
    let value = match quoted {
        Some(rendered) => format!("Value '{rendered}' at"),
        None => "Value at".to_string(),
    };
    format!(
        "1 validation error detected: {value} 'requestItems' failed to satisfy constraint: Map \
         keys must satisfy constraint: [Member must have length less than or equal to 255, \
         Member must have length greater than or equal to 3, Member must satisfy regular \
         expression pattern: [a-zA-Z0-9_.-]+]"
    )
}

#[tokio::test]
async fn a_refused_request_items_key_quotes_the_map_the_request_sent() {
    let router = emulator();
    create_active_table(&router, BATCHED, &[("pk", "HASH", "S")]).await;
    let long_key = "k".repeat(256);

    // (case, the RequestItems map the request sends, the JSON the clause must quote)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a key below the minimum name length, one key, one attribute",
            json!({"ab": {"Keys": [{"pk": {"S": "a"}}]}}),
            r#"{"ab":{"Keys":[{"pk":{"S":"a"}}]}}"#.to_string(),
        ),
        (
            "a key outside the name pattern, two keys",
            json!({"bad name!": {"Keys": [{"pk": {"S": "a"}}, {"pk": {"S": "b"}}]}}),
            r#"{"bad name!":{"Keys":[{"pk":{"S":"a"}},{"pk":{"S":"b"}}]}}"#.to_string(),
        ),
        (
            "a key past the maximum name length, with every other entry member sent",
            json!({long_key.clone(): {"Keys": [{"pk": {"S": "a"}}],
                                      "ConsistentRead": true,
                                      "ProjectionExpression": "#p",
                                      "ExpressionAttributeNames": {"#p": "pk"}}}),
            format!(
                "{{{:?}:{{\"Keys\":[{{\"pk\":{{\"S\":\"a\"}}}}],\"ConsistentRead\":true,\
                 \"ProjectionExpression\":\"#p\",\"ExpressionAttributeNames\":{{\"#p\":\"pk\"}}}}}}",
                long_key
            ),
        ),
        (
            "a key that is an ARN of another service, beside an entry that names a table",
            json!({format!("arn:aws:sqs:us-east-1:000000000000:{BATCHED}"):
                       {"Keys": [{"pk": {"S": "b"}}]},
                   BATCHED: {"Keys": [{"pk": {"S": "a"}}]}}),
            format!(
                "{{\"{BATCHED}\":{{\"Keys\":[{{\"pk\":{{\"S\":\"a\"}}}}]}},\
                 \"arn:aws:sqs:us-east-1:000000000000:{BATCHED}\":\
                 {{\"Keys\":[{{\"pk\":{{\"S\":\"b\"}}}}]}}}}"
            ),
        ),
        (
            "a key of a datatype each, and the legacy attribute list",
            json!({"ab": {"Keys": [{"pk": {"S": "a"}, "n": {"N": "1"},
                                    "b": {"B": "AQ=="}, "ss": {"SS": ["x", "y"]},
                                    "l": {"L": [{"S": "p"}, {"NULL": true}]},
                                    "m": {"M": {"inner": {"BOOL": false}}}}],
                          "AttributesToGet": ["pk", "n"]}}),
            concat!(
                r#"{"ab":{"Keys":[{"b":{"B":"AQ=="},"l":{"L":[{"S":"p"},{"NULL":true}]},"#,
                r#""m":{"M":{"inner":{"BOOL":false}}},"n":{"N":"1"},"pk":{"S":"a"},"#,
                r#""ss":{"SS":["x","y"]}}],"AttributesToGet":["pk","n"]}}"#
            )
            .to_string(),
        ),
        (
            "a key value whose characters JSON escapes",
            json!({"ab": {"Keys": [{"pk": {"S": "a\"b\\c\nd"}}]}}),
            "{\"ab\":{\"Keys\":[{\"pk\":{\"S\":\"a\\\"b\\\\c\\nd\"}}]}}".to_string(),
        ),
    ];

    for (case, request_items, quoted) in rows {
        let (status, body) = call(
            &router,
            "BatchGetItem",
            json!({"RequestItems": request_items}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body}"
        );
        let said = message(&body);
        let expected = map_key_clause(Some(&quoted));
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
    }

    // The OTHER rendering: `BatchWriteItem`'s clause quotes the same map in the OBJECT
    // form the service's own request validator states a refused structure value with, over
    // the fields of its own request objects — which is what the recorded refusals of that
    // member carry (`crate::state::rendered_batch_write_request_items`), and not the JSON
    // the request arrived as.
    let (status, body) = call(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": {"ab": [{"PutRequest": {"Item": {"pk": {"S": "a"}}}}]}}),
    )
    .await;
    let said = message(&body);
    let expected = map_key_clause(Some(
        "{ab=[WriteRequest(putRequest=PutRequest(item={pk=AttributeValue(s=a, n=null, b=null, \
         iNT=null, dECIMAL=null, dOUBLE=null, sS=null, nS=null, bS=null, iNTSET=null, \
         dECIMALSET=null, dOUBLESET=null, m=null, l=null, nULL=null, bOOL=null)}, \
         workloadProfileName=null), deleteRequest=null)]}",
    ));
    assert_eq!(
        (status, said.as_str()),
        (400, expected.as_str()),
        "case a batched WRITE: expected {expected:?}, got {status} with {body}"
    );

    // Both spellings a key accepts still address the table, so the refusals above are
    // about keys that name no table at all.
    for (case, key) in [
        ("the bare table name", BATCHED.to_string()),
        (
            "the table's own ARN",
            format!("arn:aws:dynamodb:us-east-1:000000000000:table/{BATCHED}"),
        ),
    ] {
        let served = ok(
            &router,
            "BatchGetItem",
            json!({"RequestItems": {key.clone(): {"Keys": [{"pk": {"S": "a"}}]}}}),
        )
        .await;
        assert!(
            served["Responses"][&key].is_array(),
            "case {case:?}: expected the entry to be served, got {served}"
        );
    }
}

// ===========================================================================
// RULE 2. A BillingMode outside the declared value set
// ===========================================================================

/// The table the `UpdateTable` rows address.
const MODED: &str = "Moded";

/// The whole message a capacity mode outside the declared set earns: the value echoed from
/// the request, the path of the member that carried it, and the set in the order the model
/// declares it (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#BillingMode")`).
fn throughput_mode_clause(value: &str, path: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy enum value set: [PROVISIONED, PAY_PER_REQUEST]"
    )
}

/// A `CreateTable` body for a table keyed on `pk`, with `extra` merged in.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

#[tokio::test]
async fn a_capacity_mode_outside_the_declared_set_is_refused_as_the_member_it_is() {
    let router = emulator();
    create_active_table(&router, MODED, &[("pk", "HASH", "S")]).await;

    // (case, operation, request, the value the clause must echo, the member path it names).
    // Both members the model declares for the mode are spanned, because a request states
    // it through either one.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a mode no release names, through the throughput-mode member, with no capacity",
            "CreateTable",
            create_body("created-1", json!({"TableThroughputMode": "BOGUS_MODE"})),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "the same value through the billing-mode member",
            "CreateTable",
            create_body("created-2", json!({"BillingMode": "BOGUS_MODE"})),
            "BOGUS_MODE",
            "billingMode",
        ),
        (
            "a declared mode in the wrong case, with capacity supplied",
            "CreateTable",
            create_body(
                "created-3",
                json!({"TableThroughputMode": "provisioned",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1}}),
            ),
            "provisioned",
            "tableThroughputMode",
        ),
        (
            "an empty mode",
            "CreateTable",
            create_body("created-4", json!({"TableThroughputMode": ""})),
            "",
            "tableThroughputMode",
        ),
        (
            "a mode taken from another enum of the same model",
            "CreateTable",
            create_body("created-5", json!({"BillingMode": "STANDARD"})),
            "STANDARD",
            "billingMode",
        ),
        (
            "a plausible newer mode name",
            "CreateTable",
            create_body("created-6", json!({"TableThroughputMode": "ON_DEMAND"})),
            "ON_DEMAND",
            "tableThroughputMode",
        ),
        (
            "the mode clause wins over capacity the resolved mode could not take",
            "CreateTable",
            create_body(
                "created-7",
                json!({"TableThroughputMode": "BOGUS_MODE",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1}}),
            ),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "the mode clause wins over a key schema that names an undeclared attribute",
            "CreateTable",
            json!({"TableName": "created-8",
                   "KeySchema": [{"AttributeName": "absent", "KeyType": "HASH"}],
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                   "TableThroughputMode": "BOGUS_MODE"}),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "the throughput-mode member on an update of a table that exists",
            "UpdateTable",
            json!({"TableName": MODED, "TableThroughputMode": "BOGUS_MODE"}),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "a declared mode in the wrong case, on an update",
            "UpdateTable",
            json!({"TableName": MODED, "BillingMode": "pay_per_request"}),
            "pay_per_request",
            "billingMode",
        ),
        (
            "the mode clause wins over the table not being there at all",
            "UpdateTable",
            json!({"TableName": "never-created", "TableThroughputMode": "BOGUS_MODE"}),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
    ];

    for (case, operation, request, value, path) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
             {body}"
        );
        let said = message(&body);
        let expected = throughput_mode_clause(value, path);
        assert_eq!(
            said, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {said:?}"
        );
    }

    // A request that breaks BOTH members earns one clause per member, counted by the
    // envelope and stated in the order the members are read: the billing-mode member
    // first.
    for (case, operation, request) in [
        (
            "both mode members outside the set, on a create",
            "CreateTable",
            create_body(
                "created-9",
                json!({"BillingMode": "ALSO_BOGUS", "TableThroughputMode": "BOGUS_MODE"}),
            ),
        ),
        (
            "both mode members outside the set, on an update",
            "UpdateTable",
            json!({"TableName": MODED, "BillingMode": "ALSO_BOGUS",
                   "TableThroughputMode": "BOGUS_MODE"}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        let expected = format!(
            "2 validation errors detected: {}; {}",
            throughput_mode_clause("ALSO_BOGUS", "billingMode")
                .trim_start_matches("1 validation error detected: "),
            throughput_mode_clause("BOGUS_MODE", "tableThroughputMode")
                .trim_start_matches("1 validation error detected: ")
        );
        assert_eq!(
            (status, said.as_str()),
            (400, expected.as_str()),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }

    // The BOUNDARY: once the mode IS one the set admits, what the request says about
    // capacity is judged — the two sentences that hold a mode and its capacity together,
    // each stated once by the operations that share them.
    for (case, operation, request, expected) in [
        (
            "a provisioned create with no capacity at all",
            "CreateTable",
            create_body("capacity-1", json!({"TableThroughputMode": "PROVISIONED"})),
            "One or more parameter values were invalid: ReadCapacityUnits and WriteCapacityUnits \
             must both be specified when BillingMode is PROVISIONED",
        ),
        (
            "an on-demand create that names capacity anyway",
            "CreateTable",
            create_body(
                "capacity-2",
                json!({"TableThroughputMode": "PAY_PER_REQUEST",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1}}),
            ),
            "One or more parameter values were invalid: Neither ReadCapacityUnits nor \
             WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST",
        ),
        (
            "an on-demand update that names both the mode and capacity",
            "UpdateTable",
            json!({"TableName": MODED, "TableThroughputMode": "PAY_PER_REQUEST",
                   "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                             "WriteCapacityUnits": 1}}),
            "One or more parameter values were invalid: ProvisionedThroughput cannot be \
             specified when BillingMode is PAY_PER_REQUEST",
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        assert_eq!(
            (status, said.as_str()),
            (400, expected),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }

    // Nothing above was created: a refused create registers no table.
    for name in ["created-1", "created-2", "created-3", "created-4", "created-5",
                 "created-6", "created-7", "created-8", "created-9",
                 "capacity-1", "capacity-2"] {
        let (status, body) = call(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a refused create must register no table, got {status} with {body}"
        );
    }

    // The CONTROL: each declared value is served through either member, and the table
    // reports the mode it was created or updated with.
    let provisioned = ok(
        &router,
        "CreateTable",
        create_body(
            "provisioned-table",
            json!({"BillingMode": "PROVISIONED",
                   "ProvisionedThroughput": {"ReadCapacityUnits": 2,
                                             "WriteCapacityUnits": 3}}),
        ),
    )
    .await;
    assert_eq!(
        provisioned["TableDescription"]["ProvisionedThroughput"]["ReadCapacityUnits"],
        json!(2),
        "case \"a provisioned create\": expected the capacity it asked for, got {provisioned}"
    );
    let on_demand = ok(
        &router,
        "CreateTable",
        create_body("on-demand-table", json!({"BillingMode": "PAY_PER_REQUEST"})),
    )
    .await;
    assert_eq!(
        on_demand["TableDescription"]["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"an on-demand create\": expected the mode it asked for, got {on_demand}"
    );
    let described = describe_when_active(&router, "an on-demand create", "on-demand-table").await;
    assert_eq!(
        described["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"an on-demand create\": expected the stored mode read back, got {described}"
    );

    // An UPDATE that moves a table with no stored capacity into the provisioned mode and
    // names none is the same contradiction, which the update states in its own words: the
    // MEMBER the request had to carry, where the create names the two capacity members it
    // needed.
    let (status, body) = call(
        &router,
        "UpdateTable",
        json!({"TableName": "on-demand-table", "BillingMode": "PROVISIONED"}),
    )
    .await;
    let said = message(&body);
    assert_eq!(
        (status, said.as_str()),
        (
            400,
            "One or more parameter values were invalid: ProvisionedThroughput must be \
             specified when BillingMode is PROVISIONED"
        ),
        "case \"a provisioned update with no capacity\": got {status} with {body}"
    );
    let unchanged = ok(
        &router,
        "DescribeTable",
        json!({"TableName": "on-demand-table"}),
    )
    .await;
    assert_eq!(
        unchanged["Table"]["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"a provisioned update with no capacity\": the refused update must leave the mode \
         alone, got {unchanged}"
    );

    // The same rule one level down: an index of a provisioned table carries its own
    // capacity, and the sentence names the index it is missing from.
    describe_when_active(
        &router,
        "a provisioned create whose index states no capacity",
        "provisioned-table",
    )
    .await;
    for (case, operation, request) in [
        (
            "a provisioned create whose index states no capacity",
            "CreateTable",
            create_body(
                "indexed-capacity",
                json!({"BillingMode": "PROVISIONED",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1},
                       "GlobalSecondaryIndexes": [{
                           "IndexName": "by-pk",
                           "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                           "Projection": {"ProjectionType": "KEYS_ONLY"}}]}),
            ),
        ),
        (
            "a provisioned update whose new index states no capacity",
            "UpdateTable",
            json!({"TableName": "provisioned-table",
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                   "GlobalSecondaryIndexUpdates": [{"Create": {
                       "IndexName": "by-pk",
                       "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                       "Projection": {"ProjectionType": "KEYS_ONLY"}}}]}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        assert_eq!(
            (status, said.as_str()),
            (
                400,
                "One or more parameter values were invalid: Both ReadCapacityUnits and \
                 WriteCapacityUnits must be specified for index: by-pk"
            ),
            "case {case:?} ({operation}): got {status} with {body}"
        );
    }
    let switched = ok(
        &router,
        "UpdateTable",
        json!({"TableName": MODED, "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    assert_eq!(
        switched["TableDescription"]["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"an accepted switch\": expected the mode it asked for, got {switched}"
    );

    // The same two values through the OTHER member: a request states the mode through
    // either one, so a create and an update that use the throughput-mode member are served
    // and the table reports the mode they asked for under both names it answers it with.
    let moded = ok(
        &router,
        "CreateTable",
        create_body(
            "throughput-mode-table",
            json!({"TableThroughputMode": "PAY_PER_REQUEST"}),
        ),
    )
    .await;
    assert_eq!(
        (
            moded["TableDescription"]["BillingModeSummary"]["BillingMode"].clone(),
            moded["TableDescription"]["TableThroughputModeSummary"]["TableThroughputMode"]
                .clone(),
        ),
        (json!("PAY_PER_REQUEST"), json!("PAY_PER_REQUEST")),
        "case \"a create through the throughput-mode member\": expected the mode it asked for, \
         got {moded}"
    );
    describe_when_active(
        &router,
        "a create through the throughput-mode member",
        "throughput-mode-table",
    )
    .await;
    let moved = ok(
        &router,
        "UpdateTable",
        json!({"TableName": "throughput-mode-table",
               "TableThroughputMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 4,
                                         "WriteCapacityUnits": 5}}),
    )
    .await;
    assert_eq!(
        moved["TableDescription"]["TableThroughputModeSummary"]["TableThroughputMode"],
        json!("PROVISIONED"),
        "case \"a switch through the throughput-mode member\": expected the mode it asked for, \
         got {moved}"
    );
    let settled = describe_when_active(
        &router,
        "a switch through the throughput-mode member",
        "throughput-mode-table",
    )
    .await;
    assert_eq!(
        (
            settled["BillingModeSummary"]["BillingMode"].clone(),
            settled["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            settled["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
        ),
        (json!("PROVISIONED"), json!(4), json!(5)),
        "case \"a switch through the throughput-mode member\": expected the stored mode and \
         capacity read back, got {settled}"
    );
}

// ===========================================================================
// RULE 3. A substitution table sent with no expression at all
// ===========================================================================

/// The table the expression rows address.
const SUBSTITUTED: &str = "Substituted";

/// The whole message a request that sent `member` and no expression earns, as an operation
/// whose expression members carry the `<N> validation error(s) detected: ` envelope states
/// it — the members a WRITE states its intent through.
fn only_with_expressions(member: &str) -> String {
    format!("1 validation error detected: {}", bare_sentence(member))
}

/// The same sentence where the envelope does not follow: an operation whose expression
/// members only NARROW a read, and one ELEMENT of a multi-item request, which this service
/// states every refusal about outside the request's own envelope.
fn only_with_expressions_bare(member: &str) -> String {
    bare_sentence(member)
}

/// The sentence itself, which is entirely fixed apart from the table it names.
fn bare_sentence(member: &str) -> String {
    format!("{member} can only be specified when using expressions")
}

#[tokio::test]
async fn a_substitution_table_with_no_expression_is_told_to_add_one() {
    let router = emulator();
    create_active_table(&router, SUBSTITUTED, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": SUBSTITUTED,
               "Item": {"pk": {"S": "item-1"}, "status": {"S": "active"}}}),
    )
    .await;
    let key = json!({"pk": {"S": "item-1"}});

    // (case, operation, request, the whole message it must earn)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "names, on a delete whose only expression member is absent",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeNames": {"#s": "status"}}),
            only_with_expressions("ExpressionAttributeNames"),
        ),
        (
            "values, on the same delete",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions("ExpressionAttributeValues"),
        ),
        (
            "both tables at once: the names are reported",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions("ExpressionAttributeNames"),
        ),
        (
            "names, on a write",
            "PutItem",
            json!({"TableName": SUBSTITUTED,
                   "Item": {"pk": {"S": "item-2"}},
                   "ExpressionAttributeNames": {"#s": "status"}}),
            only_with_expressions("ExpressionAttributeNames"),
        ),
        (
            "values, on an update that names neither of its expression members",
            "UpdateItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeValues": {":v": {"S": "paused"}}}),
            only_with_expressions("ExpressionAttributeValues"),
        ),
        (
            "names, on a read with no projection",
            "GetItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeNames": {"#s": "status"}}),
            only_with_expressions_bare("ExpressionAttributeNames"),
        ),
        (
            "values, on a scan with no filter",
            "Scan",
            json!({"TableName": SUBSTITUTED, "ConsistentRead": true,
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions_bare("ExpressionAttributeValues"),
        ),
        (
            "values, on a scan whose only expression is a projection",
            "Scan",
            json!({"TableName": SUBSTITUTED, "ConsistentRead": true,
                   "ProjectionExpression": "pk",
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions_bare("ExpressionAttributeValues"),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
             {body}"
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {said:?}"
        );
    }

    // The BOUNDARY: this rule is held over the members ONE ASK is stated through. The
    // substitution tables of an element of a TRANSACTIONAL call sit beside the expression
    // members of that element, so they are held to the element's OWN expressions — and an
    // element that states no expression has none to hold them to, so the call is SERVED.
    for (case, operation, request) in [
        (
            "names, on one element of a transactional read",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {
                "TableName": SUBSTITUTED, "Key": key.clone(),
                "ExpressionAttributeNames": {"#s": "status"}}}]}),
        ),
        (
            "values, on one element of a transactional write",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {
                "TableName": SUBSTITUTED,
                "Key": {"pk": {"S": "item-3"}},
                "ExpressionAttributeValues": {":v": {"S": "paused"}}}}]}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({operation}): an element's tables are held to the element's own \
             expressions, so this is served, got {status} with {body}"
        );
    }

    // A BATCHED read is the other side of that boundary: its parameter generations are read
    // over the WHOLE call, so a names table one `RequestItems` entry carries is held to the
    // expressions the REQUEST states — a batch stating no `ProjectionExpression` anywhere is
    // told to add one, and the same table beside an entry that projects is served.
    let (status, body) = call(
        &router,
        "BatchGetItem",
        json!({"RequestItems": {SUBSTITUTED: {
            "Keys": [key.clone()],
            "ExpressionAttributeNames": {"#s": "status"}}}}),
    )
    .await;
    assert_eq!(
        (status, message(&body)),
        (
            400,
            "ExpressionAttributeNames can only be specified when using expressions".to_string()
        ),
        "case names, on one entry of a batched read: expected the table told to add an \
         expression, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "BatchGetItem",
        json!({"RequestItems": {SUBSTITUTED: {
            "Keys": [key.clone()],
            "ProjectionExpression": "#s",
            "ExpressionAttributeNames": {"#s": "status"}}}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case names used by a batched read's own projection: expected the batch served, got \
         {status} with {body}"
    );

    // The BOUNDARY: the OTHER defect class keeps its own sentence. A request that DOES
    // name an expression and offers an entry it never references is told to drop the
    // entry, not to add an expression.
    for (case, operation, request, expected) in [
        (
            "an entry a delete's condition never references",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ConditionExpression": "attribute_exists(pk)",
                   "ExpressionAttributeNames": {"#s": "status"}}),
            "1 validation error detected: Value provided in ExpressionAttributeNames unused in \
             expressions: keys: {#s}"
                .to_string(),
        ),
        (
            "an entry a read's projection never references",
            "GetItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ProjectionExpression": "pk",
                   "ExpressionAttributeNames": {"#s": "status"}}),
            "Value provided in ExpressionAttributeNames unused in expressions: keys: {#s}"
                .to_string(),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        assert_eq!(
            (status, said.as_str()),
            (400, expected.as_str()),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }

    // The CONTROL: a request whose expression uses the tokens it defines is served, and
    // every refusal above left the store where it was.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": SUBSTITUTED, "Key": key.clone(),
               "ProjectionExpression": "#s",
               "ExpressionAttributeNames": {"#s": "status"}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"status": {"S": "active"}}),
        "case \"a projection that uses its names\": expected the projected item, got {read}"
    );
    for absent in ["item-2", "item-3"] {
        let answered = ok(
            &router,
            "GetItem",
            json!({"TableName": SUBSTITUTED, "Key": {"pk": {"S": absent}}}),
        )
        .await;
        assert_eq!(
            answered.get("Item"),
            None,
            "case {absent:?}: a refused write must store nothing, got {answered}"
        );
    }
}

// ===========================================================================
// RULE 4. A SELECT clause that repeats one identifier
// ===========================================================================

/// The table the statement rows read.
const STATEMENTS: &str = "Statements";

#[tokio::test]
async fn a_select_clause_that_repeats_an_identifier_is_refused() {
    let router = emulator();
    create_active_table(&router, STATEMENTS, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": STATEMENTS,
               "Item": {"pk": {"S": "p"}, "s": {"S": "alpha"}, "n": {"N": "1"},
                        "l": {"L": [{"S": "first"}, {"NULL": true}, {"S": "second"}]},
                        "m": {"M": {"nested": {"S": "deep"}, "other": {"N": "2"}}},
                        "k": {"M": {"nested": {"S": "shallow"}}}}}),
    )
    .await;

    // (case, the SELECT clause, the identifier(s) the refusal must name)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "one attribute named twice",
            format!("SELECT s, s FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "s",
        ),
        (
            "an attribute and a nested path that reports under the same name",
            format!("SELECT nested, m.nested FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "nested",
        ),
        (
            "two nested paths whose last steps are the same name",
            format!("SELECT m.nested, k.nested FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "nested",
        ),
        (
            "two items aliased to one identifier",
            format!("SELECT s AS one, n AS one FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "one",
        ),
        (
            "an alias that collides with an attribute named as itself",
            format!("SELECT s, n AS s FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "s",
        ),
        (
            "two identifiers repeated, each named once in the order they repeat",
            format!("SELECT n, s, n, s FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "n, s",
        ),
    ];

    for (case, statement, repeated) in rows {
        let (status, body) = call(&router, "ExecuteStatement", json!({"Statement": statement})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body}"
        );
        let said = message(&body);
        let expected = format!("Duplicate identifiers in select clause: {repeated}");
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
        assert!(
            body.get("Items").is_none(),
            "case {case:?}: a refused statement reports no items, got {body}"
        );
    }

    // The CONTROL: a list whose identifiers are distinct is served — including two paths
    // under one attribute whose last steps differ, and the same colliding pair with an
    // alias to tell them apart.
    for (case, statement, expected) in [
        (
            "two distinct attributes",
            format!("SELECT s, n FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"s": {"S": "alpha"}, "n": {"N": "1"}}]),
        ),
        (
            "two paths under one attribute, reported under their own last steps",
            format!("SELECT m.nested, m.other FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"nested": {"S": "deep"}, "other": {"N": "2"}}]),
        ),
        (
            // Two elements of one list are two entries of the answer: a projected path
            // is reported under the PATH, so the index tells them apart.
            "two indexed paths under one attribute, reported under their paths",
            format!("SELECT l[0], l[1] FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"l[0]": {"S": "first"}, "l[1]": {"NULL": true}}]),
        ),
        (
            "the colliding pair, told apart by an alias",
            format!("SELECT m.nested, k.nested AS shallow FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"nested": {"S": "deep"}, "shallow": {"S": "shallow"}}]),
        ),
        (
            "a list whose members include one of the NULL type",
            format!("SELECT l FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"l": {"L": [{"S": "first"}, {"NULL": true}, {"S": "second"}]}}]),
        ),
    ] {
        let served = ok(&router, "ExecuteStatement", json!({"Statement": statement})).await;
        assert_eq!(
            served["Items"], expected,
            "case {case:?}: expected {expected}, got {served}"
        );
    }

    // A `SELECT *` names no identifier at all, so it is served whatever the item holds.
    let everything = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": format!("SELECT * FROM \"{STATEMENTS}\" WHERE pk = 'p'")}),
    )
    .await;
    assert_eq!(
        everything["Items"].as_array().map(Vec::len),
        Some(1),
        "case \"a star projection\": expected the one matching item, got {everything}"
    );
}

// ===========================================================================
// RULE 5. A substitution table's KEY is held to the length a token may be
// ===========================================================================

/// The longest a key of either substitution table may be: the documented 255-byte limit on
/// a substitution token, counted over the whole key as the request spelled it — the
/// member's prefix character included.
const MAX_KEY_BYTES: usize = 255;

/// The table the key rows address.
const KEYED: &str = "SubstitutionKeys";

/// A key of `member`'s table: its prefix character and `body`.
fn token(prefix: char, body: &str) -> String {
    format!("{prefix}{body}")
}

/// The whole message a key past the limit earns, over the member that carried it and the
/// size of the key itself.
fn too_long_key(member: &str, size: usize, enveloped: bool) -> String {
    let envelope = if enveloped {
        "1 validation error detected: "
    } else {
        ""
    };
    format!(
        "{envelope}{member} contains invalid key: The expression attribute map contains a key \
         that is too long; size of key: {size}"
    )
}

#[tokio::test]
async fn a_substitution_key_past_the_token_limit_is_refused_naming_its_size() {
    let router = emulator();
    create_active_table(&router, KEYED, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": KEYED, "Item": {"pk": {"S": "k1"}, "foo": {"S": "stored"}}}),
    )
    .await;

    // One over the bound, far over it, and a key whose SPELLING is wrong too — spanning
    // both members, a read and a write (which states the refusal inside the envelope), and
    // the operations that carry each table.
    let one_over = token('#', &"a".repeat(MAX_KEY_BYTES));
    let far_over = token('#', &"a".repeat(299));
    let dashed = token('#', &format!("{}-{}", "a".repeat(140), "a".repeat(116)));
    let value_key = token(':', &"v".repeat(MAX_KEY_BYTES));
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a name key one byte over the limit, on a keyed read",
            "GetItem",
            json!({"TableName": KEYED, "Key": {"pk": {"S": "k1"}},
                   "ProjectionExpression": "#a",
                   "ExpressionAttributeNames": {one_over.clone(): "foo"}}),
            too_long_key("ExpressionAttributeNames", MAX_KEY_BYTES + 1, false),
        ),
        (
            "a name key far over the limit",
            "GetItem",
            json!({"TableName": KEYED, "Key": {"pk": {"S": "k1"}},
                   "ProjectionExpression": "#a",
                   "ExpressionAttributeNames": {far_over.clone(): "foo"}}),
            too_long_key("ExpressionAttributeNames", 300, false),
        ),
        (
            // Length BEFORE spelling: the key holds a character no token may, and what it
            // is told about is the length it has to fix anyway.
            "a name key that is too long and holds a character no token may",
            "GetItem",
            json!({"TableName": KEYED, "Key": {"pk": {"S": "k1"}},
                   "ProjectionExpression": "#a",
                   "ExpressionAttributeNames": {dashed.clone(): "foo"}}),
            too_long_key("ExpressionAttributeNames", 258, false),
        ),
        (
            "a value key one byte over the limit, on a table walk",
            "Scan",
            json!({"TableName": KEYED, "FilterExpression": "foo = :v",
                   "ExpressionAttributeValues": {value_key.clone(): {"S": "stored"}}}),
            too_long_key("ExpressionAttributeValues", MAX_KEY_BYTES + 1, false),
        ),
        (
            "a name key one byte over the limit, on a write",
            "UpdateItem",
            json!({"TableName": KEYED, "Key": {"pk": {"S": "k1"}},
                   "UpdateExpression": "SET #a = :v",
                   "ExpressionAttributeNames": {one_over.clone(): "foo"},
                   "ExpressionAttributeValues": {":v": {"S": "written"}}}),
            too_long_key("ExpressionAttributeNames", MAX_KEY_BYTES + 1, true),
        ),
        (
            // The other side of the precedence: a key INSIDE the limit whose spelling is
            // wrong is the syntax error it is.
            "a short key no expression could write",
            "GetItem",
            json!({"TableName": KEYED, "Key": {"pk": {"S": "k1"}},
                   "ProjectionExpression": "#a",
                   "ExpressionAttributeNames": {"#a-b": "foo"}}),
            "ExpressionAttributeNames contains invalid key: Syntax error; key: \"#a-b\""
                .to_string(),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case:?} ({operation}): expected 400 {expected:?}, got {status} with {body}"
        );
    }

    // The BOUND itself: a key exactly at the limit is a key, and the expression that uses
    // it is served.
    let at_the_bound = token('#', &"a".repeat(MAX_KEY_BYTES - 1));
    let served = ok(
        &router,
        "GetItem",
        json!({"TableName": KEYED, "Key": {"pk": {"S": "k1"}},
               "ProjectionExpression": at_the_bound.clone(),
               "ExpressionAttributeNames": {at_the_bound.clone(): "foo"}}),
    )
    .await;
    assert_eq!(
        served["Item"],
        json!({"foo": {"S": "stored"}}),
        "case \"a name key exactly at the limit\": expected the projected attribute, got \
         {served}"
    );
}

// ===========================================================================
// RULE 6. A key condition takes no nested operation
// ===========================================================================

/// The table the key-condition rows query: a composite key, so a condition can state one
/// clause per key attribute.
const KEYED_CONDITION: &str = "NestedKeyConditions";

/// AWS's fixed sentence for a key condition whose operand is an operation.
const NESTED_OPERATIONS: &str =
    "Invalid KeyConditionExpression: KeyConditionExpressions cannot contain nested operations";

/// AWS's fixed sentence for a key condition naming an operator the member does not take.
const INVALID_OPERATOR: &str =
    "Invalid KeyConditionExpression: Invalid operator used in KeyConditionExpression";

#[tokio::test]
async fn a_function_call_in_a_key_condition_is_the_nested_operation_refusal() {
    let router = emulator();
    create_active_table(
        &router,
        KEYED_CONDITION,
        &[("pk", "HASH", "S"), ("sk", "RANGE", "S")],
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": KEYED_CONDITION,
               "Item": {"pk": {"S": "p1"}, "sk": {"S": "s1"}, "note": {"S": "abcde"}}}),
    )
    .await;

    // The function, where in the condition it sits, how it is spelled, and which operand
    // it is applied to.
    let values = json!({":n": {"N": "5"}, ":p": {"S": "p1"}, ":s": {"S": "s1"}});
    for (case, expression) in [
        ("the size function on the partition key", "size(pk) = :n"),
        ("the same call with a space before its arguments", "size (pk) = :n"),
        (
            "the call on the right-hand side of the comparison",
            "pk = :p AND :n = size(sk)",
        ),
        (
            "a call beside a comparison the member does take",
            "pk = :p AND size(sk) = :n",
        ),
        ("a call on an aliased name", "size(#p) = :n"),
        ("a call naming no function of the language", "bogus(pk) = :p"),
    ] {
        let (status, body) = call(
            &router,
            "Query",
            json!({"TableName": KEYED_CONDITION, "KeyConditionExpression": expression,
                   "ExpressionAttributeNames": {"#p": "pk"},
                   "ExpressionAttributeValues": values.clone()}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", NESTED_OPERATIONS),
            "case {case:?}: expected the nested-operations refusal, got {status} with {body}"
        );
    }

    // The CONTROLS. The member's own function is a token of its grammar, and the CONDITION
    // functions are an operator it does not take rather than an operand it cannot read.
    for (case, expression, expected) in [
        (
            "a condition function the member does not take",
            "pk = :p AND contains(sk, :s)",
            INVALID_OPERATOR,
        ),
        (
            "another condition function",
            "pk = :p AND attribute_exists(sk)",
            INVALID_OPERATOR,
        ),
    ] {
        let (status, body) = call(
            &router,
            "Query",
            json!({"TableName": KEYED_CONDITION, "KeyConditionExpression": expression,
                   "ExpressionAttributeValues": {":p": {"S": "p1"}, ":s": {"S": "s1"}}}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", expected),
            "case {case:?}: expected {expected:?}, got {status} with {body}"
        );
    }

    // And the member's own `begins_with`, plus the SAME call in a `FilterExpression` — a
    // member that does take an operation — still selects the item.
    for (case, body) in [
        (
            "the member's own prefix function",
            json!({"TableName": KEYED_CONDITION,
                   "KeyConditionExpression": "pk = :p AND begins_with(sk, :s)",
                   "ExpressionAttributeValues": {":p": {"S": "p1"}, ":s": {"S": "s"}}}),
        ),
        (
            "the same function in a filter beside a plain key condition",
            json!({"TableName": KEYED_CONDITION, "KeyConditionExpression": "pk = :p",
                   "FilterExpression": "size(note) = :n",
                   "ExpressionAttributeValues": {":p": {"S": "p1"}, ":n": {"N": "5"}}}),
        ),
    ] {
        let selected = ok(&router, "Query", body).await;
        assert_eq!(
            selected["Count"],
            json!(1),
            "case {case:?}: expected the stored item, got {selected}"
        );
    }
}

// ===========================================================================
// RULE 7. Every violated constraint of one request is counted in one envelope
// ===========================================================================

/// The table the aggregate rows address.
const AGGREGATED: &str = "AggregatedViolations";

/// One clause of the envelope: the value as the request sent it (`None` for a member it
/// carried none for), the member path the validating walk reports, and the requirement.
fn constraint_clause(value: Option<&str>, path: &str, requirement: &str) -> String {
    let shown = value.map_or_else(|| "null".to_string(), |value| format!("'{value}'"));
    format!("Value {shown} at '{path}' failed to satisfy constraint: {requirement}")
}

/// The whole envelope over `clauses`, counted as the service counts them.
fn envelope(clauses: &[String]) -> String {
    let plural = if clauses.len() == 1 { "error" } else { "errors" };
    format!(
        "{} validation {plural} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

#[tokio::test]
async fn every_violated_constraint_of_one_request_is_reported_in_one_envelope() {
    let router = emulator();
    create_active_table(&router, AGGREGATED, &[("pk", "HASH", "S")]).await;

    let absent_key = constraint_clause(None, "key", "Member must not be null");
    let empty_name = constraint_clause(
        Some(""),
        "tableName",
        "Member must have length greater than or equal to 1",
    );
    let capacity = constraint_clause(
        Some("BOGUS"),
        "returnConsumedCapacity",
        "Member must satisfy enum value set: [INDEXES, TOTAL, NONE]",
    );
    let return_values = constraint_clause(
        Some("BOGUS2"),
        "returnValues",
        "Member must satisfy enum value set: [ALL_NEW, UPDATED_OLD, ALL_OLD, NONE, \
         UPDATED_NEW]",
    );

    // How many constraints one request breaks, which classes they are, and the operation
    // that carries them: an absent member is found while the request is READ, which is
    // what used to mask every bound beside it.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "one absent member alone",
            "UpdateItem",
            json!({"TableName": AGGREGATED}),
            envelope(std::slice::from_ref(&absent_key)),
        ),
        (
            "an absent member beside one enumerated member outside its set",
            "UpdateItem",
            json!({"TableName": AGGREGATED, "ReturnConsumedCapacity": "BOGUS"}),
            envelope(&[capacity.clone(), absent_key.clone()]),
        ),
        (
            "an absent member beside a member below its length bound",
            "UpdateItem",
            json!({"TableName": "", "ReturnConsumedCapacity": "BOGUS"}),
            envelope(&[capacity.clone(), empty_name.clone(), absent_key.clone()]),
        ),
        (
            "four violations at once",
            "UpdateItem",
            json!({"TableName": "", "ReturnConsumedCapacity": "BOGUS",
                   "ReturnValues": "BOGUS2"}),
            envelope(&[
                capacity.clone(),
                empty_name.clone(),
                return_values.clone(),
                absent_key.clone(),
            ]),
        ),
        (
            "the same pair on another operation",
            "GetItem",
            json!({"TableName": AGGREGATED, "ReturnConsumedCapacity": "BOGUS"}),
            envelope(&[capacity.clone(), absent_key.clone()]),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }

    // Two elements of ONE list, each breaking the same bound: the clauses name the element
    // the violation sits in, and the two lists are stated in the order the service states
    // them.
    let index = |name: &str| {
        json!({"IndexName": name,
               "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                             {"AttributeName": "sk", "KeyType": "RANGE"}],
               "Projection": {"ProjectionType": "KEYS_ONLY"}})
    };
    let (status, body) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "TwoShortIndexNames",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                          {"AttributeName": "sk", "KeyType": "RANGE"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"},
                                     {"AttributeName": "sk", "AttributeType": "S"}],
            "LocalSecondaryIndexes": [index("lo")],
            "GlobalSecondaryIndexes": [index("gi")],
        }),
    )
    .await;
    let short = "Member must have length greater than or equal to 3";
    assert_eq!(
        (status, message(&body)),
        (
            400,
            envelope(&[
                constraint_clause(
                    Some("gi"),
                    "globalSecondaryIndexes.1.member.indexName",
                    short
                ),
                constraint_clause(
                    Some("lo"),
                    "localSecondaryIndexes.1.member.indexName",
                    short
                ),
            ])
        ),
        "case \"two short index names\": expected both clauses counted together, got {status} \
         with {body}"
    );

    // The CONTROL: a request whose only defect is a member the service checks ITSELF is
    // refused in that check's own words, because such a check is reached only by a request
    // whose shape the validator already admitted.
    let (status, body) = call(
        &router,
        "UpdateItem",
        json!({"TableName": AGGREGATED, "Key": {"pk": {"S": "k1"}},
               "ReturnValues": "BOGUS2"}),
    )
    .await;
    assert_eq!(
        (status, message(&body)),
        (
            400,
            "1 validation error detected: Failed to satisfy constraint: Member must satisfy \
             enum value set: [ALL_OLD, UPDATED_OLD, ALL_NEW, UPDATE_NEW, NONE]"
                .to_string()
        ),
        "case \"one self-checked member alone\": expected that check's own sentence, got \
         {status} with {body}"
    );
}

// ===========================================================================
// RULE 8. A carried value is bounded before the expressions are
// ===========================================================================

/// The table the precedence rows write to.
const PRECEDENCE: &str = "NestingPrecedence";

/// The documented maximum depth a value may nest to: "DynamoDB supports nested attributes
/// up to 32 levels deep" ("Service, Account, and Table Quotas").
const MAX_DEPTH: usize = 32;

/// A value nested `levels` deep, which is `levels - 1` maps around a string.
fn nested(levels: usize) -> Value {
    let mut value = json!({"S": "leaf"});
    for _ in 1..levels {
        value = json!({"M": {"inner": value}});
    }
    value
}

#[tokio::test]
async fn a_value_past_the_nesting_limit_is_reported_before_the_expression_tables() {
    let router = emulator();
    create_active_table(&router, PRECEDENCE, &[("pk", "HASH", "S")]).await;

    let depth = "1 validation error detected: Nesting Levels have exceeded supported limits: \
                 Attributes in the item have nested levels beyond supported limit";
    let deep = nested(MAX_DEPTH + 8);

    // What the deep value is stated beside: an entry no expression uses (which the depth
    // outranks), and the two defects of the substitution TABLE itself (which outrank the
    // depth, because a table that is no substitution table at all is the defect whatever
    // the values hold).
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a deep value alone",
            json!({"TableName": PRECEDENCE, "Key": {"pk": {"S": "k1"}},
                   "UpdateExpression": "SET doc = :v",
                   "ExpressionAttributeValues": {":v": deep.clone()}}),
            depth.to_string(),
        ),
        (
            "a deep value beside an entry no expression uses",
            json!({"TableName": PRECEDENCE, "Key": {"pk": {"S": "k1"}},
                   "UpdateExpression": "SET doc = :v",
                   "ExpressionAttributeValues": {":v": deep.clone(),
                                                 ":unused": {"S": "spare"}}}),
            depth.to_string(),
        ),
        (
            "a deep value beside a names entry that names no attribute",
            json!({"TableName": PRECEDENCE, "Key": {"pk": {"S": "k1"}},
                   "UpdateExpression": "SET doc = :v",
                   "ExpressionAttributeNames": {"#e": ""},
                   "ExpressionAttributeValues": {":v": deep.clone()}}),
            "1 validation error detected: ExpressionAttributeNames contains invalid value: \
             Empty attribute name for key #e"
                .to_string(),
        ),
        (
            "a deep value beside a key no expression could write",
            json!({"TableName": PRECEDENCE, "Key": {"pk": {"S": "k1"}},
                   "UpdateExpression": "SET doc = :v",
                   "ExpressionAttributeNames": {"bad": "doc"},
                   "ExpressionAttributeValues": {":v": deep.clone()}}),
            "1 validation error detected: ExpressionAttributeNames contains invalid key: \
             Syntax error; key: \"bad\""
                .to_string(),
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = call(&router, "UpdateItem", request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {body}"
        );
        // Either way nothing is written.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": PRECEDENCE, "Key": {"pk": {"S": "k1"}}}),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            None,
            "case {case:?}: a refused update writes nothing, got {read}"
        );
    }

    // The other side: a SHALLOW document is what lets the unused entry surface.
    let (status, body) = call(
        &router,
        "UpdateItem",
        json!({"TableName": PRECEDENCE, "Key": {"pk": {"S": "k1"}},
               "UpdateExpression": "SET doc = :v",
               "ExpressionAttributeValues": {":v": nested(2), ":unused": {"S": "spare"}}}),
    )
    .await;
    assert_eq!(
        (status, message(&body)),
        (
            400,
            "1 validation error detected: Value provided in ExpressionAttributeValues unused \
             in expressions: keys: {:unused}"
                .to_string()
        ),
        "case \"a shallow value beside an unused entry\": expected the unused-entry sentence, \
         got {status} with {body}"
    );
}

// ===========================================================================
// RULE 9. A transaction past the aggregate ceiling names the payload it carried
// ===========================================================================

/// The table the transaction rows write to.
const TRANSACTED: &str = "TransactionPayload";

/// What one of those `Put` items stores: the key attribute's name and value, plus the
/// payload attribute's name and value — the documented item size, "the sum of the attribute
/// name and value lengths".
fn put_item_size(key: &str, payload: usize) -> usize {
    "pk".len() + key.len() + "p".len() + payload
}

#[tokio::test]
async fn a_transaction_past_four_megabytes_names_the_payload_it_carried() {
    let router = emulator();
    create_active_table(&router, TRANSACTED, &[("pk", "HASH", "S")]).await;

    // Eleven items, each inside the 400 KB item ceiling and together past the 4 MB
    // aggregate.
    let payload = 390 * 1024;
    let text = "x".repeat(payload);
    let keys: Vec<String> = (0..11).map(|at| format!("k{at:02}")).collect();
    let puts: Vec<Value> = keys
        .iter()
        .map(|key| {
            json!({"Put": {"TableName": TRANSACTED,
                           "Item": {"pk": {"S": key}, "p": {"S": text}}}})
        })
        .collect();
    let carried: usize = keys
        .iter()
        .map(|key| put_item_size(key, payload))
        .sum();

    let (status, body) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": puts}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), message(&body)),
        (
            400,
            "ValidationException",
            format!("Transaction payload size cannot exceed 4MB. Payload Size: {carried}")
        ),
        "case \"eleven items past the aggregate\": expected the payload size named, got \
         {status} with {body}"
    );
    assert_eq!(
        body.get("CancellationReasons"),
        None,
        "case \"eleven items past the aggregate\": no action of it was applicable, so there is \
         nothing to cancel, got {body}"
    );
    for key in &keys {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TRANSACTED, "Key": {"pk": {"S": key}}}),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            None,
            "case \"eleven items past the aggregate\": {key} must not have been written, got \
             {read}"
        );
    }

    // The other side of the ceiling: a subset of the SAME items commits.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [
            {"Put": {"TableName": TRANSACTED,
                     "Item": {"pk": {"S": keys[0]}, "p": {"S": text}}}},
            {"Put": {"TableName": TRANSACTED,
                     "Item": {"pk": {"S": keys[1]}, "p": {"S": text}}}},
        ]}),
    )
    .await;
    for key in &keys[..2] {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TRANSACTED, "Key": {"pk": {"S": key}}}),
        )
        .await;
        assert_eq!(
            read["Item"]["p"]["S"].as_str().map(str::len),
            Some(payload),
            "case \"a subset inside the aggregate\": {key} must hold the whole payload, got a \
             different length"
        );
    }
}

// ===========================================================================
// RULE 10. An update's size accounting charges each clause's own frame
// ===========================================================================

/// The table the boundary rows write to: the rows address a one-character partition key,
/// so the item's own overhead is as small as it can be.
const SIZED: &str = "ClauseFrames";

/// One row of the boundary table: the update's clauses, the values they bind, the stored
/// state the row needs, and how far the largest accepted operand sits BELOW the plain
/// `SET v = :v` baseline — the frames the clauses carry plus the bytes they leave in the
/// item.
struct Frame {
    case: &'static str,
    expression: &'static str,
    values: fn() -> Value,
    seed: Option<fn() -> Value>,
    below: usize,
}

/// The alphabet the probes draw their partition keys from, each key ONE character long so
/// every probe's item carries the same key bytes.
///
/// A probe gets a key of its own: an item this size is 400 of the write units one partition
/// may take in a second, and a boundary is read off a couple of dozen probes — so writing
/// them all to one key would measure the throughput ceiling instead of the size one.
const PROBE_KEYS: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

/// The probes of one boundary measurement.
struct Prober<'r> {
    router: &'r smithy_server_runtime::MultiServiceRouter,
    taken: usize,
}

impl Prober<'_> {
    /// A key no earlier probe wrote to.
    fn key(&mut self) -> String {
        let at = self.taken;
        self.taken += 1;
        PROBE_KEYS
            .chars()
            .nth(at)
            .expect("a boundary is read off fewer probes than this alphabet has keys")
            .to_string()
    }

    /// One probe: `expression` applied to a fresh key with an operand of `length` bytes,
    /// over the stored state `seed` leaves there. The key it used comes back, so a caller
    /// can read the item the probe left.
    async fn probe(
        &mut self,
        seed: Option<fn() -> Value>,
        expression: &str,
        values: &Value,
        length: usize,
    ) -> (String, u16, Value) {
        let key = self.key();
        if let Some(seed) = seed {
            let mut seeding = seed();
            seeding["Key"] = json!({"pk": {"S": key.clone()}});
            ok(self.router, "UpdateItem", seeding).await;
        }
        let mut bound = values.clone();
        bound[":v"] = json!({"S": "x".repeat(length)});
        let (status, body) = call(
            self.router,
            "UpdateItem",
            json!({"TableName": SIZED, "Key": {"pk": {"S": key.clone()}},
                   "UpdateExpression": expression,
                   "ExpressionAttributeValues": bound}),
        )
        .await;
        (key, status, body)
    }

    /// The largest operand `expression` accepts, bisected inside a bracket every clause
    /// shape below falls in.
    async fn largest_accepted(&mut self, expression: &str, values: &Value) -> usize {
        let (mut low, mut high) = (409_400usize, 409_650usize);
        while low < high {
            let at = low + (high - low).div_ceil(2);
            let (_, status, _) = self.probe(None, expression, values, at).await;
            if status == 200 {
                low = at;
            } else {
                high = at - 1;
            }
        }
        low
    }
}

#[tokio::test]
async fn the_largest_accepted_operand_moves_with_the_clauses_stated_beside_it() {
    let router = emulator();
    create_active_table(&router, SIZED, &[("pk", "HASH", "S")]).await;
    let mut prober = Prober {
        router: &router,
        taken: 0,
    };

    // The baseline every frame is counted from: one assignment, nothing beside it.
    let plain = json!({});
    let baseline = prober.largest_accepted("SET v = :v", &plain).await;
    assert!(
        (409_400..409_650).contains(&baseline),
        "case \"the baseline\": expected a boundary inside the bracketed range, got {baseline}"
    );

    // The boundary is a BOUND: the operand at it is stored, one byte more is refused with
    // the sentence the update path states for that quota, and the attribute is left at the
    // length last accepted.
    let (key, status, body) = prober.probe(None, "SET v = :v", &plain, baseline).await;
    assert_eq!(
        status, 200,
        "case \"the baseline\": the measured boundary must be accepted, got {body}"
    );
    let (status, body) = call(
        &router,
        "UpdateItem",
        json!({"TableName": SIZED, "Key": {"pk": {"S": key}},
               "UpdateExpression": "SET v = :v",
               "ExpressionAttributeValues": {":v": {"S": "x".repeat(baseline + 1)}}}),
    )
    .await;
    assert_eq!(
        (status, message(&body).as_str()),
        (
            400,
            "Item size to update has exceeded the maximum allowed size"
        ),
        "case \"one byte past the baseline\": expected the update-path ceiling sentence, got \
         {status} with {body}"
    );
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": SIZED, "Key": {"pk": {"S": key}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["v"]["S"].as_str().map(str::len),
        Some(baseline),
        "case \"one byte past the baseline\": the attribute keeps its last accepted length, \
         got {read}"
    );

    // One row per clause KIND. `below` is the frame that clause carries plus the bytes it
    // leaves in the item: a one-character attribute name (1 byte) and its value — a
    // one-character string (1 byte) or a one-digit number (2 bytes, "1 byte per two
    // significant digits" rounded up "plus 1 byte").
    let rows = [
        Frame {
            case: "a second assignment",
            expression: "SET v = :v, b = :s",
            values: || json!({":s": {"S": "x"}}),
            seed: None,
            below: 19 + 2,
        },
        Frame {
            case: "an ADD beside the assignment",
            expression: "SET v = :v ADD b :one",
            values: || json!({":one": {"N": "1"}}),
            seed: None,
            below: 19 + 3,
        },
        Frame {
            case: "an assignment whose operand is arithmetic",
            expression: "SET v = :v, b = :one + :zero",
            values: || json!({":one": {"N": "1"}, ":zero": {"N": "0"}}),
            seed: None,
            below: 69 + 3,
        },
        Frame {
            case: "an assignment whose operand is a function call",
            expression: "SET v = :v, b = if_not_exists(b, :one)",
            values: || json!({":one": {"N": "1"}}),
            seed: None,
            below: 74 + 3,
        },
        Frame {
            case: "a REMOVE of a one-character path",
            expression: "SET v = :v REMOVE b",
            values: || json!({}),
            seed: None,
            below: 1 + 1,
        },
        Frame {
            case: "a REMOVE of a three-character path",
            expression: "SET v = :v REMOVE bcd",
            values: || json!({}),
            seed: None,
            below: 1 + 3,
        },
        Frame {
            // A `DELETE` carries no frame of its own: what moves the boundary beside it is
            // the set the item is left holding — one name byte and the one member it keeps.
            case: "a DELETE beside the assignment",
            expression: "SET v = :v DELETE s :gone",
            values: || json!({":gone": {"SS": ["b"]}}),
            seed: Some(|| {
                json!({"TableName": SIZED, "UpdateExpression": "SET s = :seed",
                       "ExpressionAttributeValues": {":seed": {"SS": ["a", "b"]}}})
            }),
            below: 2,
        },
    ];

    for row in rows {
        let values = (row.values)();
        let at = baseline - row.below;
        let (_, status, body) = prober.probe(row.seed, row.expression, &values, at).await;
        assert_eq!(
            status,
            200,
            "case {:?}: an operand {} bytes under the baseline {baseline} must be accepted, \
             got {status} with {body}",
            row.case,
            row.below
        );
        let (_, status, body) = prober
            .probe(row.seed, row.expression, &values, at + 1)
            .await;
        assert_eq!(
            (status, message(&body).as_str()),
            (
                400,
                "Item size to update has exceeded the maximum allowed size"
            ),
            "case {:?}: one byte more must be refused, so the boundary sits exactly {} bytes \
             under the baseline {baseline}, got {status} with {body}",
            row.case,
            row.below
        );
    }
}

// ===========================================================================
// RULE 11. A value that could not be READ is a different class from one that
//          was read and then broke a rule
// ===========================================================================

/// The error class a body this service could not read into the modelled input is refused
/// with, as the wire spells it: the absolute id of the protocol's own error shape, which is
/// what an SDK branches a non-retryable marshalling failure on.
const SERIALIZATION_EXCEPTION: &str = "com.amazon.coral.service#SerializationException";

/// What the refusal states where the modelled type of a FIELD the body names holds other
/// values (a list, a map, a structure) and the body carried a single value there.
const UNEXPECTED_FIELD_TYPE: &str = "Unexpected field type";

/// What it states where the body opened a structure or a map in a slot read from neither.
const STRUCTURE_OR_MAP: &str = "Start of structure or map found where not expected";

/// What the same defect states where the value stands INSIDE a collection instead — an
/// element of a list or the value of a map entry — which no modelled member names.
const UNEXPECTED_VALUE_TYPE: &str = "Unexpected value type in payload";

/// The error class a value that WAS read and then broke a rule the model declares is refused
/// with, as the wire spells it: the absolute id of the shape the validating framework owns,
/// which is the other side of this rule's boundary.
const VALIDATION_EXCEPTION: &str = "com.amazon.coral.validate#ValidationException";

/// The one assertion both halves of the class boundary are read through: the status, the
/// error class as the wire spells it, and which body member carries the prose.
///
/// A serialization refusal states its sentence under the capitalized `Message` and carries
/// no lowercase `message`; a constraint refusal is the other way round, which is what a tool
/// extracting an error reason off one slot or the other depends on. `sentence` is `None` for
/// the refusal that states NO prose at all — the bare carrier a value no conversion was
/// attempted for earns.
fn refused_as(case: &str, status: u16, body: &Value, class: &str, sentence: Option<&str>) {
    let answered = body.get("__type").and_then(Value::as_str).unwrap_or_default();
    assert_eq!(
        (status, answered),
        (400, class),
        "case {case:?}: expected 400 {class}, got {status} with {body}"
    );
    let capital = body.get("Message").and_then(Value::as_str);
    let lower = body.get("message").and_then(Value::as_str);
    let (stated, empty, slot) = if class == SERIALIZATION_EXCEPTION {
        (capital, lower, "Message")
    } else {
        (lower, capital, "message")
    };
    assert_eq!(
        stated, sentence,
        "case {case:?}: expected ${slot} {sentence:?}, got {body}"
    );
    assert_eq!(
        empty, None,
        "case {case:?}: this class states its prose under ${slot} alone, got {body}"
    );
}

#[tokio::test]
async fn a_value_that_could_not_be_read_is_refused_as_a_serialization_failure() {
    let router = emulator();

    // The ARN rows address a real table, so nothing about the class boundary depends on
    // whether the resource exists — and a refusal that never looked it up cannot tell.
    create_active_table(&router, "Tagged", &[("pk", "HASH", "S")]).await;
    let tagged = describe_when_active(&router, "an ARN to address", "Tagged").await["TableArn"]
        .as_str()
        .expect("the table reports its ARN")
        .to_string();

    // (case, operation, the body, the sentence the refusal states)
    //
    // The modelled type of each member named here comes from
    // `smithy_query(query="operation", operation="<the operation>")`: `TableName` targets
    // `TableArn` (string) on every table operation, `ExclusiveStartTableName` targets
    // `TableName` (string), `Limit` targets `ListTablesInputLimit` (integer),
    // `ResourceArn` targets `ResourceArnString` (string), `Tags` targets `TagList` (list),
    // `TagKeys` targets `TagKeyList` (list), `Key`/`ExpressionAttributeNames` are maps,
    // `KeySchema` is a list, `ProvisionedThroughput` a structure and `ConsistentRead` a
    // boolean.
    let rows: Vec<(&str, &str, Value, Option<String>)> = vec![
        (
            "a number where a table name is a string, on the create",
            "CreateTable",
            json!({"TableName": 42,
                   "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}]}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a number where a table name is a string, on the update",
            "UpdateTable",
            json!({"TableName": 7}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a number where a start key is a string, on the list",
            "ListTables",
            json!({"ExclusiveStartTableName": 1}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a number where a resource ARN is a string, on the tag read",
            "ListTagsOfResource",
            json!({"ResourceArn": 1}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a number where a resource ARN is a string, on the tag write",
            "TagResource",
            json!({"ResourceArn": 1, "Tags": [{"Key": "k", "Value": "v"}]}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a fraction where a table name is a string",
            "UpdateTable",
            json!({"TableName": 2.75}),
            Some("DECIMAL_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a true flag where a table name is a string",
            "DescribeTable",
            json!({"TableName": true}),
            Some("TRUE_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a false flag where a table name is a string",
            "DescribeTable",
            json!({"TableName": false}),
            Some("FALSE_VALUE cannot be converted to String".to_string()),
        ),
        (
            "text where a page size is an integer",
            "ListTables",
            json!({"Limit": "10"}),
            Some("STRING_VALUE cannot be converted to Integer".to_string()),
        ),
        (
            // Text spelling no flag at all: text that DOES spell one is read as the flag it
            // spells, which is the other side of this boundary.
            "text spelling no flag where a read mode is a boolean",
            "Scan",
            json!({"TableName": "Tagged", "ConsistentRead": "maybe"}),
            Some("STRING_VALUE cannot be converted to Boolean".to_string()),
        ),
        (
            "text where a tag set is a list",
            "TagResource",
            json!({"ResourceArn": tagged.clone(), "Tags": "k=v"}),
            Some(UNEXPECTED_FIELD_TYPE.to_string()),
        ),
        (
            "a number where a tag set is a list",
            "TagResource",
            json!({"ResourceArn": tagged.clone(), "Tags": 1}),
            Some(UNEXPECTED_FIELD_TYPE.to_string()),
        ),
        (
            "text where a tag key set is a list",
            "UntagResource",
            json!({"ResourceArn": tagged.clone(), "TagKeys": "k"}),
            Some(UNEXPECTED_FIELD_TYPE.to_string()),
        ),
        (
            "a flag where a key schema is a list",
            "CreateTable",
            json!({"TableName": "Later", "KeySchema": false}),
            Some(UNEXPECTED_FIELD_TYPE.to_string()),
        ),
        (
            "a number where a throughput is a structure",
            "CreateTable",
            json!({"TableName": "Later",
                   "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                   "ProvisionedThroughput": 5}),
            Some(UNEXPECTED_FIELD_TYPE.to_string()),
        ),
        (
            "text where an item key is a map",
            "GetItem",
            json!({"TableName": "Tagged", "Key": "pk"}),
            Some(UNEXPECTED_FIELD_TYPE.to_string()),
        ),
        (
            // A COLLECTION where the member is read from none: no conversion was attempted at
            // all, so an array earns the error class alone and an object the structural
            // sentence.
            "a list where a table name is a string",
            "DeleteTable",
            json!({"TableName": ["Tagged"]}),
            None,
        ),
        (
            "a list where the batched tables are a map",
            "BatchGetItem",
            json!({"RequestItems": ["Tagged"]}),
            None,
        ),
        (
            "an object where a table name is a string",
            "DescribeTable",
            json!({"TableName": {"Name": "Tagged"}}),
            Some(STRUCTURE_OR_MAP.to_string()),
        ),
        (
            "an object where a tag set is a list",
            "TagResource",
            json!({"ResourceArn": tagged.clone(), "Tags": {"k": "v"}}),
            Some(STRUCTURE_OR_MAP.to_string()),
        ),
        (
            // Nested inside a structure: the codec's own descent reaches it, so the depth
            // of the value makes no difference to the class of the refusal.
            "text where a nested capacity is a long",
            "CreateTable",
            json!({"TableName": "Later",
                   "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                   "ProvisionedThroughput": {"ReadCapacityUnits": "5",
                                             "WriteCapacityUnits": 5}}),
            Some("STRING_VALUE cannot be converted to Long".to_string()),
        ),
        (
            "a number where a nested key attribute name is a string",
            "CreateTable",
            json!({"TableName": "Later",
                   "KeySchema": [{"AttributeName": 1, "KeyType": "HASH"}],
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}]}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a number where a tag's key is a string, inside the list",
            "TagResource",
            json!({"ResourceArn": tagged.clone(), "Tags": [{"Key": 1, "Value": "v"}]}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            // INSIDE the list, where no member names the value: the same defect, stated as
            // the payload value it is rather than as a field.
            "a scalar where a tag set's element is a structure",
            "TagResource",
            json!({"ResourceArn": tagged.clone(), "Tags": ["k=v"]}),
            Some(UNEXPECTED_VALUE_TYPE.to_string()),
        ),
        (
            "a number where a tag key set's element is a string",
            "UntagResource",
            json!({"ResourceArn": tagged.clone(), "TagKeys": [1]}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
        (
            "a number where an attribute's substituted name is a string, inside the map",
            "GetItem",
            json!({"TableName": "Tagged", "Key": {"pk": {"S": "a"}},
                   "ProjectionExpression": "#p", "ExpressionAttributeNames": {"#p": 1}}),
            Some("NUMBER_VALUE cannot be converted to String".to_string()),
        ),
    ];

    for (case, operation, body, sentence) in rows {
        let (status, answered) = call(&router, operation, body).await;
        refused_as(
            case,
            status,
            &answered,
            SERIALIZATION_EXCEPTION,
            sentence.as_deref(),
        );
    }

    // Nothing of a refused request was read, so nothing of it was applied: the creates
    // above addressed no table that exists afterwards, and the tag write that named a real
    // resource left its tag set alone.
    let listed = ok(&router, "ListTables", json!({})).await;
    assert_eq!(
        listed["TableNames"],
        json!(["Tagged"]),
        "case \"after the refusals\": expected only the table the suite made, got {listed}"
    );
    let tags = ok(
        &router,
        "ListTagsOfResource",
        json!({"ResourceArn": tagged.clone()}),
    )
    .await;
    assert_eq!(
        tags["Tags"].as_array().map(Vec::len).unwrap_or(0),
        0,
        "case \"after the refusals\": a refused tag write applied nothing, got {tags}"
    );

    // The CONTROL, on the other side of the boundary: every one of these values READS into
    // its member, so what refuses them is the rule it then breaks — the constraint class,
    // with its prose in the lowercase slot — and the ones that break no rule are served.
    for (case, operation, body, sentence) in [
        (
            "a page size below its range",
            "ListTables",
            json!({"Limit": 0}),
            "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1",
        ),
        (
            "a start key shorter than a table name may be",
            "ListTables",
            json!({"ExclusiveStartTableName": "ab"}),
            "1 validation error detected: Value 'ab' at 'exclusiveStartTableName' failed to \
             satisfy constraint: Member must have length greater than or equal to 3",
        ),
    ] {
        let (status, answered) = call(&router, operation, body).await;
        refused_as(case, status, &answered, VALIDATION_EXCEPTION, Some(sentence));
    }
    for (case, operation, body) in [
        ("a page size inside its range", "ListTables", json!({"Limit": 10})),
        (
            // Text spelling a flag IS read as that flag, so the read mode it states is the
            // one the request is served with.
            "text spelling a flag where a read mode is a boolean",
            "Scan",
            json!({"TableName": "Tagged", "ConsistentRead": "true"}),
        ),
        (
            "a tag set that is a list of tags",
            "TagResource",
            json!({"ResourceArn": tagged, "Tags": [{"Key": "k", "Value": "v"}]}),
        ),
    ] {
        let (status, answered) = call(&router, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the request to be served, got {status} with {answered}"
        );
    }
}

// ===========================================================================
// RULE 12. The item ceiling ITSELF is where an update's boundary sits
// ===========================================================================

/// The table the ceiling rows write to.
const CEILED: &str = "ItemCeiling";

/// The documented item ceiling: "The maximum item size in DynamoDB is 400 KB" ("Service,
/// Account, and Table Quotas"), which is 409 600 bytes.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// The name of the partition key the rows address their items by.
const CEILED_KEY: &str = "pk";

/// The least the part of an item an update does NOT address is charged against the item
/// ceiling when the change is stated as an expression, as the service states it
/// (`state::UNTOUCHED_PORTION_FLOOR_BYTES`) — measured behavior, so an untouched part shorter
/// than this buys no headroom there, while a change stating no expression is charged the bytes
/// the item actually holds.
const UNTOUCHED_PORTION_FLOOR: usize = 22;

/// One row of the ceiling table: which key it writes to, the attribute the update writes,
/// an attribute the item already holds before the update, and whether the update travels in
/// the legacy `AttributeUpdates` map rather than in an `UpdateExpression`.
struct Ceiling {
    case: &'static str,
    key: &'static str,
    attribute: String,
    /// `(name, value)` of an attribute a `PutItem` seeds the key's item with first, so the
    /// row sizes an update of an item that is already there.
    seeded: Option<(String, String)>,
    legacy: bool,
}

impl Ceiling {
    /// The bytes the stored item is CHARGED besides the operand this row's update writes: the
    /// part of the item the change does not address — the partition key's name and value and
    /// anything the item already holds — charged its own bytes, raised to
    /// [`UNTOUCHED_PORTION_FLOOR`] where the update is stated as an expression and it does not
    /// fill it, plus the written attribute's name. The sum "of the lengths of its attribute
    /// names and values" ("Item sizes and formats") that the operand has to complete.
    fn charged_besides_the_operand(&self) -> usize {
        let seeded = self
            .seeded
            .as_ref()
            .map_or(0, |(name, value)| name.len() + value.len());
        let untouched = CEILED_KEY.len() + self.key.len() + seeded;
        let untouched_charge = if self.legacy {
            untouched
        } else {
            untouched.max(UNTOUCHED_PORTION_FLOOR)
        };
        untouched_charge + self.attribute.len()
    }

    /// The `UpdateItem` body that writes an operand of `operand` bytes, in whichever member
    /// this row states its update through.
    fn update(&self, operand: usize) -> Value {
        let mut body = json!({
            "TableName": CEILED,
            "Key": {CEILED_KEY: {"S": self.key}},
        });
        let value = json!({"S": "x".repeat(operand)});
        if self.legacy {
            body["AttributeUpdates"] =
                json!({self.attribute.clone(): {"Action": "PUT", "Value": value}});
        } else {
            body["UpdateExpression"] = json!("SET #a = :v");
            body["ExpressionAttributeNames"] = json!({"#a": self.attribute.clone()});
            body["ExpressionAttributeValues"] = json!({":v": value});
        }
        body
    }
}

/// RULE: a single-clause `UpdateItem` is sized against the documented item ceiling over what
/// the item it leaves behind is CHARGED — the bytes of the attributes the change addresses plus
/// the part of the item it leaves alone, which is charged at least [`UNTOUCHED_PORTION_FLOOR`]
/// where the change is stated as an expression and its own bytes where it is stated through the
/// legacy map — and nothing
/// else, and the ceiling is a BOUND: the item charged exactly the ceiling is stored and one
/// byte more is refused with the update path's own sentence for that quota. The clause every
/// update carries is the baseline the per-clause frames of RULE 10 are differences from, so it
/// costs the operand nothing.
///
/// The rows differ along what the rule generalises over: the member the update travels in
/// (`UpdateExpression` and the legacy `AttributeUpdates`), the length of the attribute name
/// the bytes are written under, and whether the key already holds an item whose other
/// attributes count towards the same ceiling.
#[tokio::test]
async fn an_item_exactly_at_the_documented_ceiling_is_stored_and_one_byte_more_is_refused() {
    let router = emulator();
    create_active_table(&router, CEILED, &[(CEILED_KEY, "HASH", "S")]).await;

    let rows = [
        Ceiling {
            case: "an UpdateExpression writing a one-character attribute name",
            key: "a",
            attribute: "v".to_string(),
            seeded: None,
            legacy: false,
        },
        Ceiling {
            case: "an UpdateExpression writing a 32-character attribute name",
            key: "b",
            attribute: "n".repeat(32),
            seeded: None,
            legacy: false,
        },
        Ceiling {
            case: "an UpdateExpression over an item the key already holds",
            key: "c",
            attribute: "v".to_string(),
            seeded: Some(("w".to_string(), "y".repeat(1024))),
            legacy: false,
        },
        Ceiling {
            case: "an AttributeUpdates PUT of a one-character attribute name",
            key: "d",
            attribute: "v".to_string(),
            seeded: None,
            legacy: true,
        },
        Ceiling {
            case: "an AttributeUpdates PUT over an item the key already holds",
            key: "e",
            attribute: "v".to_string(),
            seeded: Some(("w".to_string(), "y".repeat(4096))),
            legacy: true,
        },
        Ceiling {
            case: "an AttributeUpdates PUT of a 255-character attribute name",
            key: "f",
            attribute: "n".repeat(255),
            seeded: None,
            legacy: true,
        },
    ];

    for row in rows {
        if let Some((name, value)) = row.seeded.as_ref() {
            ok(
                &router,
                "PutItem",
                json!({"TableName": CEILED,
                       "Item": {CEILED_KEY: {"S": row.key}, name.clone(): {"S": value}}}),
            )
            .await;
        }
        let at_the_ceiling = MAX_ITEM_BYTES - row.charged_besides_the_operand();

        let (status, body) = call(&router, "UpdateItem", row.update(at_the_ceiling)).await;
        assert_eq!(
            status,
            200,
            "case {:?}: an item charged exactly {MAX_ITEM_BYTES} bytes is stored, got {status} \
             with {body}",
            row.case
        );
        let (status, body) = call(&router, "UpdateItem", row.update(at_the_ceiling + 1)).await;
        assert_eq!(
            (status, message(&body).as_str()),
            (
                400,
                "Item size to update has exceeded the maximum allowed size"
            ),
            "case {:?}: one byte past the ceiling is refused with the update path's sentence, \
             got {status} with {body}",
            row.case
        );

        // The refused update left the item as the accepted one stored it, which is what
        // makes the boundary a bound rather than a rounding of it.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": CEILED, "Key": {CEILED_KEY: {"S": row.key}}}),
        )
        .await;
        assert_eq!(
            read["Item"][row.attribute.as_str()]["S"].as_str().map(str::len),
            Some(at_the_ceiling),
            "case {:?}: the attribute keeps the length the ceiling admitted, got {read}",
            row.case
        );
    }
}

