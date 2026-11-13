// SPDX-License-Identifier: CC0-1.0

//! The refusal a request earns before any handler runs, end to end over the wire.
//!
//! The rule under test: a request the modelled-constraint walk refuses is answered HTTP 400
//! `ValidationException` whose `Message` member carries AWS's constraint-violation sentence —
//! `<N> validation error(s) detected: ` over one clause per failing (member, requirement),
//! joined with `; `, each clause naming the value as SUBMITTED, the member's lowerCamel
//! dotted path and the requirement it failed. Two renderings are part of that sentence:
//!
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors a model
//!   writes to turn a find into a whole-value match;
//! * a constraint broken by a LIST's elements is stated once, at the list member, quoting the
//!   whole list and nesting the element shape's declared constraint set inside
//!   `Member must satisfy constraint: [ … ]`.
//!
//! And the text is carried in `Message`: every error shape this model declares that carries
//! prose names that member `Message` and gives it no `@jsonName`, so on `awsJson1_0` that is
//! where it goes, whether a handler or the framework produced the body.
//!
//! Each test is a TABLE spanning the class the rule generalises over — the constraint class,
//! the member, the operation (an implemented one and a still-scaffolded one), the number of
//! broken constraints, the number of offending list elements — rather than only the operations
//! and values one report happened to cite. Every call goes through the real
//! `MultiServiceRouter` (`POST /` with `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`,
//! awsJson1_0), through the same harness `key_lifecycle` uses, so what these tests assert is
//! what a signed SDK request observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::key_lifecycle::{call, error_code, ok_call, router};

// ---------------------------------------------------------------------------------------
// The model facts the sentences below are built from. Source for every one of them:
// `smithy_query(query="shape",shape="AliasName,Regions,Region,MaxResults,NextToken,TagKeys,\
// TagKey,KeyArnOrKeyAliasType,ResourceArn",model_path="services/\
// service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/smithy-model/\
// com-amazonaws-paymentcryptography-paymentcryptographycontrolplane.json")`.
// ---------------------------------------------------------------------------------------

/// `AliasName` is modelled `@pattern "^alias/[a-zA-Z0-9/_-]+$"`; this is the expression a
/// refusal quotes — the model's text without its whole-value anchors.
const ALIAS_NAME_EXPRESSION: &str = "alias/[a-zA-Z0-9/_-]+";
/// `AliasName` is modelled `@length(min: 7, max: 256)`.
const ALIAS_NAME_MAX_LENGTH: usize = 256;
/// The element shape of `Regions` is `Region`, modelled
/// `@pattern "^[a-z]{2}-[a-z]{1,16}-[0-9]+$"`, quoted unanchored.
const REGION_EXPRESSION: &str = "[a-z]{2}-[a-z]{1,16}-[0-9]+";
/// `MaxResults` is modelled `@range(min: 1, max: 100)`.
const MAX_RESULTS_MIN: i64 = 1;
/// The other published bound of `MaxResults`.
const MAX_RESULTS_MAX: i64 = 100;
/// `NextToken` is modelled `@length(min: 1, max: 8192)`.
const NEXT_TOKEN_MIN_LENGTH: usize = 1;
/// The element shape of `TagKeys` is `TagKey`, modelled `@length(min: 1, max: 128)`.
const TAG_KEY_MIN_LENGTH: usize = 1;
/// The other published bound of `TagKey`.
const TAG_KEY_MAX_LENGTH: usize = 128;
/// `ResourceArn` is modelled
/// `@pattern "^arn:aws:payment-cryptography:[a-z]{2}-[a-z]{1,16}-[0-9]+:[0-9]{12}:key/[0-9a-zA-Z]{16,64}$"`,
/// quoted unanchored.
const RESOURCE_ARN_EXPRESSION: &str =
    "arn:aws:payment-cryptography:[a-z]{2}-[a-z]{1,16}-[0-9]+:[0-9]{12}:key/[0-9a-zA-Z]{16,64}";

/// The fixed prefix of an ARN in the one form `ResourceArn` admits, in the region every
/// request below signs for; what follows is the 16..64 alphanumeric key id.
const KEY_ARN_PREFIX: &str = "arn:aws:payment-cryptography:us-west-2:000000000000:key/";

/// An ARN the modelled `ResourceArn` admits — well formed, naming no key, which is all these
/// tests need: a boundary refusal is answered before any handler looks a key up.
fn well_formed_key_arn() -> String {
    format!("{KEY_ARN_PREFIX}abcdef12345678901234")
}

/// A key identifier the modelled `KeyArnOrKeyAliasType` admits in its bare-alias form.
const WELL_FORMED_KEY_IDENTIFIER: &str = "alias/payments";

// ---------------------------------------------------------------------------------------
// The sentence, assembled the way AWS assembles it.
// ---------------------------------------------------------------------------------------

/// AWS's envelope over the clauses of one refusal.
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The clause for a value the request CARRIED: the value, the member path, the requirement.
fn value_clause(value: &str, member: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The clause for a member the request did not carry at all.
fn absent_clause(member: &str, requirement: &str) -> String {
    format!("Value null at '{member}' failed to satisfy constraint: {requirement}")
}

/// The requirement a value outside its member's `@pattern` failed.
fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The requirement a value above its member's `@length` maximum failed.
fn max_length_requirement(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a value below its member's `@length` minimum failed.
fn min_length_requirement(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement a value above its member's `@range` maximum failed.
fn max_value_requirement(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The requirement a value below its member's `@range` minimum failed.
fn min_value_requirement(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement stated at a LIST member for a constraint its ELEMENTS broke: the element
/// shape's whole declared set, nested.
fn element_set_requirement(requirements: &[String]) -> String {
    format!("Member must satisfy constraint: [{}]", requirements.join(", "))
}

/// A list as a clause quotes it: the elements in position order, comma-separated, bracketed.
fn quoted_list(elements: &[&str]) -> String {
    format!("[{}]", elements.join(", "))
}

/// Assert `operation` refuses `body` with the one sentence `expected`, carried in `Message`.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: a broken modelled constraint must be refused with 400, got {status} \
         {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case {case}: expected ValidationException, got {parsed}"
    );
    assert_eq!(
        parsed.get("Message").and_then(Value::as_str),
        Some(expected),
        "case {case}: expected the Message {expected:?}, got {parsed}"
    );
    assert!(
        parsed.get("message").is_none(),
        "case {case}: the text must be carried in the one member this model declares for it, \
         got {parsed}"
    );
}

#[tokio::test]
async fn a_pattern_violation_quotes_the_expression_without_its_whole_value_anchors() {
    let router = router();
    // One row per (operation, member, rejected value), spanning the two string members whose
    // `@pattern` a request can break directly, an operation that is implemented and one that
    // is still a 501 stub (a refusal is the service's words either way), and values that miss
    // the expression in different ways: a missing prefix, a disallowed character, the wrong
    // case, and a name that is only a prefix.
    let rows: Vec<(&str, Value, &str, String, &str)> = vec![
        (
            "GetAlias",
            json!({"AliasName": "not-a-valid-alias-name"}),
            "aliasName",
            "not-a-valid-alias-name".to_string(),
            ALIAS_NAME_EXPRESSION,
        ),
        (
            "CreateAlias",
            json!({"AliasName": "no-alias-prefix"}),
            "aliasName",
            "no-alias-prefix".to_string(),
            ALIAS_NAME_EXPRESSION,
        ),
        (
            "UpdateAlias",
            json!({"AliasName": "alias/has a space"}),
            "aliasName",
            "alias/has a space".to_string(),
            ALIAS_NAME_EXPRESSION,
        ),
        (
            "DeleteAlias",
            json!({"AliasName": "alias/semi;colon"}),
            "aliasName",
            "alias/semi;colon".to_string(),
            ALIAS_NAME_EXPRESSION,
        ),
        (
            "GetAlias",
            json!({"AliasName": "ALIAS/upper"}),
            "aliasName",
            "ALIAS/upper".to_string(),
            ALIAS_NAME_EXPRESSION,
        ),
        (
            "ListTagsForResource",
            json!({"ResourceArn": format!("{KEY_ARN_PREFIX}{}", "!".repeat(20))}),
            "resourceArn",
            format!("{KEY_ARN_PREFIX}{}", "!".repeat(20)),
            RESOURCE_ARN_EXPRESSION,
        ),
    ];

    for (operation, body, member, value, expression) in rows {
        let case = format!("{operation} {member}={value}");
        let expected = envelope(&[value_clause(
            &value,
            member,
            &pattern_requirement(expression),
        )]);
        assert_refused(&router, &case, operation, &body, &expected).await;
    }
}

#[tokio::test]
async fn a_length_violation_states_the_bound_the_model_declares() {
    let router = router();
    // Each published `@length` bound of the two members a request can break directly, and one
    // value past it. The accepted bound is checked too: a value exactly at the maximum must
    // not be refused for its length, or the clause above would be stating a bound the service
    // does not enforce.
    let over_long_alias = format!("alias/{}", "x".repeat(ALIAS_NAME_MAX_LENGTH - 5));
    assert_eq!(
        over_long_alias.len(),
        ALIAS_NAME_MAX_LENGTH + 1,
        "the over-long row must be exactly one character past the declared maximum"
    );
    let rows: Vec<(&str, Value, &str, String, String)> = vec![
        (
            "GetAlias",
            json!({"AliasName": over_long_alias.clone()}),
            "aliasName",
            over_long_alias.clone(),
            max_length_requirement(ALIAS_NAME_MAX_LENGTH),
        ),
        (
            "CreateAlias",
            json!({"AliasName": over_long_alias.clone()}),
            "aliasName",
            over_long_alias.clone(),
            max_length_requirement(ALIAS_NAME_MAX_LENGTH),
        ),
        (
            "ListKeys",
            json!({"NextToken": ""}),
            "nextToken",
            String::new(),
            min_length_requirement(NEXT_TOKEN_MIN_LENGTH),
        ),
        (
            "ListAliases",
            json!({"NextToken": ""}),
            "nextToken",
            String::new(),
            min_length_requirement(NEXT_TOKEN_MIN_LENGTH),
        ),
    ];

    for (operation, body, member, value, requirement) in rows {
        let case = format!("{operation} {member} of {} chars", value.len());
        let expected = envelope(&[value_clause(&value, member, &requirement)]);
        assert_refused(&router, &case, operation, &body, &expected).await;
    }

    // The bound itself is admitted: an alias name exactly at the maximum reaches the handler,
    // which reports that no such alias exists.
    let at_the_bound = format!("alias/{}", "x".repeat(ALIAS_NAME_MAX_LENGTH - 6));
    assert_eq!(
        at_the_bound.len(),
        ALIAS_NAME_MAX_LENGTH,
        "the accepted row must be exactly at the declared maximum"
    );
    let (status, parsed) = call(&router, "GetAlias", &json!({"AliasName": at_the_bound})).await;
    assert_eq!(
        status, 404,
        "case an alias name at the declared maximum: it must reach the handler, got {status} \
         {parsed}"
    );
}

#[tokio::test]
async fn a_range_violation_states_the_bound_the_model_declares() {
    let router = router();
    // Both published bounds of `MaxResults`, one value past each, and the value the recorded
    // refusal carried — over both operations that declare the member.
    let refused: Vec<(&str, i64, String)> = vec![
        (
            "ListKeys",
            MAX_RESULTS_MAX + 1,
            max_value_requirement(MAX_RESULTS_MAX),
        ),
        ("ListKeys", 1000, max_value_requirement(MAX_RESULTS_MAX)),
        (
            "ListKeys",
            MAX_RESULTS_MIN - 1,
            min_value_requirement(MAX_RESULTS_MIN),
        ),
        (
            "ListAliases",
            MAX_RESULTS_MAX + 1,
            max_value_requirement(MAX_RESULTS_MAX),
        ),
        ("ListAliases", -7, min_value_requirement(MAX_RESULTS_MIN)),
    ];
    for (operation, max_results, requirement) in refused {
        let case = format!("{operation} MaxResults={max_results}");
        let expected = envelope(&[value_clause(
            &max_results.to_string(),
            "maxResults",
            &requirement,
        )]);
        assert_refused(
            &router,
            &case,
            operation,
            &json!({"MaxResults": max_results}),
            &expected,
        )
        .await;
    }

    // Each published bound is admitted, or the clauses above would be stating bounds the
    // service does not enforce.
    for (operation, max_results) in [
        ("ListKeys", MAX_RESULTS_MIN),
        ("ListKeys", MAX_RESULTS_MAX),
        ("ListAliases", MAX_RESULTS_MIN),
        ("ListAliases", MAX_RESULTS_MAX),
    ] {
        ok_call(&router, operation, &json!({"MaxResults": max_results})).await;
    }
}

#[tokio::test]
async fn a_constraint_broken_by_a_list_element_is_stated_once_at_the_list() {
    let router = router();
    let region_set = element_set_requirement(&[pattern_requirement(REGION_EXPRESSION)]);
    let tag_key_set = element_set_requirement(&[
        max_length_requirement(TAG_KEY_MAX_LENGTH),
        min_length_requirement(TAG_KEY_MIN_LENGTH),
    ]);
    let over_long_tag_key = "k".repeat(TAG_KEY_MAX_LENGTH + 1);

    // Rows differ along the operation, the list member, the element shape's constraint set,
    // the number of elements the list carried and how many of them offend: one offending
    // element of one, one of several, and SEVERAL offending elements, which is still ONE
    // validation error about one member.
    let rows: Vec<(&str, Value, &str, Vec<&str>, &String)> = vec![
        (
            "RemoveKeyReplicationRegions",
            json!({
                "KeyIdentifier": WELL_FORMED_KEY_IDENTIFIER,
                "ReplicationRegions": ["not-a-region"],
            }),
            "replicationRegions",
            vec!["not-a-region"],
            &region_set,
        ),
        (
            "AddKeyReplicationRegions",
            json!({
                "KeyIdentifier": WELL_FORMED_KEY_IDENTIFIER,
                "ReplicationRegions": ["us-east-1", "NOPE"],
            }),
            "replicationRegions",
            vec!["us-east-1", "NOPE"],
            &region_set,
        ),
        (
            "EnableDefaultKeyReplicationRegions",
            json!({"ReplicationRegions": ["bad_region", "eu-west-1", "x"]}),
            "replicationRegions",
            vec!["bad_region", "eu-west-1", "x"],
            &region_set,
        ),
        (
            "DisableDefaultKeyReplicationRegions",
            json!({"ReplicationRegions": ["Us-East-1"]}),
            "replicationRegions",
            vec!["Us-East-1"],
            &region_set,
        ),
        (
            "UntagResource",
            json!({
                "ResourceArn": well_formed_key_arn(),
                "TagKeys": [over_long_tag_key.as_str()],
            }),
            "tagKeys",
            vec![over_long_tag_key.as_str()],
            &tag_key_set,
        ),
        (
            "UntagResource",
            json!({"ResourceArn": well_formed_key_arn(), "TagKeys": ["", ""]}),
            "tagKeys",
            vec!["", ""],
            &tag_key_set,
        ),
    ];

    for (operation, body, member, elements, requirement) in rows {
        let case = format!("{operation} {member} of {} elements", elements.len());
        let expected = envelope(&[value_clause(
            &quoted_list(&elements),
            member,
            requirement,
        )]);
        assert_refused(&router, &case, operation, &body, &expected).await;
    }
}

#[tokio::test]
async fn an_absent_required_member_is_stated_as_the_null_it_was() {
    let router = router();
    // One row per shape of absence: the only member an operation declares, one of two, and a
    // list member — over an implemented operation and a scaffolded one.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("GetAlias", json!({}), "aliasName"),
        ("CreateAlias", json!({}), "aliasName"),
        ("DeleteAlias", json!({}), "aliasName"),
        (
            "RemoveKeyReplicationRegions",
            json!({"KeyIdentifier": WELL_FORMED_KEY_IDENTIFIER}),
            "replicationRegions",
        ),
        (
            "EnableDefaultKeyReplicationRegions",
            json!({}),
            "replicationRegions",
        ),
        (
            "UntagResource",
            json!({"ResourceArn": well_formed_key_arn()}),
            "tagKeys",
        ),
    ];

    for (operation, body, member) in rows {
        let case = format!("{operation} without {member}");
        let expected = envelope(&[absent_clause(member, "Member must not be null")]);
        assert_refused(&router, &case, operation, &body, &expected).await;
    }
}

#[tokio::test]
async fn several_broken_constraints_are_one_envelope_over_one_clause_each() {
    let router = router();
    // Two members of one request, each outside its own `@pattern`: the envelope counts the
    // clauses and joins them, and each clause is the same sentence it carries alone. The
    // ORDER the walk reports them in is the framework's, so the assertion is about the
    // envelope and the clauses rather than about one arrangement of them.
    let alias_name = "no-alias-prefix";
    let key_arn = format!("{KEY_ARN_PREFIX}{}", "?".repeat(20));
    let (status, parsed) = call(
        &router,
        "CreateAlias",
        &json!({"AliasName": alias_name, "KeyArn": key_arn}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case two broken patterns: expected 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case two broken patterns: expected ValidationException, got {parsed}"
    );
    let stated = parsed
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "case two broken patterns: expected the two-error envelope, got {stated:?}"
    );
    for clause in [
        value_clause(
            alias_name,
            "aliasName",
            &pattern_requirement(ALIAS_NAME_EXPRESSION),
        ),
        value_clause(
            &key_arn,
            "keyArn",
            &pattern_requirement(RESOURCE_ARN_EXPRESSION),
        ),
    ] {
        assert!(
            stated.contains(&clause),
            "case two broken patterns: expected the clause {clause:?}, got {stated:?}"
        );
    }
    assert_eq!(
        stated.matches("; ").count(),
        1,
        "case two broken patterns: the clauses are joined by one separator, got {stated:?}"
    );
}

#[tokio::test]
async fn an_answer_this_rule_does_not_own_is_left_exactly_as_its_renderer_wrote_it() {
    let router = router();

    // A success: no error body at all, so nothing to report a text in.
    let created = ok_call(
        &router,
        "CreateKey",
        &json!({
            "KeyAttributes": {
                "KeyUsage": "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
                "KeyClass": "SYMMETRIC_KEY",
                "KeyAlgorithm": "AES_128",
                "KeyModesOfUse": {"Encrypt": true, "Decrypt": true},
            },
            "Exportable": true,
        }),
    )
    .await;
    assert!(
        created.get("Key").is_some(),
        "case a success: expected the created Key, got {created}"
    );

    // A modelled error whose shape declares no prose member at all: it reports the resource
    // it could not find, and no text member is invented for it.
    let (status, parsed) = call(
        &router,
        "GetAlias",
        &json!({"AliasName": "alias/never-created"}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case an unknown alias: expected 404, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFoundException",
        "case an unknown alias: expected ResourceNotFoundException, got {parsed}"
    );
    assert_eq!(
        parsed.get("ResourceId").and_then(Value::as_str),
        Some("alias/never-created"),
        "case an unknown alias: expected the resource it named, got {parsed}"
    );
    for member in ["Message", "message"] {
        assert!(
            parsed.get(member).is_none(),
            "case an unknown alias: this shape declares no prose member, got {parsed}"
        );
    }

    // The scaffold's own 501 is a class this model does not declare, so it keeps the member
    // the framework wrote its text in.
    let (status, parsed) = call(
        &router,
        "DeleteAlias",
        &json!({"AliasName": "alias/well-formed"}),
    )
    .await;
    assert_eq!(
        status, 501,
        "case a scaffolded operation: expected 501, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "NotImplemented",
        "case a scaffolded operation: expected NotImplemented, got {parsed}"
    );
    assert!(
        parsed.get("message").and_then(Value::as_str).is_some(),
        "case a scaffolded operation: the framework's own member must survive, got {parsed}"
    );
    assert!(
        parsed.get("Message").is_none(),
        "case a scaffolded operation: a class this model does not declare must not be \
         rewritten, got {parsed}"
    );
}
