// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request that breaks a modelled constraint, asserted on the
//! wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this
//! crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The RULE under test, which is this service's and not one operation's: a refused
//! request is answered HTTP 400 `ValidationException` — the CONSTRAINT layer's own error
//! class, not the operation's modelled `BadRequestException` — whose message names the
//! offending value, the member's path ROOTED AT THE OPERATION'S INPUT SHAPE
//! (`<operation>Input` in lowerCamel) when the member travels in the BODY and the member
//! NAME ALONE when it is read out of the request URI (a path label or a query-string
//! parameter), and the constraint it failed — with a `@pattern`
//! emitted without the whole-value anchors the public model adds, and an enum value set
//! listed in the order the service declares the enum in. The ONE member this service
//! checks itself ahead of that layer (`CreateAuthorizer$type`) carries the operation's own
//! `BadRequestException` and the operation's own SENTENCE for that member instead, which is
//! the exception the last table below pins.
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! twelve different operations (so the root is shown to be derived per operation, not a
//! constant), four constraint classes (an enum value set, a `@pattern`, an absent
//! required member, several violations at once), all three HTTP bindings a violated member
//! can have (a body member, a URI label and a query-string parameter), five enums whose
//! value set the service lists in its own order, and a nested member at two different list
//! positions.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{Answer, Gateway};

/// The error code a constraint failure carries on this surface: a value outside a
/// member's modelled constraint is refused by the layer in FRONT of the operation, whose
/// error class is its own. Source: the recorded refusals of an out-of-enum
/// `CreateRestApi$endpointConfiguration$types`, an out-of-pattern
/// `PutGatewayResponse$statusCode`, an out-of-enum
/// `PutIntegrationResponse$contentHandling` and an out-of-enum
/// `UpdateVpcLink$patchOperations[]$op`, all of which AWS answers `ValidationException`
/// (HTTP 400).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The code the operation's OWN refusals carry — including a violated member this service
/// checks itself ahead of the constraint layer. Source: the recorded refusal of an
/// out-of-enum `CreateAuthorizer$type`, which AWS answers `BadRequestException`
/// (HTTP 400), and `BadRequestException` is the client-input error this service's model
/// declares.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The code a HANDLER answers for identifiers that name no stored record, as every
/// operation of this service models it: `NotFoundException` (HTTP 404). A row that
/// satisfies every modelled constraint and still names an API that was never created is
/// answered by this, which is what says the request got PAST the boundary validator.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The `Op` enum's value set as API Gateway lists it. Source: the recorded refusals of an
/// invalid patch `op` on `UpdateDocumentationVersion`, `UpdateRestApi` and `UpdateApiKey`
/// — three operations, one order. (The public model declares the same six values in a
/// different order: `smithy_query(query="shape", shape="com.amazonaws.apigateway#Op")`.)
const OP_VALUES: [&str; 6] = ["add", "remove", "move", "test", "replace", "copy"];

/// The `ApiKeySourceType` value set as API Gateway lists it. Source: the recorded
/// `ResourceStatusReason` of a CloudFormation `AWS::ApiGateway::RestApi` whose declared
/// `ApiKeySourceType` is outside the set, which quotes this service's own refusal of
/// `apiKeySource`. (The public model declares the same two values in the other order:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#ApiKeySourceType")`.)
const API_KEY_SOURCE_VALUES: [&str; 2] = ["AUTHORIZER", "HEADER"];

/// The `GatewayResponseType` value set, in the order the model declares it. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#GatewayResponseType")`.
const GATEWAY_RESPONSE_TYPE_VALUES: [&str; 21] = [
    "DEFAULT_4XX",
    "DEFAULT_5XX",
    "RESOURCE_NOT_FOUND",
    "UNAUTHORIZED",
    "INVALID_API_KEY",
    "ACCESS_DENIED",
    "AUTHORIZER_FAILURE",
    "AUTHORIZER_CONFIGURATION_ERROR",
    "INVALID_SIGNATURE",
    "EXPIRED_TOKEN",
    "MISSING_AUTHENTICATION_TOKEN",
    "INTEGRATION_FAILURE",
    "INTEGRATION_TIMEOUT",
    "API_CONFIGURATION_ERROR",
    "UNSUPPORTED_MEDIA_TYPE",
    "BAD_REQUEST_PARAMETERS",
    "BAD_REQUEST_BODY",
    "REQUEST_TOO_LARGE",
    "THROTTLED",
    "QUOTA_EXCEEDED",
    "WAF_FILTERED",
];

/// The `AuthorizerType` value set, in the order the model declares it. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#AuthorizerType")`.
const AUTHORIZER_TYPE_VALUES: [&str; 4] = ["TOKEN", "REQUEST", "COGNITO_USER_POOLS", "JWT"];

/// The `IntegrationType` value set as API Gateway lists it. Source: the recorded refusal
/// of an out-of-enum `type` on `PutIntegration`. (The public model declares the same five
/// values in a different order: `smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#IntegrationType")` gives
/// `[HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]`.)
const INTEGRATION_TYPE_VALUES: [&str; 5] = ["HTTP", "MOCK", "AWS_PROXY", "HTTP_PROXY", "AWS"];

/// The `ContentHandlingStrategy` value set as API Gateway lists it. Source: the recorded
/// refusal of an out-of-enum `contentHandling` on `PutIntegrationResponse`. (The public
/// model declares the two values the other way round: `smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#ContentHandlingStrategy")`.)
const CONTENT_HANDLING_VALUES: [&str; 2] = ["CONVERT_TO_TEXT", "CONVERT_TO_BINARY"];

/// The `PutMode` value set as API Gateway lists it. Source: the recorded refusal of an
/// out-of-enum `mode` on `PutRestApi`. (The public model declares `[merge, overwrite]`:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#PutMode")`.)
const PUT_MODE_VALUES: [&str; 2] = ["overwrite", "merge"];

/// The `DocumentationPartType` value set as API Gateway lists it. Source: the recorded
/// refusal of an out-of-enum `location.type` on `CreateDocumentationPart`. (The public
/// model declares the same twelve values in another order: `smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#DocumentationPartType")`.)
const DOCUMENTATION_PART_TYPE_VALUES: [&str; 12] = [
    "RESPONSE_BODY",
    "RESPONSE",
    "METHOD",
    "MODEL",
    "AUTHORIZER",
    "RESPONSE_HEADER",
    "RESOURCE",
    "PATH_PARAMETER",
    "REQUEST_BODY",
    "QUERY_PARAMETER",
    "API",
    "REQUEST_HEADER",
];

/// The sentence `CreateAuthorizer` answers a `type` outside that value set with, written
/// out here rather than read from the crate so a change of wording is caught rather than
/// followed. Source: API Gateway's recorded refusal of a `type` outside the catalog.
const INVALID_AUTHORIZER_TYPE_SENTENCE: &str = "Invalid AuthorizerType specified.";

/// The `StatusCode` shape's regular expression as the service emits it: the modelled
/// `^[1-5]\d\d$` without the whole-value anchors. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#StatusCode")`.
const STATUS_CODE_PATTERN: &str = r"[1-5]\d\d";

/// The envelope one violation is stated in.
fn one(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a value outside its constraint.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement an enum value set states.
fn enum_value_set(values: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// The requirement a `@pattern` states.
fn regular_expression(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// A list of patch operations whose `op` at 1-based `position` is `bad` and every other
/// is a legal one.
fn patch_operations(count: usize, position: usize, bad: &str) -> Value {
    Value::Array(
        (1..=count)
            .map(|at| {
                let op = if at == position { bad } else { "replace" };
                json!({ "op": op, "path": "/description", "value": "v" })
            })
            .collect(),
    )
}

/// Assert `answer` is the 400 refusal under `code` stating exactly `message`.
fn assert_stated_as(answer: &Answer, case: &str, code: &str, message: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some(code),
        "case {case}: expected {code}, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// Assert `answer` is the 400 constraint refusal stating exactly `message`.
fn assert_stated(answer: &Answer, case: &str, message: &str) {
    assert_stated_as(answer, case, VALIDATION_EXCEPTION, message);
}

#[tokio::test]
async fn a_refused_request_names_the_member_by_its_path_from_the_operations_input_shape() {
    // (case, method, uri, body, the message the service states)
    let rows: Vec<(String, &str, String, Option<Value>, String)> = vec![
        (
            "UpdateDocumentationVersion, an invalid patch op".to_string(),
            "PATCH",
            "/restapis/abcde12345/documentation/versions/1.0.0".to_string(),
            Some(json!({ "patchOperations": patch_operations(1, 1, "invalid_op") })),
            one(&clause(
                "invalid_op",
                "updateDocumentationVersionInput.patchOperations.1.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            // The same operation and member, at a different list position: the position
            // is computed from the request, not fixed at the recorded 1.
            "UpdateDocumentationVersion, an invalid patch op third in the list".to_string(),
            "PATCH",
            "/restapis/abcde12345/documentation/versions/1.0.0".to_string(),
            Some(json!({ "patchOperations": patch_operations(4, 3, "nope") })),
            one(&clause(
                "nope",
                "updateDocumentationVersionInput.patchOperations.3.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            "UpdateRestApi, an invalid patch op".to_string(),
            "PATCH",
            "/restapis/abcde12345".to_string(),
            Some(json!({ "patchOperations": patch_operations(1, 1, "invalid_op") })),
            one(&clause(
                "invalid_op",
                "updateRestApiInput.patchOperations.1.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            "UpdateApiKey, an invalid patch op".to_string(),
            "PATCH",
            "/apikeys/abcde12345".to_string(),
            Some(json!({ "patchOperations": patch_operations(2, 2, "invalid_op") })),
            one(&clause(
                "invalid_op",
                "updateApiKeyInput.patchOperations.2.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            // An operation no recording covers, carrying the same member: the root
            // follows the operation, so the rule holds without a case for each one.
            "UpdateStage, an invalid patch op".to_string(),
            "PATCH",
            "/restapis/abcde12345/stages/prod".to_string(),
            Some(json!({ "patchOperations": patch_operations(1, 1, "upsert") })),
            one(&clause(
                "upsert",
                "updateStageInput.patchOperations.1.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            // A `@pattern` on a BODY member, emitted without the model's anchors.
            "PutGatewayResponse, a status code outside its pattern".to_string(),
            "PUT",
            "/restapis/abcde12345/gatewayresponses/DEFAULT_4XX".to_string(),
            Some(json!({ "statusCode": "999" })),
            one(&clause(
                "999",
                "putGatewayResponseInput.statusCode",
                &regular_expression(STATUS_CODE_PATTERN),
            )),
        ),
        (
            // The SAME pattern on a member bound to the URI, not the body: such a member
            // is read out of the URI one value at a time rather than out of the document
            // the input shape describes, so the clause names it alone.
            "PutIntegrationResponse, a status code label outside its pattern".to_string(),
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration/responses/abc"
                .to_string(),
            Some(json!({})),
            one(&clause(
                "abc",
                "statusCode",
                &regular_expression(STATUS_CODE_PATTERN),
            )),
        ),
        (
            // An enum value set on another member bound to the URI.
            "PutGatewayResponse, a response type outside its enum".to_string(),
            "PUT",
            "/restapis/abcde12345/gatewayresponses/NOT_A_RESPONSE_TYPE".to_string(),
            Some(json!({ "statusCode": "404" })),
            one(&clause(
                "NOT_A_RESPONSE_TYPE",
                "responseType",
                &enum_value_set(&GATEWAY_RESPONSE_TYPE_VALUES),
            )),
        ),
        (
            // A `@pattern` on a URI LABEL of another operation: named alone, as the query
            // member above is — both are read out of the request URI.
            "PutMethodResponse, a status code label outside its pattern".to_string(),
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/responses/not-a-status"
                .to_string(),
            Some(json!({})),
            one(&clause(
                "not-a-status",
                "statusCode",
                &regular_expression(STATUS_CODE_PATTERN),
            )),
        ),
        (
            // An enum value set whose order is the SERVICE's and not the model's, on a
            // body member of a third operation.
            "PutIntegration, a type outside its enum".to_string(),
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration".to_string(),
            Some(json!({ "type": "NOT_A_REAL_TYPE" })),
            one(&clause(
                "NOT_A_REAL_TYPE",
                "putIntegrationInput.type",
                &enum_value_set(&INTEGRATION_TYPE_VALUES),
            )),
        ),
        (
            "PutIntegrationResponse, a content handling outside its enum".to_string(),
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration/responses/200"
                .to_string(),
            Some(json!({ "contentHandling": "NOT_A_REAL_CONTENT_HANDLING" })),
            one(&clause(
                "NOT_A_REAL_CONTENT_HANDLING",
                "putIntegrationResponseInput.contentHandling",
                &enum_value_set(&CONTENT_HANDLING_VALUES),
            )),
        ),
        (
            // The same, on a NESTED body member.
            "CreateDocumentationPart, a location type outside its enum".to_string(),
            "POST",
            "/restapis/abcde12345/documentation/parts".to_string(),
            Some(json!({
                "location": { "type": "NOT_A_REAL_LOCATION_TYPE" },
                "properties": "{}",
            })),
            one(&clause(
                "NOT_A_REAL_LOCATION_TYPE",
                "createDocumentationPartInput.location.type",
                &enum_value_set(&DOCUMENTATION_PART_TYPE_VALUES),
            )),
        ),
        (
            // The same, on a member read out of the QUERY STRING: the service's own value
            // order and the member named alone.
            "PutRestApi, a mode outside its enum".to_string(),
            "PUT",
            "/restapis/abcde12345?mode=sideways".to_string(),
            Some(json!({ "openapi": "3.0.1" })),
            one(&clause(
                "sideways",
                "mode",
                &enum_value_set(&PUT_MODE_VALUES),
            )),
        ),
        (
            "CreateRestApi, an api key source outside its enum".to_string(),
            "POST",
            "/restapis".to_string(),
            Some(json!({ "name": "petstore", "apiKeySource": "SOMEWHERE_ELSE" })),
            one(&clause(
                "SOMEWHERE_ELSE",
                "createRestApiInput.apiKeySource",
                &enum_value_set(&API_KEY_SOURCE_VALUES),
            )),
        ),
        (
            // An absent required member is found while READING the request rather than
            // while walking it, and is stated with the same rooted path.
            "CreateRestApi, no name at all".to_string(),
            "POST",
            "/restapis".to_string(),
            Some(json!({ "description": "nameless" })),
            one(
                "Value null at 'createRestApiInput.name' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
    ];

    let gateway = Gateway::new();
    for (case, method, uri, body, expected) in rows {
        let answer = gateway.call(method, &uri, body).await;
        assert_stated(&answer, &case, &expected);
    }
}

#[tokio::test]
async fn a_member_this_service_checks_itself_is_refused_as_the_operations_own_bad_request() {
    // The ONE violated member of this service whose refusal is the OPERATION's rather
    // than the constraint layer's: `CreateAuthorizer$type`, whose recorded refusal carries
    // `BadRequestException` AND the operation's own sentence for that member — which
    // reports no constraint and quotes back neither the rejected value nor the catalog.
    // The rows span the ways a value can miss that enum — an unknown word, a declared
    // value in the wrong case, the empty string — so both follow the MEMBER and not one
    // recorded value.
    let rows: [(&str, &str); 3] = [
        ("an unknown word", "NOT_A_REAL_TYPE"),
        ("a declared value in the wrong case", "token"),
        ("the empty string", ""),
    ];

    let gateway = Gateway::new();
    for (case, authorizer_type) in rows {
        let answer = gateway
            .call(
                "POST",
                "/restapis/abcde12345/authorizers",
                Some(json!({
                    "name": "auth",
                    "type": authorizer_type,
                    "identitySource": "method.request.header.Auth",
                })),
            )
            .await;
        assert_stated_as(
            &answer,
            case,
            BAD_REQUEST_EXCEPTION,
            INVALID_AUTHORIZER_TYPE_SENTENCE,
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        // The sentence states the member and nothing else: not the rejected value, not
        // the catalog it missed, and not the constraint envelope.
        for absent in AUTHORIZER_TYPE_VALUES
            .iter()
            .copied()
            .chain([authorizer_type, "validation error"])
        {
            assert!(
                absent.is_empty() || !message.contains(absent),
                "case {case}: expected the sentence to carry no {absent:?}, got {message:?}"
            );
        }
    }

    // Another member of the SAME operation stays on the constraint layer's class, which
    // is what says the code follows the member rather than the operation.
    let answer = gateway
        .call(
            "POST",
            "/restapis/abcde12345/authorizers",
            Some(json!({
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
            })),
        )
        .await;
    assert_stated(
        &answer,
        "CreateAuthorizer, no name at all",
        &one(
            "Value null at 'createAuthorizerInput.name' failed to satisfy constraint: \
             Member must not be null",
        ),
    );
}

#[tokio::test]
async fn every_violation_of_one_request_is_stated_with_the_same_rooted_path() {
    // Two members of one request break their constraints at once: the envelope counts
    // them and each clause names its member the way that member travels — the body member
    // from the operation's root, the URI label alone. The ORDER the walk reports
    // them in belongs to the framework, so the clauses are looked for rather than
    // sequenced.
    let gateway = Gateway::new();
    let answer = gateway
        .call(
            "PUT",
            "/restapis/abcde12345/gatewayresponses/NOT_A_RESPONSE_TYPE",
            Some(json!({ "statusCode": "99" })),
        )
        .await;
    let case = "PutGatewayResponse(bad responseType and statusCode)";

    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    let message = answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a message, got {:?}", answer.body))
        .to_string();
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case}: expected the two-violation envelope, got {message:?}"
    );
    for expected in [
        clause(
            "NOT_A_RESPONSE_TYPE",
            "responseType",
            &enum_value_set(&GATEWAY_RESPONSE_TYPE_VALUES),
        ),
        clause(
            "99",
            "putGatewayResponseInput.statusCode",
            &regular_expression(STATUS_CODE_PATTERN),
        ),
    ] {
        assert!(
            message.contains(&expected),
            "case {case}: expected the clause {expected:?} in {message:?}"
        );
    }
}

/// One row of the accepted-request table below: the case name, the HTTP method, the URI,
/// the request body, and the answer the OPERATION itself makes — `None` for its own
/// success, or the modelled error code it answers identifiers that name no record with.
type AcceptedRow<'a> = (&'a str, &'a str, &'a str, Option<Value>, Option<&'a str>);

#[tokio::test]
async fn a_request_within_every_modelled_constraint_is_not_refused_as_a_validation_error() {
    // The wording seam must not refuse what the model accepts: each row satisfies every
    // constraint its operation declares, so none of them may answer the validation
    // error above. What they DO answer is their own operation's business, which the row
    // states: a success for a request naming records that exist, and the modelled
    // `NotFoundException` for one whose identifiers name an API that was never created —
    // a refusal the HANDLER makes, which is what says the request got past the boundary
    // validator rather than being refused by it.
    let rows: [AcceptedRow; 5] = [
        (
            "CreateRestApi with every modelled member",
            "POST",
            "/restapis",
            Some(json!({
                "name": "petstore",
                "apiKeySource": "HEADER",
                "endpointConfiguration": { "types": ["REGIONAL"] },
            })),
            None,
        ),
        (
            "UpdateRestApi with a legal op at each position",
            "PATCH",
            "/restapis/abcde12345",
            Some(json!({ "patchOperations": patch_operations(3, 0, "unused") })),
            Some(NOT_FOUND_EXCEPTION),
        ),
        (
            "PutGatewayResponse within the status-code pattern",
            "PUT",
            "/restapis/abcde12345/gatewayresponses/DEFAULT_4XX",
            Some(json!({ "statusCode": "404" })),
            Some(NOT_FOUND_EXCEPTION),
        ),
        (
            "PutIntegrationResponse within the status-code pattern",
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration/responses/200",
            Some(json!({})),
            Some(NOT_FOUND_EXCEPTION),
        ),
        ("GetRestApis, which constrains nothing", "GET", "/restapis", None, None),
    ];

    let gateway = Gateway::new();
    for (case, method, uri, body, expected) in rows {
        let answer = gateway.call(method, uri, body).await;
        // Not the framework's generic code, which this surface never answers at all,
        // and not the constraint envelope either — which is what says the request was
        // not refused as a constraint violation, whatever code the answer carries.
        assert_ne!(
            answer.error.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case}: expected no validation refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("validation error"),
            "case {case}: expected no validation refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
        // The POSITIVE expectation, so the row cannot go vacuous: the answer is the one
        // the operation itself makes.
        match expected {
            None => assert!(
                (200..300).contains(&answer.status),
                "case {case}: expected the operation's own success, got {} {:?} with {:?}",
                answer.status,
                answer.error,
                answer.body
            ),
            Some(code) => {
                assert_eq!(
                    answer.error.as_deref(),
                    Some(code),
                    "case {case}: expected the handler's own {code}, got {} {:?} with {:?}",
                    answer.status,
                    answer.error,
                    answer.body
                );
                assert_eq!(
                    answer.status, 404,
                    "case {case}: expected HTTP 404 with {code}, got {}",
                    answer.status
                );
            }
        }
    }
}
