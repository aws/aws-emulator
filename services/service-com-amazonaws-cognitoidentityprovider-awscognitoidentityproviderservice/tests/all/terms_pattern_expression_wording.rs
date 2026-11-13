// SPDX-License-Identifier: CC0-1.0

//! Which EXPRESSION a `@pattern` constraint clause of this endpoint quotes, over the wire,
//! driven exactly as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a clause states the expression Amazon Cognito publishes for the
//! TARGET SHAPE of the member it names — for the two members that address a set of terms
//! documents (`TermsId`, `TermsName`) the modelled `@pattern` character for character, the
//! `^`/`$` included, and for every other patterned member of this model the modelled text
//! without that whole-value pair. The pair the clause drops or keeps never changes the
//! CHECK: a value is refused by the modelled expression as written either way, which the
//! rows carrying a value that merely CONTAINS an admissible one pin.
//!
//! Each table row spans what the rule generalises over rather than one request: the
//! operation the member arrives on, the member position (required and optional), the form
//! the bad value takes (malformed, admissible-but-embedded, wrong case, empty) and the shape
//! the expression itself has (a fixed-length hexadecimal id, an alternation of literals, a
//! character class). Every assertion names its case and reports what it saw.

use serde_json::{json, Value};

use super::log_delivery_configuration::{call, error_code, error_message, ok, router};

/// The modelled `@pattern` of `TermsIdType`, character for character
/// (`smithy_query(query="shape",shape="TermsIdType")`).
const TERMS_ID_PATTERN: &str =
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$";

/// The modelled `@pattern` of `TermsNameType`, character for character
/// (`smithy_query(query="shape",shape="TermsNameType")`).
const TERMS_NAME_PATTERN: &str = r"^(terms-of-use|privacy-policy)$";

/// The expression a clause quotes for `UserPoolIdType`, whose modelled pattern is
/// `^[\w-]+_[0-9a-zA-Z]+$` (`smithy_query(query="shape",shape="UserPoolIdType")`).
const USER_POOL_ID_EXPRESSION: &str = r"[\w-]+_[0-9a-zA-Z]+";

/// The expression a clause quotes for `ClientIdType`, whose modelled pattern is `^[\w+]+$`
/// (`smithy_query(query="shape",shape="ClientIdType")`).
const CLIENT_ID_EXPRESSION: &str = r"[\w+]+";

/// The expression a clause quotes for `UserPoolNameType`, whose modelled pattern is
/// `^[\w\s+=,.@-]+$` (`smithy_query(query="shape",shape="UserPoolNameType")`).
const POOL_NAME_EXPRESSION: &str = r"[\w\s+=,.@-]+";

/// A value `TermsIdType`'s expression admits, used wherever a row needs a well-formed id in
/// a member it is not testing.
const WELL_FORMED_TERMS_ID: &str = "11111111-2222-4333-8444-555555555555";

/// The error code every refusal below carries: Amazon Cognito's own bad-parameter class,
/// which every operation of this service declares (OPERATIONS.md).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The clause a single `@pattern` violation states.
fn pattern_clause(value: &str, member: &str, expression: &str) -> String {
    format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {expression}"
    )
}

/// The whole message a refusal of ONE broken constraint carries.
fn one_validation_error(value: &str, member: &str, expression: &str) -> String {
    format!(
        "1 validation error detected: {}",
        pattern_clause(value, member, expression)
    )
}

/// One row: the operation, the request, the member the clause names, the value it quotes and
/// the expression it states.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    member: &'static str,
    value: &'static str,
    expression: String,
}

/// A pool and an app client to address, so every row below is refused on the member it
/// tests rather than on a pool or a client that does not exist.
async fn fixture(router: &smithy_server_runtime::MultiServiceRouter) -> (String, String) {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": "terms-pattern-pool" })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case the fixture: a minted pool id: {created}"))
        .to_string();
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": "terms-pattern-client" }),
    )
    .await;
    let client_id = created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the fixture: a minted client id: {created}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Drive every row and assert the message it states.
async fn assert_rows(router: &smithy_server_runtime::MultiServiceRouter, rows: Vec<Row>) {
    for row in rows {
        let case = row.case;
        let (status, payload) = call(router, row.operation, &row.body).await;
        let expected = one_validation_error(row.value, row.member, &row.expression);
        assert_eq!(
            (status, error_code(&payload)),
            (400, INVALID_PARAMETER),
            "case {case:?}: a value outside a modelled @pattern is a 400 \
             {INVALID_PARAMETER}, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&payload)
        );
    }
}

#[tokio::test]
async fn a_terms_members_clause_quotes_the_modelled_expression_as_written() {
    let router = router();
    let (user_pool_id, client_id) = fixture(&router).await;

    let rows = vec![
        Row {
            case: "a terms id whose version nibble the expression excludes",
            operation: "DeleteTerms",
            body: json!({
                "UserPoolId": user_pool_id,
                "TermsId": "00000000-0000-0000-0000-000000000000",
            }),
            member: "termsId",
            value: "00000000-0000-0000-0000-000000000000",
            expression: TERMS_ID_PATTERN.to_string(),
        },
        Row {
            case: "a terms id that is not a UUID at all",
            operation: "DescribeTerms",
            body: json!({ "UserPoolId": user_pool_id, "TermsId": "not-a-uuid" }),
            member: "termsId",
            value: "not-a-uuid",
            expression: TERMS_ID_PATTERN.to_string(),
        },
        Row {
            case: "a terms id a hexadecimal group short",
            operation: "UpdateTerms",
            body: json!({
                "UserPoolId": user_pool_id,
                "TermsId": "1111111-2222-4333-8444-555555555555",
            }),
            member: "termsId",
            value: "1111111-2222-4333-8444-555555555555",
            expression: TERMS_ID_PATTERN.to_string(),
        },
        Row {
            // The expression without its pair would FIND an admissible id inside this
            // value, so the refusal shows the check runs the modelled text as written.
            case: "a value that merely contains an admissible terms id",
            operation: "DeleteTerms",
            body: json!({
                "UserPoolId": user_pool_id,
                "TermsId": format!("prefix-{WELL_FORMED_TERMS_ID}-suffix"),
            }),
            member: "termsId",
            value: "prefix-11111111-2222-4333-8444-555555555555-suffix",
            expression: TERMS_ID_PATTERN.to_string(),
        },
        Row {
            case: "a terms name that names neither document type",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": "not-a-valid-terms-name",
            }),
            member: "termsName",
            value: "not-a-valid-terms-name",
            expression: TERMS_NAME_PATTERN.to_string(),
        },
        Row {
            case: "a terms name in the wrong case",
            operation: "CreateTerms",
            body: json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": "Terms-Of-Use",
                "TermsSource": "LINK",
                "Enforcement": "NONE",
            }),
            member: "termsName",
            value: "Terms-Of-Use",
            expression: TERMS_NAME_PATTERN.to_string(),
        },
        Row {
            // The same alternation, reached through the OPTIONAL member position of the
            // update, whose required id is well formed so this is the only violation.
            case: "a terms name carrying a space, in an optional member",
            operation: "UpdateTerms",
            body: json!({
                "UserPoolId": user_pool_id,
                "TermsId": WELL_FORMED_TERMS_ID,
                "TermsName": "privacy policy",
            }),
            member: "termsName",
            value: "privacy policy",
            expression: TERMS_NAME_PATTERN.to_string(),
        },
        Row {
            // Unanchored, the alternation would FIND `terms-of-use` inside this name.
            case: "a value that merely contains an admissible terms name",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": "xx-terms-of-use-yy",
            }),
            member: "termsName",
            value: "xx-terms-of-use-yy",
            expression: TERMS_NAME_PATTERN.to_string(),
        },
        Row {
            case: "an empty terms name",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": "",
            }),
            member: "termsName",
            value: "",
            expression: TERMS_NAME_PATTERN.to_string(),
        },
    ];

    assert_rows(&router, rows).await;
}

#[tokio::test]
async fn every_other_patterned_member_keeps_the_expression_published_without_the_pair() {
    let router = router();
    let (user_pool_id, _client_id) = fixture(&router).await;

    let rows = vec![
        Row {
            case: "a user pool id carrying no underscore, on a terms deletion",
            operation: "DeleteTerms",
            body: json!({ "UserPoolId": "no-underscore", "TermsId": WELL_FORMED_TERMS_ID }),
            member: "userPoolId",
            value: "no-underscore",
            expression: USER_POOL_ID_EXPRESSION.to_string(),
        },
        Row {
            case: "a user pool id carrying a character its class excludes, on a terms read",
            operation: "DescribeTerms",
            body: json!({ "UserPoolId": "us-east-1_bad!id", "TermsId": WELL_FORMED_TERMS_ID }),
            member: "userPoolId",
            value: "us-east-1_bad!id",
            expression: USER_POOL_ID_EXPRESSION.to_string(),
        },
        Row {
            // A `@sensitive` member, whose value this service's clause quotes anyway.
            case: "an app client id carrying characters its class excludes",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": user_pool_id,
                "ClientId": "bad client id!",
                "TermsName": "terms-of-use",
            }),
            member: "clientId",
            value: "bad client id!",
            expression: CLIENT_ID_EXPRESSION.to_string(),
        },
        Row {
            case: "a pool name carrying characters its class excludes",
            operation: "CreateUserPool",
            body: json!({ "PoolName": "bad/pool*name!" }),
            member: "poolName",
            value: "bad/pool*name!",
            expression: POOL_NAME_EXPRESSION.to_string(),
        },
    ];

    assert_rows(&router, rows).await;
}

#[tokio::test]
async fn one_request_breaking_both_kinds_states_each_member_as_its_own_shape_publishes_it() {
    let router = router();

    // Both members of this request are outside their modelled expression, so one message
    // carries both clauses — each rendered by the shape its member targets.
    let (status, payload) = call(
        &router,
        "DeleteTerms",
        &json!({ "UserPoolId": "no-underscore", "TermsId": "not-a-uuid" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, INVALID_PARAMETER),
        "case two broken expressions in one request: expected a 400 {INVALID_PARAMETER}, got \
         {status} with {payload}"
    );
    let message = error_message(&payload);
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case two broken expressions in one request: the envelope counts both clauses, got \
         {message:?}"
    );
    for (case, clause) in [
        (
            "the terms id, quoted as the model writes it",
            pattern_clause("not-a-uuid", "termsId", TERMS_ID_PATTERN),
        ),
        (
            "the user pool id, quoted without the whole-value pair",
            pattern_clause("no-underscore", "userPoolId", USER_POOL_ID_EXPRESSION),
        ),
    ] {
        assert!(
            message.contains(&clause),
            "case {case:?}: expected the message to carry {clause:?}, got {message:?}"
        );
    }
}
